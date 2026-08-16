//! Shared interactive reliability primitives for semantic query operations.
//!
//! Phase 2 R1 delivered the typed execution-context layer described by the
//! unified reliability runtime plan §4: operation-provided deadline windows,
//! aggregated cancellation, the lifecycle latch, request-level attempt
//! ledgers, the Provider handoff-aware attempt failure, the internal failure
//! taxonomy, and its closed retry disposition matrix.
//!
//! Phase 2 R2 adds the shared Provider reliability executor over that layer:
//! one `reservation -> wait -> routing trust -> egress confirmation` sequence
//! plus one deadline-bounded `encode_once` handoff, adopted by all four
//! semantic operations with zero policy. The executor never retries, backs
//! off, opens a circuit, or chooses a public error; every neutral outcome is
//! mapped by the owning closed operation into its own frozen public surface.
//! Ticket admission, Stage A observation, traversal, release fences, and
//! public result contracts stay with the closed coordinators.
//!
//! Every type here is content-free by construction: no query text, overview,
//! Coordinate identity, vector, credential, or project content is stored,
//! formatted, or logged.

// Cancellation aggregation, the lifecycle latch, Provider handoff
// classification, and the retry disposition matrix are adopted by the closed
// coordinators from R3/R4 on, so those not-yet-wired items stay explicit dead
// code rather than being deleted by a cleanup pass.
#![allow(dead_code)]

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use buzz_db::semantic_query::{
    SemanticContextEgressExpectation, SemanticGraphQueryEgressConfirmation,
    SemanticGraphQueryEgressConfirmationRequest, SemanticGraphQueryEgressRequest,
    SemanticGraphQueryEgressReservation, SemanticGraphQueryTicket,
};
use buzz_semantic::SemanticError;
use buzz_semantic_query::SemanticGraphQueryRoutingTrust;
use chrono::{DateTime, Utc};
use tokio::sync::watch;

use crate::semantic_graph_observability::{
    record_provider_failure, record_provider_wait, stage_timer, SemanticGraphMetricStage,
    SemanticGraphProviderFailure, SemanticGraphStageTimer,
};
use crate::state::AppState;

/// Closed source that ended a semantic request before normal completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticCancellationSource {
    /// The authenticated caller disconnected before the response was sent.
    CallerDisconnected,
    /// The Relay process began a controlled shutdown.
    ServerShutdown,
    /// An operation-provided deadline window expired.
    DeadlineExceeded,
    /// An explicit internal cancellation (for example a forced abort).
    ExplicitCancel,
}

impl SemanticCancellationSource {
    /// Closed low-cardinality metric label for this source.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::CallerDisconnected => "caller_disconnected",
            Self::ServerShutdown => "server_shutdown",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::ExplicitCancel => "explicit_cancel",
        }
    }
}

/// Aggregated, first-wins cancellation token for one logical request.
///
/// The token carries no content. `cancel` is idempotent: only the first
/// source wins and every later caller observes that source.
pub(crate) struct SemanticCancellation {
    source_tx: watch::Sender<Option<SemanticCancellationSource>>,
}

/// Cloneable observation handle for [`SemanticCancellation`].
pub(crate) struct SemanticCancellationHandle {
    source_rx: watch::Receiver<Option<SemanticCancellationSource>>,
}

impl SemanticCancellation {
    /// Create an uncancelled token.
    pub(crate) fn new() -> Self {
        let (source_tx, _) = watch::channel(None);
        Self { source_tx }
    }

    /// Return a cloneable handle observing this token.
    pub(crate) fn handle(&self) -> SemanticCancellationHandle {
        SemanticCancellationHandle {
            source_rx: self.source_tx.subscribe(),
        }
    }

    /// Cancel with `source`; returns the winning source.
    pub(crate) fn cancel(&self, source: SemanticCancellationSource) -> SemanticCancellationSource {
        let won = self.source_tx.send_if_modified(|current| {
            if current.is_none() {
                *current = Some(source);
                true
            } else {
                false
            }
        });
        if won {
            source
        } else {
            self.source_tx.borrow().unwrap_or(source)
        }
    }
}

impl SemanticCancellationHandle {
    /// Currently observed cancellation source, if the token was cancelled.
    pub(crate) fn cancelled(&self) -> Option<SemanticCancellationSource> {
        *self.source_rx.borrow()
    }

    /// True when the token was cancelled.
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled().is_some()
    }

    /// Resolve when the token is cancelled; returns the winning source.
    ///
    /// The owning context always outlives its handles, so a dropped sender
    /// cannot occur in practice; it is mapped to the closest closed source.
    pub(crate) async fn wait(&mut self) -> SemanticCancellationSource {
        loop {
            if let Some(source) = self.cancelled() {
                return source;
            }
            if self.source_rx.changed().await.is_err() {
                return SemanticCancellationSource::ServerShutdown;
            }
        }
    }
}

impl fmt::Debug for SemanticCancellation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticCancellation")
            .field("source", &self.source_tx.borrow())
            .finish_non_exhaustive()
    }
}

impl fmt::Debug for SemanticCancellationHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticCancellationHandle")
            .field("source", &self.source_rx.borrow())
            .finish_non_exhaustive()
    }
}

/// Terminal lifecycle state of one semantic request.
///
/// `Active` may transition atomically to exactly one of `Finalizing`,
/// `Cancelling`, or `TimedOut`; only `Finalizing` may later reach
/// `Completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticLifecycleState {
    /// The request may still start new semantic work.
    Active,
    /// A release permit was won and synchronous finalization started.
    Finalizing,
    /// Cancellation won the arbitration; no new semantic work may start.
    Cancelling,
    /// A deadline won the arbitration; no new semantic work may start.
    TimedOut,
    /// Synchronous finalization completed.
    Completed,
}

impl SemanticLifecycleState {
    /// Closed low-cardinality metric label for this state.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Finalizing => "finalizing",
            Self::Cancelling => "cancelling",
            Self::TimedOut => "timed_out",
            Self::Completed => "completed",
        }
    }

    /// True when no new semantic work may start in this state.
    ///
    /// Mandatory rollback, abort, transaction close, and RAII cleanup stay
    /// allowed in every state; this flag only governs new semantic work.
    pub(crate) const fn forbids_new_semantic_work(self) -> bool {
        !matches!(self, Self::Active | Self::Finalizing)
    }
}

const LIFECYCLE_ACTIVE: u8 = 0;
const LIFECYCLE_FINALIZING: u8 = 1;
const LIFECYCLE_CANCELLING: u8 = 2;
const LIFECYCLE_TIMED_OUT: u8 = 3;
const LIFECYCLE_COMPLETED: u8 = 4;

/// Outcome of competing for the lifecycle latch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticLatchOutcome {
    /// This caller won the arbitration and the latch moved.
    Won(SemanticLifecycleState),
    /// `Finalizing` already won; the running finalizer must discard its
    /// result at the post-check instead of sending it.
    LostToFinalizing(SemanticCancellationSource),
    /// The latch was already terminal; nothing changed.
    LostTerminal(SemanticLifecycleState),
}

/// Single-winner lifecycle arbitration for one semantic request.
///
/// `Active -> Finalizing | Cancelling | TimedOut` happens through one atomic
/// compare-and-swap, so exactly one transition wins. When `Finalizing` wins
/// first, a later cancel or deadline only records a discard request: the
/// already-started synchronous signing may complete, but its result must be
/// dropped at the finalizer's post-check and never sent.
pub(crate) struct SemanticLifecycleLatch {
    state: AtomicU8,
    discard_requested: AtomicBool,
    discard_source: AtomicU8,
    discard_source_known: AtomicBool,
}

impl SemanticLifecycleLatch {
    /// Create a latch in the `Active` state.
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU8::new(LIFECYCLE_ACTIVE),
            discard_requested: AtomicBool::new(false),
            discard_source: AtomicU8::new(0),
            discard_source_known: AtomicBool::new(false),
        }
    }

    /// Arbitrate for the synchronous finalize path.
    ///
    /// Wins only from `Active`; the winner may run the already-authorized
    /// synchronous signing and must post-check [`Self::discard_requested`].
    pub(crate) fn begin_finalize(&self) -> SemanticLatchOutcome {
        match self.try_leave_active(LIFECYCLE_FINALIZING) {
            Some(()) => SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing),
            None => SemanticLatchOutcome::LostTerminal(self.state()),
        }
    }

    /// Arbitrate for cancellation.
    pub(crate) fn cancel(&self, source: SemanticCancellationSource) -> SemanticLatchOutcome {
        if let Some(()) = self.try_leave_active(LIFECYCLE_CANCELLING) {
            return SemanticLatchOutcome::Won(SemanticLifecycleState::Cancelling);
        }
        self.lost_arbitration(source)
    }

    /// Arbitrate for deadline expiry.
    pub(crate) fn timeout(&self) -> SemanticLatchOutcome {
        self.cancel(SemanticCancellationSource::DeadlineExceeded)
            .as_timeout_outcome()
    }

    /// Mark a completed synchronous finalize.
    ///
    /// Valid from `Finalizing` or from `Active` (an operation that never
    /// finalizes signed content still terminates); other states are
    /// returned unchanged.
    pub(crate) fn complete(&self) -> SemanticLifecycleState {
        let _ = self
            .state
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                matches!(current, LIFECYCLE_ACTIVE | LIFECYCLE_FINALIZING)
                    .then_some(LIFECYCLE_COMPLETED)
            });
        self.state()
    }

    /// Current lifecycle state.
    pub(crate) fn state(&self) -> SemanticLifecycleState {
        state_from_u8(self.state.load(Ordering::SeqCst))
    }

    /// True when cancel or deadline arrived during synchronous finalization
    /// and the finalized result must be discarded instead of sent.
    pub(crate) fn discard_requested(&self) -> bool {
        self.discard_requested.load(Ordering::SeqCst)
    }

    /// The cancellation source that requested the discard, when recorded.
    pub(crate) fn discard_source(&self) -> Option<SemanticCancellationSource> {
        if self.discard_source_known.load(Ordering::SeqCst) {
            cancellation_source_from_u8(self.discard_source.load(Ordering::SeqCst))
        } else {
            None
        }
    }

    /// Classify a lost arbitration against the current latch state.
    fn lost_arbitration(&self, source: SemanticCancellationSource) -> SemanticLatchOutcome {
        if self.state.load(Ordering::SeqCst) == LIFECYCLE_FINALIZING {
            // The synchronous finalizer keeps running; it must post-check
            // and discard its result instead of sending it.
            self.record_discard(source);
            SemanticLatchOutcome::LostToFinalizing(source)
        } else {
            SemanticLatchOutcome::LostTerminal(self.state())
        }
    }

    /// Record the first discard request observed during finalization.
    fn record_discard(&self, source: SemanticCancellationSource) {
        if !self.discard_requested.swap(true, Ordering::SeqCst) {
            self.discard_source
                .store(cancellation_source_to_u8(source), Ordering::SeqCst);
            self.discard_source_known.store(true, Ordering::SeqCst);
        }
    }

    /// Atomically move from `Active` to `next`; `None` when another caller
    /// already left `Active`.
    fn try_leave_active(&self, next: u8) -> Option<()> {
        self.state
            .compare_exchange(LIFECYCLE_ACTIVE, next, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .ok()
    }
}

impl SemanticLatchOutcome {
    /// Re-label a won cancellation outcome as deadline arbitration.
    fn as_timeout_outcome(self) -> SemanticLatchOutcome {
        match self {
            SemanticLatchOutcome::Won(_) => {
                SemanticLatchOutcome::Won(SemanticLifecycleState::TimedOut)
            }
            // Lost arbitrations already carry the winning state or source.
            otherwise => otherwise,
        }
    }
}

fn state_from_u8(value: u8) -> SemanticLifecycleState {
    match value {
        LIFECYCLE_ACTIVE => SemanticLifecycleState::Active,
        LIFECYCLE_FINALIZING => SemanticLifecycleState::Finalizing,
        LIFECYCLE_CANCELLING => SemanticLifecycleState::Cancelling,
        LIFECYCLE_TIMED_OUT => SemanticLifecycleState::TimedOut,
        _ => SemanticLifecycleState::Completed,
    }
}

fn cancellation_source_to_u8(source: SemanticCancellationSource) -> u8 {
    match source {
        SemanticCancellationSource::CallerDisconnected => 0,
        SemanticCancellationSource::ServerShutdown => 1,
        SemanticCancellationSource::DeadlineExceeded => 2,
        SemanticCancellationSource::ExplicitCancel => 3,
    }
}

fn cancellation_source_from_u8(value: u8) -> Option<SemanticCancellationSource> {
    match value {
        0 => Some(SemanticCancellationSource::CallerDisconnected),
        1 => Some(SemanticCancellationSource::ServerShutdown),
        2 => Some(SemanticCancellationSource::DeadlineExceeded),
        3 => Some(SemanticCancellationSource::ExplicitCancel),
        _ => None,
    }
}

impl Default for SemanticLifecycleLatch {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for SemanticLifecycleLatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticLifecycleLatch")
            .field("state", &self.state())
            .field("discard_requested", &self.discard_requested())
            .finish_non_exhaustive()
    }
}

/// Operation-owned monotonic deadline windows for one semantic request.
///
/// Windows are created by each closed operation from its own total budget
/// and are immutable afterwards. The shared runtime may only read them; it
/// must not derive, reset, or extend them, and no retry or restart may reset
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SemanticDeadlineWindows {
    provider_start_before: Instant,
    work: Instant,
    snapshot_close: Instant,
    absolute: Instant,
}

/// One of the four operation-provided deadline windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticDeadlineWindow {
    /// Latest instant at which a new physical Provider attempt may start.
    ProviderStart,
    /// Latest instant for Provider, database, and traversal work.
    Work,
    /// Latest instant for closing the read-only snapshot.
    SnapshotClose,
    /// Latest instant for packing, release, and signing.
    Absolute,
}

impl SemanticDeadlineWindow {
    /// Closed low-cardinality metric label for this window.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::ProviderStart => "provider_start",
            Self::Work => "work",
            Self::SnapshotClose => "snapshot_close",
            Self::Absolute => "absolute",
        }
    }
}

impl SemanticDeadlineWindows {
    /// Validate and freeze operation-provided windows.
    ///
    /// Windows must satisfy `provider_start_before <= work <= snapshot_close
    /// <= absolute`. Equal windows are allowed so the R2 zero-policy
    /// migration can pin every one-shot window to the existing single hard
    /// deadline.
    pub(crate) fn new(
        provider_start_before: Instant,
        work: Instant,
        snapshot_close: Instant,
        absolute: Instant,
    ) -> Result<Self, SemanticReliabilityFailure> {
        if provider_start_before <= work && work <= snapshot_close && snapshot_close <= absolute {
            Ok(Self {
                provider_start_before,
                work,
                snapshot_close,
                absolute,
            })
        } else {
            Err(SemanticReliabilityFailure::ContractInvalid(
                SemanticContractInvalid::DeadlineWindowOrder,
            ))
        }
    }

    /// The R2 zero-policy one-shot shape: every window equals the existing
    /// fixed hard deadline, preserving today's single-deadline behavior.
    ///
    /// Before R4 enables retry, a one-shot coordinator must instead provide
    /// an explicit earlier `provider_start_before` window; this constructor
    /// exists only for the zero-behavior migration step.
    pub(crate) fn for_one_shot_hard_deadline(deadline: Instant) -> Self {
        Self {
            provider_start_before: deadline,
            work: deadline,
            snapshot_close: deadline,
            absolute: deadline,
        }
    }

    /// The frozen instant of one window.
    pub(crate) fn window(&self, window: SemanticDeadlineWindow) -> Instant {
        match window {
            SemanticDeadlineWindow::ProviderStart => self.provider_start_before,
            SemanticDeadlineWindow::Work => self.work,
            SemanticDeadlineWindow::SnapshotClose => self.snapshot_close,
            SemanticDeadlineWindow::Absolute => self.absolute,
        }
    }

    /// Remaining time in one window, or `None` when it already expired.
    pub(crate) fn remaining(
        &self,
        window: SemanticDeadlineWindow,
        now: Instant,
    ) -> Option<std::time::Duration> {
        self.window(window).checked_duration_since(now)
    }

    /// True when a new physical Provider attempt may still start.
    pub(crate) fn may_start_provider_attempt(&self, now: Instant) -> bool {
        now < self.provider_start_before
    }

    /// Earliest expired window at `now`, checked from earliest to latest.
    pub(crate) fn expired_window(&self, now: Instant) -> Option<SemanticDeadlineWindow> {
        [
            SemanticDeadlineWindow::ProviderStart,
            SemanticDeadlineWindow::Work,
            SemanticDeadlineWindow::SnapshotClose,
            SemanticDeadlineWindow::Absolute,
        ]
        .into_iter()
        .find(|window| now >= self.window(*window))
    }
}

/// Closed class of the logical operation owning an attempt ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticOperationAttemptClass {
    /// Coordinate discovery and the two one-hop variants.
    OneShot,
    /// The bounded complete-path traversal.
    CompletePath,
}

impl SemanticOperationAttemptClass {
    /// Hard cap on physical Provider attempts per logical request.
    ///
    /// One-shot allows one safe Provider retry; the complete path allows one
    /// safe Provider retry plus one churn-driven root restart, but never a
    /// fourth physical call.
    pub(crate) const fn physical_provider_attempt_cap(self) -> u32 {
        match self {
            Self::OneShot => 2,
            Self::CompletePath => 3,
        }
    }

    /// Hard cap on operation or root attempts per logical request.
    pub(crate) const fn operation_attempt_cap(self) -> u32 {
        2
    }
}

/// Closed attempt counter that was exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SemanticAttemptExhausted {
    /// The physical Provider attempt cap for the operation class was reached.
    #[error("semantic physical provider attempt cap reached")]
    ProviderAttempts,
    /// The single Provider transport retry budget was consumed.
    #[error("semantic provider transport retry budget exhausted")]
    ProviderTransportRetry,
    /// The operation or root attempt cap was reached.
    #[error("semantic operation attempt cap reached")]
    OperationAttempts,
    /// The release confirmation retry budget was consumed.
    #[error("semantic release confirmation retry budget exhausted")]
    ReleaseConfirmationRetry,
}

/// Request-level ledger bounding every retry dimension of one logical
/// request.
///
/// Counters never nest into new budgets and are never reset. The physical
/// Provider attempt count is monotonic across operation restarts so nested
/// retries cannot multiply past the compiled caps (one-shot 2, complete path
/// 3). The single transport-retry token is shared across operation restarts;
/// the fresh attempt of a restart does not consume it, which is exactly what
/// allows "one safe Provider retry + one churn root restart" while forbidding
/// a fourth physical call. These caps enter the compiled reliability runtime
/// digest when the R2 route matrix lands.
pub(crate) struct SemanticAttemptLedger {
    class: SemanticOperationAttemptClass,
    provider_attempts: AtomicU32,
    provider_attempts_in_operation: AtomicU32,
    provider_transport_retries: AtomicU32,
    operation_attempts: AtomicU32,
    release_confirmation_attempts: AtomicU32,
}

impl SemanticAttemptLedger {
    /// Create a ledger for one logical request of `class`.
    pub(crate) const fn new(class: SemanticOperationAttemptClass) -> Self {
        Self {
            class,
            provider_attempts: AtomicU32::new(0),
            provider_attempts_in_operation: AtomicU32::new(0),
            provider_transport_retries: AtomicU32::new(0),
            operation_attempts: AtomicU32::new(0),
            release_confirmation_attempts: AtomicU32::new(0),
        }
    }

    /// Physical Provider attempts begun so far in this logical request.
    pub(crate) fn provider_attempts(&self) -> u32 {
        self.provider_attempts.load(Ordering::SeqCst)
    }

    /// Provider transport retries consumed so far.
    pub(crate) fn provider_transport_retries(&self) -> u32 {
        self.provider_transport_retries.load(Ordering::SeqCst)
    }

    /// Operation or root attempts begun so far.
    pub(crate) fn operation_attempts(&self) -> u32 {
        self.operation_attempts.load(Ordering::SeqCst)
    }

    /// Release confirmation attempts begun so far.
    pub(crate) fn release_confirmation_attempts(&self) -> u32 {
        self.release_confirmation_attempts.load(Ordering::SeqCst)
    }

    /// Begin one operation or root attempt.
    ///
    /// The first attempt is free; the single allowed restart consumes the
    /// restart budget. Further restarts exhaust the ledger.
    pub(crate) fn begin_operation_attempt(&self) -> Result<u32, SemanticAttemptExhausted> {
        let _ = self
            .operation_attempts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |begun| {
                (begun < self.class.operation_attempt_cap()).then_some(begun + 1)
            })
            .map_err(|_| SemanticAttemptExhausted::OperationAttempts)?;
        self.provider_attempts_in_operation
            .store(0, Ordering::SeqCst);
        Ok(self.operation_attempts.load(Ordering::SeqCst))
    }

    /// Begin one physical Provider attempt, returning its monotonic ordinal.
    ///
    /// Every attempt counts against the class physical cap. A second or
    /// later attempt *within the same operation attempt* must consume the
    /// single shared transport-retry token; the fresh attempt of a new
    /// operation attempt does not. Deadline windows are checked by the
    /// executor, not by this ledger.
    pub(crate) fn begin_provider_attempt(&self) -> Result<u32, SemanticAttemptExhausted> {
        if self.provider_attempts.load(Ordering::SeqCst)
            >= self.class.physical_provider_attempt_cap()
        {
            return Err(SemanticAttemptExhausted::ProviderAttempts);
        }
        if self.provider_attempts_in_operation.load(Ordering::SeqCst) > 0 {
            let _ = self
                .provider_transport_retries
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |used| {
                    (used < 1).then_some(used + 1)
                })
                .map_err(|_| SemanticAttemptExhausted::ProviderTransportRetry)?;
        }
        let ordinal = self
            .provider_attempts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |begun| {
                (begun < self.class.physical_provider_attempt_cap()).then_some(begun + 1)
            })
            .map_err(|_| SemanticAttemptExhausted::ProviderAttempts)?;
        self.provider_attempts_in_operation
            .fetch_add(1, Ordering::SeqCst);
        Ok(ordinal + 1)
    }

    /// Begin one release confirmation; the retry budget allows two total.
    pub(crate) fn begin_release_confirmation(&self) -> Result<u32, SemanticAttemptExhausted> {
        let previous = self
            .release_confirmation_attempts
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |begun| {
                (begun < 2).then_some(begun + 1)
            })
            .map_err(|_| SemanticAttemptExhausted::ReleaseConfirmationRetry)?;
        Ok(previous + 1)
    }
}

impl fmt::Debug for SemanticAttemptLedger {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticAttemptLedger")
            .field("class", &self.class)
            .field("provider_attempts", &self.provider_attempts())
            .field(
                "provider_transport_retries",
                &self.provider_transport_retries(),
            )
            .field("operation_attempts", &self.operation_attempts())
            .field(
                "release_confirmation_attempts",
                &self.release_confirmation_attempts(),
            )
            .finish_non_exhaustive()
    }
}

/// Certainty about whether a failed Provider attempt reached the Provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProviderHandoffCertainty {
    /// The request provably never left this process.
    NotStarted,
    /// A full Provider response was received before the failure.
    ConfirmedResponse,
    /// The request may have been delivered; its outcome cannot be known.
    OutcomeUnknown,
}

/// Closed classification of one failed physical Provider attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProviderAttemptFailureKind {
    /// A connect-phase failure that provably never handed off the request.
    ConnectNotStarted,
    /// The Provider throttled the attempt.
    RateLimited {
        /// Whether a syntactically valid `Retry-After` was present.
        valid_retry_after: bool,
    },
    /// A definitive Provider 5xx response.
    RetryableResponse {
        /// Closed HTTP status-class code, for example `500`.
        status_class: u16,
    },
    /// A definitive Provider rejection that will not recover on retry.
    Rejected {
        /// Closed HTTP status code.
        status: u16,
    },
    /// The attempt may have been delivered; its outcome is unknown.
    OutcomeUnknown,
    /// The response violated the closed Provider response contract.
    ProtocolInvalid,
}

/// One failed physical Provider attempt with its handoff certainty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ProviderAttemptFailure {
    /// Closed failure classification.
    pub(crate) kind: ProviderAttemptFailureKind,
    /// Whether the request provably reached the Provider.
    pub(crate) handoff: ProviderHandoffCertainty,
}

impl ProviderAttemptFailure {
    /// Conservatively classify the current shared Provider transport error.
    ///
    /// Today's transport cannot distinguish a pre-connect failure from a
    /// broken response stream, so `SemanticError::ProviderTransport` maps to
    /// the safe `OutcomeUnknown` classification; the R2 single-attempt
    /// adapter replaces this with the real distinction at the transport
    /// boundary.
    pub(crate) fn from_semantic_error(error: &SemanticError) -> Self {
        match error {
            SemanticError::ProviderTransport => Self {
                kind: ProviderAttemptFailureKind::OutcomeUnknown,
                handoff: ProviderHandoffCertainty::OutcomeUnknown,
            },
            SemanticError::ProviderRateLimited {
                retry_after_seconds,
            } => Self {
                kind: ProviderAttemptFailureKind::RateLimited {
                    valid_retry_after: retry_after_seconds.is_some(),
                },
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            },
            SemanticError::ProviderRetryable { status } => Self {
                kind: ProviderAttemptFailureKind::RetryableResponse {
                    status_class: status / 100 * 100,
                },
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            },
            SemanticError::ProviderRejected { status } => Self {
                kind: ProviderAttemptFailureKind::Rejected { status: *status },
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            },
            // Response-contract violations always follow a full response.
            SemanticError::ProviderResponse
            | SemanticError::EmbeddingDimensionMismatch { .. }
            | SemanticError::NonFiniteEmbedding { .. }
            | SemanticError::ZeroNormEmbedding => Self {
                kind: ProviderAttemptFailureKind::ProtocolInvalid,
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            },
            // Boundary and contract errors are raised before any transport
            // work leaves this process.
            _ => Self {
                kind: ProviderAttemptFailureKind::ProtocolInvalid,
                handoff: ProviderHandoffCertainty::NotStarted,
            },
        }
    }
}

/// Closed reason a typed contract was invalid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticContractInvalid {
    /// Operation-provided deadline windows were misordered.
    DeadlineWindowOrder,
    /// A generated result violated its closed validation contract.
    ResultValidation,
}

/// Effect phase of a database operation, as observed by the classifier.
///
/// The authoritative enum lives in `buzz_db`; the alias keeps the taxonomy
/// readable inside the runtime module without a second phase definition.
pub(crate) use buzz_db::SemanticDbEffectPhase as SemanticDbPhase;

/// Closed site that observed a snapshot or generation change.
///
/// The plan §4.5 matrix gives the two sites different owners: a change the
/// operation observes while rebuilding fresh context returns for input
/// rebuild, while a one-shot release whose expected snapshot is no longer
/// current returns for snapshot restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticSnapshotChangeSite {
    /// Fresh operation observation changed the semantic inputs.
    OperationObservation,
    /// A release confirmation saw its expected snapshot no longer current.
    ReleaseConfirmation,
}

impl SemanticSnapshotChangeSite {
    /// Closed low-cardinality metric label for this site.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::OperationObservation => "operation_observation",
            Self::ReleaseConfirmation => "release_confirmation",
        }
    }
}

/// Typed, content-free internal failure taxonomy for semantic requests.
///
/// Classification is produced at the closest source of truth and never
/// reverse-engineered from strings or public HTTP codes. Each surface keeps
/// mapping these into its frozen public errors; [`Self::retry_disposition`]
/// encodes the closed plan §4.5 matrix the shared executor honors from R2
/// and R4.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SemanticReliabilityFailure {
    /// A closed request, contract, or invariant was violated.
    #[error("semantic contract invalid")]
    ContractInvalid(SemanticContractInvalid),
    /// The caller is no longer authorized for this request.
    #[error("semantic authorization denied")]
    AuthorizationDenied,
    /// A policy or feature gate disabled the capability.
    #[error("semantic policy disabled")]
    PolicyDisabled,
    /// The deployment fleet trust was unavailable or stale.
    #[error("semantic fleet unavailable")]
    FleetUnavailable,
    /// Process or Provider admission rejected the request.
    #[error("semantic admission busy")]
    AdmissionBusy,
    /// An operation-provided deadline window expired.
    #[error("semantic deadline exceeded")]
    DeadlineExceeded,
    /// The request was cancelled before completion.
    #[error("semantic request cancelled")]
    Cancelled(SemanticCancellationSource),
    /// A Provider connect-phase failure that never handed off.
    #[error("semantic provider connect not started")]
    ProviderConnectNotStarted,
    /// The Provider throttled the attempt.
    #[error("semantic provider rate limited")]
    ProviderRateLimited {
        /// Whether a valid `Retry-After` was present.
        valid_retry_after: bool,
    },
    /// The Provider returned a definitive retryable response.
    #[error("semantic provider retryable response")]
    ProviderRetryableResponse {
        /// Closed HTTP status class.
        status_class: u16,
    },
    /// The Provider definitively rejected the attempt.
    #[error("semantic provider rejected")]
    ProviderRejected,
    /// The Provider attempt outcome cannot be known; never replay blindly.
    #[error("semantic provider outcome unknown")]
    ProviderOutcomeUnknown,
    /// The Provider response violated the closed protocol contract.
    #[error("semantic provider protocol invalid")]
    ProviderProtocolInvalid,
    /// A classified read-only snapshot transient from the closed SQLSTATE
    /// allowlist.
    #[error("semantic database read transient")]
    DbReadSnapshotTransient {
        /// Effect phase that observed the transient.
        phase: SemanticDbPhase,
        /// Closed SQLSTATE class from the frozen allowlist.
        sqlstate_class: buzz_db::SemanticDbSqlstateClass,
    },
    /// Closing a read-only snapshot ended with an unknown outcome.
    ///
    /// Handed back to the operation, which must first close or drop the old
    /// read-only transaction before any snapshot restart (plan §4.5).
    #[error("semantic database snapshot close unknown")]
    DbReadSnapshotCloseUnknown,
    /// The observed snapshot or generation changed under the request.
    ///
    /// The observing site decides the owner (plan §4.5): a change seen while
    /// the operation observes fresh context returns for input rebuild; a
    /// one-shot release whose expected snapshot is no longer current returns
    /// for snapshot restart, where the operation may drop its unsigned
    /// result and redo the short operation.
    #[error("semantic database snapshot changed")]
    DbSnapshotChanged {
        /// Closed site that observed the change.
        site: SemanticSnapshotChangeSite,
    },
    /// The database denied authorization for this request.
    #[error("semantic database authorization denied")]
    DbAuthorizationDenied,
    /// A database invariant was violated.
    #[error("semantic database invariant violation")]
    DbInvariantViolation,
    /// An unclassified database failure; terminal by default.
    #[error("semantic database unclassified terminal")]
    DbUnclassifiedTerminal {
        /// Effect phase that observed the failure.
        phase: SemanticDbPhase,
    },
    /// A Provider reservation commit ended with an unknown outcome.
    #[error("semantic provider reservation commit outcome unknown")]
    ProviderReservationCommitOutcomeUnknown,
    /// A release confirmation hit a classified transient.
    #[error("semantic release confirmation transient")]
    ReleaseConfirmationTransient {
        /// Closed SQLSTATE class from the frozen allowlist.
        sqlstate_class: buzz_db::SemanticDbSqlstateClass,
    },
    /// A release confirmation ended with an unknown outcome.
    #[error("semantic release confirmation outcome unknown")]
    ReleaseConfirmationOutcomeUnknown,
    /// A generated result failed closed validation.
    #[error("semantic result invalid")]
    ResultInvalid,
    /// The result exceeded its frozen response cap.
    #[error("semantic response too large")]
    ResponseTooLarge,
    /// Signing the released result failed.
    #[error("semantic signing failed")]
    SigningFailed,
}

/// Closed retry disposition owned by the shared reliability layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum SemanticRetryDisposition {
    /// Never retry; project the failure through the surface adapter.
    Terminal,
    /// The shared executor may retry with a freshly authorized plan.
    RetryProviderWithFreshPlan,
    /// Return control so the operation can rebuild its semantic inputs.
    ReturnToOperationForInputRebuild,
    /// Return control so the operation can restart against a fresh snapshot.
    ReturnToOperationForSnapshotRestart,
    /// Retry only the release confirmation, never the completed work.
    RetryReleaseConfirmation,
}

impl SemanticRetryDisposition {
    /// Closed low-cardinality metric label for this disposition.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::RetryProviderWithFreshPlan => "retry_provider_fresh_plan",
            Self::ReturnToOperationForInputRebuild => "return_input_rebuild",
            Self::ReturnToOperationForSnapshotRestart => "return_snapshot_restart",
            Self::RetryReleaseConfirmation => "retry_release_confirmation",
        }
    }
}

impl SemanticReliabilityFailure {
    /// The closed default disposition for this failure (plan §4.5).
    ///
    /// Additional gating (fresh plan availability, remaining deadline
    /// windows, ledger budgets) is applied by the executor; this mapping only
    /// decides which owner may act.
    pub(crate) fn retry_disposition(&self) -> SemanticRetryDisposition {
        match self {
            Self::ContractInvalid(_)
            | Self::AuthorizationDenied
            | Self::PolicyDisabled
            | Self::AdmissionBusy
            | Self::FleetUnavailable
            | Self::DeadlineExceeded
            | Self::Cancelled(_)
            | Self::ProviderRejected
            | Self::ProviderOutcomeUnknown
            | Self::ProviderProtocolInvalid
            | Self::ProviderReservationCommitOutcomeUnknown
            | Self::DbAuthorizationDenied
            | Self::DbInvariantViolation
            | Self::DbUnclassifiedTerminal { .. }
            | Self::ResultInvalid
            | Self::ResponseTooLarge
            | Self::SigningFailed => SemanticRetryDisposition::Terminal,
            Self::ProviderConnectNotStarted
            | Self::ProviderRateLimited { .. }
            | Self::ProviderRetryableResponse { .. } => {
                SemanticRetryDisposition::RetryProviderWithFreshPlan
            }
            Self::DbReadSnapshotTransient { .. } | Self::DbReadSnapshotCloseUnknown => {
                SemanticRetryDisposition::ReturnToOperationForSnapshotRestart
            }
            Self::DbSnapshotChanged {
                site: SemanticSnapshotChangeSite::OperationObservation,
            } => SemanticRetryDisposition::ReturnToOperationForInputRebuild,
            Self::DbSnapshotChanged {
                site: SemanticSnapshotChangeSite::ReleaseConfirmation,
            } => SemanticRetryDisposition::ReturnToOperationForSnapshotRestart,
            Self::ReleaseConfirmationTransient { .. } | Self::ReleaseConfirmationOutcomeUnknown => {
                SemanticRetryDisposition::RetryReleaseConfirmation
            }
        }
    }

    /// Closed low-cardinality failure-class label for metrics.
    pub(crate) fn failure_class(&self) -> &'static str {
        match self {
            Self::ContractInvalid(_) => "contract_invalid",
            Self::AuthorizationDenied => "authorization_denied",
            Self::DbAuthorizationDenied => "db_authorization_denied",
            Self::PolicyDisabled => "policy_disabled",
            Self::FleetUnavailable => "fleet_unavailable",
            Self::AdmissionBusy => "admission_busy",
            Self::DeadlineExceeded => "deadline_exceeded",
            Self::Cancelled(_) => "cancelled",
            Self::ProviderConnectNotStarted => "provider_connect_not_started",
            Self::ProviderRateLimited { .. } => "provider_rate_limited",
            Self::ProviderRetryableResponse { .. } => "provider_retryable_response",
            Self::ProviderRejected => "provider_rejected",
            Self::ProviderOutcomeUnknown => "provider_outcome_unknown",
            Self::ProviderProtocolInvalid => "provider_protocol_invalid",
            Self::DbReadSnapshotTransient { .. } => "db_read_snapshot_transient",
            Self::DbReadSnapshotCloseUnknown => "db_read_snapshot_close_unknown",
            Self::DbSnapshotChanged { .. } => "db_snapshot_changed",
            Self::DbInvariantViolation => "db_invariant_violation",
            Self::DbUnclassifiedTerminal { .. } => "db_unclassified_terminal",
            Self::ProviderReservationCommitOutcomeUnknown => {
                "provider_reservation_commit_outcome_unknown"
            }
            Self::ReleaseConfirmationTransient { .. } => "release_confirmation_transient",
            Self::ReleaseConfirmationOutcomeUnknown => "release_confirmation_outcome_unknown",
            Self::ResultInvalid => "result_invalid",
            Self::ResponseTooLarge => "response_too_large",
            Self::SigningFailed => "signing_failed",
        }
    }

    /// Map a classified database failure plus phase into this taxonomy.
    pub(crate) fn from_db_failure(
        kind: buzz_db::SemanticDbFailureKind,
        phase: SemanticDbPhase,
    ) -> Self {
        match kind {
            buzz_db::SemanticDbFailureKind::AuthorizationDenied => Self::DbAuthorizationDenied,
            buzz_db::SemanticDbFailureKind::InvariantViolation => Self::DbInvariantViolation,
            buzz_db::SemanticDbFailureKind::SnapshotReadTransient { sqlstate_class } => {
                Self::DbReadSnapshotTransient {
                    phase,
                    sqlstate_class,
                }
            }
            buzz_db::SemanticDbFailureKind::ReleaseConfirmationTransient { sqlstate_class } => {
                Self::ReleaseConfirmationTransient { sqlstate_class }
            }
            buzz_db::SemanticDbFailureKind::UnclassifiedTerminal => {
                Self::DbUnclassifiedTerminal { phase }
            }
        }
    }
}

/// Per-process source of in-memory logical request identities.
///
/// The identity exists only to correlate one in-flight request across its
/// stages; it is never a metric label and never leaves the process.
static NEXT_LOGICAL_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Shared execution context for one logical semantic request.
///
/// The context is pure control state: it stores no query text, context
/// overview, Coordinate identity, vector, public result, traversal budget,
/// snapshot policy, release policy, priority, or resource weight. Operations
/// create it after request validation and hand it to the shared executor;
/// every window value inside it stays owned by the creating operation.
pub(crate) struct SemanticExecutionContext {
    windows: SemanticDeadlineWindows,
    cancellation: SemanticCancellation,
    latch: SemanticLifecycleLatch,
    ledger: SemanticAttemptLedger,
    logical_request_id: u64,
}

impl SemanticExecutionContext {
    /// Create a context from operation-provided windows for one request.
    pub(crate) fn new(
        class: SemanticOperationAttemptClass,
        windows: SemanticDeadlineWindows,
    ) -> Self {
        Self {
            windows,
            cancellation: SemanticCancellation::new(),
            latch: SemanticLifecycleLatch::new(),
            ledger: SemanticAttemptLedger::new(class),
            logical_request_id: NEXT_LOGICAL_REQUEST_ID.fetch_add(1, Ordering::SeqCst),
        }
    }

    /// Immutable operation-provided deadline windows.
    pub(crate) fn windows(&self) -> &SemanticDeadlineWindows {
        &self.windows
    }

    /// Aggregated cancellation token owned by this request.
    pub(crate) fn cancellation(&self) -> &SemanticCancellation {
        &self.cancellation
    }

    /// Lifecycle latch arbitrating finalization against cancellation.
    pub(crate) fn latch(&self) -> &SemanticLifecycleLatch {
        &self.latch
    }

    /// Request-level attempt ledger.
    pub(crate) fn ledger(&self) -> &SemanticAttemptLedger {
        &self.ledger
    }

    /// In-memory logical request identity; never a metric label.
    pub(crate) const fn logical_request_id(&self) -> u64 {
        self.logical_request_id
    }

    /// Cancel the request and arbitrate the latch in one step.
    ///
    /// Returns the latch outcome so the caller knows whether new semantic
    /// work must stop (`Won`) or an in-flight finalizer must discard its
    /// result (`LostToFinalizing`).
    pub(crate) fn cancel(&self, source: SemanticCancellationSource) -> SemanticLatchOutcome {
        let _winner = self.cancellation.cancel(source);
        self.latch.cancel(source)
    }

    /// Arbitrate a deadline expiry against cancellation and the latch.
    pub(crate) fn deadline_expired(&self) -> SemanticLatchOutcome {
        let _winner = self
            .cancellation
            .cancel(SemanticCancellationSource::DeadlineExceeded);
        self.latch.timeout()
    }
}

impl fmt::Debug for SemanticExecutionContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticExecutionContext")
            .field("windows", &self.windows)
            .field("lifecycle", &self.latch.state())
            .field("ledger", &self.ledger)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// R2 shared Provider reliability executor (zero policy).
// ---------------------------------------------------------------------------

/// Neutral admission failure of the shared Provider egress executor.
///
/// Every discriminator is mapped by the owning closed operation into its own
/// frozen public error; the executor never chooses a public surface.
///
/// `AttemptLedgerExhausted` is unreachable while R2 runs with zero retry
/// policy — no operation begins more physical attempts than its compiled cap
/// — and exists so the counting ledger has a total, fail-closed mapping once
/// R4 owns real retry decisions.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticProviderEgressFailure {
    /// The work window expired before an admitted step could finish.
    #[error("semantic provider egress deadline exceeded")]
    DeadlineExceeded,
    /// A reservation or confirmation database call failed at the transport.
    #[error("semantic provider egress database operation failed")]
    Database(#[source] buzz_db::DbError),
    /// The Provider slot could not start before the request deadline.
    #[error("semantic provider egress admission is busy")]
    AdmissionBusy,
    /// A conditioned context head changed before the egress point.
    #[error("semantic provider egress context state changed")]
    ContextChanged,
    /// No routing assertion is currently available for this serving instance.
    #[error("semantic provider egress routing fleet assertion is unavailable")]
    FleetUnavailable,
    /// Principal, capability, generation, fence, or graph readiness no longer
    /// matches the ticket.
    #[error("semantic provider egress authorization or readiness no longer matches")]
    ProviderUnavailable,
    /// The committed reservation did not carry the ticket's generation.
    #[error("semantic provider reservation violated its ticket contract")]
    ReservationContractViolated,
    /// The final egress permit did not match its committed reservation.
    #[error("semantic provider egress permit violated its reservation contract")]
    PermitContractViolated,
    /// The remaining work window cannot be represented as a wall-clock bound.
    #[error("semantic provider egress latest start cannot be represented")]
    LatestStartUnrepresentable,
    /// The request-level attempt ledger refused another attempt.
    #[error("semantic provider egress attempt ledger is exhausted")]
    AttemptLedgerExhausted(SemanticAttemptExhausted),
}

/// Which closed surface observes one executor attempt.
///
/// The one-shot envelopes stay silent exactly as they are today; the bounded
/// complete path keeps recording its existing Provider failure and wait
/// metrics. Observation is not a policy: it never changes an outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ProviderEgressObservation {
    /// One-shot coordinate and one-hop envelopes: no Provider metrics.
    Silent,
    /// The bounded complete path: record its existing Provider metrics.
    CompletePathQuery,
}

impl ProviderEgressObservation {
    fn provider_admission_busy(self) {
        if matches!(self, Self::CompletePathQuery) {
            record_provider_failure(SemanticGraphProviderFailure::Busy);
        }
    }

    fn provider_wait_stage(self) -> Option<SemanticGraphStageTimer> {
        match self {
            Self::Silent => None,
            Self::CompletePathQuery => Some(stage_timer(SemanticGraphMetricStage::ProviderWait)),
        }
    }

    fn provider_wait_completed(self, elapsed: Duration) {
        if matches!(self, Self::CompletePathQuery) {
            record_provider_wait(elapsed);
        }
    }

    fn provider_wait_deadline(self, elapsed: Duration) {
        if matches!(self, Self::CompletePathQuery) {
            record_provider_wait(elapsed);
            record_provider_failure(SemanticGraphProviderFailure::Deadline);
        }
    }

    fn provider_encode_deadline(self) {
        if matches!(self, Self::CompletePathQuery) {
            record_provider_failure(SemanticGraphProviderFailure::Deadline);
        }
    }
}

/// Borrowed inputs for one shared Provider egress admission.
///
/// `'state` outlives the per-attempt borrows so the returned routing trust
/// can live in the caller's execution state after the plan's own borrows
/// end.
pub(crate) struct ProviderEgressPlan<'state, 'plan> {
    /// Serving state owning the database, fleet, and relay signer.
    pub(crate) state: &'state AppState,
    /// Execution context of the logical request owning this attempt.
    pub(crate) context: &'plan SemanticExecutionContext,
    /// Exact authorized ticket being revalidated.
    pub(crate) ticket: &'plan SemanticGraphQueryTicket,
    /// Current authenticated principal pubkey bytes.
    pub(crate) reader_pubkey: &'plan [u8],
    /// Complete accepted/omitted context state that shaped the Provider
    /// channel set; one-shot envelopes pass an empty slice.
    pub(crate) expected_contexts: &'plan [SemanticContextEgressExpectation],
    /// Which surface observes this attempt.
    pub(crate) observation: ProviderEgressObservation,
}

/// Wrap one bounded step in the work window of the owning request.
async fn timeout_before<T, F>(
    deadline: Instant,
    future: F,
) -> Result<T, tokio::time::error::Elapsed>
where
    F: Future<Output = T>,
{
    tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), future).await
}

/// Derive the Provider-reservation start bound from the work window.
///
/// R2 keeps the historical one-shot rule — a zero-length remainder is still
/// admitted and fails at the next bounded step — so all four operations share
/// one shape without changing any public deadline behavior.
fn latest_start_at(work: Instant) -> Result<DateTime<Utc>, SemanticProviderEgressFailure> {
    let remaining = work
        .checked_duration_since(Instant::now())
        .ok_or(SemanticProviderEgressFailure::DeadlineExceeded)?;
    let duration = chrono::Duration::from_std(remaining)
        .map_err(|_| SemanticProviderEgressFailure::LatestStartUnrepresentable)?;
    Ok(Utc::now() + duration)
}

/// Run the shared `reservation -> wait -> routing trust -> egress
/// confirmation` admission sequence for exactly one physical Provider
/// attempt.
///
/// This is the R2 zero-policy primitive: it adds no retry, backoff, circuit,
/// or route fallback, and it never chooses a public error. Ticket admission,
/// Stage A observation, and the Provider encode call stay with the closed
/// operation. The returned routing trust belongs to the caller's later
/// release fence.
pub(crate) async fn execute_provider_egress<'state>(
    plan: ProviderEgressPlan<'state, '_>,
) -> Result<SemanticGraphQueryRoutingTrust<'state>, SemanticProviderEgressFailure> {
    let state = plan.state;
    let work = plan.context.windows().window(SemanticDeadlineWindow::Work);
    let relay_pubkey = state.relay_keypair.public_key();
    let latest_start_at = latest_start_at(work)?;
    plan.context
        .ledger()
        .begin_provider_attempt()
        .map_err(SemanticProviderEgressFailure::AttemptLedgerExhausted)?;
    let reservation = timeout_before(
        work,
        state
            .db
            .reserve_semantic_graph_query_egress(SemanticGraphQueryEgressRequest {
                expected_ticket: plan.ticket,
                reader_pubkey: plan.reader_pubkey,
                expected_projection_pubkey: &relay_pubkey,
                expected_contexts: plan.expected_contexts,
                provider: &plan.ticket.generation.model_contract.provider,
                interval: state.config.semantic_worker.request_interval,
                latest_start_at,
            }),
    )
    .await
    .map_err(|_| SemanticProviderEgressFailure::DeadlineExceeded)?
    .map_err(SemanticProviderEgressFailure::Database)?;
    let reservation = match reservation {
        SemanticGraphQueryEgressReservation::Reserved(reservation) => reservation,
        SemanticGraphQueryEgressReservation::Busy => {
            plan.observation.provider_admission_busy();
            return Err(SemanticProviderEgressFailure::AdmissionBusy);
        }
        SemanticGraphQueryEgressReservation::ContextChanged => {
            return Err(SemanticProviderEgressFailure::ContextChanged);
        }
        SemanticGraphQueryEgressReservation::Unavailable => {
            return Err(SemanticProviderEgressFailure::ProviderUnavailable);
        }
    };
    let (wait, reserved_generation, reserved_context_digest) = reservation.into_parts();
    if reserved_generation != plan.ticket.generation.generation_id {
        return Err(SemanticProviderEgressFailure::ReservationContractViolated);
    }
    let wait_started = Instant::now();
    let _wait_stage = plan.observation.provider_wait_stage();
    if timeout_before(work, tokio::time::sleep(wait))
        .await
        .is_err()
    {
        let elapsed = wait_started.elapsed();
        plan.observation.provider_wait_deadline(elapsed);
        return Err(SemanticProviderEgressFailure::DeadlineExceeded);
    }
    let elapsed = wait_started.elapsed();
    plan.observation.provider_wait_completed(elapsed);

    // The provider-slot reservation may wait, so it cannot authorize egress.
    // The final confirmation revalidates principal, generation, graph, and
    // routing state under the shared Community writer fence.
    let routing_trust = crate::semantic_fleet::semantic_graph_query_routing_trust(state)
        .map_err(|_| SemanticProviderEgressFailure::FleetUnavailable)?;
    let confirmation = timeout_before(
        work,
        state
            .db
            .confirm_semantic_graph_query_egress(SemanticGraphQueryEgressConfirmationRequest {
                expected_ticket: plan.ticket,
                reader_pubkey: plan.reader_pubkey,
                expected_projection_pubkey: &relay_pubkey,
                expected_contexts: plan.expected_contexts,
                routing_trust,
            }),
    )
    .await
    .map_err(|_| SemanticProviderEgressFailure::DeadlineExceeded)?
    .map_err(SemanticProviderEgressFailure::Database)?;
    let permit = match confirmation {
        SemanticGraphQueryEgressConfirmation::Permitted(permit) => permit,
        SemanticGraphQueryEgressConfirmation::ContextChanged => {
            return Err(SemanticProviderEgressFailure::ContextChanged);
        }
        SemanticGraphQueryEgressConfirmation::FleetUnavailable => {
            return Err(SemanticProviderEgressFailure::FleetUnavailable);
        }
        SemanticGraphQueryEgressConfirmation::Unavailable => {
            return Err(SemanticProviderEgressFailure::ProviderUnavailable);
        }
    };
    let (permitted_generation, permitted_context_digest) = permit.into_parts();
    if permitted_generation != reserved_generation
        || permitted_context_digest != reserved_context_digest
    {
        return Err(SemanticProviderEgressFailure::PermitContractViolated);
    }
    Ok(routing_trust)
}

/// Failure of one deadline-bounded single Provider invocation.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SemanticEncodeOnceFailure<E> {
    /// The work window expired before the single call completed.
    #[error("semantic provider encode deadline exceeded")]
    DeadlineExceeded,
    /// The Provider call returned a typed failure.
    #[error("semantic provider encode call failed")]
    Provider(#[source] E),
}

/// Run exactly one Provider invocation inside the work window.
///
/// `encode_once` is the only sanctioned Provider handoff shape: one physical
/// call, bounded by the frozen work window, with no internal retry, fallback,
/// or detached follow-up work. Mapping the Provider error and every public
/// outcome stays with the closed operation.
pub(crate) async fn encode_once<T, E, F>(
    work: Instant,
    observation: ProviderEgressObservation,
    future: F,
) -> Result<T, SemanticEncodeOnceFailure<E>>
where
    F: Future<Output = Result<T, E>>,
{
    match timeout_before(work, future).await {
        Ok(Ok(encoded)) => Ok(encoded),
        Ok(Err(error)) => Err(SemanticEncodeOnceFailure::Provider(error)),
        Err(_elapsed) => {
            observation.provider_encode_deadline();
            Err(SemanticEncodeOnceFailure::DeadlineExceeded)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn deadline_windows_require_monotonic_order() {
        let now = Instant::now();
        let at = |ms: u64| now + Duration::from_millis(ms);
        assert!(SemanticDeadlineWindows::new(at(10), at(5), at(20), at(30)).is_err());
        assert!(SemanticDeadlineWindows::new(at(0), at(10), at(5), at(30)).is_err());
        assert!(SemanticDeadlineWindows::new(at(0), at(10), at(20), at(15)).is_err());
        // Equal windows are the legal R2 one-shot zero-policy shape.
        assert!(SemanticDeadlineWindows::new(at(5), at(5), at(5), at(5)).is_ok());
        let one_shot = SemanticDeadlineWindows::for_one_shot_hard_deadline(at(45_000));
        assert_eq!(
            one_shot.window(SemanticDeadlineWindow::ProviderStart),
            at(45_000)
        );
        assert_eq!(
            one_shot.window(SemanticDeadlineWindow::Absolute),
            at(45_000)
        );
    }

    #[test]
    fn deadline_windows_report_expiry_in_order() {
        // Build every window from one captured `now` so expiry checks never
        // race wall-clock progress between construction and assertion.
        let now = Instant::now();
        let at = |ms: u64| now + Duration::from_millis(ms);
        let windows =
            SemanticDeadlineWindows::new(at(50), at(60), at(70), at(80)).expect("ordered windows");
        assert!(windows.may_start_provider_attempt(now));
        assert_eq!(windows.expired_window(now), None);
        let late = now + Duration::from_millis(55);
        assert!(!windows.may_start_provider_attempt(late));
        assert_eq!(
            windows.expired_window(late),
            Some(SemanticDeadlineWindow::ProviderStart)
        );
        let after_all = now + Duration::from_millis(90);
        assert_eq!(
            windows.expired_window(after_all),
            Some(SemanticDeadlineWindow::ProviderStart)
        );
    }

    #[test]
    fn cancellation_is_first_wins_and_idempotent() {
        let token = SemanticCancellation::new();
        let handle = token.handle();
        assert_eq!(handle.cancelled(), None);
        assert_eq!(
            token.cancel(SemanticCancellationSource::CallerDisconnected),
            SemanticCancellationSource::CallerDisconnected
        );
        assert_eq!(
            token.cancel(SemanticCancellationSource::ServerShutdown),
            SemanticCancellationSource::CallerDisconnected
        );
        assert_eq!(
            handle.cancelled(),
            Some(SemanticCancellationSource::CallerDisconnected)
        );
        assert!(handle.is_cancelled());
    }

    #[tokio::test]
    async fn cancellation_wait_resolves_after_cancel() {
        let token = SemanticCancellation::new();
        let mut handle = token.handle();
        token.cancel(SemanticCancellationSource::ExplicitCancel);
        assert_eq!(
            handle.wait().await,
            SemanticCancellationSource::ExplicitCancel
        );
    }

    #[test]
    fn lifecycle_latch_finalize_wins_then_records_discard() {
        let latch = SemanticLifecycleLatch::new();
        assert_eq!(latch.state(), SemanticLifecycleState::Active);
        assert_eq!(
            latch.begin_finalize(),
            SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing)
        );
        // A cancel during finalize loses arbitration but requests discard.
        assert_eq!(
            latch.cancel(SemanticCancellationSource::CallerDisconnected),
            SemanticLatchOutcome::LostToFinalizing(SemanticCancellationSource::CallerDisconnected)
        );
        assert_eq!(latch.state(), SemanticLifecycleState::Finalizing);
        assert!(latch.discard_requested());
        assert_eq!(
            latch.discard_source(),
            Some(SemanticCancellationSource::CallerDisconnected)
        );
        assert_eq!(latch.complete(), SemanticLifecycleState::Completed);
        // Post-terminal arbitration changes nothing and no new semantic work
        // may start from a terminal state.
        assert_eq!(
            latch.timeout(),
            SemanticLatchOutcome::LostTerminal(SemanticLifecycleState::Completed)
        );
        assert!(latch.state().forbids_new_semantic_work());
    }

    #[test]
    fn lifecycle_latch_cancel_wins_over_finalize() {
        let latch = SemanticLifecycleLatch::new();
        assert_eq!(
            latch.cancel(SemanticCancellationSource::ServerShutdown),
            SemanticLatchOutcome::Won(SemanticLifecycleState::Cancelling)
        );
        assert_eq!(
            latch.begin_finalize(),
            SemanticLatchOutcome::LostTerminal(SemanticLifecycleState::Cancelling)
        );
        assert!(!latch.discard_requested());
        assert_eq!(latch.complete(), SemanticLifecycleState::Cancelling);
        assert!(latch.state().forbids_new_semantic_work());
    }

    #[test]
    fn lifecycle_latch_timeout_wins_from_active() {
        let latch = SemanticLifecycleLatch::new();
        assert_eq!(
            latch.timeout(),
            SemanticLatchOutcome::Won(SemanticLifecycleState::TimedOut)
        );
        assert!(latch.state().forbids_new_semantic_work());
    }

    #[test]
    fn one_shot_ledger_caps_physical_attempts_at_two() {
        let ledger = SemanticAttemptLedger::new(SemanticOperationAttemptClass::OneShot);
        assert_eq!(ledger.begin_operation_attempt(), Ok(1));
        assert_eq!(ledger.begin_provider_attempt(), Ok(1));
        // Second physical attempt consumes the single transport retry token.
        assert_eq!(ledger.begin_provider_attempt(), Ok(2));
        // A third physical call breaches the class cap; the retry token is
        // exhausted at the same point, and the cap is the outer invariant.
        assert_eq!(
            ledger.begin_provider_attempt(),
            Err(SemanticAttemptExhausted::ProviderAttempts)
        );
        assert_eq!(ledger.provider_transport_retries(), 1);
    }

    #[test]
    fn complete_path_ledger_allows_retry_plus_restart_without_fourth_call() {
        let ledger = SemanticAttemptLedger::new(SemanticOperationAttemptClass::CompletePath);
        assert_eq!(ledger.begin_operation_attempt(), Ok(1));
        assert_eq!(ledger.begin_provider_attempt(), Ok(1));
        // One safe Provider transport retry within the first root attempt.
        assert_eq!(ledger.begin_provider_attempt(), Ok(2));
        // One churn-driven root restart starts its own attempt without
        // consuming another transport-retry token...
        assert_eq!(ledger.begin_operation_attempt(), Ok(2));
        assert_eq!(ledger.begin_provider_attempt(), Ok(3));
        // ...but a fourth physical call is impossible in either dimension.
        assert_eq!(
            ledger.begin_provider_attempt(),
            Err(SemanticAttemptExhausted::ProviderAttempts)
        );
        assert_eq!(
            ledger.begin_operation_attempt(),
            Err(SemanticAttemptExhausted::OperationAttempts)
        );
        assert_eq!(ledger.provider_attempts(), 3);
        assert_eq!(ledger.provider_transport_retries(), 1);
    }

    #[test]
    fn one_shot_ledger_cannot_retry_after_operation_restart() {
        let ledger = SemanticAttemptLedger::new(SemanticOperationAttemptClass::OneShot);
        assert_eq!(ledger.begin_operation_attempt(), Ok(1));
        assert_eq!(ledger.begin_provider_attempt(), Ok(1));
        // The one-shot snapshot restart consumes the operation budget...
        assert_eq!(ledger.begin_operation_attempt(), Ok(2));
        // ...and its fresh attempt is allowed...
        assert_eq!(ledger.begin_provider_attempt(), Ok(2));
        // ...but a third physical call is denied by the class cap.
        assert_eq!(
            ledger.begin_provider_attempt(),
            Err(SemanticAttemptExhausted::ProviderAttempts)
        );
    }

    #[test]
    fn release_confirmation_retry_is_bounded_to_two_attempts() {
        let ledger = SemanticAttemptLedger::new(SemanticOperationAttemptClass::OneShot);
        assert_eq!(ledger.begin_release_confirmation(), Ok(1));
        assert_eq!(ledger.begin_release_confirmation(), Ok(2));
        assert_eq!(
            ledger.begin_release_confirmation(),
            Err(SemanticAttemptExhausted::ReleaseConfirmationRetry)
        );
    }

    #[test]
    fn provider_attempt_failure_maps_current_transport_conservatively() {
        let transport =
            ProviderAttemptFailure::from_semantic_error(&SemanticError::ProviderTransport);
        assert_eq!(
            transport,
            ProviderAttemptFailure {
                kind: ProviderAttemptFailureKind::OutcomeUnknown,
                handoff: ProviderHandoffCertainty::OutcomeUnknown,
            }
        );
        let limited =
            ProviderAttemptFailure::from_semantic_error(&SemanticError::ProviderRateLimited {
                retry_after_seconds: Some(3),
            });
        assert_eq!(
            limited,
            ProviderAttemptFailure {
                kind: ProviderAttemptFailureKind::RateLimited {
                    valid_retry_after: true
                },
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            }
        );
        let server_error =
            ProviderAttemptFailure::from_semantic_error(&SemanticError::ProviderRetryable {
                status: 503,
            });
        assert_eq!(
            server_error,
            ProviderAttemptFailure {
                kind: ProviderAttemptFailureKind::RetryableResponse { status_class: 500 },
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            }
        );
    }

    #[test]
    fn failure_dispositions_follow_the_closed_matrix() {
        assert_eq!(
            SemanticReliabilityFailure::ProviderOutcomeUnknown.retry_disposition(),
            SemanticRetryDisposition::Terminal
        );
        assert_eq!(
            SemanticReliabilityFailure::ProviderConnectNotStarted.retry_disposition(),
            SemanticRetryDisposition::RetryProviderWithFreshPlan
        );
        assert_eq!(
            SemanticReliabilityFailure::ProviderReservationCommitOutcomeUnknown.retry_disposition(),
            SemanticRetryDisposition::Terminal
        );
        assert_eq!(
            SemanticReliabilityFailure::DbAuthorizationDenied.retry_disposition(),
            SemanticRetryDisposition::Terminal
        );
        assert_eq!(
            SemanticReliabilityFailure::DbReadSnapshotTransient {
                phase: SemanticDbPhase::SnapshotRead,
                sqlstate_class: buzz_db::SemanticDbSqlstateClass::TransactionRollback,
            }
            .retry_disposition(),
            SemanticRetryDisposition::ReturnToOperationForSnapshotRestart
        );
        // Close-unknown hands back to the operation, which must first close
        // or drop the old read-only transaction (plan §4.5 row condition).
        assert_eq!(
            SemanticReliabilityFailure::DbReadSnapshotCloseUnknown.retry_disposition(),
            SemanticRetryDisposition::ReturnToOperationForSnapshotRestart
        );
        // The two snapshot-change sites have different owners: input rebuild
        // for an operation observation, snapshot restart for a one-shot
        // release whose expected snapshot is no longer current.
        assert_eq!(
            SemanticReliabilityFailure::DbSnapshotChanged {
                site: SemanticSnapshotChangeSite::OperationObservation,
            }
            .retry_disposition(),
            SemanticRetryDisposition::ReturnToOperationForInputRebuild
        );
        assert_eq!(
            SemanticReliabilityFailure::DbSnapshotChanged {
                site: SemanticSnapshotChangeSite::ReleaseConfirmation,
            }
            .retry_disposition(),
            SemanticRetryDisposition::ReturnToOperationForSnapshotRestart
        );
        assert_eq!(
            SemanticReliabilityFailure::ReleaseConfirmationOutcomeUnknown.retry_disposition(),
            SemanticRetryDisposition::RetryReleaseConfirmation
        );
    }

    #[test]
    fn failure_labels_are_content_free_and_unique() {
        let failures = [
            SemanticReliabilityFailure::ContractInvalid(
                SemanticContractInvalid::DeadlineWindowOrder,
            ),
            SemanticReliabilityFailure::AuthorizationDenied,
            SemanticReliabilityFailure::PolicyDisabled,
            SemanticReliabilityFailure::FleetUnavailable,
            SemanticReliabilityFailure::AdmissionBusy,
            SemanticReliabilityFailure::DeadlineExceeded,
            SemanticReliabilityFailure::Cancelled(SemanticCancellationSource::ExplicitCancel),
            SemanticReliabilityFailure::ProviderConnectNotStarted,
            SemanticReliabilityFailure::ProviderRateLimited {
                valid_retry_after: false,
            },
            SemanticReliabilityFailure::ProviderRetryableResponse { status_class: 500 },
            SemanticReliabilityFailure::ProviderRejected,
            SemanticReliabilityFailure::ProviderOutcomeUnknown,
            SemanticReliabilityFailure::ProviderProtocolInvalid,
            SemanticReliabilityFailure::DbReadSnapshotTransient {
                phase: SemanticDbPhase::SnapshotRead,
                sqlstate_class: buzz_db::SemanticDbSqlstateClass::TransactionRollback,
            },
            SemanticReliabilityFailure::DbReadSnapshotCloseUnknown,
            SemanticReliabilityFailure::DbSnapshotChanged {
                site: SemanticSnapshotChangeSite::ReleaseConfirmation,
            },
            SemanticReliabilityFailure::DbAuthorizationDenied,
            SemanticReliabilityFailure::DbInvariantViolation,
            SemanticReliabilityFailure::DbUnclassifiedTerminal {
                phase: SemanticDbPhase::SnapshotRead,
            },
            SemanticReliabilityFailure::ProviderReservationCommitOutcomeUnknown,
            SemanticReliabilityFailure::ReleaseConfirmationTransient {
                sqlstate_class: buzz_db::SemanticDbSqlstateClass::LockUnavailable,
            },
            SemanticReliabilityFailure::ReleaseConfirmationOutcomeUnknown,
            SemanticReliabilityFailure::ResultInvalid,
            SemanticReliabilityFailure::ResponseTooLarge,
            SemanticReliabilityFailure::SigningFailed,
        ];
        let mut labels = std::collections::BTreeSet::new();
        for failure in &failures {
            let label = failure.failure_class();
            assert!(labels.insert(label), "duplicate failure class {label}");
            let rendered = failure.to_string();
            assert!(!rendered.contains("query"), "content leaked: {rendered}");
            assert!(!rendered.contains("vector"), "content leaked: {rendered}");
        }
    }

    #[test]
    fn context_aggregates_cancellation_and_latch() {
        let windows = SemanticDeadlineWindows::for_one_shot_hard_deadline(
            Instant::now() + Duration::from_secs(45),
        );
        let context =
            SemanticExecutionContext::new(SemanticOperationAttemptClass::OneShot, windows);
        assert!(context.logical_request_id() > 0);
        assert_eq!(
            context.cancel(SemanticCancellationSource::CallerDisconnected),
            SemanticLatchOutcome::Won(SemanticLifecycleState::Cancelling)
        );
        assert!(context.cancellation().handle().is_cancelled());
        assert!(context.latch().state().forbids_new_semantic_work());
        assert_eq!(
            context.deadline_expired(),
            SemanticLatchOutcome::LostTerminal(SemanticLifecycleState::Cancelling)
        );
    }

    #[test]
    fn egress_failure_labels_are_content_free() {
        let failures = [
            SemanticProviderEgressFailure::DeadlineExceeded,
            SemanticProviderEgressFailure::Database(buzz_db::DbError::AccessDenied(
                "denied".to_owned(),
            )),
            SemanticProviderEgressFailure::AdmissionBusy,
            SemanticProviderEgressFailure::ContextChanged,
            SemanticProviderEgressFailure::FleetUnavailable,
            SemanticProviderEgressFailure::ProviderUnavailable,
            SemanticProviderEgressFailure::ReservationContractViolated,
            SemanticProviderEgressFailure::PermitContractViolated,
            SemanticProviderEgressFailure::LatestStartUnrepresentable,
            SemanticProviderEgressFailure::AttemptLedgerExhausted(
                SemanticAttemptExhausted::ProviderAttempts,
            ),
        ];
        let mut labels = std::collections::BTreeSet::new();
        for failure in &failures {
            let rendered = failure.to_string();
            assert!(
                labels.insert(rendered.clone()),
                "duplicate failure label {rendered}"
            );
            assert!(!rendered.contains("query"), "content leaked: {rendered}");
            assert!(!rendered.contains("vector"), "content leaked: {rendered}");
        }
    }

    #[test]
    fn latest_start_at_rejects_expired_work_window() {
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(matches!(
            latest_start_at(expired),
            Err(SemanticProviderEgressFailure::DeadlineExceeded)
        ));
        assert!(latest_start_at(Instant::now() + Duration::from_secs(30)).is_ok());
    }

    #[tokio::test]
    async fn encode_once_runs_exactly_one_bounded_invocation() {
        let work = Instant::now() + Duration::from_secs(30);
        let encoded = encode_once::<u32, u64, _>(work, ProviderEgressObservation::Silent, async {
            Ok(7_u32)
        })
        .await
        .expect("immediate success is admitted");
        assert_eq!(encoded, 7);
        assert!(matches!(
            encode_once::<u32, u32, _>(work, ProviderEgressObservation::Silent, async {
                Err(9_u32)
            })
            .await,
            Err(SemanticEncodeOnceFailure::Provider(9))
        ));
        let expired = Instant::now() - Duration::from_secs(1);
        assert!(matches!(
            encode_once::<(), (), _>(
                expired,
                ProviderEgressObservation::Silent,
                std::future::pending()
            )
            .await,
            Err(SemanticEncodeOnceFailure::DeadlineExceeded)
        ));
    }
}
