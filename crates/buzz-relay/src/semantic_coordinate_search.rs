//! One-shot natural-language Project Context Coordinate search.
//!
//! This orchestration intentionally does not call semantic graph root
//! selection or traversal. It emits one Provider input, obtains one vector,
//! ranks current active-edge Coordinates in one repeatable-read snapshot, and
//! signs one response-only Event after current release authorization passes.

use buzz_db::semantic_coordinate_search::SemanticCoordinateSearchVector;
use buzz_db::semantic_query::{SemanticGraphQueryTicket, SemanticGraphReadTimeouts};
use buzz_semantic::Digest32;
use buzz_semantic_query::{
    build_coordinate_search_encoder_input, derive_coordinate_search_http_request_binding,
    derive_coordinate_search_v2_http_request_binding, CoordinateSearchError,
    ProjectContextCoordinateSearchObservations, ProjectContextCoordinateSearchQuery,
    ProjectContextCoordinateSearchResult, SemanticGraphQueryError,
    MAX_COORDINATE_SEARCH_WALL_TIME_MS,
};
use nostr::Event;

use crate::semantic_one_shot::{
    read_snapshot_transient, SemanticOneShotEncodeFailure, SemanticOneShotError,
    SemanticOneShotExecution,
};
use crate::semantic_query_runtime::{
    record_vector_reuse, ProviderRetryRoute, SemanticDeadlineWindow, SemanticOperationAttemptClass,
    SemanticVectorReuseOutcome,
};
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
    let mut execution = SemanticOneShotExecution::prepare(
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
    let encoded = match execution
        .encode_with_retry(ProviderRetryRoute::R4, |provider| {
            provider.encode_coordinate_search_tracked(&encoder_input)
        })
        .await
    {
        Ok(encoded) => encoded,
        Err(SemanticOneShotEncodeFailure::DeadlineExceeded)
        | Err(SemanticOneShotEncodeFailure::Cancelled(_)) => {
            return Err(CoordinateSearchExecutionError::Timeout);
        }
        Err(SemanticOneShotEncodeFailure::Provider(tracked)) => {
            return Err(CoordinateSearchExecutionError::Provider(tracked.error));
        }
        Err(SemanticOneShotEncodeFailure::FreshPlan(error)) => {
            return Err(map_one_shot(error));
        }
    };
    if encoded.request_id() != query.request_id {
        return Err(CoordinateSearchExecutionError::Conflict);
    }
    let query_vector = SemanticCoordinateSearchVector::new(execution.ticket(), encoded)
        .map_err(classify_database)?;

    // R4 items 4 and 6: one classified read transient reopens the short
    // snapshot and reuses the exact-compatible bound vector. The loop is
    // bounded by the ledger's single restart; every other failure keeps its
    // frozen projection. The old repeatable-read transaction is dropped
    // before the restart (plan §4.5), and the reopened read re-fences the
    // ticket while the search re-validates the vector against it.
    let (batch, snapshot_ticket, snapshot_projection_generation) = loop {
        match coordinate_short_snapshot(state, &execution, &reader_pubkey, &query_vector, &query)
            .await
        {
            ShortSnapshotOutcome::Ranked(payload) => {
                let (batch, snapshot_ticket) = *payload;
                let snapshot_projection_generation = snapshot_ticket.projection_generation;
                break (batch, snapshot_ticket, snapshot_projection_generation);
            }
            ShortSnapshotOutcome::Transient(db_error) => {
                if !execution.read_transient_restart_available() {
                    return Err(classify_database(db_error));
                }
                execution
                    .begin_read_transient_restart()
                    .map_err(map_one_shot)?;
                record_vector_reuse(
                    SemanticOperationAttemptClass::OneShot,
                    SemanticVectorReuseOutcome::Reused,
                );
            }
            ShortSnapshotOutcome::Failed(error) => return Err(error),
        }
    };
    // F2 item 1: the unsigned result, its request binding, its response cap,
    // and its canonical validation all finish before the release is
    // confirmed, so a contract or size failure can never consume a release
    // permit or latch `Finalizing` for a result that was never valid.
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
    let release_permit = execution
        .confirm_release(&snapshot_ticket)
        .await
        .map_err(map_one_shot)?;
    // The confirmed permit moves by value into the single synchronous
    // signer: the closure runs with no intervening await, consumes the
    // permit whether it signs or fails, and the §4.1 post-check discards the
    // signed Event instead of sending it when cancellation or the deadline
    // arrived during that work.
    let signed = execution
        .sign_released(release_permit, || {
            builder.sign_with_keys(&state.relay_keypair)
        })
        .map_err(map_one_shot)?
        .map_err(|_| CoordinateSearchExecutionError::Signing)?;
    Ok(signed)
}

/// Outcome of one short-snapshot attempt for the Coordinate-search surface.
enum ShortSnapshotOutcome {
    /// The ranked batch with its closed snapshot ticket.
    Ranked(
        Box<(
            buzz_db::semantic_coordinate_search::SemanticCoordinateSearchBatch,
            SemanticGraphQueryTicket,
        )>,
    ),
    /// A classified read transient; the caller may consume its single
    /// restart budget and reopen the snapshot.
    Transient(buzz_db::DbError),
    /// A terminal failure with its frozen public projection.
    Failed(CoordinateSearchExecutionError),
}

/// Open one short repeatable-read snapshot, rank, and close it.
///
/// A classified transient surfaces as [`ShortSnapshotOutcome::Transient`]
/// only after the still-open transaction was explicitly dropped, which is the
/// plan §4.5 precondition for handing control back to the operation.
async fn coordinate_short_snapshot(
    state: &AppState,
    execution: &SemanticOneShotExecution<'_>,
    reader_pubkey: &[u8],
    query_vector: &SemanticCoordinateSearchVector,
    query: &ProjectContextCoordinateSearchQuery,
) -> ShortSnapshotOutcome {
    let mut read = match execution
        .before_deadline(
            SemanticDeadlineWindow::SnapshotClose,
            state.db.begin_semantic_graph_read(
                execution.ticket(),
                reader_pubkey,
                execution.relay_pubkey(),
                SemanticGraphReadTimeouts::default(),
            ),
        )
        .await
    {
        Ok(Ok(read)) => read,
        Ok(Err(db_error)) => {
            return if read_snapshot_transient(&db_error) {
                ShortSnapshotOutcome::Transient(db_error)
            } else {
                ShortSnapshotOutcome::Failed(classify_database(db_error))
            };
        }
        Err(_) => return ShortSnapshotOutcome::Failed(CoordinateSearchExecutionError::Timeout),
    };
    let search = async {
        match query.coordinate_types.as_ref() {
            Some(coordinate_types) => {
                read.search_coordinate_starts_filtered(query_vector, coordinate_types, query.limit)
                    .await
            }
            None => {
                read.search_coordinate_starts(query_vector, query.limit)
                    .await
            }
        }
    };
    let batch = match execution
        .before_deadline(SemanticDeadlineWindow::SnapshotClose, search)
        .await
    {
        Ok(Ok(batch)) => batch,
        Ok(Err(db_error)) => {
            // The failed read transaction is dropped before control returns.
            drop(read);
            return if read_snapshot_transient(&db_error) {
                ShortSnapshotOutcome::Transient(db_error)
            } else {
                ShortSnapshotOutcome::Failed(classify_database(db_error))
            };
        }
        Err(_) => return ShortSnapshotOutcome::Failed(CoordinateSearchExecutionError::Timeout),
    };
    let snapshot_ticket = read.ticket().clone();
    match execution
        .before_deadline(SemanticDeadlineWindow::SnapshotClose, read.commit())
        .await
    {
        Ok(Ok(())) => ShortSnapshotOutcome::Ranked(Box::new((batch, snapshot_ticket))),
        Ok(Err(db_error)) => {
            if read_snapshot_transient(&db_error) {
                ShortSnapshotOutcome::Transient(db_error)
            } else {
                ShortSnapshotOutcome::Failed(classify_database(db_error))
            }
        }
        Err(_) => ShortSnapshotOutcome::Failed(CoordinateSearchExecutionError::Timeout),
    }
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
