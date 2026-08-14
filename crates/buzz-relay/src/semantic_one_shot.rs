//! Shared safety envelope for one-shot semantic HTTP operations.
//!
//! This module owns only the common process admission, authorized ticket,
//! physical Provider reservation, final egress confirmation, deadline, and
//! result-release fence. Query text, Provider encoding shape, database
//! ranking, and public result contracts remain owned by each closed surface.

use std::future::Future;
use std::time::{Duration, Instant};

use buzz_core::CommunityId;
use buzz_db::semantic_query::{
    SemanticGraphQueryEgressConfirmation, SemanticGraphQueryEgressConfirmationRequest,
    SemanticGraphQueryEgressRequest, SemanticGraphQueryEgressReservation,
    SemanticGraphQueryReleaseConfirmation, SemanticGraphQueryReleaseRequest,
    SemanticGraphQueryTicket,
};
use buzz_semantic_query::SemanticGraphQueryRoutingTrust;
use chrono::Utc;
use nostr::PublicKey;
use tokio::sync::OwnedSemaphorePermit;

use crate::semantic_provider::VolcengineSemanticProvider;
use crate::state::AppState;

/// Content-free failures shared by one-shot semantic surfaces.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticOneShotError {
    #[error("Semantic one-shot caller is no longer authorized")]
    Restricted,
    #[error("Semantic one-shot runtime is unavailable")]
    Unavailable,
    #[error("Semantic one-shot process or Provider admission is busy")]
    Busy,
    #[error("Semantic one-shot generation or graph snapshot changed")]
    Conflict,
    #[error("Semantic one-shot deadline exceeded")]
    Timeout,
    #[error("Semantic one-shot verification failed")]
    VerificationFailed,
    #[error("Semantic one-shot database operation failed")]
    Database(#[source] buzz_db::DbError),
}

/// Authorized, single-use execution state immediately before Provider egress.
pub(crate) struct SemanticOneShotExecution<'a> {
    state: &'a AppState,
    deadline: Instant,
    ticket: SemanticGraphQueryTicket,
    reader_pubkey: Vec<u8>,
    relay_pubkey: PublicKey,
    routing_trust: SemanticGraphQueryRoutingTrust<'a>,
    provider: &'a VolcengineSemanticProvider,
    _process_permit: OwnedSemaphorePermit,
}

impl<'a> SemanticOneShotExecution<'a> {
    /// Prepare one request through the shared no-wait Provider egress fence.
    pub(crate) async fn prepare(
        state: &'a AppState,
        community_id: CommunityId,
        reader_pubkey: &[u8],
        deployment_master: bool,
        maximum_wall_time_ms: u32,
    ) -> Result<Self, SemanticOneShotError> {
        if !deployment_master {
            return Err(SemanticOneShotError::Unavailable);
        }
        let deadline = Instant::now() + Duration::from_millis(u64::from(maximum_wall_time_ms));
        let process_permit = state
            .semantic_graph_query_semaphore
            .clone()
            .try_acquire_owned()
            .map_err(|_| SemanticOneShotError::Busy)?;
        let provider = state
            .semantic_provider()
            .ok()
            .flatten()
            .ok_or(SemanticOneShotError::Unavailable)?;
        let relay_pubkey = state.relay_keypair.public_key();
        let ticket = before_deadline(
            deadline,
            state
                .db
                .semantic_graph_query_ticket(community_id, reader_pubkey, &relay_pubkey),
        )
        .await?
        .map_err(classify_database)?;
        if provider.source_contract() != &ticket.generation.model_contract {
            return Err(SemanticOneShotError::Unavailable);
        }

        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(SemanticOneShotError::Timeout)?;
        let latest_start_at = Utc::now()
            + chrono::Duration::from_std(remaining).map_err(|_| SemanticOneShotError::Timeout)?;
        let reservation = before_deadline(
            deadline,
            state
                .db
                .reserve_semantic_graph_query_egress(SemanticGraphQueryEgressRequest {
                    expected_ticket: &ticket,
                    reader_pubkey,
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
            SemanticGraphQueryEgressReservation::Busy => return Err(SemanticOneShotError::Busy),
            SemanticGraphQueryEgressReservation::ContextChanged => {
                return Err(SemanticOneShotError::Conflict);
            }
            SemanticGraphQueryEgressReservation::Unavailable => {
                return Err(SemanticOneShotError::Unavailable);
            }
        };
        let (wait, reserved_generation, reserved_context_digest) =
            provider_reservation.into_parts();
        before_deadline(deadline, tokio::time::sleep(wait)).await?;

        let routing_trust = crate::semantic_fleet::semantic_graph_query_routing_trust(state)
            .map_err(|_| SemanticOneShotError::Unavailable)?;
        let egress = before_deadline(
            deadline,
            state.db.confirm_semantic_graph_query_egress(
                SemanticGraphQueryEgressConfirmationRequest {
                    expected_ticket: &ticket,
                    reader_pubkey,
                    expected_projection_pubkey: &relay_pubkey,
                    expected_contexts: &[],
                    routing_trust,
                },
            ),
        )
        .await?
        .map_err(classify_database)?;
        let permit = match egress {
            SemanticGraphQueryEgressConfirmation::Permitted(permit) => permit,
            SemanticGraphQueryEgressConfirmation::ContextChanged => {
                return Err(SemanticOneShotError::Conflict);
            }
            SemanticGraphQueryEgressConfirmation::FleetUnavailable
            | SemanticGraphQueryEgressConfirmation::Unavailable => {
                return Err(SemanticOneShotError::Unavailable);
            }
        };
        let (permitted_generation, permitted_context_digest) = permit.into_parts();
        if permitted_generation != reserved_generation
            || permitted_context_digest != reserved_context_digest
        {
            return Err(SemanticOneShotError::Conflict);
        }

        Ok(Self {
            state,
            deadline,
            ticket,
            reader_pubkey: reader_pubkey.to_vec(),
            relay_pubkey,
            routing_trust,
            provider,
            _process_permit: process_permit,
        })
    }

    /// Active generation and topology observation authorized for egress.
    pub(crate) const fn ticket(&self) -> &SemanticGraphQueryTicket {
        &self.ticket
    }

    /// Shared Provider whose model contract exactly matches the ticket.
    pub(crate) const fn provider(&self) -> &VolcengineSemanticProvider {
        self.provider
    }

    /// Run one operation inside the request's absolute work deadline.
    pub(crate) async fn before_deadline<T, F>(&self, future: F) -> Result<T, SemanticOneShotError>
    where
        F: Future<Output = T>,
    {
        before_deadline(self.deadline, future).await
    }

    /// Recheck authorization, topology, generation, and Fleet after the
    /// snapshot closes and before a signed result can be released.
    pub(crate) async fn confirm_release(
        &self,
        snapshot: &SemanticGraphQueryTicket,
    ) -> Result<(), SemanticOneShotError> {
        let release = self
            .before_deadline(self.state.db.confirm_semantic_graph_query_release(
                SemanticGraphQueryReleaseRequest {
                    community_id: self.ticket.community_id,
                    reader_pubkey: &self.reader_pubkey,
                    expected_projection_pubkey: &self.relay_pubkey,
                    expected_snapshot: Some(snapshot),
                    routing_trust: self.routing_trust,
                },
            ))
            .await?
            .map_err(classify_database)?;
        match release {
            SemanticGraphQueryReleaseConfirmation::Permitted(_permit) => Ok(()),
            SemanticGraphQueryReleaseConfirmation::Denied => Err(SemanticOneShotError::Restricted),
            SemanticGraphQueryReleaseConfirmation::SnapshotChanged => {
                Err(SemanticOneShotError::Conflict)
            }
            SemanticGraphQueryReleaseConfirmation::FleetUnavailable => {
                Err(SemanticOneShotError::Unavailable)
            }
        }
    }

    /// Relay projection signer bound into the request/result transcript.
    pub(crate) const fn relay_pubkey(&self) -> PublicKey {
        self.relay_pubkey
    }
}

async fn before_deadline<T, F>(deadline: Instant, future: F) -> Result<T, SemanticOneShotError>
where
    F: Future<Output = T>,
{
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future)
        .await
        .map_err(|_| SemanticOneShotError::Timeout)
}

fn classify_database(error: buzz_db::DbError) -> SemanticOneShotError {
    match error {
        buzz_db::DbError::AccessDenied(_) => SemanticOneShotError::Restricted,
        buzz_db::DbError::InvalidData(_) => SemanticOneShotError::VerificationFailed,
        other => SemanticOneShotError::Database(other),
    }
}

#[cfg(test)]
mod tests {
    use super::SemanticOneShotError;

    #[test]
    fn shared_failures_are_content_free() {
        for error in [
            SemanticOneShotError::Restricted,
            SemanticOneShotError::Unavailable,
            SemanticOneShotError::Busy,
            SemanticOneShotError::Conflict,
            SemanticOneShotError::Timeout,
            SemanticOneShotError::VerificationFailed,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains("query="));
            assert!(!rendered.contains("scope="));
        }
    }
}
