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
    SemanticGraphQueryReleaseConfirmation, SemanticGraphQueryReleaseRequest,
    SemanticGraphQueryTicket,
};
use buzz_semantic_query::SemanticGraphQueryRoutingTrust;
use nostr::PublicKey;
use tokio::sync::OwnedSemaphorePermit;

use crate::semantic_provider::VolcengineSemanticProvider;
use crate::semantic_query_runtime::{
    execute_provider_egress, ProviderEgressObservation, ProviderEgressPlan, SemanticDeadlineWindow,
    SemanticDeadlineWindows, SemanticEncodeOnceFailure, SemanticExecutionContext,
    SemanticOperationAttemptClass, SemanticProviderEgressFailure,
};
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
    context: SemanticExecutionContext,
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

        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(deadline),
        );
        // Unreachable under the R2 zero-policy single attempt; kept so the
        // counting ledger owns the operation-attempt dimension from R4 on.
        context
            .ledger()
            .begin_operation_attempt()
            .map_err(|_| SemanticOneShotError::Busy)?;
        let routing_trust = execute_provider_egress(ProviderEgressPlan {
            state,
            context: &context,
            ticket: &ticket,
            reader_pubkey,
            expected_contexts: &[],
            observation: ProviderEgressObservation::Silent,
        })
        .await
        .map_err(map_egress_failure)?;

        Ok(Self {
            state,
            deadline,
            ticket,
            reader_pubkey: reader_pubkey.to_vec(),
            relay_pubkey,
            routing_trust,
            provider,
            context,
            _process_permit: process_permit,
        })
    }

    /// Run exactly one Provider invocation inside this request's work window.
    ///
    /// This is the only sanctioned Provider handoff for a one-shot surface;
    /// every other database step keeps using [`Self::before_deadline`].
    pub(crate) async fn encode_once<T, E, F>(
        &self,
        future: F,
    ) -> Result<T, SemanticEncodeOnceFailure<E>>
    where
        F: Future<Output = Result<T, E>>,
    {
        crate::semantic_query_runtime::encode_once(
            self.context.windows().window(SemanticDeadlineWindow::Work),
            ProviderEgressObservation::Silent,
            future,
        )
        .await
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
        // Unreachable while R2 keeps the single frozen release attempt; kept
        // so the counting ledger owns the release dimension from R4 on.
        self.context
            .ledger()
            .begin_release_confirmation()
            .map_err(|_| SemanticOneShotError::Busy)?;
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

/// Map one neutral shared-executor outcome onto the frozen one-shot envelope
/// error.
///
/// This table preserves every public mapping of the pre-R2 inline envelope.
/// `ReservationContractViolated` and `AttemptLedgerExhausted` have no
/// pre-R2 counterpart: both are unreachable per the database egress contract
/// and the R2 zero-policy single attempt, so they map onto the closest
/// existing closed failures until R4 owns real retry decisions.
fn map_egress_failure(failure: SemanticProviderEgressFailure) -> SemanticOneShotError {
    match failure {
        SemanticProviderEgressFailure::DeadlineExceeded
        | SemanticProviderEgressFailure::LatestStartUnrepresentable => {
            SemanticOneShotError::Timeout
        }
        SemanticProviderEgressFailure::Database(error) => classify_database(error),
        SemanticProviderEgressFailure::AdmissionBusy
        | SemanticProviderEgressFailure::AttemptLedgerExhausted(_) => SemanticOneShotError::Busy,
        SemanticProviderEgressFailure::ContextChanged => SemanticOneShotError::Conflict,
        SemanticProviderEgressFailure::FleetUnavailable
        | SemanticProviderEgressFailure::ProviderUnavailable => SemanticOneShotError::Unavailable,
        SemanticProviderEgressFailure::ReservationContractViolated => {
            SemanticOneShotError::VerificationFailed
        }
        SemanticProviderEgressFailure::PermitContractViolated => SemanticOneShotError::Conflict,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        map_egress_failure, SemanticOneShotError, SemanticProviderEgressFailure as EgressFailure,
    };

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

    #[test]
    fn egress_failures_keep_the_frozen_one_shot_public_errors() {
        use SemanticOneShotError as OneShot;
        assert!(matches!(
            map_egress_failure(EgressFailure::DeadlineExceeded),
            OneShot::Timeout
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::LatestStartUnrepresentable),
            OneShot::Timeout
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::Database(buzz_db::DbError::AccessDenied(
                "denied".to_owned()
            ))),
            OneShot::Restricted
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::Database(buzz_db::DbError::InvalidData(
                "invalid".to_owned()
            ))),
            OneShot::VerificationFailed
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::Database(buzz_db::DbError::AuthEventRejected)),
            OneShot::Database(_)
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::AdmissionBusy),
            OneShot::Busy
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::ContextChanged),
            OneShot::Conflict
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::FleetUnavailable),
            OneShot::Unavailable
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::ProviderUnavailable),
            OneShot::Unavailable
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::PermitContractViolated),
            OneShot::Conflict
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::ReservationContractViolated),
            OneShot::VerificationFailed
        ));
        assert!(matches!(
            map_egress_failure(EgressFailure::AttemptLedgerExhausted(
                crate::semantic_query_runtime::SemanticAttemptExhausted::ProviderAttempts,
            )),
            OneShot::Busy
        ));
    }
}
