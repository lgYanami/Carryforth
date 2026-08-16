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
    SemanticGraphQueryReleaseConfirmation, SemanticGraphQueryReleasePermit,
    SemanticGraphQueryReleaseRequest, SemanticGraphQueryTicket,
};
use buzz_semantic_query::{SemanticGraphQueryError, SemanticGraphQueryRoutingTrust};
use nostr::PublicKey;
use tokio::sync::OwnedSemaphorePermit;

use crate::semantic_provider::{TrackedProviderFailure, VolcengineSemanticProvider};
use crate::semantic_query_runtime::{
    encode_once, execute_provider_egress, propagate_relay_shutdown, provider_retry_backoff,
    provider_retry_decision, ProviderEgressObservation, ProviderEgressPlan, ProviderRetryDecision,
    ProviderRetryRoute, SemanticCancellationSource, SemanticDeadlineWindow,
    SemanticDeadlineWindows, SemanticEncodeOnceFailure, SemanticExecutionContext,
    SemanticLatchOutcome, SemanticOperationAttemptClass, SemanticProviderEgressFailure,
    SemanticStageAbort,
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
    ticket: SemanticGraphQueryTicket,
    reader_pubkey: Vec<u8>,
    relay_pubkey: PublicKey,
    routing_trust: SemanticGraphQueryRoutingTrust<'a>,
    provider: &'a VolcengineSemanticProvider,
    context: SemanticExecutionContext,
    _process_permit: OwnedSemaphorePermit,
}

/// Failure of the R4 bounded Provider encode after its retry policy ran.
///
/// `Provider` carries the last typed failure — with retries exhausted or
/// declined, the surface projects exactly the error its frozen single-attempt
/// path always produced. `FreshPlan` carries the one-shot error of the fresh
/// ticket or egress admission that a retry needed.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticOneShotEncodeFailure {
    #[error("semantic one-shot encode deadline exceeded")]
    DeadlineExceeded,
    #[error("semantic one-shot encode was cancelled")]
    Cancelled(SemanticCancellationSource),
    #[error("semantic one-shot encode call failed")]
    Provider(#[source] TrackedProviderFailure<SemanticGraphQueryError>),
    #[error("semantic one-shot encode fresh plan failed")]
    FreshPlan(#[source] SemanticOneShotError),
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
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(deadline),
        );
        propagate_relay_shutdown(state, &context);
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
        let ticket = context
            .run_stage(
                SemanticDeadlineWindow::Absolute,
                state
                    .db
                    .semantic_graph_query_ticket(community_id, reader_pubkey, &relay_pubkey),
            )
            .await
            .map_err(|_| SemanticOneShotError::Timeout)?
            .map_err(classify_database)?;
        if provider.source_contract() != &ticket.generation.model_contract {
            return Err(SemanticOneShotError::Unavailable);
        }

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
            ticket,
            reader_pubkey: reader_pubkey.to_vec(),
            relay_pubkey,
            routing_trust,
            provider,
            context,
            _process_permit: process_permit,
        })
    }

    /// Run the bounded Provider encode with the R4 retry policy applied.
    ///
    /// Each attempt is one sanctioned `encode_once` handoff inside the work
    /// window. When the closed policy advises a retry, this envelope sleeps
    /// the bounded backoff and assembles the fresh plan itself — a fresh
    /// authorized ticket plus a fresh reservation/confirmation through the
    /// shared executor (plan §4.3) — because only the envelope owns that
    /// state. The fresh attempt re-enters through the same admission, so the
    /// attempt ledger caps and the cancellation latch stay binding, and an
    /// exhausted or declined retry returns the last typed failure for the
    /// surface's frozen projection.
    pub(crate) async fn encode_with_retry<T, Fut>(
        &mut self,
        route: ProviderRetryRoute,
        mut encode: impl FnMut(&'a VolcengineSemanticProvider) -> Fut,
    ) -> Result<T, SemanticOneShotEncodeFailure>
    where
        Fut: Future<Output = Result<T, TrackedProviderFailure<SemanticGraphQueryError>>>,
    {
        loop {
            let tracked = match encode_once(
                &self.context,
                ProviderEgressObservation::Silent,
                encode(self.provider),
            )
            .await
            {
                Ok(encoded) => return Ok(encoded),
                Err(SemanticEncodeOnceFailure::DeadlineExceeded) => {
                    return Err(SemanticOneShotEncodeFailure::DeadlineExceeded);
                }
                Err(SemanticEncodeOnceFailure::Cancelled(source)) => {
                    return Err(SemanticOneShotEncodeFailure::Cancelled(source));
                }
                Err(SemanticEncodeOnceFailure::Provider(tracked)) => tracked,
            };
            match provider_retry_decision(route, tracked.failure, &self.context) {
                ProviderRetryDecision::Terminal => {
                    return Err(SemanticOneShotEncodeFailure::Provider(tracked));
                }
                ProviderRetryDecision::Retry { backoff } => {
                    if let Err(abort) = provider_retry_backoff(&self.context, backoff).await {
                        return Err(match abort {
                            SemanticStageAbort::Deadline(_) => {
                                SemanticOneShotEncodeFailure::DeadlineExceeded
                            }
                            SemanticStageAbort::Cancelled(source) => {
                                SemanticOneShotEncodeFailure::Cancelled(source)
                            }
                        });
                    }
                    self.refresh_plan_for_retry()
                        .await
                        .map_err(SemanticOneShotEncodeFailure::FreshPlan)?;
                }
            }
        }
    }

    /// Assemble the fresh plan one Provider retry needs (plan §4.3).
    ///
    /// Fresh authorization first: the ticket is re-read and the Provider
    /// contract re-checked before any reservation, then the shared executor
    /// runs its full reservation/confirmation admission for the new physical
    /// attempt. The updated ticket and routing trust stay bound into this
    /// execution so the release fence validates against the same attempt.
    async fn refresh_plan_for_retry(&mut self) -> Result<(), SemanticOneShotError> {
        let fresh = self
            .context
            .run_stage(
                SemanticDeadlineWindow::Absolute,
                self.state.db.semantic_graph_query_ticket(
                    self.ticket.community_id,
                    &self.reader_pubkey,
                    &self.relay_pubkey,
                ),
            )
            .await
            .map_err(|_| SemanticOneShotError::Timeout)?
            .map_err(classify_database)?;
        if self.provider.source_contract() != &fresh.generation.model_contract {
            return Err(SemanticOneShotError::Unavailable);
        }
        self.routing_trust = execute_provider_egress(ProviderEgressPlan {
            state: self.state,
            context: &self.context,
            ticket: &fresh,
            reader_pubkey: &self.reader_pubkey,
            expected_contexts: &[],
            observation: ProviderEgressObservation::Silent,
        })
        .await
        .map_err(map_egress_failure)?;
        self.ticket = fresh;
        Ok(())
    }

    /// Active generation and topology observation authorized for egress.
    pub(crate) const fn ticket(&self) -> &SemanticGraphQueryTicket {
        &self.ticket
    }

    /// Run one operation inside the request's absolute work deadline.
    pub(crate) async fn before_deadline<T, F>(&self, future: F) -> Result<T, SemanticOneShotError>
    where
        F: Future<Output = T>,
    {
        self.context
            .run_stage(SemanticDeadlineWindow::Absolute, future)
            .await
            .map_err(|_| SemanticOneShotError::Timeout)
    }

    /// Recheck authorization, topology, generation, and Fleet after the
    /// snapshot closes and before a signed result can be released.
    ///
    /// When the release is permitted, the single-use permit is handed to the
    /// caller and the latch moves to `Finalizing`: the immediately following
    /// synchronous signing is authorized, and [`Self::finalize_completed`]
    /// must run before the signed result may be sent. If cancellation or a
    /// deadline won while the confirmation was in flight, the permit is
    /// discarded here instead.
    ///
    /// R4 item 7: one classified release-confirmation transient may be
    /// retried in place (plan §4.5) — only the confirmation itself is redone,
    /// never scoring or packing, and only while nothing was signed and no
    /// permit was received, which is exactly this failed-database-call path.
    /// The ledger bounds the loop at two total attempts; an exhausted retry
    /// projects the last typed failure through the frozen mapping, and a
    /// fresh denial seen by the retry overrides the earlier transient (plan
    /// §4.5 fixed priority).
    pub(crate) async fn confirm_release(
        &self,
        snapshot: &SemanticGraphQueryTicket,
    ) -> Result<SemanticGraphQueryReleasePermit, SemanticOneShotError> {
        let mut last_transient: Option<buzz_db::DbError> = None;
        loop {
            if let Err(_exhausted) = self.context.ledger().begin_release_confirmation() {
                return Err(match last_transient.take() {
                    Some(db_error) => {
                        record_release_retry_decision(false);
                        classify_database(db_error)
                    }
                    // Unreachable on a fresh request ledger; kept fail-closed.
                    None => SemanticOneShotError::Busy,
                });
            }
            let release = match self
                .before_deadline(self.state.db.confirm_semantic_graph_query_release(
                    SemanticGraphQueryReleaseRequest {
                        community_id: self.ticket.community_id,
                        reader_pubkey: &self.reader_pubkey,
                        expected_projection_pubkey: &self.relay_pubkey,
                        expected_snapshot: Some(snapshot),
                        routing_trust: self.routing_trust,
                    },
                ))
                .await
            {
                Ok(release) => release,
                Err(_) => return Err(SemanticOneShotError::Timeout),
            };
            let release = match release {
                Ok(confirmed) => confirmed,
                Err(db_error) => {
                    if release_confirmation_transient(&db_error) {
                        record_release_retry_decision(true);
                        last_transient = Some(db_error);
                        continue;
                    }
                    record_release_retry_decision(false);
                    return Err(classify_database(db_error));
                }
            };
            return match release {
                SemanticGraphQueryReleaseConfirmation::Permitted(permit) => {
                    match self.context.latch().begin_finalize() {
                        SemanticLatchOutcome::Won(_) => Ok(permit),
                        // Cancellation or a deadline already won the latch: the
                        // won permit is consumed here and nothing is signed.
                        SemanticLatchOutcome::LostTerminal(_)
                        | SemanticLatchOutcome::LostToFinalizing(_) => {
                            Err(SemanticOneShotError::Timeout)
                        }
                    }
                }
                SemanticGraphQueryReleaseConfirmation::Denied => {
                    Err(SemanticOneShotError::Restricted)
                }
                SemanticGraphQueryReleaseConfirmation::SnapshotChanged => {
                    Err(SemanticOneShotError::Conflict)
                }
                SemanticGraphQueryReleaseConfirmation::FleetUnavailable => {
                    Err(SemanticOneShotError::Unavailable)
                }
            };
        }
    }

    /// Post-check the synchronous finalization and consume the release permit
    /// with the accepted Event signature.
    ///
    /// Runs after the closed surface finished building and signing its
    /// result — with no intervening await since the permit was issued. A
    /// cancellation or deadline that arrived during that synchronous work
    /// requested a discard: the signed result must be dropped, the permit is
    /// consumed anyway, and the caller observes the deadline error. Only a
    /// clean post-check completes the latch.
    pub(crate) fn finalize_completed(
        &self,
        permit: SemanticGraphQueryReleasePermit,
    ) -> Result<(), SemanticOneShotError> {
        if self
            .context
            .windows()
            .expired_window(Instant::now())
            .is_some()
        {
            let _ = self.context.deadline_expired();
        }
        if self.context.latch().discard_requested() {
            // The single-use permit is consumed with the discarded result.
            let _ = permit;
            return Err(SemanticOneShotError::Timeout);
        }
        self.context.latch().complete();
        // The single-use permit is consumed with the accepted Event signature.
        let _ = permit;
        Ok(())
    }

    /// Relay projection signer bound into the request/result transcript.
    pub(crate) const fn relay_pubkey(&self) -> PublicKey {
        self.relay_pubkey
    }

    /// Whether the single one-shot snapshot-restart budget is still available
    /// (plan §4.5 request-level counters: one restart, two total attempts).
    pub(crate) fn read_transient_restart_available(&self) -> bool {
        self.context.ledger().can_begin_operation_attempt()
    }

    /// Consume the one-shot snapshot restart after a classified read
    /// transient (plan §4.5/§4.6).
    ///
    /// The caller has already closed or dropped the old short repeatable-read
    /// transaction and reuses its exact-compatible bound vector: the reopened
    /// read revalidates the ticket under the database fence, the search
    /// revalidates the vector against that read ticket, and the release fence
    /// revalidates everything under the writer fence. No Provider slot is
    /// reserved because no Provider call happens.
    pub(crate) fn begin_read_transient_restart(&self) -> Result<(), SemanticOneShotError> {
        self.context
            .ledger()
            .begin_operation_attempt()
            .map_err(|_| SemanticOneShotError::Busy)?;
        if self.context.admit_stage().is_err() {
            return Err(SemanticOneShotError::Timeout);
        }
        Ok(())
    }
}

/// R4 item 4: classify one short-snapshot database failure as a restartable
/// read transient.
///
/// Only the closed SQLSTATE/phase allowlist qualifies (plan §4.5); every
/// other database failure keeps its frozen terminal projection.
pub(crate) fn read_snapshot_transient(error: &buzz_db::DbError) -> bool {
    matches!(
        error.semantic_failure_kind(buzz_db::SemanticDbEffectPhase::SnapshotRead),
        buzz_db::SemanticDbFailureKind::SnapshotReadTransient { .. }
    )
}

/// R4 item 7: classify one release-confirmation database failure as a
/// same-phase bounded-retry transient.
fn release_confirmation_transient(error: &buzz_db::DbError) -> bool {
    matches!(
        error.semantic_failure_kind(buzz_db::SemanticDbEffectPhase::ReleaseConfirmation),
        buzz_db::SemanticDbFailureKind::ReleaseConfirmationTransient { .. }
    )
}

/// Record one closed release-retry decision (content-free; plan §7).
fn record_release_retry_decision(retry: bool) {
    metrics::counter!(
        "buzz_semantic_release_retry_total",
        "disposition" => if retry {
            "retry_release_confirmation"
        } else {
            "terminal"
        },
    )
    .increment(1);
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
/// existing closed failures until R4 owns real retry decisions. R3 adds
/// `Cancelled`: a cancelled, disconnected, or shutting-down request keeps the
/// deadline error, the only closed one-shot surface it can observably share.
fn map_egress_failure(failure: SemanticProviderEgressFailure) -> SemanticOneShotError {
    match failure {
        SemanticProviderEgressFailure::DeadlineExceeded
        | SemanticProviderEgressFailure::LatestStartUnrepresentable => {
            SemanticOneShotError::Timeout
        }
        SemanticProviderEgressFailure::Cancelled(_) => SemanticOneShotError::Timeout,
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
        map_egress_failure, read_snapshot_transient, release_confirmation_transient,
        SemanticOneShotEncodeFailure, SemanticOneShotError,
        SemanticProviderEgressFailure as EgressFailure,
    };
    use crate::semantic_query_runtime::{
        ProviderAttemptFailure, ProviderAttemptFailureKind, ProviderHandoffCertainty,
    };
    use buzz_semantic_query::SemanticGraphQueryError;

    /// Minimal `sqlx` database error carrying one fixed SQLSTATE.
    struct StubSqlstateError(&'static str);

    impl std::fmt::Debug for StubSqlstateError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("StubSqlstateError")
        }
    }

    impl std::fmt::Display for StubSqlstateError {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("stub semantic sqlstate")
        }
    }

    impl std::error::Error for StubSqlstateError {}

    impl sqlx::error::DatabaseError for StubSqlstateError {
        fn message(&self) -> &str {
            "stub semantic sqlstate"
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(std::borrow::Cow::Borrowed(self.0))
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn db_error_with_sqlstate(code: &'static str) -> buzz_db::DbError {
        buzz_db::DbError::Sqlx(sqlx::Error::Database(Box::new(StubSqlstateError(code))))
    }

    #[test]
    fn read_snapshot_transient_accepts_only_the_classified_allowlist() {
        assert!(read_snapshot_transient(&db_error_with_sqlstate("40001")));
        assert!(read_snapshot_transient(&db_error_with_sqlstate("40P01")));
        assert!(read_snapshot_transient(&db_error_with_sqlstate("55P03")));
        assert!(read_snapshot_transient(&db_error_with_sqlstate("57014")));
        // Unlisted SQLSTATEs and non-driver failures stay terminal.
        assert!(!read_snapshot_transient(&db_error_with_sqlstate("42P01")));
        assert!(!read_snapshot_transient(&db_error_with_sqlstate("23505")));
        assert!(!read_snapshot_transient(&buzz_db::DbError::AccessDenied(
            "denied".to_owned()
        )));
        assert!(!read_snapshot_transient(&buzz_db::DbError::InvalidData(
            "invalid".to_owned()
        )));
    }

    #[test]
    fn release_confirmation_transient_accepts_only_the_classified_allowlist() {
        assert!(release_confirmation_transient(&db_error_with_sqlstate(
            "55P03"
        )));
        assert!(release_confirmation_transient(&db_error_with_sqlstate(
            "57014"
        )));
        assert!(!release_confirmation_transient(&db_error_with_sqlstate(
            "42P01"
        )));
        assert!(!release_confirmation_transient(
            &buzz_db::DbError::AccessDenied("denied".to_owned())
        ));
    }

    #[test]
    fn encode_failures_are_content_free() {
        let failures = [
            SemanticOneShotEncodeFailure::DeadlineExceeded,
            SemanticOneShotEncodeFailure::Cancelled(
                crate::semantic_query_runtime::SemanticCancellationSource::ServerShutdown,
            ),
            SemanticOneShotEncodeFailure::Provider(
                crate::semantic_provider::TrackedProviderFailure {
                    failure: ProviderAttemptFailure {
                        kind: ProviderAttemptFailureKind::Rejected { status: 400 },
                        handoff: ProviderHandoffCertainty::ConfirmedResponse,
                    },
                    error: SemanticGraphQueryError::ProviderRejected { status: 400 },
                },
            ),
            SemanticOneShotEncodeFailure::FreshPlan(SemanticOneShotError::Busy),
        ];
        for failure in &failures {
            let rendered = failure.to_string();
            assert!(!rendered.contains("query"), "content leaked: {rendered}");
            assert!(!rendered.contains("vector"), "content leaked: {rendered}");
        }
    }

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
        assert!(matches!(
            map_egress_failure(EgressFailure::Cancelled(
                crate::semantic_query_runtime::SemanticCancellationSource::ServerShutdown,
            )),
            OneShot::Timeout
        ));
    }
}
