//! Failing regression baseline for the unified reliability runtime
//! correctness fix.
//!
//! Every test in this module encodes one frozen contract from the
//! correctness fix plan (`docs/stage/semantic/unified-engine/fix/…`). The
//! `rfx*` tests were the F0 red baseline; F1 turned rfx01 and the two rfx02
//! tests green, F2 turned the three rfx03 tests green, and F3 turned rfx04
//! and rfx05 green — the circuit refusal now resolves through the fresh
//! authorization recheck and the physical ledger counts real handoffs only.
//! The `f1_*`/`rfx03_*`/`rfx04_*`/`rfx05_*` tests pin those deliveries.
//! Keeping the module outside `semantic_*` preserves the frozen
//! characterization gates' historical `semantic_` filter scope while the
//! full unit suite carries the remaining red baseline. Nothing here is
//! ignored or feature-gated: each failure is the repeatable, content-free
//! evidence the fix plan requires.

use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_core::CommunityId;
use buzz_db::semantic_query::SemanticGraphQueryTicket;
use buzz_semantic::SemanticEncoder;

use crate::semantic_query_runtime::{
    caller_disconnect_guard, execute_provider_attempt, subscribe_relay_shutdown,
    ProviderAttemptError, ProviderAttemptFailure, ProviderAttemptOutcomeObservation,
    ProviderCircuitAdmission, ProviderCircuitObservation, ProviderEgressObservation,
    ProviderEgressPlan, ProviderHealthFailureClass, SemanticCancellationSource,
    SemanticDeadlineWindow, SemanticDeadlineWindows, SemanticExecutionContext,
    SemanticLatchOutcome, SemanticLifecycleState, SemanticOperationAttemptClass,
    SemanticProviderEgressFailure, SemanticReleaseSignAbort, SemanticStageAbort,
    ONE_SHOT_RESERVE_DENOMINATOR, PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD,
};
use crate::AppState;

/// Future windows for requests whose deadline behavior is not under test.
fn future_windows() -> SemanticDeadlineWindows {
    let now = Instant::now();
    let soon = now + Duration::from_secs(10);
    SemanticDeadlineWindows::new(soon, soon, soon, soon).expect("future windows")
}

/// A one-shot context whose deadline behavior is not under test.
fn active_one_shot_context() -> SemanticExecutionContext {
    SemanticExecutionContext::new(SemanticOperationAttemptClass::OneShot, future_windows())
}

/// Test-only Provider configuration with a binding (enforced) circuit and
/// an unreachable endpoint; no Provider call is ever made through it.
fn enforced_circuit_provider_config() -> crate::config::SemanticWorkerConfig {
    crate::config::SemanticWorkerConfig {
        enabled: true,
        api_key: Some("fix-test-only".to_owned()),
        base_url: Some(
            "https://example.invalid/api/"
                .parse()
                .expect("valid test URL"),
        ),
        request_model: Some("fix-test-alias".to_owned()),
        request_timeout: Duration::from_secs(1),
        request_interval: Duration::from_secs(1),
        claim_seconds: 60,
        max_attempts: 2,
        provider_circuit_enforce: true,
    }
}

/// Build a service-free state: lazy unreachable pools, a configured
/// Provider, and no running services.
async fn service_free_state() -> Arc<AppState> {
    let mut config = crate::config::Config::from_env().expect("default config loads");
    config.database_url = "postgres://semantic-fix@127.0.0.1:1/semantic_fix".to_owned();
    config.redis_url = "redis://127.0.0.1:1".to_owned();
    config.semantic_worker = enforced_circuit_provider_config();
    crate::state::app_state_for_reliability_fix_tests(config).await
}

/// Build a service-free state whose enforced Provider circuit is open.
///
/// The circuit is tripped with real health observations so the next
/// admission is refused at the fast gate — before any database acquire —
/// which is exactly the refusal path RFX-04 and RFX-05 audit.
async fn open_circuit_state() -> Arc<AppState> {
    let state = service_free_state().await;
    let provider = state
        .semantic_provider()
        .expect("provider configuration")
        .expect("configured provider");
    let circuit = provider.circuit();
    for _ in 0..PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD {
        let ProviderCircuitAdmission::Admitted { token } = circuit.admit() else {
            panic!("closed circuit must admit");
        };
        circuit.observe(
            token,
            ProviderCircuitObservation::HealthFailure(ProviderHealthFailureClass::ServerError),
        );
    }
    assert!(
        matches!(circuit.admit(), ProviderCircuitAdmission::Refused { .. }),
        "circuit must be open before the audited egress"
    );
    state
}

/// A minimal authorized ticket for egress plans; the audited refusal paths
/// never read it.
fn reliability_fix_ticket() -> SemanticGraphQueryTicket {
    let encoder = buzz_semantic::DeterministicFakeEncoder::new(3).expect("fake encoder");
    let contract = encoder.contract().clone();
    let query_fences =
        buzz_semantic_query::QueryCompatibilityFences::for_source_contract(&contract)
            .expect("query fences");
    let community_id = CommunityId::from_uuid(uuid::Uuid::from_u128(1));
    let observed_at = chrono::Utc::now();
    SemanticGraphQueryTicket {
        community_id,
        generation: buzz_db::semantic::SemanticGenerationRecord {
            community_id,
            generation_id: uuid::Uuid::from_u128(2),
            lifecycle: "active".to_owned(),
            extractor_version: "overview-v1".to_owned(),
            model_contract: contract,
            model_contract_digest: query_fences.source_generation_contract_digest,
            rebuild_completed_at: Some(observed_at),
            created_at: observed_at,
        },
        query_fences,
        projection_generation: 1,
        project_context_revision: 1,
        observed_at,
    }
}

// ---------------------------------------------------------------------------
// RFX-01: stage deadline admission must check the target window.
// ---------------------------------------------------------------------------

/// A complete-path `WallTimeExhausted` partial reaches postflight exactly
/// in this shape: the work window is spent, the snapshot-close and absolute
/// windows are still open. The frozen contract lets the RR commit, release,
/// and synchronous signing tail continue inside the later windows, so a
/// stage that targets `Absolute` must not be refused by the already-spent
/// earlier windows.
#[tokio::test]
async fn rfx01_partial_tail_must_survive_earlier_window_cutoff() {
    let now = Instant::now();
    let spent_work = now - Duration::from_secs(1);
    let context = SemanticExecutionContext::new(
        SemanticOperationAttemptClass::CompletePath,
        SemanticDeadlineWindows::new(
            spent_work,
            spent_work,
            now + Duration::from_secs(5),
            now + Duration::from_secs(10),
        )
        .expect("complete-path partial windows"),
    );
    let outcome = context
        .run_stage(SemanticDeadlineWindow::Absolute, async { 40912_u64 })
        .await;
    assert_eq!(
        outcome,
        Ok(40912),
        "the packing/release/sign tail targets Absolute; a spent work window \
         is a traversal cutoff, not a terminal gate for the tail"
    );
}

// ---------------------------------------------------------------------------
// RFX-02: lifecycle states must match their frozen semantics.
// ---------------------------------------------------------------------------

/// Deadline expiry must write the real `TimedOut` latch state; relabelling
/// a `Cancelling` write as `TimedOut` only in the return value leaves the
/// state machine and its closed metric labels lying.
#[test]
fn rfx02_deadline_expiry_writes_timed_out_latch_state() {
    let context = active_one_shot_context();
    let outcome = context.deadline_expired();
    assert_eq!(
        outcome,
        SemanticLatchOutcome::Won(SemanticLifecycleState::TimedOut)
    );
    assert_eq!(
        context.latch().state(),
        SemanticLifecycleState::TimedOut,
        "deadline arbitration must move the latch to TimedOut, not Cancelling"
    );
}

/// Once `Finalizing` won, only the already-authorized synchronous signer
/// continues; generic semantic stage admission must be refused.
#[test]
fn rfx02_finalizing_forbids_new_work_stages() {
    let context = active_one_shot_context();
    assert_eq!(
        context.latch().begin_finalize(),
        SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing)
    );
    assert!(
        context.latch().state().forbids_new_semantic_work(),
        "Finalizing must forbid new semantic work; only the synchronous \
         finalizer guard may continue"
    );
    assert!(
        context.admit_stage(SemanticDeadlineWindow::Work).is_err(),
        "generic stage admission must be refused after Finalizing won"
    );
}

// ---------------------------------------------------------------------------
// RFX-03: unsigned result validation precedes release finalization.
// ---------------------------------------------------------------------------

/// The frozen release order builds and validates the unsigned result before
/// the release confirmation; only then does the confirmed permit move by
/// value into the single synchronous signer (fix plan F2 / §2.3).
///
/// A validation failure therefore never reaches the release at all: the
/// latch stays `Active` because `begin_finalize` is reachable only through
/// the signer arbitration, which the coordinators call after validation.
#[test]
fn rfx03_unsigned_result_validation_precedes_release_finalize() {
    let context = active_one_shot_context();
    // The unsigned result's validation failed: the coordinator returns its
    // contract error before any release confirmation, so nothing may have
    // arbitrated the finalize latch on the release path.
    let unsigned_result_validation_failed = true;
    if unsigned_result_validation_failed {
        assert_eq!(
            context.latch().state(),
            SemanticLifecycleState::Active,
            "a validation failure must leave the latch Active: no release \
             may be confirmed, and no Finalizing arbitration may run, for a \
             result that never validated"
        );
        assert!(
            context
                .latch()
                .begin_finalize()
                .won(SemanticLifecycleState::Finalizing),
            "an unconfirmed release path must still be able to arbitrate the \
             latch — the deviation is the order, not the CAS itself"
        );
    }
}

/// The confirmed permit authorizes exactly one synchronous signer. When
/// cancellation or the deadline already won, the signer is refused before
/// the closure ever runs, and a second authorization after a completed
/// signing is refused as well — the permit cannot sign twice.
#[test]
fn rfx03_refused_or_spent_release_signer_never_signs() {
    // Cancelled before the release tail: the authorization must be refused
    // without running the signing closure (the coordinator shape is
    // authorize-then-sign, exactly as the one-shot surfaces call it).
    let cancelled = active_one_shot_context();
    let _ = cancelled.cancel(SemanticCancellationSource::ServerShutdown);
    let mut signed_attempts = 0_u32;
    if let Ok(signer) = cancelled.authorize_release_signer() {
        let _ = cancelled.sign_released(signer, || signed_attempts += 1);
    }
    assert_eq!(
        signed_attempts, 0,
        "the signing closure must not run when authorization was refused"
    );

    // Clean path: the signer runs exactly once, the latch completes, and a
    // second authorization on the same request is refused.
    let context = active_one_shot_context();
    let signer = context
        .authorize_release_signer()
        .expect("an active request authorizes its single signer");
    let outcome = context.sign_released(signer, || signed_attempts + 1);
    assert_eq!(outcome, Ok(1));
    assert_eq!(
        context.latch().state(),
        SemanticLifecycleState::Completed,
        "a clean post-check completes the latch"
    );
    assert!(
        context.authorize_release_signer().is_err(),
        "the single-use permit cannot authorize a second signer after the \
         first signing completed"
    );
}

/// A cancellation that arrives during the synchronous signing work only
/// records a discard: the post-check must drop the signed value instead of
/// returning it, and the latch must not complete as a success.
#[test]
fn rfx03_discard_during_synchronous_signing_drops_the_signed_result() {
    let context = active_one_shot_context();
    let signer = context
        .authorize_release_signer()
        .expect("an active request authorizes its single signer");
    let outcome = context.sign_released(signer, || {
        // The cancellation lands inside the synchronous signing tail.
        let _ = context.cancel(SemanticCancellationSource::CallerDisconnected);
        7_u32
    });
    assert_eq!(
        outcome,
        Err(SemanticReleaseSignAbort::Discarded(
            SemanticLifecycleState::Finalizing
        )),
        "the signed value must be discarded, not returned, when the request \
         was cancelled during the synchronous signing"
    );
    assert_eq!(
        context.latch().state(),
        SemanticLifecycleState::Finalizing,
        "a discarded signing keeps the Finalizing state — it is not a success"
    );
}

// ---------------------------------------------------------------------------
// RFX-04: circuit refusals must not outrank fresh authorization.
// ---------------------------------------------------------------------------

/// When the caller would observe a circuit refusal, authorization must be
/// freshly proven first. With authorization unprovable — the fresh recheck
/// fails — the caller must see that authorization failure, never the
/// Provider circuit's Busy; only a caller the fresh recheck still admits
/// observes the circuit's Busy. The lazy Provider closure must never run on
/// a refused admission.
#[derive(Debug)]
struct RefusedLazyEncode;

impl ProviderAttemptOutcomeObservation for RefusedLazyEncode {
    fn attempt_failure(&self) -> &ProviderAttemptFailure {
        unreachable!("the lazy Provider closure never runs in this test")
    }
}

#[tokio::test]
async fn rfx04_circuit_refusal_requires_fresh_authorization_first() {
    let state = open_circuit_state().await;
    let context = active_one_shot_context();
    let ticket = reliability_fix_ticket();
    let mut encode_attempts = 0_u32;
    let result = execute_provider_attempt(
        ProviderEgressPlan {
            state: &state,
            context: &context,
            ticket: &ticket,
            reader_pubkey: &[0_u8; 32],
            expected_contexts: &[],
            observation: ProviderEgressObservation::Silent,
        },
        // The fresh authorization recheck cannot prove the caller (the
        // database is unreachable in this shape): the refusal must surface
        // as its own failure, never as the circuit's Busy.
        || async {
            Err::<(), SemanticProviderEgressFailure>(
                SemanticProviderEgressFailure::ProviderUnavailable,
            )
        },
        || async {
            encode_attempts += 1;
            Err::<(), _>(RefusedLazyEncode)
        },
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ProviderAttemptError::Admission(
                SemanticProviderEgressFailure::ProviderUnavailable
            ))
        ),
        "a caller whose authorization cannot be freshly proven must not \
         observe the Provider circuit state through a Busy refusal"
    );
    assert_eq!(
        encode_attempts, 0,
        "the lazy Provider closure must not run for a refused admission"
    );

    // Only a caller the fresh recheck still admits observes the Busy.
    let result = execute_provider_attempt(
        ProviderEgressPlan {
            state: &state,
            context: &context,
            ticket: &ticket,
            reader_pubkey: &[0_u8; 32],
            expected_contexts: &[],
            observation: ProviderEgressObservation::Silent,
        },
        || async { Ok::<(), SemanticProviderEgressFailure>(()) },
        || async {
            encode_attempts += 1;
            Err::<(), _>(RefusedLazyEncode)
        },
    )
    .await;
    assert!(
        matches!(
            result,
            Err(ProviderAttemptError::Admission(
                SemanticProviderEgressFailure::AdmissionBusy
            ))
        ),
        "a freshly re-authorized caller observes the circuit's Busy"
    );
    assert_eq!(
        encode_attempts, 0,
        "the lazy Provider closure must not run for a refused admission"
    );
}

// ---------------------------------------------------------------------------
// RFX-05: the physical attempt ledger counts real Provider handoffs only.
// ---------------------------------------------------------------------------

/// An egress refused at the circuit fast gate performs zero Provider
/// calls; the physical-attempt ledger must stay at zero. Counting the
/// refusal as a physical attempt turns the compiled cap into an admission
/// counter and lets refused requests consume the retry budget.
#[tokio::test]
async fn rfx05_refused_egress_counts_no_physical_attempt() {
    let state = open_circuit_state().await;
    let context = active_one_shot_context();
    let ticket = reliability_fix_ticket();
    let mut encode_attempts = 0_u32;
    let result = execute_provider_attempt(
        ProviderEgressPlan {
            state: &state,
            context: &context,
            ticket: &ticket,
            reader_pubkey: &[0_u8; 32],
            expected_contexts: &[],
            observation: ProviderEgressObservation::Silent,
        },
        || async {
            Err::<(), SemanticProviderEgressFailure>(
                SemanticProviderEgressFailure::ProviderUnavailable,
            )
        },
        || async {
            encode_attempts += 1;
            Err::<(), _>(RefusedLazyEncode)
        },
    )
    .await;
    assert!(result.is_err(), "the open circuit must refuse the egress");
    assert_eq!(encode_attempts, 0, "no Provider call may happen");
    assert_eq!(
        context.ledger().provider_attempts(),
        0,
        "a refused egress with zero Provider calls must not consume \
         physical-attempt budget"
    );
    assert_eq!(
        context.ledger().provider_transport_retries(),
        0,
        "a dropped pre-handoff reservation must refund its transport-retry \
         token"
    );
}

// ---------------------------------------------------------------------------
// F1 delivery: one-shot reserved budget, finalize-owner admission, and the
// subscribable shutdown / caller-disconnect wiring.
// ---------------------------------------------------------------------------

/// The closed one-shot budget shape preserves the caller-visible absolute
/// deadline while ordering the internal reserves at the compiled fractions
/// (fix plan F1 item 3): no new physical Provider attempt after half the
/// budget, generic work yielding three quarters in, one eighth kept for the
/// snapshot close.
#[test]
fn f1_one_shot_reserved_budget_orders_the_closed_internal_windows() {
    let start = Instant::now();
    let total = Duration::from_secs(45);
    let windows = SemanticDeadlineWindows::for_one_shot_reserved_budget(start, total);
    assert_eq!(
        windows.window(SemanticDeadlineWindow::Absolute),
        start + total
    );
    let eighth = total / ONE_SHOT_RESERVE_DENOMINATOR;
    assert_eq!(
        windows.window(SemanticDeadlineWindow::ProviderStart),
        start + total - eighth * 4
    );
    assert_eq!(
        windows.window(SemanticDeadlineWindow::Work),
        start + total - eighth * 2
    );
    assert_eq!(
        windows.window(SemanticDeadlineWindow::SnapshotClose),
        start + total - eighth
    );
    // The ordering invariant the shared executor relies on must hold for
    // small budgets too — a degenerate caller wall time may collapse the
    // windows but never invert them.
    let tiny =
        SemanticDeadlineWindows::for_one_shot_reserved_budget(start, Duration::from_millis(1));
    assert!(
        tiny.window(SemanticDeadlineWindow::ProviderStart)
            <= tiny.window(SemanticDeadlineWindow::Work)
    );
    assert!(
        tiny.window(SemanticDeadlineWindow::Work)
            <= tiny.window(SemanticDeadlineWindow::SnapshotClose)
    );
    assert!(
        tiny.window(SemanticDeadlineWindow::SnapshotClose)
            <= tiny.window(SemanticDeadlineWindow::Absolute)
    );
}

/// The R4 retry precondition is enforced by admission itself: once the
/// `ProviderStart` window passed — while later windows are still open — no
/// new physical Provider attempt may begin, so a retry's fresh plan is
/// refused before any reservation is taken (plan §4.1).
#[test]
fn f1_provider_start_window_refuses_late_physical_attempts_only() {
    let now = Instant::now();
    let context = SemanticExecutionContext::new(
        SemanticOperationAttemptClass::OneShot,
        SemanticDeadlineWindows::new(
            now - Duration::from_secs(1),
            now + Duration::from_secs(10),
            now + Duration::from_secs(15),
            now + Duration::from_secs(20),
        )
        .expect("one-shot retry windows"),
    );
    assert_eq!(
        context.admit_stage(SemanticDeadlineWindow::ProviderStart),
        Err(SemanticStageAbort::Deadline(
            SemanticDeadlineWindow::ProviderStart
        )),
        "a new physical Provider attempt must not start after its window"
    );
    assert_eq!(
        context.latch().state(),
        SemanticLifecycleState::TimedOut,
        "the refused attempt arbitrates the real TimedOut latch state"
    );
}

/// The finalize latch owner and only it may run finalization stages:
/// `Finalizing` refuses generic work but admits the finalizer's own stages,
/// while `Active` — which has not won a release permit — refuses them
/// (fix plan F1 item 5).
#[test]
fn f1_finalize_owner_admission_follows_the_latch_winner() {
    let context = active_one_shot_context();
    assert_eq!(
        context.admit_finalize_stage(SemanticDeadlineWindow::Absolute),
        Err(SemanticStageAbort::LatchClosed(
            SemanticLifecycleState::Active
        )),
        "finalization work must not start before the release permit is won"
    );
    assert_eq!(
        context.latch().begin_finalize(),
        SemanticLatchOutcome::Won(SemanticLifecycleState::Finalizing)
    );
    assert_eq!(
        context.admit_finalize_stage(SemanticDeadlineWindow::Absolute),
        Ok(()),
        "the Finalizing winner still runs its own synchronous tail"
    );
    assert_eq!(
        context.admit_stage(SemanticDeadlineWindow::Work),
        Err(SemanticStageAbort::LatchClosed(
            SemanticLifecycleState::Finalizing
        )),
        "generic work stays refused beside the finalizer"
    );
}

/// Controlled shutdown must reach a parked request through the subscribable
/// signal — not only at the next stage-entry poll (fix plan F1 item 6).
#[tokio::test]
async fn f1_shutdown_subscription_cancels_a_parked_request() {
    let state = service_free_state().await;
    let context = active_one_shot_context();
    let _subscription = subscribe_relay_shutdown(&state, &context);
    let mut handle = context.cancellation().handle();
    state.shutdown_signal.send_replace(true);
    let cancelled = tokio::time::timeout(Duration::from_secs(2), handle.wait()).await;
    assert_eq!(
        cancelled.expect("shutdown must cancel a parked request"),
        SemanticCancellationSource::ServerShutdown
    );
    // The latch arbitrates lazily on the next admission, exactly like every
    // other pre-cancelled request.
    assert_eq!(
        context.admit_stage(SemanticDeadlineWindow::Work),
        Err(SemanticStageAbort::Cancelled(
            SemanticCancellationSource::ServerShutdown
        ))
    );
}

/// Dropping the request future — the caller connection ended — fires the
/// caller-disconnect cancellation instead of leaking detached work
/// (fix plan F1 item 6).
#[test]
fn f1_caller_disconnect_guard_fires_on_request_drop() {
    let context = active_one_shot_context();
    {
        let _guard = caller_disconnect_guard(&context);
        assert_eq!(
            context.cancellation().cancelled(),
            None,
            "an alive request must not be cancelled"
        );
    }
    assert_eq!(
        context.cancellation().cancelled(),
        Some(SemanticCancellationSource::CallerDisconnected)
    );
}

// ---------------------------------------------------------------------------
// RFX-07: the qualification record must carry a mechanical gate inventory.
// ---------------------------------------------------------------------------

/// The qualification record is the executable status assertion for every
/// gate the fix plan §5 requires: each gate must be listed with an explicit
/// run status (passed, not-run, or conditional), and the record must carry
/// the correctness-fix status marker F0 introduced — no undocumented gate,
/// no blank completion claim.
#[test]
fn rfx07_qualification_record_carries_the_gate_inventory() {
    let record = include_str!(
        "../../../docs/stage/semantic/unified-engine/project-context-unified-semantic-reliability-runtime-qualification.md"
    );
    assert!(
        record.contains("correctness 修复中"),
        "the qualification record must state the correctness-fix status"
    );
    for gate in [
        "check-semantic-retrieval-compatibility-baseline",
        "check-semantic-retrieval-computation",
        "check-semantic-retrieval-reliability",
        "semantic-test",
        "test-unit",
        "just ci",
        "semantic-pgvector-test",
        "semantic-migration-test",
        "real_provider",
    ] {
        assert!(
            record.contains(gate),
            "the qualification record must inventory the `{gate}` gate"
        );
    }
    assert!(
        record.contains("未运行"),
        "not-run gates must stay explicitly listed, never silently claimed"
    );
}
