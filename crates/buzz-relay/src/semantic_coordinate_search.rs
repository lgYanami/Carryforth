//! One-shot natural-language Project Context Coordinate search.
//!
//! This orchestration intentionally does not call semantic graph root
//! selection or traversal. It emits one Provider input, obtains one vector,
//! ranks current active-edge Coordinates in one repeatable-read snapshot, and
//! signs one response-only Event after current release authorization passes.

use std::future::Future;
use std::time::{Duration, Instant};

use buzz_db::semantic_coordinate_search::SemanticCoordinateSearchVector;
use buzz_db::semantic_query::{
    SemanticGraphQueryEgressConfirmation, SemanticGraphQueryEgressConfirmationRequest,
    SemanticGraphQueryEgressRequest, SemanticGraphQueryEgressReservation,
    SemanticGraphQueryReleaseConfirmation, SemanticGraphQueryReleaseRequest,
    SemanticGraphReadTimeouts,
};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    build_coordinate_search_encoder_input, derive_coordinate_search_http_request_binding,
    CoordinateSearchError, ProjectContextCoordinateSearchObservations,
    ProjectContextCoordinateSearchQuery, ProjectContextCoordinateSearchResult,
    SemanticGraphQueryError, MAX_COORDINATE_SEARCH_WALL_TIME_MS,
};
use chrono::Utc;
use nostr::Event;

use crate::state::AppState;

/// Closed, content-free failures for the Coordinate-search HTTP surface.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CoordinateSearchExecutionError {
    #[error("Coordinate-search request is invalid")]
    InvalidRequest,
    #[error("Coordinate-search caller is no longer authorized")]
    Restricted,
    #[error("Coordinate-search runtime is unavailable")]
    Unavailable,
    #[error("Coordinate-search process or Provider admission is busy")]
    Busy,
    #[error("Coordinate-search generation or graph snapshot changed")]
    Conflict,
    #[error("Coordinate-search deadline exceeded")]
    Timeout,
    #[error("Coordinate-search database operation failed")]
    Database(#[source] buzz_db::DbError),
    #[error("Coordinate-search Provider operation failed")]
    Provider(#[source] SemanticGraphQueryError),
    #[error("Coordinate-search contract operation failed")]
    Contract(#[source] CoordinateSearchError),
    #[error("Coordinate-search result signing failed")]
    Signing,
}

/// Execute one authenticated Coordinate search and return its signed Event.
pub(crate) async fn execute_project_context_coordinate_search(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextCoordinateSearchQuery,
) -> Result<Event, CoordinateSearchExecutionError> {
    let query = query
        .validate_and_canonicalize()
        .map_err(CoordinateSearchExecutionError::Contract)?;
    if query.project_id != *community_id.as_uuid() {
        return Err(CoordinateSearchExecutionError::InvalidRequest);
    }
    if !state
        .config
        .project_context_coordinate_search_http_available
    {
        return Err(CoordinateSearchExecutionError::Unavailable);
    }

    let deadline =
        Instant::now() + Duration::from_millis(u64::from(MAX_COORDINATE_SEARCH_WALL_TIME_MS));
    let _process_permit = state
        .semantic_graph_query_semaphore
        .clone()
        .try_acquire_owned()
        .map_err(|_| CoordinateSearchExecutionError::Busy)?;
    let provider = state
        .semantic_provider()
        .ok()
        .flatten()
        .ok_or(CoordinateSearchExecutionError::Unavailable)?;
    let reader_pubkey = authenticated_caller.to_bytes();
    let relay_pubkey = state.relay_keypair.public_key();

    let ticket = before_deadline(
        deadline,
        state
            .db
            .semantic_graph_query_ticket(community_id, &reader_pubkey, &relay_pubkey),
    )
    .await?
    .map_err(classify_database)?;
    if provider.source_contract() != &ticket.generation.model_contract {
        return Err(CoordinateSearchExecutionError::Unavailable);
    }
    let encoder_input = build_coordinate_search_encoder_input(&query)
        .map_err(CoordinateSearchExecutionError::Contract)?;

    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(CoordinateSearchExecutionError::Timeout)?;
    let latest_start_at = Utc::now()
        + chrono::Duration::from_std(remaining)
            .map_err(|_| CoordinateSearchExecutionError::Timeout)?;
    let reservation = before_deadline(
        deadline,
        state
            .db
            .reserve_semantic_graph_query_egress(SemanticGraphQueryEgressRequest {
                expected_ticket: &ticket,
                reader_pubkey: &reader_pubkey,
                expected_projection_pubkey: &relay_pubkey,
                expected_contexts: &[],
                provider: &ticket.generation.model_contract.provider,
                interval: state.config.semantic_worker.request_interval,
                latest_start_at,
            }),
    )
    .await?
    .map_err(classify_database)?;
    let provider_reservation = match reservation {
        SemanticGraphQueryEgressReservation::Reserved(reservation) => reservation,
        SemanticGraphQueryEgressReservation::Busy => {
            return Err(CoordinateSearchExecutionError::Busy);
        }
        SemanticGraphQueryEgressReservation::ContextChanged => {
            return Err(CoordinateSearchExecutionError::Conflict);
        }
        SemanticGraphQueryEgressReservation::Unavailable => {
            return Err(CoordinateSearchExecutionError::Unavailable);
        }
    };
    let (wait, reserved_generation, reserved_context_digest) = provider_reservation.into_parts();
    before_deadline(deadline, tokio::time::sleep(wait)).await?;

    let routing_trust = crate::semantic_fleet::semantic_graph_query_routing_trust(state)
        .map_err(|_| CoordinateSearchExecutionError::Unavailable)?;
    let egress = before_deadline(
        deadline,
        state
            .db
            .confirm_semantic_graph_query_egress(SemanticGraphQueryEgressConfirmationRequest {
                expected_ticket: &ticket,
                reader_pubkey: &reader_pubkey,
                expected_projection_pubkey: &relay_pubkey,
                expected_contexts: &[],
                routing_trust,
            }),
    )
    .await?
    .map_err(classify_database)?;
    let egress_permit = match egress {
        SemanticGraphQueryEgressConfirmation::Permitted(permit) => permit,
        SemanticGraphQueryEgressConfirmation::ContextChanged => {
            return Err(CoordinateSearchExecutionError::Conflict);
        }
        SemanticGraphQueryEgressConfirmation::FleetUnavailable
        | SemanticGraphQueryEgressConfirmation::Unavailable => {
            return Err(CoordinateSearchExecutionError::Unavailable);
        }
    };
    let (permitted_generation, permitted_context_digest) = egress_permit.into_parts();
    if permitted_generation != reserved_generation
        || permitted_context_digest != reserved_context_digest
    {
        return Err(CoordinateSearchExecutionError::Conflict);
    }

    metrics::histogram!("carryforth_coordinate_search_provider_input_bytes")
        .record(encoder_input.text().len() as f64);
    let encoded = before_deadline(deadline, provider.encode_coordinate_search(&encoder_input))
        .await?
        .map_err(CoordinateSearchExecutionError::Provider)?;
    if encoded.request_id() != query.request_id {
        return Err(CoordinateSearchExecutionError::Conflict);
    }
    let query_vector =
        SemanticCoordinateSearchVector::new(&ticket, encoded).map_err(classify_database)?;

    let mut read = before_deadline(
        deadline,
        state.db.begin_semantic_graph_read(
            &ticket,
            &reader_pubkey,
            relay_pubkey,
            SemanticGraphReadTimeouts::default(),
        ),
    )
    .await?
    .map_err(classify_database)?;
    let batch = before_deadline(
        deadline,
        read.search_coordinate_starts(&query_vector, query.limit),
    )
    .await?
    .map_err(classify_database)?;
    let snapshot_ticket = read.ticket().clone();
    let snapshot_projection_generation = snapshot_ticket.projection_generation;
    before_deadline(deadline, read.commit())
        .await?
        .map_err(classify_database)?;

    let release = before_deadline(
        deadline,
        state
            .db
            .confirm_semantic_graph_query_release(SemanticGraphQueryReleaseRequest {
                community_id,
                reader_pubkey: &reader_pubkey,
                expected_projection_pubkey: &relay_pubkey,
                expected_snapshot: Some(&snapshot_ticket),
                routing_trust,
            }),
    )
    .await?
    .map_err(classify_database)?;
    match release {
        SemanticGraphQueryReleaseConfirmation::Permitted(_permit) => {}
        SemanticGraphQueryReleaseConfirmation::Denied => {
            return Err(CoordinateSearchExecutionError::Restricted);
        }
        SemanticGraphQueryReleaseConfirmation::SnapshotChanged => {
            return Err(CoordinateSearchExecutionError::Conflict);
        }
        SemanticGraphQueryReleaseConfirmation::FleetUnavailable => {
            return Err(CoordinateSearchExecutionError::Unavailable);
        }
    }

    let request_binding_digest = derive_coordinate_search_http_request_binding(
        query.project_id,
        &reader_pubkey,
        Digest32::from_bytes(nip98_auth_event_id),
        exact_authenticated_body,
    )
    .map_err(CoordinateSearchExecutionError::Contract)?;
    let result = ProjectContextCoordinateSearchResult {
        request_id: query.request_id,
        project_id: query.project_id,
        request_binding_digest,
        observations: ProjectContextCoordinateSearchObservations {
            semantic_generation_id: batch.snapshot.generation_id,
            embedding_space_fence: batch.snapshot.query_fences.embedding_space_fence,
            query_contract_digest: query_vector.query_contract_digest(),
            projection_generation: snapshot_projection_generation,
            project_context_revision: batch.snapshot.project_context_revision,
            snapshot_observed_at: batch.snapshot.observed_at,
        },
        coordinates: batch.coordinates,
        truncated: batch.truncated,
    };
    result
        .validate_for_request(&query)
        .map_err(CoordinateSearchExecutionError::Contract)?;
    let builder =
        buzz_sdk::semantic_coordinate_search::build_project_context_coordinate_search_result(
            &result,
            &authenticated_caller,
        )
        .map_err(|_| CoordinateSearchExecutionError::Signing)?;
    builder
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| CoordinateSearchExecutionError::Signing)
}

async fn before_deadline<T, F>(
    deadline: Instant,
    future: F,
) -> Result<T, CoordinateSearchExecutionError>
where
    F: Future<Output = T>,
{
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
        .await
        .map_err(|_| CoordinateSearchExecutionError::Timeout)
}

fn classify_database(error: buzz_db::DbError) -> CoordinateSearchExecutionError {
    match error {
        buzz_db::DbError::AccessDenied(_) => CoordinateSearchExecutionError::Restricted,
        other => CoordinateSearchExecutionError::Database(other),
    }
}

#[cfg(test)]
mod tests {
    use super::CoordinateSearchExecutionError;

    #[test]
    fn public_failures_are_content_free() {
        for error in [
            CoordinateSearchExecutionError::InvalidRequest,
            CoordinateSearchExecutionError::Restricted,
            CoordinateSearchExecutionError::Unavailable,
            CoordinateSearchExecutionError::Busy,
            CoordinateSearchExecutionError::Conflict,
            CoordinateSearchExecutionError::Timeout,
            CoordinateSearchExecutionError::Signing,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains("query="));
            assert!(!rendered.contains("problem="));
        }
    }
}
