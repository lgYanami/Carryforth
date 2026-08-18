//! Failing regression baseline for the unified reliability runtime
//! correctness fix.
//!
//! Every test in this module encodes one frozen contract from the
//! correctness fix plan (`docs/stage/semantic/unified-engine/fix/…`) and
//! fails on the current code. They stay red until fix stages F1–F4 land;
//! keeping them in a module outside `semantic_*` preserves the frozen
//! characterization gates' historical `semantic_` filter scope while the
//! full unit suite carries the red baseline. Nothing here is ignored or
//! feature-gated: each failure is the repeatable, content-free evidence
//! the fix plan's F0 exit requires.

use std::sync::Arc;
use std::time::{Duration, Instant};

use buzz_core::CommunityId;
use buzz_db::semantic_query::SemanticGraphQueryTicket;
use buzz_semantic::SemanticEncoder;

use crate::semantic_query_runtime::{
    execute_provider_egress, ProviderCircuitAdmission, ProviderCircuitObservation,
    ProviderEgressObservation, ProviderEgressPlan, ProviderHealthFailureClass,
    SemanticDeadlineWindow, SemanticDeadlineWindows, SemanticExecutionContext,
    SemanticLatchOutcome, SemanticLifecycleState, SemanticOperationAttemptClass,
    SemanticProviderEgressFailure, PROVIDER_CIRCUIT_HEALTH_FAILURE_THRESHOLD,
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

/// Build a service-free state whose enforced Provider circuit is open.
///
/// The circuit is tripped with real health observations so the next
/// admission is refused at the fast gate — before any database acquire —
/// which is exactly the refusal path RFX-04 and RFX-05 audit.
async fn open_circuit_state() -> Arc<AppState> {
    let mut config = crate::config::Config::from_env().expect("default config loads");
    config.database_url = "postgres://semantic-fix@127.0.0.1:1/semantic_fix".to_owned();
    config.redis_url = "redis://127.0.0.1:1".to_owned();
    config.semantic_worker = enforced_circuit_provider_config();
    let state = crate::state::app_state_for_reliability_fix_tests(config).await;
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
        context.admit_stage().is_err(),
        "generic stage admission must be refused after Finalizing won"
    );
}

// ---------------------------------------------------------------------------
// RFX-03: unsigned result validation precedes release finalization.
// ---------------------------------------------------------------------------

/// The frozen release order builds and validates the unsigned result before
/// the release confirmation may move the latch to `Finalizing`. Simulating
/// the current one-shot surface order — release/finalize first, then a
/// result whose validation fails — must leave the latch `Active`: no
/// release may be confirmed for a result that never validated.
#[test]
fn rfx03_unsigned_result_validation_precedes_release_finalize() {
    let context = active_one_shot_context();
    let _ = context.latch().begin_finalize();
    let unsigned_result_validation_failed = true;
    if unsigned_result_validation_failed {
        assert_eq!(
            context.latch().state(),
            SemanticLifecycleState::Active,
            "a validation failure after release leaves a confirmed release \
             and a Finalizing latch for a result that was never valid"
        );
    }
}

// ---------------------------------------------------------------------------
// RFX-04: circuit refusals must not outrank fresh authorization.
// ---------------------------------------------------------------------------

/// When the caller would observe a circuit refusal, authorization must be
/// freshly proven first. With authorization unprovable — the database is
/// unreachable — the caller must see the authorization/unavailable
/// failure, never the Provider circuit's Busy.
#[tokio::test]
async fn rfx04_circuit_refusal_requires_fresh_authorization_first() {
    let state = open_circuit_state().await;
    let context = active_one_shot_context();
    let ticket = reliability_fix_ticket();
    let result = execute_provider_egress(ProviderEgressPlan {
        state: &state,
        context: &context,
        ticket: &ticket,
        reader_pubkey: &[0_u8; 32],
        expected_contexts: &[],
        observation: ProviderEgressObservation::Silent,
    })
    .await;
    assert!(
        !matches!(result, Err(SemanticProviderEgressFailure::AdmissionBusy)),
        "a caller whose authorization cannot be freshly proven must not \
         observe the Provider circuit state through a Busy refusal"
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
    let result = execute_provider_egress(ProviderEgressPlan {
        state: &state,
        context: &context,
        ticket: &ticket,
        reader_pubkey: &[0_u8; 32],
        expected_contexts: &[],
        observation: ProviderEgressObservation::Silent,
    })
    .await;
    assert!(result.is_err(), "the open circuit must refuse the egress");
    assert_eq!(
        context.ledger().provider_attempts(),
        0,
        "a refused egress with zero Provider calls must not consume \
         physical-attempt budget"
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
