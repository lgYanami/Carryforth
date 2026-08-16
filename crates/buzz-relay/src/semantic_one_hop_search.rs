//! One-shot, structure-scoped semantic selection for Agent graph traversal.
//!
//! Both closed operations share one Q0 Provider call and the common one-shot
//! authorization envelope. The database owns structure enumeration, direct
//! scoring, canonical preview hydration, and coverage accounting.

use buzz_db::semantic_query::{
    SemanticEdgeCoordinateSearchOutcome, SemanticExactQueryVector, SemanticGraphReadTimeouts,
    SemanticIncidentEdgeSearchOutcome,
};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    build_one_hop_semantic_query_encoder_input, derive_one_hop_semantic_http_request_binding,
    derive_one_hop_semantic_v2_http_request_binding,
    edge_coordinate_filtered_ranking_contract_digest, edge_coordinate_ranking_contract_digest,
    incident_edge_ranking_contract_digest, OneHopSemanticError, OneHopSemanticObservations,
    OneHopSemanticScope, ProjectContextOneHopSemanticQuery,
    ProjectContextOneHopSemanticQueryResult, SemanticGraphQueryError, SemanticQueryEncoder,
    MAX_ONE_HOP_SEMANTIC_WALL_TIME_MS,
};
use nostr::Event;

use crate::semantic_one_shot::{SemanticOneShotError, SemanticOneShotExecution};
use crate::state::AppState;

/// Closed, content-free failures for the one-hop semantic HTTP surface.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OneHopSemanticExecutionError {
    #[error("One-hop semantic request is invalid")]
    InvalidRequest,
    #[error("One-hop semantic caller is no longer authorized")]
    Restricted,
    #[error("One-hop semantic runtime is unavailable")]
    Unavailable,
    #[error("One-hop semantic process or Provider admission is busy")]
    Busy {
        /// Optional bounded Provider retry hint.
        retry_after_seconds: Option<u64>,
    },
    #[error("One-hop semantic generation or graph snapshot changed")]
    Conflict,
    #[error("One-hop semantic deadline exceeded")]
    Timeout,
    #[error("One-hop semantic scope was not found")]
    NotFound,
    #[error("One-hop semantic scope exceeds its materialization bound")]
    ScopeTooLarge,
    #[error("One-hop semantic Hyperedge identity exceeds its bound")]
    HyperedgeTooLarge,
    #[error("One-hop semantic response exceeds its byte bound")]
    ResponseTooLarge,
    #[error("One-hop semantic result verification failed")]
    VerificationFailed,
    #[error("One-hop semantic database operation failed")]
    Database(#[source] buzz_db::DbError),
    #[error("One-hop semantic Provider operation failed")]
    Provider(#[source] SemanticGraphQueryError),
    #[error("One-hop semantic contract operation failed")]
    Contract(#[source] OneHopSemanticError),
    #[error("One-hop semantic result signing failed")]
    Signing,
}

/// Execute one authenticated one-hop selection and return one signed Event.
pub(crate) async fn execute_project_context_one_hop_semantic_search(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextOneHopSemanticQuery,
) -> Result<Event, OneHopSemanticExecutionError> {
    execute_one_hop_semantic_search(
        state,
        community_id,
        authenticated_caller,
        nip98_auth_event_id,
        exact_authenticated_body,
        query,
        false,
    )
    .await
}

/// Execute one authenticated filtered Edge-to-Coordinate v2 selection.
pub(crate) async fn execute_project_context_one_hop_semantic_search_v2(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextOneHopSemanticQuery,
) -> Result<Event, OneHopSemanticExecutionError> {
    execute_one_hop_semantic_search(
        state,
        community_id,
        authenticated_caller,
        nip98_auth_event_id,
        exact_authenticated_body,
        query,
        true,
    )
    .await
}

async fn execute_one_hop_semantic_search(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextOneHopSemanticQuery,
    filtered: bool,
) -> Result<Event, OneHopSemanticExecutionError> {
    let query = query
        .validate_and_canonicalize()
        .map_err(OneHopSemanticExecutionError::Contract)?;
    let request_is_filtered = matches!(
        query.scope,
        OneHopSemanticScope::EdgeCoordinates {
            coordinate_types: Some(_),
            ..
        }
    );
    if filtered != request_is_filtered {
        return Err(OneHopSemanticExecutionError::InvalidRequest);
    }
    if query.project_id != *community_id.as_uuid() {
        return Err(OneHopSemanticExecutionError::InvalidRequest);
    }
    let reader_pubkey = authenticated_caller.to_bytes();
    let execution = SemanticOneShotExecution::prepare(
        state,
        community_id,
        &reader_pubkey,
        state
            .config
            .project_context_one_hop_semantic_search_http_available,
        MAX_ONE_HOP_SEMANTIC_WALL_TIME_MS,
    )
    .await
    .map_err(map_one_shot)?;

    let encoder_input = build_one_hop_semantic_query_encoder_input(&query)
        .map_err(OneHopSemanticExecutionError::Provider)?;
    metrics::histogram!("carryforth_one_hop_semantic_provider_input_bytes")
        .record(encoder_input.text().len() as f64);
    let encoded = execution
        .before_deadline(
            execution
                .provider()
                .encode_queries(std::slice::from_ref(&encoder_input)),
        )
        .await
        .map_err(map_one_shot)?
        .map_err(map_provider)?;
    let query_vector = bind_one_hop_query_vector(execution.ticket(), &encoder_input, encoded)?;

    let mut read = execution
        .before_deadline(state.db.begin_semantic_graph_read(
            execution.ticket(),
            &reader_pubkey,
            execution.relay_pubkey(),
            SemanticGraphReadTimeouts::default(),
        ))
        .await
        .map_err(map_one_shot)?
        .map_err(classify_database)?;
    if read.ticket().projection_generation != execution.ticket().projection_generation
        || read.ticket().project_context_revision != execution.ticket().project_context_revision
    {
        return Err(OneHopSemanticExecutionError::Conflict);
    }

    let batch = match &query.scope {
        OneHopSemanticScope::IncidentEdges { coordinate } => execution
            .before_deadline(read.search_incident_edges_one_hop(
                coordinate,
                &query_vector,
                query.limit,
            ))
            .await
            .map_err(map_one_shot)?
            .map_err(classify_database)
            .and_then(|outcome| match outcome {
                SemanticIncidentEdgeSearchOutcome::Ranked(batch) => Ok(batch),
                SemanticIncidentEdgeSearchOutcome::NotFound => {
                    Err(OneHopSemanticExecutionError::NotFound)
                }
                SemanticIncidentEdgeSearchOutcome::ScopeTooLarge { .. } => {
                    Err(OneHopSemanticExecutionError::ScopeTooLarge)
                }
            })?,
        OneHopSemanticScope::EdgeCoordinates {
            edge_key,
            coordinate_types,
        } => execution
            .before_deadline(async {
                match coordinate_types.as_ref() {
                    Some(coordinate_types) => {
                        read.search_edge_coordinates_one_hop_filtered(
                            *edge_key,
                            &query_vector,
                            coordinate_types,
                            query.limit,
                        )
                        .await
                    }
                    None => {
                        read.search_edge_coordinates_one_hop(*edge_key, &query_vector, query.limit)
                            .await
                    }
                }
            })
            .await
            .map_err(map_one_shot)?
            .map_err(classify_database)
            .and_then(|outcome| match outcome {
                SemanticEdgeCoordinateSearchOutcome::Ranked(batch) => Ok(batch),
                SemanticEdgeCoordinateSearchOutcome::NotFound => {
                    Err(OneHopSemanticExecutionError::NotFound)
                }
                SemanticEdgeCoordinateSearchOutcome::ScopeTooLarge { .. } => {
                    Err(OneHopSemanticExecutionError::ScopeTooLarge)
                }
                SemanticEdgeCoordinateSearchOutcome::HyperedgeTooLarge { .. } => {
                    Err(OneHopSemanticExecutionError::HyperedgeTooLarge)
                }
            })?,
    };
    let snapshot_ticket = read.ticket().clone();
    execution
        .before_deadline(read.commit())
        .await
        .map_err(map_one_shot)?
        .map_err(classify_database)?;
    execution
        .confirm_release(&snapshot_ticket)
        .await
        .map_err(map_one_shot)?;

    let request_binding_digest = if filtered {
        derive_one_hop_semantic_v2_http_request_binding(
            query.project_id,
            &reader_pubkey,
            &execution.relay_pubkey().to_bytes(),
            Digest32::from_bytes(nip98_auth_event_id),
            exact_authenticated_body,
        )
    } else {
        derive_one_hop_semantic_http_request_binding(
            query.project_id,
            &reader_pubkey,
            &execution.relay_pubkey().to_bytes(),
            Digest32::from_bytes(nip98_auth_event_id),
            exact_authenticated_body,
        )
    }
    .map_err(OneHopSemanticExecutionError::Contract)?;
    let ranking_contract_digest = match query.scope {
        OneHopSemanticScope::IncidentEdges { .. } => incident_edge_ranking_contract_digest(),
        OneHopSemanticScope::EdgeCoordinates {
            coordinate_types: Some(_),
            ..
        } => edge_coordinate_filtered_ranking_contract_digest(),
        OneHopSemanticScope::EdgeCoordinates { .. } => edge_coordinate_ranking_contract_digest(),
    };
    let result = ProjectContextOneHopSemanticQueryResult {
        request_id: query.request_id,
        project_id: query.project_id,
        request_binding_digest,
        observations: OneHopSemanticObservations {
            semantic_generation_id: batch.snapshot.generation_id,
            source_generation_contract_digest: batch
                .snapshot
                .query_fences
                .source_generation_contract_digest,
            embedding_space_fence: batch.snapshot.query_fences.embedding_space_fence,
            query_contract_digest: batch.snapshot.query_fences.query_contract_digest,
            ranking_contract_digest,
            projection_generation: snapshot_ticket.projection_generation,
            project_context_revision: batch.snapshot.project_context_revision,
            snapshot_observed_at: batch.snapshot.observed_at,
        },
        selection: batch.selection,
    };
    result
        .validate_for_request(&query)
        .map_err(|_| verification_failed("result_contract"))?;
    let builder = if filtered {
        buzz_sdk::semantic_one_hop_search::build_project_context_one_hop_semantic_search_v2_result(
            &result,
            &authenticated_caller,
        )
    } else {
        buzz_sdk::semantic_one_hop_search::build_project_context_one_hop_semantic_search_result(
            &result,
            &authenticated_caller,
        )
    }
    .map_err(|error| match error {
        buzz_sdk::SdkError::ContentTooLarge { .. } => {
            OneHopSemanticExecutionError::ResponseTooLarge
        }
        _ => verification_failed("result_event_builder"),
    })?;
    builder
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| OneHopSemanticExecutionError::Signing)
}

fn bind_one_hop_query_vector(
    ticket: &buzz_db::semantic_query::SemanticGraphQueryTicket,
    input: &buzz_semantic_query::SemanticQueryEncoderInput,
    encoded: Vec<buzz_semantic_query::EncodedSemanticQuery>,
) -> Result<SemanticExactQueryVector, OneHopSemanticExecutionError> {
    if encoded.len() != 1 {
        return Err(verification_failed("provider_result_count"));
    }
    let mut encoded = encoded.into_iter();
    let encoded = encoded
        .next()
        .ok_or_else(|| verification_failed("provider_result_missing"))?;
    if encoded.request_id() != input.request_id()
        || encoded.channel_id() != input.channel_id()
        || encoded.response_model() != ticket.generation.model_contract.model
    {
        return Err(verification_failed("provider_result_identity"));
    }
    SemanticExactQueryVector::new(ticket, encoded.into_provider_encoded())
        .map_err(classify_database)
}

fn classify_database(error: buzz_db::DbError) -> OneHopSemanticExecutionError {
    match error {
        buzz_db::DbError::AccessDenied(_) => OneHopSemanticExecutionError::Restricted,
        buzz_db::DbError::InvalidData(_) => verification_failed("database_result"),
        other => OneHopSemanticExecutionError::Database(other),
    }
}

fn map_one_shot(error: SemanticOneShotError) -> OneHopSemanticExecutionError {
    match error {
        SemanticOneShotError::Restricted => OneHopSemanticExecutionError::Restricted,
        SemanticOneShotError::Unavailable => OneHopSemanticExecutionError::Unavailable,
        SemanticOneShotError::Busy => OneHopSemanticExecutionError::Busy {
            retry_after_seconds: None,
        },
        SemanticOneShotError::Conflict => OneHopSemanticExecutionError::Conflict,
        SemanticOneShotError::Timeout => OneHopSemanticExecutionError::Timeout,
        SemanticOneShotError::VerificationFailed => verification_failed("one_shot_fence"),
        SemanticOneShotError::Database(error) => OneHopSemanticExecutionError::Database(error),
    }
}

fn map_provider(error: SemanticGraphQueryError) -> OneHopSemanticExecutionError {
    match error {
        SemanticGraphQueryError::ProviderRateLimited {
            retry_after_seconds,
        } => OneHopSemanticExecutionError::Busy {
            retry_after_seconds: retry_after_seconds.filter(|value| (1..=3_600).contains(value)),
        },
        SemanticGraphQueryError::ProviderResponse => verification_failed("provider_response"),
        other => OneHopSemanticExecutionError::Provider(other),
    }
}

fn verification_failed(stage: &'static str) -> OneHopSemanticExecutionError {
    tracing::warn!(
        verification_stage = stage,
        "One-hop semantic verification failed"
    );
    OneHopSemanticExecutionError::VerificationFailed
}

#[cfg(test)]
mod tests {
    use super::{classify_database, map_provider, OneHopSemanticExecutionError};

    #[test]
    fn public_failures_are_content_free() {
        for error in [
            OneHopSemanticExecutionError::InvalidRequest,
            OneHopSemanticExecutionError::Restricted,
            OneHopSemanticExecutionError::Unavailable,
            OneHopSemanticExecutionError::Busy {
                retry_after_seconds: Some(3),
            },
            OneHopSemanticExecutionError::Conflict,
            OneHopSemanticExecutionError::Timeout,
            OneHopSemanticExecutionError::NotFound,
            OneHopSemanticExecutionError::ScopeTooLarge,
            OneHopSemanticExecutionError::HyperedgeTooLarge,
            OneHopSemanticExecutionError::ResponseTooLarge,
            OneHopSemanticExecutionError::VerificationFailed,
            OneHopSemanticExecutionError::Signing,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains("query="));
            assert!(!rendered.contains("scope="));
        }
    }

    #[test]
    fn canonical_and_provider_response_corruption_is_not_retryable_unavailability() {
        assert!(matches!(
            classify_database(buzz_db::DbError::InvalidData(
                "canonical preview mismatch".to_owned()
            )),
            OneHopSemanticExecutionError::VerificationFailed
        ));
        assert!(matches!(
            map_provider(buzz_semantic_query::SemanticGraphQueryError::ProviderResponse),
            OneHopSemanticExecutionError::VerificationFailed
        ));
    }
}
