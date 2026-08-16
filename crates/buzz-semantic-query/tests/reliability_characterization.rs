//! Phase 2 reliability characterization manifest.
//!
//! This fixture freezes the *current* interactive semantic-query execution
//! profile: per-operation Provider attempts, repeatable-read transaction
//! lifecycle, release parameters, deadline shapes, and existing retry. The
//! unified reliability runtime plan requires this record so later zero-policy
//! migration steps can prove exactly which behaviors changed and which were
//! intentionally preserved.
//!
//! Facts recomputed from live code (compiled computation routes, runtime
//! contract identity, runtime digest, one-shot wall-time constants) must track
//! the binary automatically. Relay-level execution facts that this crate
//! cannot observe are frozen observations; they change only through an
//! explicit fixture and digest update together with the migrating change.

use buzz_semantic_query::{
    semantic_graph_http_runtime_digest, SemanticComputationRoute,
    MAX_COORDINATE_SEARCH_WALL_TIME_MS, MAX_ONE_HOP_SEMANTIC_WALL_TIME_MS,
    SEMANTIC_COMPUTATION_ROUTES, SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT,
};
use serde_json::{json, Value};

const FIXTURE_ID: &str = "carryforth.semantic-retrieval-reliability-characterization.v1";
const CODE_BASELINE: &str = "4364deae89";

fn route_token(route: SemanticComputationRoute) -> &'static str {
    match route {
        SemanticComputationRoute::Legacy => "legacy",
        SemanticComputationRoute::Migrated => "migrated",
    }
}

fn one_shot_operation(
    logical_operation: &str,
    surface: &str,
    result_kind: u16,
    provider_batch_calls: Value,
    scoring_stage: &str,
) -> Value {
    json!({
        "logical_operation": logical_operation,
        "surface": surface,
        "result_kind": result_kind,
        "execution_lifecycle": "one_shot",
        "provider_batch_calls": provider_batch_calls,
        "provider_attempts_per_traversal_hop": 0,
        "rr_transactions": [
            {
                "stage": scoring_stage,
                "lifecycle": "short_repeatable_read_open_score_commit",
            },
        ],
        "release": {
            "expected_snapshot": "exact_actual_rr_ticket",
            "permit_consumption":
                "dropped_by_shared_envelope_surface_signs_separately",
        },
        "deadline_shape": {
            "windows": "single_fixed_hard_deadline",
            "maximum_wall_time_ms":
                (MAX_COORDINATE_SEARCH_WALL_TIME_MS == MAX_ONE_HOP_SEMANTIC_WALL_TIME_MS)
                    .then_some(MAX_COORDINATE_SEARCH_WALL_TIME_MS),
            "derived_tail_reserves": false,
        },
        "existing_retry": "none",
    })
}

fn complete_path_operation() -> Value {
    // The complete path is the only multi-stage operation. Its Provider
    // egress happens once per root attempt as one ordered Q0/Qi bundle; every
    // traversal hop after Stage C reuses that vector bundle and makes no
    // Provider call.
    json!({
        "logical_operation": "bounded_complete_path",
        "surface": "semantic_graph_query",
        "result_kind": 40_912,
        "execution_lifecycle": "multi_stage_traversal",
        "provider_batch_calls": "one_ordered_q0_qi_bundle_per_root_attempt",
        "provider_attempts_per_traversal_hop": 0,
        "rr_transactions": [
            {
                "stage": "stage_a_context_observation",
                "lifecycle": "short_repeatable_read_observe_conditioned_contexts_commit",
            },
            {
                "stage": "stage_c_root_recall_through_traversal",
                "lifecycle": "single_long_repeatable_read_held_to_completion",
            },
        ],
        "release": {
            "expected_snapshot": "none_current_authorization_only",
            "permit_consumption": "single_use_synchronous_to_signing",
        },
        "deadline_shape": {
            "windows":
                "work_snapshot_close_absolute_with_response_and_close_tail_reserves",
            "partial_result": "wall_time_exhausted_signed_partial_result_is_legal",
        },
        "existing_retry": "generation_or_context_churn_replays_one_full_root_attempt",
    })
}

fn manifest() -> Value {
    let routes = &SEMANTIC_COMPUTATION_ROUTES;
    let runtime_contract_id = SEMANTIC_GRAPH_HTTP_RUNTIME_CONTRACT
        .lines()
        .next()
        .unwrap_or_default();
    let runtime_digest = semantic_graph_http_runtime_digest()
        .expect("compiled semantic graph HTTP runtime digest")
        .to_hex();

    json!({
        "schema_version": 1,
        "fixture_id": FIXTURE_ID,
        "code_baseline": CODE_BASELINE,
        "phase": "pre_phase2_reliability_runtime",
        "compiled_profile": {
            "routes": {
                "edge_member_coordinate": route_token(routes.edge_member_coordinate),
                "coordinate_incident_edge": route_token(routes.coordinate_incident_edge),
                "whole_graph_coordinate_discovery":
                    route_token(routes.whole_graph_coordinate_discovery),
                "bounded_complete_path": route_token(routes.bounded_complete_path),
            },
            "canonical_route_profile": routes.canonical_profile(),
            "http_runtime_contract_id": runtime_contract_id,
            "http_runtime_digest": runtime_digest,
        },
        "operations": [
            one_shot_operation(
                "whole_graph_coordinate_discovery",
                "coordinate_search",
                40_913,
                json!(1),
                "post_provider_whole_graph_coordinate_scorer",
            ),
            one_shot_operation(
                "coordinate_incident_edge",
                "one_hop_tagged_family",
                40_914,
                json!(1),
                "post_provider_incident_edge_scoped_search",
            ),
            one_shot_operation(
                "edge_member_coordinate",
                "one_hop_tagged_family",
                40_914,
                json!(1),
                "post_provider_edge_coordinate_scoped_search",
            ),
            complete_path_operation(),
        ],
        "retry_ledger_bounds": {
            "provider_transport_retry_per_logical_request": 0,
            "one_shot_operation_attempts": 1,
            "complete_path_root_attempts": 2,
            "complete_path_physical_provider_attempts": 2,
        },
        "cross_operation_reliability": {
            "provider_transport_retry": "none",
            "circuit_breaker": "absent",
            "cancellation": "future_timeout_and_drop_only",
            "provider_failure_taxonomy":
                "transport_conflates_connect_failure_and_outcome_unknown",
            "db_failure_taxonomy":
                "sqlx_error_unclassified_by_effect_phase_or_sqlstate",
            "provider_attempt_timeout":
                "configured_client_timeout_ignores_remaining_deadline",
        },
        "known_gaps_frozen": [
            {
                "gap_id": "one_shot_release_permit_dropped",
                "current_shape": "the shared one-shot envelope returns Ok(()) \
    from Permitted(_permit) and each surface builds and signs its Event afterwards",
                "remediation_owner": "phase2_r3_release_permit_sync_consume",
                "oracle": "bounded_complete_path_sync_sign_after_permit",
            },
            {
                "gap_id": "no_unified_request_cancellation",
                "current_shape": "caller disconnect, server shutdown, deadline, \
    and explicit cancel are not aggregated into one request lifecycle token",
                "remediation_owner": "phase2_r3_cancellation",
            },
            {
                "gap_id": "one_shot_deadline_does_not_bind_post_release_work",
                "current_shape": "result build, signing, and bridge serialization \
    after the one-shot release are not inside one absolute-deadline tail contract",
                "remediation_owner": "phase2_r3_deadline_and_finalize",
            },
        ],
    })
}

#[test]
fn reliability_characterization_matches_golden() {
    let actual = manifest();
    println!(
        "semantic_retrieval_reliability_characterization={}",
        serde_json::to_string(&actual).expect("compact reliability manifest")
    );

    // Pin the characterization invariants the reliability plan depends on,
    // independent of full-document equality.
    let complete_path = &actual["operations"][3];
    assert_eq!(
        complete_path["logical_operation"], "bounded_complete_path",
        "operation order is part of the frozen characterization"
    );
    assert_eq!(
        complete_path["provider_attempts_per_traversal_hop"], 0,
        "traversal hops must not call the Provider"
    );
    assert_eq!(
        actual["retry_ledger_bounds"]["provider_transport_retry_per_logical_request"], 0,
        "no Provider transport retry exists before Phase 2 R4"
    );
    let gap_ids: Vec<&str> = actual["known_gaps_frozen"]
        .as_array()
        .expect("known gaps array")
        .iter()
        .map(|gap| gap["gap_id"].as_str().expect("gap id"))
        .collect();
    assert!(
        gap_ids.contains(&"one_shot_release_permit_dropped"),
        "the one-shot permit shape must stay frozen as a known gap until R3"
    );

    let expected: Value = serde_json::from_str(include_str!(
        "fixtures/semantic_retrieval_reliability_characterization_v1.json"
    ))
    .expect("tracked reliability characterization manifest");
    assert_eq!(actual, expected);
}
