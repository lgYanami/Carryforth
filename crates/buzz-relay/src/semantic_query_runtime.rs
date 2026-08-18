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
//! Phase 2 R3 wires the context into stage admission: every new Provider,
//! database, traversal, or signing stage must pass [`admit_stage`] (directly
//! or through [`run_stage`]), which refuses work once cancellation won the
//! latch or an operation window expired. Bounded stages race the aggregated
//! cancellation token through the same `run_stage`, so a mid-flight future is
//! dropped — its rollback, abort, and RAII cleanup are the mandatory cleanup
//! that still runs in terminal states. Release finalization follows §4.1:
//! `begin_finalize` wins the latch after the release permit is issued, the
//! synchronous signing may complete, and a cancel or deadline that arrives
//! during it only records a discard that the finalizer's post-check must
//! honor by never sending the signed result.
//!
//! Phase 2 R4 adds the closed retry policy core: the per-item Provider retry
//! route, the disposition-and-budget decision, and the deadline-raced
//! backoff. The policy is owned here in one place; the closed coordinators
//! run the mechanical retry loops because only they can assemble a fresh
//! plan (fresh ticket, fresh observations, fresh inputs) from their own
//! state. Every retry still passes through the R2 executor's reservation and
//! confirmation and the R3 admission, so the attempt ledger caps, the
//! cancellation latch, and the frozen public error projections stay binding.
//!
//! Phase 2 R5 adds the shared process-local Provider circuit over the one
//! physical failure domain all four operations encode into. The circuit is
//! owned by the Provider client (so every clone shares it), keyed by a
//! content-free digest of the endpoint identity, request model, and a
//! config epoch that increments on every construction, and gated inside the
//! shared executor — before the reservation, revalidated with no wait after
//! the reservation wait and again after the final egress confirmation, the
//! last check adjacent to the Provider call. No coordinator can bypass an
//! open circuit because no Provider egress exists outside the executor.
//! Refusals ride the existing `AdmissionBusy` neutral failure, authorization
//! always runs first at ticket admission, and 429s feed an independent
//! throttle that never touches the health-failure count. Enforcement is
//! shadow-first: the flag admits spectators whose outcomes cannot move the
//! simulated state, so the canary observes exactly what enforcement would
//! have done.
//!
//! Every type here is content-free by construction: no query text, overview,
//! Coordinate identity, vector, credential, or project content is stored,
//! formatted, or logged.

// The remaining not-yet-wired taxonomy items stay explicit dead code rather
// than being deleted by a cleanup pass.
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

    /// Currently winning cancellation source, if the token was cancelled.
    pub(crate) fn cancelled(&self) -> Option<SemanticCancellationSource> {
        *self.source_tx.borrow()
    }

    /// Return a cloneable handle observing this token.
    pub(crate) fn handle(&self) -> SemanticCancellationHandle {
        SemanticCancellationHandle {
            source_rx: self.source_tx.subscribe(),
        }
    }

    /// Cancel with `source`; returns the winning source.
    pub(crate) fn cancel(&self, source: SemanticCancellationSource) -> SemanticCancellationSource {
        self.source_tx.cancel(source)
    }

    /// Return a cloneable handle that can cancel this token.
    ///
    /// Request guards hold one so a detached watcher — the shutdown
    /// subscription or a dropped request future — can fire the aggregated
    /// cancellation without owning the context. The latch is arbitrated
    /// lazily by the next admission, exactly like every other pre-cancelled
    /// request (fix plan F1 item 6).
    pub(crate) fn signal(&self) -> SemanticCancellationSignal {
        SemanticCancellationSignal {
            source_tx: self.source_tx.clone(),
        }
    }
}

/// Cloneable cancel side of one request's aggregated cancellation token.
pub(crate) struct SemanticCancellationSignal {
    source_tx: watch::Sender<Option<SemanticCancellationSource>>,
}

impl SemanticCancellationSignal {
    /// Cancel with `source`; returns the winning source. Idempotent and
    /// first-wins, exactly like [`SemanticCancellation::cancel`].
    pub(crate) fn cancel(&self, source: SemanticCancellationSource) -> SemanticCancellationSource {
        self.source_tx.cancel(source)
    }
}

impl SemanticCancellationSenderExt for watch::Sender<Option<SemanticCancellationSource>> {
    fn cancel(&self, source: SemanticCancellationSource) -> SemanticCancellationSource {
        let won = self.send_if_modified(|current| {
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
            self.borrow().unwrap_or(source)
        }
    }
}

/// Private cancel implementation shared by the token and its signal.
trait SemanticCancellationSenderExt {
    fn cancel(&self, source: SemanticCancellationSource) -> SemanticCancellationSource;
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
    /// `Finalizing` forbids it too: the synchronous finalizer owns the
    /// request from the moment its release permit is won, and no concurrent
    /// generic stage may start beside it (fix plan F1 item 5).
    pub(crate) const fn forbids_new_semantic_work(self) -> bool {
        !matches!(self, Self::Active)
    }

    /// True when the winner of the finalize latch may still run its own
    /// synchronous finalization stages.
    pub(crate) const fn admits_finalize_stage(self) -> bool {
        matches!(self, Self::Finalizing)
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
    ///
    /// A deadline that wins leaves the real `TimedOut` latch state (fix plan
    /// F1 item 4): it is observable after the fact instead of being relabeled
    /// onto a cancellation win. A deadline that loses to a running finalizer
    /// still records its discard request; one that loses to another terminal
    /// state changes nothing.
    pub(crate) fn timeout(&self) -> SemanticLatchOutcome {
        if let Some(()) = self.try_leave_active(LIFECYCLE_TIMED_OUT) {
            return SemanticLatchOutcome::Won(SemanticLifecycleState::TimedOut);
        }
        self.lost_arbitration(SemanticCancellationSource::DeadlineExceeded)
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
    /// True when this outcome won a transition into `state`.
    pub(crate) fn won(self, state: SemanticLifecycleState) -> bool {
        matches!(self, SemanticLatchOutcome::Won(won) if won == state)
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
    /// Production one-shots use [`Self::for_one_shot_reserved_budget`]
    /// instead (fix plan F1 item 3); this equal-window shape remains for the
    /// gated real-Provider canary, which exercises one bounded attempt
    /// against a plain wall clock.
    pub(crate) fn for_one_shot_hard_deadline(deadline: Instant) -> Self {
        Self {
            provider_start_before: deadline,
            work: deadline,
            snapshot_close: deadline,
            absolute: deadline,
        }
    }

    /// The closed one-shot budget shape: the caller-visible absolute
    /// deadline is preserved, while the three internal windows reserve
    /// closed tail fractions of the total budget (fix plan F1 item 3).
    ///
    /// The public one-shot contract stays "one hard deadline": every error
    /// still projects onto the same frozen timeout surface. Internally, a
    /// new physical Provider attempt may not start once half the budget is
    /// spent, generic work must yield three quarters in, and the snapshot
    /// close keeps a one-eighth reserve — so the short repeatable read,
    /// release confirmation, and synchronous finalize can always complete
    /// before the public absolute deadline (plan §4.1).
    pub(crate) fn for_one_shot_reserved_budget(start: Instant, total: Duration) -> Self {
        let eighth = total / ONE_SHOT_RESERVE_DENOMINATOR;
        Self {
            provider_start_before: start + total - (eighth * 4),
            work: start + total - (eighth * 2),
            snapshot_close: start + total - eighth,
            absolute: start + total,
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

/// Closed denominator of the one-shot internal tail reserves (fix plan F1).
///
/// One eighth of the caller's total budget: the snapshot close keeps one
/// eighth, generic work yields at two eighths, and a new physical Provider
/// attempt may not start after four eighths. Part of the compiled
/// reliability contract — the descriptor and the fleet digest pin it, so
/// changing it is a dated behavior change, not a silent tune.
pub(crate) const ONE_SHOT_RESERVE_DENOMINATOR: u32 = 8;

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

    /// Closed low-cardinality metric label for this class.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::OneShot => "one_shot",
            Self::CompletePath => "complete_path",
        }
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

    /// Whether one more physical Provider attempt fits the compiled caps.
    ///
    /// The decision core consults this before advising a retry so a retry is
    /// never begun that the very next [`Self::begin_provider_attempt`] would
    /// refuse (plan §9.1: an insufficient remaining budget never starts an
    /// attempt).
    pub(crate) fn can_begin_provider_attempt(&self) -> bool {
        if self.provider_attempts.load(Ordering::SeqCst)
            >= self.class.physical_provider_attempt_cap()
        {
            return false;
        }
        if self.provider_attempts_in_operation.load(Ordering::SeqCst) > 0
            && self.provider_transport_retries.load(Ordering::SeqCst) >= 1
        {
            return false;
        }
        true
    }

    /// Whether one more operation or root attempt fits the compiled cap.
    pub(crate) fn can_begin_operation_attempt(&self) -> bool {
        self.operation_attempts.load(Ordering::SeqCst) < self.class.operation_attempt_cap()
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
        /// The syntactically valid `Retry-After` delay, when the Provider
        /// supplied one.
        retry_after_seconds: Option<u64>,
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
                    retry_after_seconds: *retry_after_seconds,
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

    /// Classify one `.send()`-phase transport failure using the transport's
    /// own connect knowledge (plan §4.4).
    ///
    /// The pre-R4 adapter collapsed every send failure into the conservative
    /// outcome-unknown classification. A reqwest connect error provably never
    /// handed the request off, which is exactly the closed precondition the
    /// R4 connect retry item requires; every other send-phase failure keeps
    /// the conservative outcome-unknown treatment.
    pub(crate) fn transport_send_failure(connect_phase: bool) -> Self {
        if connect_phase {
            Self {
                kind: ProviderAttemptFailureKind::ConnectNotStarted,
                handoff: ProviderHandoffCertainty::NotStarted,
            }
        } else {
            Self {
                kind: ProviderAttemptFailureKind::OutcomeUnknown,
                handoff: ProviderHandoffCertainty::OutcomeUnknown,
            }
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

    /// Admit one new generic semantic stage targeted at `window`; refuse it
    /// once cancellation won, the latch left `Active`, or the target window
    /// itself expired (fix plan F1 items 1 and 2).
    ///
    /// Admission is target-window scoped: an earlier window that already
    /// expired (for example a spent `Work` window during the complete-path
    /// packing tail) is a legal cutoff, not a terminal refusal — only the
    /// window the stage actually targets can refuse it. Mandatory cleanup
    /// (rollback, abort, transaction close, RAII drops) is not a stage and
    /// stays allowed in every state; this gate governs only new Provider,
    /// database, traversal, release, or signing work.
    pub(crate) fn admit_stage(
        &self,
        window: SemanticDeadlineWindow,
    ) -> Result<(), SemanticStageAbort> {
        self.admit(SemanticStageOwner::Generic, window)
    }

    /// Admit one synchronous finalization stage owned by the latch winner.
    ///
    /// Only the `Finalizing` winner may run it; every other state —
    /// including `Active`, which has not yet won a release permit —
    /// refuses, so finalization work can never start beside generic work.
    pub(crate) fn admit_finalize_stage(
        &self,
        window: SemanticDeadlineWindow,
    ) -> Result<(), SemanticStageAbort> {
        self.admit(SemanticStageOwner::Finalizer, window)
    }

    /// Shared admission arbitration for one stage owner and target window.
    fn admit(
        &self,
        owner: SemanticStageOwner,
        window: SemanticDeadlineWindow,
    ) -> Result<(), SemanticStageAbort> {
        if let Some(source) = self.cancellation.cancelled() {
            let _ = self.cancel(source);
            return Err(SemanticStageAbort::Cancelled(source));
        }
        let state = self.latch.state();
        let admitted = match owner {
            SemanticStageOwner::Generic => !state.forbids_new_semantic_work(),
            SemanticStageOwner::Finalizer => state.admits_finalize_stage(),
        };
        if !admitted {
            return Err(SemanticStageAbort::LatchClosed(state));
        }
        if Instant::now() >= self.windows.window(window) {
            let _ = self.deadline_expired();
            return Err(SemanticStageAbort::Deadline(window));
        }
        Ok(())
    }

    /// Run one bounded stage inside `window`, racing the aggregated
    /// cancellation token.
    ///
    /// The stage future is dropped when cancellation wins, so its own RAII
    /// rollback and abort remain the mandatory cleanup path. A deadline
    /// expiry or cancellation that wins the race is arbitrated into the latch
    /// before returning, and cancellation wins any tie so no new value is
    /// delivered after the request ended.
    pub(crate) async fn run_stage<T, F>(
        &self,
        window: SemanticDeadlineWindow,
        future: F,
    ) -> Result<T, SemanticStageAbort>
    where
        F: Future<Output = T>,
    {
        self.run_owned_stage(SemanticStageOwner::Generic, window, future)
            .await
    }

    /// Run one bounded finalization stage owned by the latch winner.
    pub(crate) async fn run_finalize_stage<T, F>(
        &self,
        window: SemanticDeadlineWindow,
        future: F,
    ) -> Result<T, SemanticStageAbort>
    where
        F: Future<Output = T>,
    {
        self.run_owned_stage(SemanticStageOwner::Finalizer, window, future)
            .await
    }

    /// Shared bounded-stage runner for one stage owner.
    async fn run_owned_stage<T, F>(
        &self,
        owner: SemanticStageOwner,
        window: SemanticDeadlineWindow,
        future: F,
    ) -> Result<T, SemanticStageAbort>
    where
        F: Future<Output = T>,
    {
        self.admit(owner, window)?;
        let deadline = tokio::time::Instant::from_std(self.windows.window(window));
        let mut cancellation = self.cancellation.handle();
        tokio::select! {
            biased;
            source = cancellation.wait() => {
                let _ = self.cancel(source);
                Err(SemanticStageAbort::Cancelled(source))
            }
            outcome = tokio::time::timeout_at(deadline, future) => match outcome {
                Ok(value) => Ok(value),
                Err(_elapsed) => {
                    // In both owners a deadline expiry arbitrates through the
                    // latch: a generic stage leaves the `TimedOut` state,
                    // while the finalizer's own tail keeps its `Finalizing`
                    // state and records the discard of the signed result.
                    let _ = self.deadline_expired();
                    Err(SemanticStageAbort::Deadline(window))
                }
            },
        }
    }
}

/// Which side of the lifecycle latch owns one admitted stage.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum SemanticStageOwner {
    /// New Provider, database, traversal, release, or signing work; only the
    /// `Active` latch admits it.
    Generic,
    /// The synchronous finalization owned by the latch winner; only the
    /// `Finalizing` latch admits it.
    Finalizer,
}

/// Why a new semantic stage was refused or aborted mid-flight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticStageAbort {
    /// The target window expired before the stage could start or finish.
    Deadline(SemanticDeadlineWindow),
    /// The aggregated cancellation token fired while the stage ran.
    Cancelled(SemanticCancellationSource),
    /// The lifecycle latch no longer admits stages for this owner: the
    /// request already arbitrated to finalization or a terminal state.
    LatchClosed(SemanticLifecycleState),
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
    /// The aggregated cancellation token fired during the egress sequence.
    ///
    /// Carries no error detail of its own: the owning surface maps it onto
    /// its frozen deadline-equivalent public error, because a cancelled
    /// request has no caller left to distinguish.
    #[error("semantic provider egress was cancelled")]
    Cancelled(SemanticCancellationSource),
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

/// One completed shared Provider egress admission.
pub(crate) struct ProviderEgressAdmission<'state> {
    /// Routing trust for the caller's later release fence.
    pub(crate) routing_trust: SemanticGraphQueryRoutingTrust<'state>,
    /// Epoch-fenced circuit token for the admitted physical attempt.
    ///
    /// `None` only when this process has no configured Provider (unreachable
    /// behind the coordinators' own Provider resolution). The coordinator
    /// reports the attempt's outcome through [`observe_provider_circuit`]
    /// exactly once; a deadline or cancellation reports nothing.
    pub(crate) circuit: Option<ProviderCircuitToken>,
}

/// Cancel `context` when the Relay process entered its controlled shutdown.
///
/// The shutdown flag flips before listener drain, so every semantic surface
/// that holds serving state consults it before admitting new work. The
/// subscription created by [`subscribe_relay_shutdown`] covers the waits
/// between these polls.
pub(crate) fn propagate_relay_shutdown(state: &AppState, context: &SemanticExecutionContext) {
    if state.shutting_down.load(Ordering::SeqCst) {
        let _ = context.cancel(SemanticCancellationSource::ServerShutdown);
    }
}

/// Subscription that cancels the request the moment controlled shutdown
/// begins (fix plan F1 item 6).
///
/// The polls in [`propagate_relay_shutdown`] only fire at stage entries;
/// this subscription closes the gap by observing the host shutdown signal
/// while the request is parked on any await. Dropping the subscription
/// detaches it, so a finished request leaves no live task behind.
pub(crate) struct SemanticShutdownSubscription {
    task: Option<tokio::task::JoinHandle<()>>,
}

/// Subscribe `context` to the host shutdown signal.
pub(crate) fn subscribe_relay_shutdown(
    state: &AppState,
    context: &SemanticExecutionContext,
) -> SemanticShutdownSubscription {
    let signal = context.cancellation().signal();
    if state.shutting_down.load(Ordering::SeqCst) {
        signal.cancel(SemanticCancellationSource::ServerShutdown);
    }
    let mut shutdown = state.shutdown_signal.subscribe();
    let task = tokio::spawn(async move {
        loop {
            if shutdown.changed().await.is_err() {
                return;
            }
            if *shutdown.borrow() {
                signal.cancel(SemanticCancellationSource::ServerShutdown);
                return;
            }
        }
    });
    SemanticShutdownSubscription { task: Some(task) }
}

impl Drop for SemanticShutdownSubscription {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Cancels the request with `CallerDisconnected` when the request future is
/// dropped (fix plan F1 item 6).
///
/// The guard rides the coordinator that owns the request's execution
/// context: when the host drops the request future — the caller connection
/// ended — the cancellation token fires first-wins, every waiting stage
/// observes it, and the lifecycle latch arbitrates away from sending a
/// signed result to a caller that is no longer there.
pub(crate) struct SemanticCallerGuard {
    signal: SemanticCancellationSignal,
}

/// Arm the caller-disconnect guard for `context`.
pub(crate) fn caller_disconnect_guard(context: &SemanticExecutionContext) -> SemanticCallerGuard {
    SemanticCallerGuard {
        signal: context.cancellation().signal(),
    }
}

impl Drop for SemanticCallerGuard {
    fn drop(&mut self) {
        self.signal
            .cancel(SemanticCancellationSource::CallerDisconnected);
    }
}

/// Map one refused or aborted bounded step onto the neutral egress failure.
fn egress_stage_abort(abort: SemanticStageAbort) -> SemanticProviderEgressFailure {
    match abort {
        SemanticStageAbort::Deadline(_) => SemanticProviderEgressFailure::DeadlineExceeded,
        SemanticStageAbort::Cancelled(source) => SemanticProviderEgressFailure::Cancelled(source),
        // A physical Provider attempt is always generic work: a latch that
        // already arbitrated to finalization or a terminal state makes this
        // attempt over for the caller, on the frozen deadline-equivalent
        // surface every neutral refusal already projects onto.
        SemanticStageAbort::LatchClosed(_) => SemanticProviderEgressFailure::DeadlineExceeded,
    }
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

/// Run the shared `circuit gate -> reservation -> wait -> routing trust ->
/// egress confirmation` admission sequence for exactly one physical Provider
/// attempt.
///
/// This is the R2 zero-policy primitive grown by R5's circuit fence: it adds
/// no retry, backoff, or route fallback, and it never chooses a public
/// error. Ticket admission, Stage A observation, and the Provider encode
/// call stay with the closed operation. The returned routing trust belongs
/// to the caller's later release fence.
///
/// The circuit gate is taken here — not in any coordinator — because this is
/// the only Provider egress path: no operation can bypass an open circuit,
/// and authorization always precedes it (the ticket was admitted and the
/// principal capability-checked by the closed coordinator before this
/// executor runs, so a caller without authorization never observes a circuit
/// outcome). Every refusal rides the existing `AdmissionBusy` neutral
/// failure, which each surface already projects onto its frozen Busy public
/// error; the gate is fetched from the serving state itself so a coordinator
/// cannot forget to pass it.
pub(crate) async fn execute_provider_egress<'state>(
    plan: ProviderEgressPlan<'state, '_>,
) -> Result<ProviderEgressAdmission<'state>, SemanticProviderEgressFailure> {
    let state = plan.state;
    let work = plan.context.windows().window(SemanticDeadlineWindow::Work);
    let relay_pubkey = state.relay_keypair.public_key();
    propagate_relay_shutdown(state, plan.context);
    // A new physical Provider attempt is admitted against its own
    // `ProviderStart` window (fix plan F1 item 1): the one-shot reserves
    // refuse a retry that can no longer start-and-complete, while the
    // complete path keeps its historical work-deadline equivalence because
    // its windows freeze `provider_start == work`.
    plan.context
        .admit_stage(SemanticDeadlineWindow::ProviderStart)
        .map_err(egress_stage_abort)?;
    let latest_start_at = latest_start_at(work)?;
    plan.context
        .ledger()
        .begin_provider_attempt()
        .map_err(SemanticProviderEgressFailure::AttemptLedgerExhausted)?;
    // R5 fast gate: take the circuit admission (and, when the cooldown has
    // elapsed, the exclusive half-open probe lease) before any reservation
    // is taken (plan §4.7).
    let circuit = state
        .semantic_provider()
        .ok()
        .flatten()
        .map(crate::semantic_provider::VolcengineSemanticProvider::circuit);
    let circuit_token = match circuit {
        Some(circuit) => match circuit.admit() {
            ProviderCircuitAdmission::Admitted { token } => Some(token),
            ProviderCircuitAdmission::Refused { .. } => {
                return Err(SemanticProviderEgressFailure::AdmissionBusy);
            }
        },
        None => None,
    };
    let reservation = plan
        .context
        .run_stage(
            SemanticDeadlineWindow::Work,
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
        .map_err(egress_stage_abort)?
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
    match plan
        .context
        .run_stage(SemanticDeadlineWindow::Work, tokio::time::sleep(wait))
        .await
    {
        Ok(()) => {
            plan.observation
                .provider_wait_completed(wait_started.elapsed());
        }
        Err(SemanticStageAbort::Deadline(_)) => {
            plan.observation
                .provider_wait_deadline(wait_started.elapsed());
            return Err(SemanticProviderEgressFailure::DeadlineExceeded);
        }
        Err(SemanticStageAbort::Cancelled(source)) => {
            return Err(SemanticProviderEgressFailure::Cancelled(source));
        }
        Err(SemanticStageAbort::LatchClosed(_)) => {
            return Err(SemanticProviderEgressFailure::DeadlineExceeded);
        }
    }

    // R5 no-wait epoch revalidation after the reservation wait: a circuit
    // that transitioned while this attempt was waiting is no longer one this
    // attempt may speak for. A refused revalidation abandons the already
    // consumed slot — the database contract already treats a reserved slot
    // as rate-limit capacity, not authorization — leaving Provider delta
    // zero.
    if let (Some(circuit), Some(token)) = (circuit, circuit_token) {
        if !circuit.revalidate(token) && circuit.enforce() {
            return Err(SemanticProviderEgressFailure::AdmissionBusy);
        }
    }

    // The provider-slot reservation may wait, so it cannot authorize egress.
    // The final confirmation revalidates principal, generation, graph, and
    // routing state under the shared Community writer fence.
    let routing_trust = crate::semantic_fleet::semantic_graph_query_routing_trust(state)
        .map_err(|_| SemanticProviderEgressFailure::FleetUnavailable)?;
    let confirmation = plan
        .context
        .run_stage(
            SemanticDeadlineWindow::Work,
            state.db.confirm_semantic_graph_query_egress(
                SemanticGraphQueryEgressConfirmationRequest {
                    expected_ticket: plan.ticket,
                    reader_pubkey: plan.reader_pubkey,
                    expected_projection_pubkey: &relay_pubkey,
                    expected_contexts: plan.expected_contexts,
                    routing_trust,
                },
            ),
        )
        .await
        .map_err(egress_stage_abort)?
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
    // R5 final no-wait epoch revalidation, adjacent to the Provider call:
    // this confirmation is the last await before the coordinator's
    // `encode_once`, so a stale attempt is refused here with Provider delta
    // zero rather than speaking for a circuit that already transitioned.
    if let (Some(circuit), Some(token)) = (circuit, circuit_token) {
        if !circuit.revalidate(token) && circuit.enforce() {
            return Err(SemanticProviderEgressFailure::AdmissionBusy);
        }
    }
    Ok(ProviderEgressAdmission {
        routing_trust,
        circuit: circuit_token,
    })
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
    /// The aggregated cancellation token fired during the single call.
    #[error("semantic provider encode was cancelled")]
    Cancelled(SemanticCancellationSource),
}

/// Run exactly one Provider invocation inside the work window.
///
/// `encode_once` is the only sanctioned Provider handoff shape: one physical
/// call, admitted by and bounded to the owning context's work window, raced
/// against its aggregated cancellation, with no internal retry, fallback, or
/// detached follow-up work. Mapping the Provider error and every public
/// outcome stays with the closed operation.
pub(crate) async fn encode_once<T, E, F>(
    context: &SemanticExecutionContext,
    observation: ProviderEgressObservation,
    future: F,
) -> Result<T, SemanticEncodeOnceFailure<E>>
where
    F: Future<Output = Result<T, E>>,
{
    match context
        .run_stage(SemanticDeadlineWindow::Work, future)
        .await
    {
        Ok(Ok(encoded)) => Ok(encoded),
        Ok(Err(error)) => Err(SemanticEncodeOnceFailure::Provider(error)),
        Err(SemanticStageAbort::Deadline(_)) => {
            observation.provider_encode_deadline();
            Err(SemanticEncodeOnceFailure::DeadlineExceeded)
        }
        Err(SemanticStageAbort::Cancelled(source)) => {
            Err(SemanticEncodeOnceFailure::Cancelled(source))
        }
        Err(SemanticStageAbort::LatchClosed(_)) => Err(SemanticEncodeOnceFailure::DeadlineExceeded),
    }
}

// ---------------------------------------------------------------------------
// R4 closed retry policy core.
// ---------------------------------------------------------------------------

/// Compiled per-item Provider retry route (plan §8 R4).
///
/// Each flag enables exactly one row of the closed §4.5 retry matrix and is
/// meant to roll out with its own failure matrix and canary; nothing outside
/// these rows may retry, and every enabled retry still passes through the
/// R2 executor admission and the R3 stage arbitration, so the ledger caps,
/// the cancellation latch, and the frozen public error projections remain
/// binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderRetryRoute {
    /// Item 1: a connect-phase failure that provably never left this process
    /// may retry with a freshly authorized plan.
    pub(crate) connect_not_started: bool,
    /// Item 2: a 429 may retry when a syntactically valid `Retry-After`
    /// fully fits the remaining work window.
    pub(crate) rate_limited_full_retry_after: bool,
    /// Item 3: a definitive 5xx response may retry under the bounded
    /// Provider retry ledger.
    pub(crate) retryable_server_error: bool,
}

impl ProviderRetryRoute {
    /// The compiled R4 route matrix: every closed item enabled.
    ///
    /// The route and its bounds enter the compiled reliability runtime
    /// digest together with the attempt caps and the backoff contract; a
    /// later route change is a digest change, never a silent policy edit.
    pub(crate) const R4: Self = Self {
        connect_not_started: true,
        rate_limited_full_retry_after: true,
        retryable_server_error: true,
    };
}

/// Server-owned full-jitter base for Provider retries that carry no
/// `Retry-After` (plan §4.5).
///
/// The backoff draw is uniform over `0..=base` (full jitter) so concurrent
/// retried requests cannot phase-lock onto the Provider. This constant is
/// part of the compiled reliability runtime contract digest.
pub(crate) const PROVIDER_RETRY_FULL_JITTER_BASE_MS: u64 = 250;

/// What the closed retry policy decided for one failed Provider attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderRetryDecision {
    /// Retry with a freshly authorized plan after this bounded backoff.
    Retry { backoff: Duration },
    /// Do not retry; project the last typed failure through the surface.
    Terminal,
}

/// Closed outcome label for the vector-reuse observability (plan §4.6/§7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SemanticVectorReuseOutcome {
    /// A restart or root attempt reused its exact-compatible vector.
    Reused,
    /// The operation re-encoded because no compatible vector was stashed.
    Reencoded,
    /// A stashed vector failed its fresh revalidation fence.
    ReuseRejected,
}

impl SemanticVectorReuseOutcome {
    /// Closed low-cardinality metric label for this outcome.
    pub(crate) const fn metric_label(self) -> &'static str {
        match self {
            Self::Reused => "reused",
            Self::Reencoded => "reencoded",
            Self::ReuseRejected => "reuse_rejected",
        }
    }
}

/// Record one closed vector-reuse outcome (content-free; plan §7).
pub(crate) fn record_vector_reuse(
    class: SemanticOperationAttemptClass,
    outcome: SemanticVectorReuseOutcome,
) {
    metrics::counter!(
        "buzz_semantic_vector_reuse_total",
        "class" => class.metric_label(),
        "outcome" => outcome.metric_label(),
    )
    .increment(1);
}

/// Record one closed retry-policy decision (content-free; plan §7).
pub(crate) fn record_provider_retry_decision(retry: bool) {
    metrics::counter!(
        "buzz_semantic_provider_retry_total",
        "disposition" => if retry {
            SemanticRetryDisposition::RetryProviderWithFreshPlan.metric_label()
        } else {
            SemanticRetryDisposition::Terminal.metric_label()
        },
    )
    .increment(1);
}

/// Decide the closed disposition for one failed physical Provider attempt.
///
/// This is the single owner of the retry policy: it applies the §4.5 matrix
/// row selected by `route`, the request ledger budgets, and the operation's
/// own work window. A `Retry` advises the coordinator to assemble a fresh
/// plan and re-enter the executor; anything else must project the last typed
/// failure exactly as the pre-R4 single attempt did.
pub(crate) fn provider_retry_decision(
    route: ProviderRetryRoute,
    failure: ProviderAttemptFailure,
    context: &SemanticExecutionContext,
) -> ProviderRetryDecision {
    let backoff = match failure.kind {
        ProviderAttemptFailureKind::ConnectNotStarted if route.connect_not_started => {
            full_jitter_backoff()
        }
        ProviderAttemptFailureKind::RateLimited {
            retry_after_seconds: Some(seconds),
        } if route.rate_limited_full_retry_after => Duration::from_secs(seconds),
        ProviderAttemptFailureKind::RetryableResponse { .. } if route.retryable_server_error => {
            full_jitter_backoff()
        }
        _ => {
            record_provider_retry_decision(false);
            return ProviderRetryDecision::Terminal;
        }
    };
    if !context.ledger().can_begin_provider_attempt() {
        record_provider_retry_decision(false);
        return ProviderRetryDecision::Terminal;
    }
    let work = context.windows().window(SemanticDeadlineWindow::Work);
    let Some(remaining) = work.checked_duration_since(Instant::now()) else {
        record_provider_retry_decision(false);
        return ProviderRetryDecision::Terminal;
    };
    // The backoff must fully fit the remaining window and still leave
    // nonzero execution time after it (plan §4.5: a `Retry-After` that
    // cannot fully fit is not requested early).
    if remaining <= backoff {
        record_provider_retry_decision(false);
        return ProviderRetryDecision::Terminal;
    }
    record_provider_retry_decision(true);
    ProviderRetryDecision::Retry { backoff }
}

/// Draw one full-jitter backoff over the compiled base.
fn full_jitter_backoff() -> Duration {
    let bound = PROVIDER_RETRY_FULL_JITTER_BASE_MS;
    let millis = rand::random::<u64>() % (bound + 1);
    Duration::from_millis(millis)
}

/// Sleep one retry backoff inside the work window, racing the request's
/// aggregated cancellation (plan §6.3: retry sleeps never detach).
///
/// The sleep holds no repeatable-read transaction and no traversal permit by
/// construction — the closed coordinators only back off between executor
/// admissions, before those resources exist.
pub(crate) async fn provider_retry_backoff(
    context: &SemanticExecutionContext,
    backoff: Duration,
) -> Result<(), SemanticStageAbort> {
    context
        .run_stage(SemanticDeadlineWindow::Work, tokio::time::sleep(backoff))
        .await
}

// ---------------------------------------------------------------------------
// R5 shared Provider circuit (process-local failure domain).
// ---------------------------------------------------------------------------

/// Compiled consecutive health-failure count that trips the circuit Open.
///
/// Part of the compiled reliability runtime contract: changing it is a
/// behavior change that must ride a new route/digest, not a silent tune.
pub(crate) const PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD: u32 = 5;

/// Compiled Open cooldown before one exclusive half-open probe may start.
pub(crate) const PROVIDER_CIRCUIT_OPEN_COOLDOWN: Duration = Duration::from_secs(15);

/// Compiled budget granted to one half-open probe before it is reclaimed.
///
/// A probe holder that never observes (deadline, cancellation, or a lost
/// task) must not wedge the circuit in `HalfOpen` forever, so the lease is
/// reclaimed into a fresh Open cooldown once this budget expires.
pub(crate) const PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET: Duration = Duration::from_secs(15);

/// Compiled throttle cooldown when the Provider 429s without a syntactically
/// valid `Retry-After` (plan §4.7: the throttle is independent of health).
pub(crate) const PROVIDER_CIRCUIT_THROTTLE_DEFAULT: Duration = Duration::from_millis(1_000);

/// Compiled cap applied to a Provider-supplied `Retry-After` throttle window.
///
/// A `Retry-After` longer than this cap can never fit a request work window
/// anyway (the R4 full-fit rule declines it), so capping keeps a hostile or
/// misbehaving header from parking the shared domain for minutes.
pub(crate) const PROVIDER_CIRCUIT_THROTTLE_MAX: Duration = Duration::from_secs(60);

/// Phase of one shared Provider failure-domain circuit (plan §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderCircuitPhase {
    /// The domain is serving; consecutive health failures accumulate.
    Closed,
    /// The domain refused admission until the cooldown elapses.
    Open,
    /// One exclusive real-request probe is deciding the domain's health.
    HalfOpen,
}

/// Epoch-fenced admission token for one physical Provider attempt.
///
/// Every state transition bumps the circuit epoch, so a token only ever
/// speaks for the phase it was admitted into; a late outcome from an older
/// epoch is ignored (the epoch fence of plan §4.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderCircuitToken {
    epoch: u64,
    /// Shadow admission that enforcement would have refused. Spectators run
    /// (shadow never changes production behavior) but their outcomes cannot
    /// move the simulated state, keeping the shadow state machine exactly
    /// what enforcement would have produced.
    spectator: bool,
}

/// What the circuit fast gate decided for one new physical attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderCircuitAdmission {
    /// The attempt may proceed; the token fences its later observation.
    Admitted { token: ProviderCircuitToken },
    /// The enforcing circuit refused the attempt.
    Refused { reason: ProviderCircuitRefusal },
}

/// Closed reason the circuit refused one attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderCircuitRefusal {
    /// The domain is Open and cooling down.
    Open,
    /// The domain is HalfOpen and the exclusive probe is in flight.
    HalfOpenProbeBusy,
    /// The independent 429 throttle window is active.
    Throttled,
}

impl ProviderCircuitRefusal {
    /// Closed low-cardinality metric label for this refusal.
    const fn metric_label(self) -> &'static str {
        match self {
            Self::Open => "refused_open",
            Self::HalfOpenProbeBusy => "refused_half_open_probe_busy",
            Self::Throttled => "refused_throttled",
        }
    }
}

/// Closed health classification of one failed Provider attempt (plan §4.7).
///
/// Only connect failures, definitive 5xx responses, transport failures of
/// unknown outcome, and protocol-invalid responses are Provider health;
/// authorization, input, empty-result, database, snapshot, and cancellation
/// outcomes never touch the circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderHealthFailureClass {
    /// A connect-phase failure that never handed the request off.
    Connect,
    /// A definitive Provider 5xx response.
    ServerError,
    /// A transport failure whose delivery outcome is unknown.
    TransportUnknown,
    /// A response that violated the closed Provider response contract.
    ProtocolInvalid,
}

impl ProviderHealthFailureClass {
    /// Closed low-cardinality metric label for this health class.
    const fn metric_label(self) -> &'static str {
        match self {
            Self::Connect => "health_connect",
            Self::ServerError => "health_server_error",
            Self::TransportUnknown => "health_transport_unknown",
            Self::ProtocolInvalid => "health_protocol_invalid",
        }
    }
}

/// What one completed physical attempt reports to its circuit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderCircuitObservation {
    /// The Provider answered within the closed response contract.
    Success,
    /// The attempt failed with a Provider health failure.
    HealthFailure(ProviderHealthFailureClass),
    /// The Provider throttled the attempt (429); independent of health.
    Throttled {
        /// The syntactically valid `Retry-After` delay, when supplied.
        retry_after_seconds: Option<u64>,
    },
    /// The attempt failed in a way that is not Provider health.
    NotCounted,
}

impl ProviderCircuitObservation {
    /// Classify one failed attempt through the closed §4.7 matrix.
    pub(crate) fn from_attempt_failure(failure: &ProviderAttemptFailure) -> Self {
        match failure.kind {
            ProviderAttemptFailureKind::RateLimited {
                retry_after_seconds,
            } => Self::Throttled {
                retry_after_seconds,
            },
            ProviderAttemptFailureKind::ConnectNotStarted => {
                Self::HealthFailure(ProviderHealthFailureClass::Connect)
            }
            ProviderAttemptFailureKind::RetryableResponse { .. } => {
                Self::HealthFailure(ProviderHealthFailureClass::ServerError)
            }
            ProviderAttemptFailureKind::OutcomeUnknown => {
                Self::HealthFailure(ProviderHealthFailureClass::TransportUnknown)
            }
            ProviderAttemptFailureKind::ProtocolInvalid
                if failure.handoff == ProviderHandoffCertainty::ConfirmedResponse =>
            {
                Self::HealthFailure(ProviderHealthFailureClass::ProtocolInvalid)
            }
            // Pre-transport contract violations are input/boundary failures,
            // and a definitive 4xx rejection is per-request: neither says
            // anything about the shared domain's health.
            ProviderAttemptFailureKind::ProtocolInvalid
            | ProviderAttemptFailureKind::Rejected { .. } => Self::NotCounted,
        }
    }

    /// Closed low-cardinality metric label for this observation.
    fn metric_label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::HealthFailure(class) => class.metric_label(),
            Self::Throttled { .. } => "throttled",
            Self::NotCounted => "not_counted",
        }
    }
}

/// Mutable core of one circuit; every transition happens under this lock and
/// bumps the epoch, so the lock is the linearization point of the fence.
struct ProviderCircuitCore {
    phase: ProviderCircuitPhase,
    epoch: u64,
    consecutive_health_failures: u32,
    open_until: Instant,
    probe_deadline: Instant,
    probe_held: bool,
    throttle_until: Option<Instant>,
}

/// Process-local circuit over one shared Provider physical failure domain.
///
/// The circuit is owned by the Provider client and shared by every clone, so
/// all four semantic operations — and every retry attempt of each — admit
/// through the same domain state. It is deliberately process-local (plan
/// §4.7): without fleet-shared epoch/lease state it cannot claim to prevent
/// multi-Pod half-open probe storms, and that limitation is recorded in the
/// qualification notes rather than papered over.
pub(crate) struct SemanticProviderCircuit {
    failure_domain: String,
    enforce: bool,
    core: std::sync::Mutex<ProviderCircuitCore>,
}

impl SemanticProviderCircuit {
    /// Build the circuit for one failure-domain key.
    pub(crate) fn new(failure_domain: String, enforce: bool) -> Self {
        Self {
            failure_domain,
            enforce,
            core: std::sync::Mutex::new(ProviderCircuitCore {
                phase: ProviderCircuitPhase::Closed,
                epoch: 1,
                consecutive_health_failures: 0,
                open_until: Instant::now(),
                probe_deadline: Instant::now(),
                probe_held: false,
                throttle_until: None,
            }),
        }
    }

    /// Content-free failure-domain identity (a digest, never the URL).
    pub(crate) fn failure_domain(&self) -> &str {
        &self.failure_domain
    }

    /// Whether this process enforces circuit refusals (canary flag).
    ///
    /// Shadow mode still runs the full state machine and records every
    /// decision; it just never refuses a request.
    pub(crate) fn enforce(&self) -> bool {
        self.enforce
    }

    fn lock_core(&self) -> std::sync::MutexGuard<'_, ProviderCircuitCore> {
        // A poisoned lock can only follow a panic while holding it; this
        // module never panics under the lock, and recovering the (still
        // structurally valid) core keeps the circuit serving instead of
        // failing every subsequent admission.
        self.core
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Current phase snapshot (used by `Debug` and the tests).
    fn phase(&self) -> ProviderCircuitPhase {
        self.lock_core().phase
    }

    /// Fast gate / half-open probe lease for one new physical attempt.
    pub(crate) fn admit(&self) -> ProviderCircuitAdmission {
        self.admit_at(Instant::now())
    }

    fn admit_at(&self, now: Instant) -> ProviderCircuitAdmission {
        let admission = self.admit_locked(now);
        let decision = match admission {
            ProviderCircuitAdmission::Admitted { token } => {
                if token.spectator {
                    "shadow_admitted_spectator"
                } else {
                    "admitted"
                }
            }
            ProviderCircuitAdmission::Refused { reason } => reason.metric_label(),
        };
        record_circuit_gate(decision);
        admission
    }

    fn admit_locked(&self, now: Instant) -> ProviderCircuitAdmission {
        let mut core = self.lock_core();
        if let Some(throttle_until) = core.throttle_until {
            if throttle_until > now {
                return self.refuse(&core, ProviderCircuitRefusal::Throttled);
            }
        }
        match core.phase {
            ProviderCircuitPhase::Closed => ProviderCircuitAdmission::Admitted {
                token: ProviderCircuitToken {
                    epoch: core.epoch,
                    spectator: false,
                },
            },
            ProviderCircuitPhase::Open => {
                if now < core.open_until {
                    return self.refuse(&core, ProviderCircuitRefusal::Open);
                }
                // The cooldown elapsed: this admission is the one exclusive
                // real-request probe (plan §4.7 — no synthetic queries).
                core.phase = ProviderCircuitPhase::HalfOpen;
                core.epoch += 1;
                core.probe_held = true;
                core.probe_deadline = now + PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET;
                record_circuit_transition("open_to_half_open");
                record_circuit_probe("granted");
                ProviderCircuitAdmission::Admitted {
                    token: ProviderCircuitToken {
                        epoch: core.epoch,
                        spectator: false,
                    },
                }
            }
            ProviderCircuitPhase::HalfOpen => {
                if core.probe_held {
                    if now < core.probe_deadline {
                        return self.refuse(&core, ProviderCircuitRefusal::HalfOpenProbeBusy);
                    }
                    // The probe holder never observed; reclaim the lease and
                    // cool down again rather than staying wedged in HalfOpen.
                    core.phase = ProviderCircuitPhase::Open;
                    core.epoch += 1;
                    core.open_until = now + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
                    core.probe_held = false;
                    record_circuit_transition("half_open_probe_timeout");
                    record_circuit_probe("timeout");
                    return self.refuse(&core, ProviderCircuitRefusal::Open);
                }
                // The previous probe was throttled and released without a
                // transition; the next real request probes again.
                core.probe_held = true;
                core.probe_deadline = now + PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET;
                record_circuit_probe("granted");
                ProviderCircuitAdmission::Admitted {
                    token: ProviderCircuitToken {
                        epoch: core.epoch,
                        spectator: false,
                    },
                }
            }
        }
    }

    /// Record one would-be refusal: enforced refusals return [`Refused`],
    /// shadow mode admits the request anyway as a spectator.
    fn refuse(
        &self,
        core: &ProviderCircuitCore,
        reason: ProviderCircuitRefusal,
    ) -> ProviderCircuitAdmission {
        if self.enforce {
            ProviderCircuitAdmission::Refused { reason }
        } else {
            ProviderCircuitAdmission::Admitted {
                token: ProviderCircuitToken {
                    epoch: core.epoch,
                    spectator: true,
                },
            }
        }
    }

    /// No-wait epoch revalidation for one in-flight admitted attempt.
    ///
    /// Staleness means the circuit transitioned after this attempt was
    /// admitted; the caller decides whether that refusal is binding
    /// (enforcement) or shadow-recorded.
    pub(crate) fn revalidate(&self, token: ProviderCircuitToken) -> bool {
        let core = self.lock_core();
        let current = token.epoch == core.epoch;
        record_circuit_recheck(current);
        current
    }

    /// Report one completed physical attempt to the circuit.
    pub(crate) fn observe(
        &self,
        token: ProviderCircuitToken,
        observation: ProviderCircuitObservation,
    ) {
        self.observe_at(token, observation, Instant::now());
    }

    fn observe_at(
        &self,
        token: ProviderCircuitToken,
        observation: ProviderCircuitObservation,
        now: Instant,
    ) {
        let mut core = self.lock_core();
        if token.spectator {
            record_circuit_observation("spectator_ignored");
            return;
        }
        if token.epoch != core.epoch {
            // Epoch fence (plan §4.7): a late success from before a
            // transition cannot close the circuit that transition opened.
            record_circuit_observation("stale_epoch");
            return;
        }
        match core.phase {
            ProviderCircuitPhase::Closed => match observation {
                ProviderCircuitObservation::Success => {
                    core.consecutive_health_failures = 0;
                    record_circuit_observation(observation.metric_label());
                }
                ProviderCircuitObservation::HealthFailure(_) => {
                    core.consecutive_health_failures += 1;
                    record_circuit_observation(observation.metric_label());
                    if core.consecutive_health_failures >= PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD
                    {
                        core.phase = ProviderCircuitPhase::Open;
                        core.epoch += 1;
                        core.open_until = now + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
                        core.probe_held = false;
                        record_circuit_transition("closed_to_open");
                    }
                }
                ProviderCircuitObservation::Throttled {
                    retry_after_seconds,
                } => {
                    self.apply_throttle(&mut core, retry_after_seconds, now);
                    record_circuit_observation(observation.metric_label());
                }
                ProviderCircuitObservation::NotCounted => {
                    record_circuit_observation(observation.metric_label());
                }
            },
            ProviderCircuitPhase::HalfOpen => match observation {
                // Only the exclusive probe holder can carry a current-epoch
                // token in HalfOpen, so these outcomes are the probe's.
                ProviderCircuitObservation::Success => {
                    core.phase = ProviderCircuitPhase::Closed;
                    core.epoch += 1;
                    core.consecutive_health_failures = 0;
                    core.probe_held = false;
                    record_circuit_transition("half_open_to_closed");
                    record_circuit_probe("succeeded");
                    record_circuit_observation(observation.metric_label());
                }
                ProviderCircuitObservation::HealthFailure(_) => {
                    core.phase = ProviderCircuitPhase::Open;
                    core.epoch += 1;
                    core.open_until = now + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
                    core.probe_held = false;
                    record_circuit_transition("half_open_to_open");
                    record_circuit_probe("failed");
                    record_circuit_observation(observation.metric_label());
                }
                ProviderCircuitObservation::Throttled {
                    retry_after_seconds,
                } => {
                    // A throttled probe proves neither health nor sickness:
                    // release the lease, wait out the throttle, and let the
                    // next real request probe again.
                    core.probe_held = false;
                    self.apply_throttle(&mut core, retry_after_seconds, now);
                    record_circuit_probe("throttled");
                    record_circuit_observation(observation.metric_label());
                }
                ProviderCircuitObservation::NotCounted => {
                    // A per-request failure (input or 4xx) resolves nothing;
                    // the probe lease simply times out if the holder is done.
                    record_circuit_observation(observation.metric_label());
                }
            },
            ProviderCircuitPhase::Open => {
                // Unreachable with a current epoch: entering Open bumps the
                // epoch and Open admits nothing. Kept as a safe fence no-op.
                record_circuit_observation("stale_epoch");
            }
        }
    }

    /// Extend the independent 429 throttle window (never shorten one).
    ///
    /// The window is exactly the `Retry-After` when it is present and within
    /// the compiled cap, which keeps it self-consistent with the R4 full-fit
    /// retry: a retried 429 re-enters the gate only after sleeping the same
    /// delay, so the throttle has already expired by then.
    fn apply_throttle(
        &self,
        core: &mut ProviderCircuitCore,
        retry_after_seconds: Option<u64>,
        now: Instant,
    ) {
        let delay = match retry_after_seconds {
            Some(seconds) => Duration::from_secs(seconds).min(PROVIDER_CIRCUIT_THROTTLE_MAX),
            None => PROVIDER_CIRCUIT_THROTTLE_DEFAULT,
        };
        let until = now + delay;
        if core.throttle_until.is_none_or(|current| current < until) {
            core.throttle_until = Some(until);
        }
    }
}

impl fmt::Debug for SemanticProviderCircuit {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SemanticProviderCircuit")
            .field("failure_domain", &self.failure_domain)
            .field("enforce", &self.enforce)
            .field("phase", &self.phase())
            .finish()
    }
}

/// Derive the content-free identity of one Provider physical failure domain
/// (plan §4.7: endpoint identity + config epoch + request model).
///
/// The key is a SHA-256 digest: the circuit never stores, logs, or labels
/// the endpoint URL itself. The config epoch is process-local and increments
/// on every Provider construction, so a reconfigured Provider is a new
/// failure domain with fresh `Closed` state rather than an inherited one.
pub(crate) fn provider_failure_domain_key(
    endpoint: &url::Url,
    request_model: &str,
    config_epoch: u64,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"buzz-semantic-provider-failure-domain-v1");
    hasher.update([0]);
    hasher.update(endpoint.as_str().as_bytes());
    hasher.update([0]);
    hasher.update(request_model.as_bytes());
    hasher.update([0]);
    hasher.update(config_epoch.to_le_bytes());
    hex::encode(hasher.finalize())
}

/// Record one closed circuit gate decision (content-free; plan §7).
fn record_circuit_gate(decision: &'static str) {
    metrics::counter!("buzz_semantic_provider_circuit_gate_total", "decision" => decision)
        .increment(1);
}

/// Record one closed circuit state transition (content-free; plan §7).
fn record_circuit_transition(transition: &'static str) {
    metrics::counter!(
        "buzz_semantic_provider_circuit_transition_total",
        "transition" => transition,
    )
    .increment(1);
}

/// Record one closed half-open probe outcome (content-free; plan §7).
fn record_circuit_probe(outcome: &'static str) {
    metrics::counter!("buzz_semantic_provider_circuit_probe_total", "outcome" => outcome)
        .increment(1);
}

/// Record one closed circuit observation disposition (content-free; §7).
fn record_circuit_observation(disposition: &'static str) {
    metrics::counter!(
        "buzz_semantic_provider_circuit_observation_total",
        "disposition" => disposition,
    )
    .increment(1);
}

/// Record one closed epoch-revalidation outcome (content-free; plan §7).
fn record_circuit_recheck(current: bool) {
    metrics::counter!(
        "buzz_semantic_provider_circuit_recheck_total",
        "outcome" => if current { "current" } else { "stale" },
    )
    .increment(1);
}

/// Report one completed physical Provider attempt to its circuit.
///
/// Reporting exactly once per physical attempt is the coordinator's duty:
/// the one-shot envelope observes after each sanctioned `encode_once`, the
/// complete path observes after its root-attempt encode, and the reuse path
/// (no physical egress) observes nothing. Deadline and cancellation outcomes
/// observe nothing — plan §4.7 excludes them from Provider health, and an
/// abandoned half-open probe is reclaimed by its lease budget instead.
pub(crate) fn observe_provider_circuit(
    provider: &crate::semantic_provider::VolcengineSemanticProvider,
    token: Option<ProviderCircuitToken>,
    observation: ProviderCircuitObservation,
) {
    if let Some(token) = token {
        provider.circuit().observe(token, observation);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
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
                    retry_after_seconds: Some(3)
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
        let far = Instant::now() + Duration::from_secs(30);
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
        );
        let encoded =
            encode_once::<u32, u64, _>(&context, ProviderEgressObservation::Silent, async {
                Ok(7_u32)
            })
            .await
            .expect("immediate success is admitted");
        assert_eq!(encoded, 7);
        assert!(matches!(
            encode_once::<u32, u32, _>(&context, ProviderEgressObservation::Silent, async {
                Err(9_u32)
            })
            .await,
            Err(SemanticEncodeOnceFailure::Provider(9))
        ));
        let expired = Instant::now() - Duration::from_secs(1);
        let expired_context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(expired),
        );
        assert!(matches!(
            encode_once::<(), (), _>(
                &expired_context,
                ProviderEgressObservation::Silent,
                std::future::pending()
            )
            .await,
            Err(SemanticEncodeOnceFailure::DeadlineExceeded)
        ));
        assert_eq!(
            expired_context.latch().state(),
            SemanticLifecycleState::TimedOut
        );
        assert_eq!(
            expired_context.cancellation().cancelled(),
            Some(SemanticCancellationSource::DeadlineExceeded)
        );
        let cancelled_context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
        );
        let _ = cancelled_context.cancel(SemanticCancellationSource::ServerShutdown);
        assert!(matches!(
            encode_once::<(), (), _>(
                &cancelled_context,
                ProviderEgressObservation::Silent,
                std::future::pending()
            )
            .await,
            Err(SemanticEncodeOnceFailure::Cancelled(
                SemanticCancellationSource::ServerShutdown
            ))
        ));
    }

    #[test]
    fn admit_stage_refuses_work_after_cancellation_won() {
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(
                Instant::now() + Duration::from_secs(30),
            ),
        );
        assert!(context.admit_stage(SemanticDeadlineWindow::Work).is_ok());
        let _ = context.cancel(SemanticCancellationSource::CallerDisconnected);
        assert_eq!(
            context.admit_stage(SemanticDeadlineWindow::Work),
            Err(SemanticStageAbort::Cancelled(
                SemanticCancellationSource::CallerDisconnected
            ))
        );
        assert_eq!(context.latch().state(), SemanticLifecycleState::Cancelling);
        // The refusal is stable: a later deadline does not un-cancel it.
        assert_eq!(
            context.admit_stage(SemanticDeadlineWindow::Work),
            Err(SemanticStageAbort::Cancelled(
                SemanticCancellationSource::CallerDisconnected
            ))
        );
    }

    #[test]
    fn admit_stage_refuses_expired_windows_and_arbitrates_timeout() {
        let expired = Instant::now() - Duration::from_secs(1);
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(expired),
        );
        assert_eq!(
            context.admit_stage(SemanticDeadlineWindow::ProviderStart),
            Err(SemanticStageAbort::Deadline(
                SemanticDeadlineWindow::ProviderStart
            ))
        );
        // F1 item 4: a deadline that wins the latch leaves the real
        // `TimedOut` state instead of relabelling a cancellation win.
        assert_eq!(context.latch().state(), SemanticLifecycleState::TimedOut);
        assert_eq!(
            context.cancellation().cancelled(),
            Some(SemanticCancellationSource::DeadlineExceeded)
        );
    }

    #[tokio::test]
    async fn run_stage_drops_pending_work_when_cancelled() {
        struct StageGuard(Arc<AtomicBool>);
        impl Drop for StageGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let far = Instant::now() + Duration::from_secs(30);
        let context = Arc::new(SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
        ));
        let dropped = Arc::new(AtomicBool::new(false));
        let guard_dropped = Arc::clone(&dropped);
        let cancelled_context = Arc::clone(&context);
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            let _ = cancelled_context.cancel(SemanticCancellationSource::ServerShutdown);
        });
        let stage = Box::pin(async move {
            let _guard = StageGuard(guard_dropped);
            std::future::pending::<()>().await;
        });
        assert_eq!(
            context.run_stage(SemanticDeadlineWindow::Work, stage).await,
            Err(SemanticStageAbort::Cancelled(
                SemanticCancellationSource::ServerShutdown
            ))
        );
        assert!(dropped.load(Ordering::SeqCst), "cancelled stage must drop");
        canceller.await.expect("canceller task must finish");
        assert_eq!(context.latch().state(), SemanticLifecycleState::Cancelling);
    }

    #[tokio::test]
    async fn run_stage_drops_pending_work_when_window_expires() {
        struct StageGuard(Arc<AtomicBool>);
        impl Drop for StageGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::new(
                Instant::now() + Duration::from_millis(10),
                Instant::now() + Duration::from_millis(10),
                Instant::now() + Duration::from_secs(30),
                Instant::now() + Duration::from_secs(30),
            )
            .expect("ordered windows"),
        );
        let dropped = Arc::new(AtomicBool::new(false));
        let guard = StageGuard(Arc::clone(&dropped));
        let stage = Box::pin(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        assert_eq!(
            context.run_stage(SemanticDeadlineWindow::Work, stage).await,
            Err(SemanticStageAbort::Deadline(SemanticDeadlineWindow::Work))
        );
        assert!(dropped.load(Ordering::SeqCst), "timed-out stage must drop");
        assert_eq!(context.latch().state(), SemanticLifecycleState::TimedOut);
        assert_eq!(
            context.cancellation().cancelled(),
            Some(SemanticCancellationSource::DeadlineExceeded)
        );
    }

    #[test]
    fn finalization_latch_discards_only_after_post_check() {
        let far = Instant::now() + Duration::from_secs(30);
        let context = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
        );
        // Cancel won before the release permit: finalization may not start.
        let _ = context.cancel(SemanticCancellationSource::ServerShutdown);
        assert!(matches!(
            context.latch().begin_finalize(),
            SemanticLatchOutcome::LostTerminal(SemanticLifecycleState::Cancelling)
        ));

        let finalizer = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
        );
        assert!(matches!(
            finalizer.latch().begin_finalize(),
            SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing)
        ));
        // Finalization is a single winner.
        assert!(matches!(
            finalizer.latch().begin_finalize(),
            SemanticLatchOutcome::LostTerminal(SemanticLifecycleState::Finalizing)
        ));
        // A cancel arriving during synchronous signing only requests discard.
        assert!(matches!(
            finalizer.cancel(SemanticCancellationSource::DeadlineExceeded),
            SemanticLatchOutcome::LostToFinalizing(SemanticCancellationSource::DeadlineExceeded)
        ));
        assert!(finalizer.latch().discard_requested());
        assert_eq!(
            finalizer.latch().discard_source(),
            Some(SemanticCancellationSource::DeadlineExceeded)
        );
        // The mandatory post-check runs before completion; the still-running
        // synchronous signing may finish, but its result is never sent.
        assert_eq!(
            finalizer.latch().state(),
            SemanticLifecycleState::Finalizing
        );
        assert_eq!(
            finalizer.latch().complete(),
            SemanticLifecycleState::Completed
        );
    }

    // -------------------------------------------------------------------
    // R4 closed retry policy.
    // -------------------------------------------------------------------

    fn retry_context(
        class: SemanticOperationAttemptClass,
        work: Duration,
    ) -> SemanticExecutionContext {
        SemanticExecutionContext::new(
            class,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(Instant::now() + work),
        )
    }

    fn connect_not_started() -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::ConnectNotStarted,
            handoff: ProviderHandoffCertainty::NotStarted,
        }
    }

    fn rate_limited(retry_after_seconds: Option<u64>) -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::RateLimited {
                retry_after_seconds,
            },
            handoff: ProviderHandoffCertainty::ConfirmedResponse,
        }
    }

    fn retryable_server_error() -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::RetryableResponse { status_class: 500 },
            handoff: ProviderHandoffCertainty::ConfirmedResponse,
        }
    }

    #[test]
    fn retry_matrix_enables_exactly_the_compiled_route_rows() {
        let context = retry_context(
            SemanticOperationAttemptClass::OneShot,
            Duration::from_secs(60),
        );
        // Item 1: connect-not-started retries with a full-jitter backoff.
        match provider_retry_decision(ProviderRetryRoute::R4, connect_not_started(), &context) {
            ProviderRetryDecision::Retry { backoff } => assert!(
                backoff <= Duration::from_millis(PROVIDER_RETRY_FULL_JITTER_BASE_MS),
                "jittered backoff must stay within the compiled base"
            ),
            ProviderRetryDecision::Terminal => {
                panic!("connect-not-started must retry under the R4 route")
            }
        }
        // Item 2: only a syntactically valid Retry-After retries, verbatim.
        assert_eq!(
            provider_retry_decision(ProviderRetryRoute::R4, rate_limited(Some(3)), &context),
            ProviderRetryDecision::Retry {
                backoff: Duration::from_secs(3)
            }
        );
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, rate_limited(None), &context),
            ProviderRetryDecision::Terminal
        ));
        // Item 3: a definitive 5xx retries with a full-jitter backoff.
        match provider_retry_decision(ProviderRetryRoute::R4, retryable_server_error(), &context) {
            ProviderRetryDecision::Retry { backoff } => assert!(
                backoff <= Duration::from_millis(PROVIDER_RETRY_FULL_JITTER_BASE_MS),
                "jittered backoff must stay within the compiled base"
            ),
            ProviderRetryDecision::Terminal => panic!("5xx must retry under the R4 route"),
        }

        // Each disabled route flag keeps its row terminal.
        let no_connect = ProviderRetryRoute {
            connect_not_started: false,
            rate_limited_full_retry_after: true,
            retryable_server_error: true,
        };
        assert!(matches!(
            provider_retry_decision(no_connect, connect_not_started(), &context),
            ProviderRetryDecision::Terminal
        ));
        let no_rate_limit = ProviderRetryRoute {
            connect_not_started: true,
            rate_limited_full_retry_after: false,
            retryable_server_error: true,
        };
        assert!(matches!(
            provider_retry_decision(no_rate_limit, rate_limited(Some(3)), &context),
            ProviderRetryDecision::Terminal
        ));
        let no_server_error = ProviderRetryRoute {
            connect_not_started: true,
            rate_limited_full_retry_after: true,
            retryable_server_error: false,
        };
        assert!(matches!(
            provider_retry_decision(no_server_error, retryable_server_error(), &context),
            ProviderRetryDecision::Terminal
        ));

        // Nothing outside the matrix retries, whatever the route says.
        for kind in [
            ProviderAttemptFailureKind::Rejected { status: 400 },
            ProviderAttemptFailureKind::OutcomeUnknown,
            ProviderAttemptFailureKind::ProtocolInvalid,
        ] {
            let failure = ProviderAttemptFailure {
                kind,
                handoff: ProviderHandoffCertainty::ConfirmedResponse,
            };
            assert!(
                matches!(
                    provider_retry_decision(ProviderRetryRoute::R4, failure, &context),
                    ProviderRetryDecision::Terminal
                ),
                "kind {kind:?} must stay terminal"
            );
        }
    }

    #[test]
    fn retry_backoff_must_fully_fit_the_remaining_work_window() {
        // A Retry-After longer than the remaining window is not honored.
        let short = retry_context(
            SemanticOperationAttemptClass::OneShot,
            Duration::from_secs(2),
        );
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, rate_limited(Some(5)), &short),
            ProviderRetryDecision::Terminal
        ));
        // An expired window never retries, even for the jittered rows.
        let expired = SemanticExecutionContext::new(
            SemanticOperationAttemptClass::OneShot,
            SemanticDeadlineWindows::for_one_shot_hard_deadline(
                Instant::now() - Duration::from_secs(1),
            ),
        );
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, connect_not_started(), &expired),
            ProviderRetryDecision::Terminal
        ));
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, retryable_server_error(), &expired),
            ProviderRetryDecision::Terminal
        ));
    }

    #[test]
    fn retry_requires_remaining_ledger_budget() {
        let context = retry_context(
            SemanticOperationAttemptClass::OneShot,
            Duration::from_secs(60),
        );
        let ledger = context.ledger();
        assert!(ledger.begin_provider_attempt().is_ok());
        // The single transport-retry token is still available.
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, connect_not_started(), &context),
            ProviderRetryDecision::Retry { .. }
        ));
        assert!(ledger.begin_provider_attempt().is_ok());
        assert!(!ledger.can_begin_provider_attempt());
        assert!(matches!(
            provider_retry_decision(ProviderRetryRoute::R4, connect_not_started(), &context),
            ProviderRetryDecision::Terminal
        ));
    }

    #[test]
    fn ledger_probes_agree_with_the_begin_outcomes() {
        // One-shot: the physical cap covers the initial send plus the single
        // transport retry.
        let one_shot = retry_context(
            SemanticOperationAttemptClass::OneShot,
            Duration::from_secs(60),
        );
        let one_shot_ledger = one_shot.ledger();
        assert!(one_shot_ledger.can_begin_provider_attempt());
        assert!(one_shot_ledger.begin_provider_attempt().is_ok());
        assert!(one_shot_ledger.can_begin_provider_attempt());
        assert!(one_shot_ledger.begin_provider_attempt().is_ok());
        assert!(!one_shot_ledger.can_begin_provider_attempt());
        assert!(one_shot_ledger.begin_provider_attempt().is_err());

        // Complete path: three physical attempts across two root attempts —
        // two in the first attempt (initial plus the transport retry), then
        // one more after the fresh root restart resets the per-attempt
        // transport token.
        let complete = retry_context(
            SemanticOperationAttemptClass::CompletePath,
            Duration::from_secs(60),
        );
        let complete_ledger = complete.ledger();
        assert!(complete_ledger.begin_operation_attempt().is_ok());
        assert!(complete_ledger.begin_provider_attempt().is_ok());
        assert!(complete_ledger.begin_provider_attempt().is_ok());
        assert!(!complete_ledger.can_begin_provider_attempt());
        assert!(complete_ledger.begin_operation_attempt().is_ok());
        assert!(complete_ledger.can_begin_provider_attempt());
        assert!(complete_ledger.begin_provider_attempt().is_ok());
        assert!(!complete_ledger.can_begin_provider_attempt());
        assert!(complete_ledger.begin_provider_attempt().is_err());

        // Operation restart budget: the first attempt is free, exactly one
        // restart is allowed, and further restarts exhaust the ledger.
        let restarts = retry_context(
            SemanticOperationAttemptClass::OneShot,
            Duration::from_secs(60),
        );
        let restart_ledger = restarts.ledger();
        assert!(restart_ledger.can_begin_operation_attempt());
        assert!(restart_ledger.begin_operation_attempt().is_ok());
        assert!(restart_ledger.can_begin_operation_attempt());
        assert!(restart_ledger.begin_operation_attempt().is_ok());
        assert!(!restart_ledger.can_begin_operation_attempt());
        assert!(restart_ledger.begin_operation_attempt().is_err());
    }

    #[test]
    fn transport_send_failure_uses_the_transports_connect_knowledge() {
        let connect = ProviderAttemptFailure::transport_send_failure(true);
        assert_eq!(connect.kind, ProviderAttemptFailureKind::ConnectNotStarted);
        assert_eq!(connect.handoff, ProviderHandoffCertainty::NotStarted);
        let unknown = ProviderAttemptFailure::transport_send_failure(false);
        assert_eq!(unknown.kind, ProviderAttemptFailureKind::OutcomeUnknown);
        assert_eq!(unknown.handoff, ProviderHandoffCertainty::OutcomeUnknown);
    }

    #[test]
    fn full_jitter_backoff_stays_within_the_compiled_base() {
        for _ in 0..256 {
            let backoff = full_jitter_backoff();
            assert!(backoff <= Duration::from_millis(PROVIDER_RETRY_FULL_JITTER_BASE_MS));
        }
    }

    // -------------------------------------------------------------------
    // R5 shared Provider circuit.
    // -------------------------------------------------------------------

    fn transport_unknown() -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::OutcomeUnknown,
            handoff: ProviderHandoffCertainty::OutcomeUnknown,
        }
    }

    fn protocol_invalid(handoff: ProviderHandoffCertainty) -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::ProtocolInvalid,
            handoff,
        }
    }

    fn rejected() -> ProviderAttemptFailure {
        ProviderAttemptFailure {
            kind: ProviderAttemptFailureKind::Rejected { status: 400 },
            handoff: ProviderHandoffCertainty::ConfirmedResponse,
        }
    }

    fn new_circuit(enforce: bool) -> SemanticProviderCircuit {
        SemanticProviderCircuit::new("test-failure-domain".to_owned(), enforce)
    }

    fn admitted(admission: ProviderCircuitAdmission) -> ProviderCircuitToken {
        match admission {
            ProviderCircuitAdmission::Admitted { token } => token,
            ProviderCircuitAdmission::Refused { reason } => {
                panic!("expected an admission, got refused: {reason:?}")
            }
        }
    }

    fn refused_reason(admission: ProviderCircuitAdmission) -> ProviderCircuitRefusal {
        match admission {
            ProviderCircuitAdmission::Refused { reason } => reason,
            ProviderCircuitAdmission::Admitted { token } => {
                panic!("expected a refusal, got admitted: {token:?}")
            }
        }
    }

    /// Drive one full (admit, observe) physical attempt at `now`.
    fn attempt(
        circuit: &SemanticProviderCircuit,
        observation: ProviderCircuitObservation,
        now: Instant,
    ) -> ProviderCircuitAdmission {
        let admission = circuit.admit_at(now);
        if let ProviderCircuitAdmission::Admitted { token } = admission {
            circuit.observe_at(token, observation, now);
        }
        admission
    }

    #[test]
    fn health_classification_matches_the_compiled_circuit_rows() {
        use ProviderCircuitObservation::*;
        use ProviderHealthFailureClass::*;
        let cases = [
            (connect_not_started(), HealthFailure(Connect)),
            (retryable_server_error(), HealthFailure(ServerError)),
            (transport_unknown(), HealthFailure(TransportUnknown)),
            (
                protocol_invalid(ProviderHandoffCertainty::ConfirmedResponse),
                HealthFailure(ProtocolInvalid),
            ),
            (
                protocol_invalid(ProviderHandoffCertainty::NotStarted),
                NotCounted,
            ),
            (rejected(), NotCounted),
            (
                rate_limited(Some(2)),
                Throttled {
                    retry_after_seconds: Some(2),
                },
            ),
            (
                rate_limited(None),
                Throttled {
                    retry_after_seconds: None,
                },
            ),
        ];
        for (failure, expected) in cases {
            assert_eq!(
                ProviderCircuitObservation::from_attempt_failure(&failure),
                expected,
                "wrong circuit classification for {failure:?}"
            );
        }
    }

    #[test]
    fn circuit_constants_form_the_compiled_contract() {
        assert_eq!(PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD, 5);
        assert_eq!(PROVIDER_CIRCUIT_OPEN_COOLDOWN, Duration::from_secs(15));
        assert_eq!(
            PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET,
            Duration::from_secs(15)
        );
        assert_eq!(
            PROVIDER_CIRCUIT_THROTTLE_DEFAULT,
            Duration::from_millis(1_000)
        );
        assert_eq!(PROVIDER_CIRCUIT_THROTTLE_MAX, Duration::from_secs(60));
    }

    #[test]
    fn fleet_runtime_digest_binds_the_compiled_reliability_contract() {
        // R6 qualification (plan §12.2): the reliability contract hashed into
        // the fleet runtime digest must restate the constants compiled into
        // this runtime, so changing either side without an explicit dated
        // descriptor bump fails here and fails the fleet digest.
        let contract = buzz_semantic_query::SEMANTIC_RELIABILITY_RUNTIME_CONTRACT;
        let one_shot = SemanticOperationAttemptClass::OneShot;
        let complete_path = SemanticOperationAttemptClass::CompletePath;
        let expected_lines = [
            format!(
                "attempt-caps=one-shot-physical-{};complete-path-physical-{};operation-attempt-{}",
                one_shot.physical_provider_attempt_cap(),
                complete_path.physical_provider_attempt_cap(),
                one_shot.operation_attempt_cap(),
            ),
            format!(
                "backoff=full-jitter-base-{}ms",
                PROVIDER_RETRY_FULL_JITTER_BASE_MS
            ),
            format!(
                "one-shot-reserves=eighths-{};provider-start-4-eighths;work-2-eighths;snapshot-close-1-eighth;absolute-public-unchanged",
                ONE_SHOT_RESERVE_DENOMINATOR,
            ),
            format!(
                "circuit-caps=health-threshold-{};open-cooldown-{}s;probe-budget-{}s;throttle-default-{}ms;throttle-max-{}s",
                PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD,
                PROVIDER_CIRCUIT_OPEN_COOLDOWN.as_secs(),
                PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET.as_secs(),
                PROVIDER_CIRCUIT_THROTTLE_DEFAULT.as_millis(),
                PROVIDER_CIRCUIT_THROTTLE_MAX.as_secs(),
            ),
        ];
        for line in expected_lines {
            assert!(
                contract.contains(&line),
                "fleet reliability descriptor must pin the compiled constants: missing {line}"
            );
        }

        // The two literal ledger budgets in the descriptor are pinned by
        // exercising the ledger itself: two release confirmations, and a
        // single transport-retry token per operation attempt.
        let ledger = SemanticAttemptLedger::new(SemanticOperationAttemptClass::CompletePath);
        let mut release_budget = 0;
        while ledger.begin_release_confirmation().is_ok() {
            release_budget += 1;
        }
        assert!(contract.contains(&format!("release-confirmation-{release_budget}")));
        assert!(
            ledger.begin_provider_attempt().is_ok(),
            "first physical attempt must be admitted"
        );
        let mut transport_tokens = 0;
        while ledger.begin_provider_attempt().is_ok() {
            transport_tokens += 1;
        }
        // One admitted retry within the attempt plus the exhausted token:
        // the descriptor's "transport-retry-token-1-per-attempt" is the
        // budget that stopped this loop.
        assert_eq!(transport_tokens, 1);
        assert!(contract.contains("transport-retry-token-1-per-attempt"));
    }

    #[test]
    fn consecutive_health_failures_trip_open_at_the_compiled_threshold() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD - 1 {
            // Interspersed non-health outcomes neither reset nor extend the
            // consecutive streak.
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&rejected()),
                t0,
            );
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
        attempt(
            &circuit,
            ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
            t0,
        );
        assert_eq!(
            refused_reason(circuit.admit_at(t0 + Duration::from_secs(1))),
            ProviderCircuitRefusal::Open,
            "the threshold-th consecutive health failure must trip Open"
        );
        // A success resets the streak, so the same count is needed again.
        let healthy = new_circuit(true);
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &healthy,
                ProviderCircuitObservation::from_attempt_failure(&connect_not_started()),
                t0,
            );
            attempt(&healthy, ProviderCircuitObservation::Success, t0);
        }
        assert_eq!(healthy.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn late_old_epoch_success_cannot_close_the_new_circuit() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        // Hold a token from the Closed epoch across the trip to Open.
        let stale = admitted(circuit.admit_at(t0));
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Open);
        circuit.observe_at(stale, ProviderCircuitObservation::Success, t0);
        assert_eq!(
            circuit.phase(),
            ProviderCircuitPhase::Open,
            "a late old-epoch success must not close the new circuit"
        );
        assert!(!circuit.revalidate(stale));
        // Even after the cooldown the circuit probes rather than trusting
        // the stale success.
        let probe = admitted(circuit.admit_at(t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN));
        circuit.observe_at(probe, ProviderCircuitObservation::Success, t0);
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn open_circuit_grants_exactly_one_half_open_probe() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        let after_cooldown = t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
        let first = circuit.admit_at(after_cooldown);
        assert_eq!(circuit.phase(), ProviderCircuitPhase::HalfOpen);
        assert_eq!(
            refused_reason(circuit.admit_at(after_cooldown + Duration::from_millis(1))),
            ProviderCircuitRefusal::HalfOpenProbeBusy,
            "the half-open probe must be exclusive"
        );
        // The probe holder succeeds: the domain closes for everyone.
        circuit.observe_at(
            admitted(first),
            ProviderCircuitObservation::Success,
            after_cooldown,
        );
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
        assert!(matches!(
            circuit.admit_at(after_cooldown + Duration::from_millis(2)),
            ProviderCircuitAdmission::Admitted { .. }
        ));
    }

    #[test]
    fn probe_failure_reopens_and_probe_timeout_reclaims_the_lease() {
        let t0 = Instant::now();
        // A failing probe reopens with a fresh cooldown.
        let failed = new_circuit(true);
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &failed,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        let probe_at = t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
        failed.observe_at(
            admitted(failed.admit_at(probe_at)),
            ProviderCircuitObservation::from_attempt_failure(&connect_not_started()),
            probe_at,
        );
        assert_eq!(failed.phase(), ProviderCircuitPhase::Open);
        assert_eq!(
            refused_reason(failed.admit_at(probe_at + Duration::from_secs(1))),
            ProviderCircuitRefusal::Open
        );

        // A probe holder that never observes is reclaimed by its budget.
        let wedged = new_circuit(true);
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &wedged,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        let probe_start = t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
        let _lease = wedged.admit_at(probe_start);
        assert_eq!(
            refused_reason(wedged.admit_at(probe_start + Duration::from_secs(1))),
            ProviderCircuitRefusal::HalfOpenProbeBusy
        );
        let expired = probe_start + PROVIDER_CIRCUIT_HALF_OPEN_PROBE_BUDGET;
        assert_eq!(
            refused_reason(wedged.admit_at(expired)),
            ProviderCircuitRefusal::Open,
            "an abandoned probe must be reclaimed into a fresh Open cooldown"
        );
        let reprobe = wedged.admit_at(expired + PROVIDER_CIRCUIT_OPEN_COOLDOWN);
        wedged.observe_at(
            admitted(reprobe),
            ProviderCircuitObservation::Success,
            expired,
        );
        assert_eq!(wedged.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn rate_limiting_feeds_the_independent_throttle_not_health() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        // Far more 429s than the health threshold never trip Open.
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD * 20 {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&rate_limited(None)),
                t0,
            );
        }
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
        // The throttle window itself still sheds load...
        assert_eq!(
            refused_reason(circuit.admit_at(t0 + Duration::from_millis(1))),
            ProviderCircuitRefusal::Throttled
        );
        // ...expires on schedule...
        assert!(matches!(
            circuit.admit_at(t0 + PROVIDER_CIRCUIT_THROTTLE_DEFAULT + Duration::from_millis(1)),
            ProviderCircuitAdmission::Admitted { .. }
        ));
        // ...and the health streak is untouched: the full threshold is
        // still required to trip Open.
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD - 1 {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn throttle_windows_use_retry_after_within_the_compiled_cap() {
        let t0 = Instant::now();
        // A supplied Retry-After sets the window verbatim (within the cap),
        // which is exactly the delay an R4 full-fit retry sleeps.
        let supplied = new_circuit(true);
        attempt(
            &supplied,
            ProviderCircuitObservation::Throttled {
                retry_after_seconds: Some(3),
            },
            t0,
        );
        assert_eq!(
            refused_reason(supplied.admit_at(t0 + Duration::from_secs(2))),
            ProviderCircuitRefusal::Throttled
        );
        assert!(matches!(
            supplied.admit_at(t0 + Duration::from_secs(3)),
            ProviderCircuitAdmission::Admitted { .. }
        ));
        // A hostile Retry-After is capped, and a later shorter window never
        // shortens an active one.
        let capped = new_circuit(true);
        attempt(
            &capped,
            ProviderCircuitObservation::Throttled {
                retry_after_seconds: Some(3_600),
            },
            t0,
        );
        assert_eq!(
            refused_reason(
                capped.admit_at(t0 + PROVIDER_CIRCUIT_THROTTLE_MAX - Duration::from_millis(1))
            ),
            ProviderCircuitRefusal::Throttled
        );
        assert!(matches!(
            capped.admit_at(t0 + PROVIDER_CIRCUIT_THROTTLE_MAX + Duration::from_millis(1)),
            ProviderCircuitAdmission::Admitted { .. }
        ));
    }

    #[test]
    fn throttled_probe_releases_the_lease_without_transition() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        let probe_at = t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN;
        circuit.observe_at(
            admitted(circuit.admit_at(probe_at)),
            ProviderCircuitObservation::Throttled {
                retry_after_seconds: Some(1),
            },
            probe_at,
        );
        assert_eq!(circuit.phase(), ProviderCircuitPhase::HalfOpen);
        assert_eq!(
            refused_reason(circuit.admit_at(probe_at + Duration::from_millis(1))),
            ProviderCircuitRefusal::Throttled,
            "the throttle window gates the next probe too"
        );
        let reprobe = circuit.admit_at(probe_at + Duration::from_secs(1));
        circuit.observe_at(
            admitted(reprobe),
            ProviderCircuitObservation::Success,
            probe_at,
        );
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn shadow_mode_admits_spectators_and_keeps_the_simulated_state() {
        let circuit = new_circuit(false);
        let t0 = Instant::now();
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&retryable_server_error()),
                t0,
            );
        }
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Open);
        assert!(!circuit.enforce());
        // Shadow still serves the request, but as a spectator whose outcome
        // cannot move the simulated state.
        let spectator = admitted(circuit.admit_at(t0 + Duration::from_secs(1)));
        assert!(spectator.spectator);
        circuit.observe_at(spectator, ProviderCircuitObservation::Success, t0);
        assert_eq!(
            circuit.phase(),
            ProviderCircuitPhase::Open,
            "a spectator outcome must not close the simulated circuit"
        );
        // The simulation matches enforcement: after the cooldown exactly
        // one probe decides.
        let probe = admitted(circuit.admit_at(t0 + PROVIDER_CIRCUIT_OPEN_COOLDOWN));
        assert!(!probe.spectator);
        circuit.observe_at(probe, ProviderCircuitObservation::Success, t0);
        assert_eq!(circuit.phase(), ProviderCircuitPhase::Closed);
    }

    #[test]
    fn failure_domain_key_is_content_free_and_fenced_by_config_epoch() {
        let endpoint: url::Url = "https://provider.example/api/"
            .parse()
            .expect("valid test URL");
        let key = provider_failure_domain_key(&endpoint, "model-a", 1);
        assert_eq!(key.len(), 64, "the key must be a SHA-256 digest hex");
        assert!(
            key.chars().all(|c| c.is_ascii_hexdigit()),
            "the key must be hex"
        );
        assert!(
            !key.contains("provider.example") && !key.contains("model-a"),
            "the key must not embed the endpoint or model text"
        );
        assert_eq!(key, provider_failure_domain_key(&endpoint, "model-a", 1));
        // Endpoint identity, request model, and config epoch each fence the
        // domain: any change is a new failure domain, never a shared one.
        let other_endpoint: url::Url = "https://other.example/api/"
            .parse()
            .expect("valid test URL");
        assert_ne!(
            key,
            provider_failure_domain_key(&other_endpoint, "model-a", 1)
        );
        assert_ne!(key, provider_failure_domain_key(&endpoint, "model-b", 1));
        assert_ne!(key, provider_failure_domain_key(&endpoint, "model-a", 2));
    }

    #[test]
    fn not_counted_outcomes_never_trip_the_circuit() {
        let circuit = new_circuit(true);
        let t0 = Instant::now();
        for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD * 10 {
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&rejected()),
                t0,
            );
            attempt(
                &circuit,
                ProviderCircuitObservation::from_attempt_failure(&protocol_invalid(
                    ProviderHandoffCertainty::NotStarted,
                )),
                t0,
            );
        }
        assert_eq!(
            circuit.phase(),
            ProviderCircuitPhase::Closed,
            "per-request 4xx and pre-transport input failures are not Provider health"
        );
    }

    // -------------------------------------------------------------------
    // R6 qualification soak (plan §8 R6): cancellation and shutdown
    // propagation under repetition. Every iteration races one of the four
    // closed cancellation sources against one of three lifecycle shapes and
    // must finish inside its own small budget, drop its pending stage work,
    // keep later stage admission refused, and never let a losing finalizer
    // escape the post-check. A wedge or leak in the unified lifecycle shows
    // up here as a hard iteration timeout instead of a hanging test.
    // -------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancellation_and_shutdown_soak_stays_bounded_and_clean() {
        struct StageGuard(Arc<AtomicBool>);
        impl Drop for StageGuard {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        const ITERATIONS: usize = 240;
        for index in 0..ITERATIONS {
            let source = match index % 4 {
                0 => SemanticCancellationSource::CallerDisconnected,
                1 => SemanticCancellationSource::ServerShutdown,
                2 => SemanticCancellationSource::DeadlineExceeded,
                _ => SemanticCancellationSource::ExplicitCancel,
            };
            let shape = index % 3;
            tokio::time::timeout(Duration::from_secs(5), async move {
                match shape {
                    0 => {
                        // Mid-stage cancellation: the pending future is
                        // dropped, the refusal is stable, and the latch
                        // settles in Cancelling.
                        let far = Instant::now() + Duration::from_secs(30);
                        let context = Arc::new(SemanticExecutionContext::new(
                            SemanticOperationAttemptClass::OneShot,
                            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
                        ));
                        let dropped = Arc::new(AtomicBool::new(false));
                        let guard_dropped = Arc::clone(&dropped);
                        let cancelled_context = Arc::clone(&context);
                        let canceller = tokio::spawn(async move {
                            tokio::time::sleep(Duration::from_millis(1)).await;
                            let _ = cancelled_context
                                .cancel(SemanticCancellationSource::ServerShutdown);
                        });
                        let stage = Box::pin(async move {
                            let _guard = StageGuard(guard_dropped);
                            std::future::pending::<()>().await;
                        });
                        assert_eq!(
                            context.run_stage(SemanticDeadlineWindow::Work, stage).await,
                            Err(SemanticStageAbort::Cancelled(
                                SemanticCancellationSource::ServerShutdown
                            ))
                        );
                        assert!(dropped.load(Ordering::SeqCst));
                        assert_eq!(context.latch().state(), SemanticLifecycleState::Cancelling);
                        assert_eq!(
                            context.admit_stage(SemanticDeadlineWindow::Work),
                            Err(SemanticStageAbort::Cancelled(
                                SemanticCancellationSource::ServerShutdown
                            ))
                        );
                        canceller.await.expect("canceller task must finish");
                    }
                    1 => {
                        // Work-window expiry inside the stage: mandatory
                        // cleanup still runs and the deadline source is
                        // recorded once.
                        let soon = Instant::now() + Duration::from_millis(1);
                        let far = Instant::now() + Duration::from_secs(30);
                        let context = SemanticExecutionContext::new(
                            SemanticOperationAttemptClass::OneShot,
                            SemanticDeadlineWindows::new(soon, soon, far, far)
                                .expect("ordered windows"),
                        );
                        let dropped = Arc::new(AtomicBool::new(false));
                        let guard = StageGuard(Arc::clone(&dropped));
                        let stage = Box::pin(async move {
                            let _guard = guard;
                            std::future::pending::<()>().await;
                        });
                        assert_eq!(
                            context.run_stage(SemanticDeadlineWindow::Work, stage).await,
                            Err(SemanticStageAbort::Deadline(SemanticDeadlineWindow::Work))
                        );
                        assert!(dropped.load(Ordering::SeqCst));
                        assert_eq!(
                            context.cancellation().cancelled(),
                            Some(SemanticCancellationSource::DeadlineExceeded)
                        );
                    }
                    _ => {
                        // Finalize/cancel race: a finalizer that already won
                        // records the later cancellation as a discard and may
                        // never send; the cancellation side loses cleanly.
                        let far = Instant::now() + Duration::from_secs(30);
                        let context = SemanticExecutionContext::new(
                            SemanticOperationAttemptClass::OneShot,
                            SemanticDeadlineWindows::for_one_shot_hard_deadline(far),
                        );
                        assert_eq!(
                            context.latch().begin_finalize(),
                            SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing)
                        );
                        assert_eq!(
                            context.cancel(source),
                            SemanticLatchOutcome::LostToFinalizing(source)
                        );
                        assert!(context.latch().discard_requested());
                        assert_eq!(context.latch().discard_source(), Some(source));
                        // F1 item 5: the post-check discards the signed
                        // result; generic new work is forbidden beside the
                        // finalizer, whose own stages still admit, and the
                        // state completes.
                        assert!(context.latch().state().forbids_new_semantic_work());
                        assert!(context.latch().state().admits_finalize_stage());
                        assert_eq!(
                            context.latch().complete(),
                            SemanticLifecycleState::Completed
                        );
                    }
                }
            })
            .await
            .expect("soak iteration must stay inside its budget");
        }
    }
}
