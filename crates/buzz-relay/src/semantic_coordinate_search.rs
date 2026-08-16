//! One-shot natural-language Project Context Coordinate search.
//!
//! This orchestration intentionally does not call semantic graph root
//! selection or traversal. It emits one Provider input, obtains one vector,
//! ranks current active-edge Coordinates in one repeatable-read snapshot, and
//! signs one response-only Event after current release authorization passes.

use buzz_db::semantic_coordinate_search::SemanticCoordinateSearchVector;
use buzz_db::semantic_query::SemanticGraphReadTimeouts;
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    build_coordinate_search_encoder_input, derive_coordinate_search_http_request_binding,
    derive_coordinate_search_v2_http_request_binding, CoordinateSearchError,
    ProjectContextCoordinateSearchObservations, ProjectContextCoordinateSearchQuery,
    ProjectContextCoordinateSearchResult, SemanticGraphQueryError,
    MAX_COORDINATE_SEARCH_WALL_TIME_MS,
};
use nostr::Event;

use crate::semantic_one_shot::{SemanticOneShotError, SemanticOneShotExecution};
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
    execute_coordinate_search(
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

/// Execute one authenticated filtered Coordinate-search v2 request.
pub(crate) async fn execute_project_context_coordinate_search_v2(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextCoordinateSearchQuery,
) -> Result<Event, CoordinateSearchExecutionError> {
    execute_coordinate_search(
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

async fn execute_coordinate_search(
    state: &AppState,
    community_id: buzz_core::CommunityId,
    authenticated_caller: nostr::PublicKey,
    nip98_auth_event_id: [u8; 32],
    exact_authenticated_body: &[u8],
    query: ProjectContextCoordinateSearchQuery,
    filtered: bool,
) -> Result<Event, CoordinateSearchExecutionError> {
    let query = query
        .validate_and_canonicalize()
        .map_err(CoordinateSearchExecutionError::Contract)?;
    if filtered != query.coordinate_types.is_some() {
        return Err(CoordinateSearchExecutionError::InvalidRequest);
    }
    if query.project_id != *community_id.as_uuid() {
        return Err(CoordinateSearchExecutionError::InvalidRequest);
    }
    if !state
        .config
        .project_context_coordinate_search_http_available
    {
        return Err(CoordinateSearchExecutionError::Unavailable);
    }

    let reader_pubkey = authenticated_caller.to_bytes();
    let execution = SemanticOneShotExecution::prepare(
        state,
        community_id,
        &reader_pubkey,
        state
            .config
            .project_context_coordinate_search_http_available,
        MAX_COORDINATE_SEARCH_WALL_TIME_MS,
    )
    .await
    .map_err(map_one_shot)?;
    let encoder_input = build_coordinate_search_encoder_input(&query)
        .map_err(CoordinateSearchExecutionError::Contract)?;

    metrics::histogram!("carryforth_coordinate_search_provider_input_bytes")
        .record(encoder_input.text().len() as f64);
    let encoded = execution
        .before_deadline(
            execution
                .provider()
                .encode_coordinate_search(&encoder_input),
        )
        .await
        .map_err(map_one_shot)?
        .map_err(CoordinateSearchExecutionError::Provider)?;
    if encoded.request_id() != query.request_id {
        return Err(CoordinateSearchExecutionError::Conflict);
    }
    let query_vector = SemanticCoordinateSearchVector::new(execution.ticket(), encoded)
        .map_err(classify_database)?;

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
    let search = async {
        match query.coordinate_types.as_ref() {
            Some(coordinate_types) => {
                read.search_coordinate_starts_filtered(&query_vector, coordinate_types, query.limit)
                    .await
            }
            None => {
                read.search_coordinate_starts(&query_vector, query.limit)
                    .await
            }
        }
    };
    let batch = execution
        .before_deadline(search)
        .await
        .map_err(map_one_shot)?
        .map_err(classify_database)?;
    let snapshot_ticket = read.ticket().clone();
    let snapshot_projection_generation = snapshot_ticket.projection_generation;
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
        derive_coordinate_search_v2_http_request_binding(
            query.project_id,
            &reader_pubkey,
            Digest32::from_bytes(nip98_auth_event_id),
            exact_authenticated_body,
        )
    } else {
        derive_coordinate_search_http_request_binding(
            query.project_id,
            &reader_pubkey,
            Digest32::from_bytes(nip98_auth_event_id),
            exact_authenticated_body,
        )
    }
    .map_err(CoordinateSearchExecutionError::Contract)?;
    let result = ProjectContextCoordinateSearchResult {
        request_id: query.request_id,
        project_id: query.project_id,
        request_binding_digest,
        observations: ProjectContextCoordinateSearchObservations {
            semantic_generation_id: batch.snapshot.generation_id,
            embedding_space_fence: batch.snapshot.query_fences.embedding_space_fence,
            query_contract_digest: query_vector.query_contract_digest(),
            coordinate_types: batch.coordinate_types,
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
    let builder = if filtered {
        buzz_sdk::semantic_coordinate_search::build_project_context_coordinate_search_v2_result(
            &result,
            &authenticated_caller,
        )
    } else {
        buzz_sdk::semantic_coordinate_search::build_project_context_coordinate_search_result(
            &result,
            &authenticated_caller,
        )
    }
    .map_err(|_| CoordinateSearchExecutionError::Signing)?;
    builder
        .sign_with_keys(&state.relay_keypair)
        .map_err(|_| CoordinateSearchExecutionError::Signing)
}

fn classify_database(error: buzz_db::DbError) -> CoordinateSearchExecutionError {
    match error {
        buzz_db::DbError::AccessDenied(_) => CoordinateSearchExecutionError::Restricted,
        other => CoordinateSearchExecutionError::Database(other),
    }
}

fn map_one_shot(error: SemanticOneShotError) -> CoordinateSearchExecutionError {
    match error {
        SemanticOneShotError::Restricted => CoordinateSearchExecutionError::Restricted,
        SemanticOneShotError::Unavailable => CoordinateSearchExecutionError::Unavailable,
        SemanticOneShotError::Busy => CoordinateSearchExecutionError::Busy,
        SemanticOneShotError::Conflict => CoordinateSearchExecutionError::Conflict,
        SemanticOneShotError::Timeout => CoordinateSearchExecutionError::Timeout,
        SemanticOneShotError::VerificationFailed => CoordinateSearchExecutionError::Unavailable,
        SemanticOneShotError::Database(error) => CoordinateSearchExecutionError::Database(error),
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
