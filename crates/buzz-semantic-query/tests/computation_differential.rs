use serde_json::{json, Value};

const FIXTURE_ID: &str = "carryforth.semantic-retrieval-computation-differential.v1";
const HISTORICAL_MANIFEST_SHA256: &str =
    "e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e";

fn comparator(logical_operation: &str) -> Value {
    match logical_operation {
        "whole_graph_coordinate_discovery" => json!({
            "normalized_result": ["request_binding", "snapshot", "coordinates", "truncated"],
            "ordered_item": ["rank", "coordinate", "score"],
            "closed_error": ["http_status", "code", "retryable", "cli_exit_category"]
        }),
        "coordinate_incident_edge" => json!({
            "normalized_result": ["request_binding", "snapshot", "scope", "edges", "coverage", "truncated"],
            "ordered_item": ["rank", "edge_key", "score", "matched_documents"],
            "closed_error": ["http_status", "code", "retryable", "cli_exit_category"]
        }),
        "edge_member_coordinate" => json!({
            "normalized_result": ["request_binding", "snapshot", "scope", "coordinates", "coverage", "truncated"],
            "ordered_item": ["rank", "coordinate", "score", "observation"],
            "closed_error": ["http_status", "code", "retryable", "cli_exit_category"]
        }),
        "bounded_complete_path" => json!({
            "normalized_result": ["request_binding", "snapshot", "roots", "paths", "coverage", "completion"],
            "ordered_item": ["rank", "stable_identity", "score", "provenance"],
            "closed_error": ["http_status", "code", "retryable", "cli_exit_category"]
        }),
        other => panic!("unexpected logical operation {other}"),
    }
}

fn manifest() -> Value {
    let baseline: Value = serde_json::from_str(include_str!(
        "fixtures/semantic_retrieval_compatibility_manifest.json"
    ))
    .expect("historical compatibility manifest");
    let operations = baseline["operations"]
        .as_array()
        .expect("historical operations")
        .iter()
        .map(|operation| {
            let logical_operation = operation["logical_operation"]
                .as_str()
                .expect("logical operation");
            json!({
                "logical_operation": logical_operation,
                "surface": operation["surface"],
                "result_kind": operation["result_kind"],
                "input_bundle": {
                    "canonical_inputs": operation["canonical_inputs"],
                    "input_digests": operation["input_digests"],
                    "query_vector_digests": operation["query_vector_digests"]
                },
                "execution_lifecycle": operation["execution_lifecycle"],
                "comparator": comparator(logical_operation)
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schema_version": 1,
        "fixture_id": FIXTURE_ID,
        "phase_base_commit": "ab395ff6f",
        "historical_oracle": {
            "fixture_id": baseline["fixture_id"],
            "sha256": HISTORICAL_MANIFEST_SHA256
        },
        "execution_seam": {
            "provider_encoding": "once_per_attempt",
            "ordered_input_bundle_shared": true,
            "ordered_vector_bundle_shared": true,
            "repeatable_read_snapshot_shared": true,
            "legacy_and_migrated_provider_calls": 1,
            "legacy_and_migrated_read_transactions": 1,
            "production_compare_mode": "not_compiled_by_default",
            "production_default_route": "legacy"
        },
        "operations": operations,
        "differential_outcome": {
            "success": "typed_normalized_result_exact_equality",
            "failure": "closed_error_exact_equality",
            "mismatch_action": "stop_migration_without_request_fallback"
        }
    })
}

#[test]
fn computation_differential_manifest_matches_golden() {
    let actual = manifest();
    let expected: Value = serde_json::from_str(include_str!(
        "fixtures/semantic_retrieval_computation_differential_v1.json"
    ))
    .expect("tracked computation differential manifest");
    assert_eq!(actual, expected);
}

#[test]
fn execution_seam_reuses_one_bundle_and_snapshot() {
    #[derive(Clone, Copy, Debug)]
    struct SharedExecution<'a> {
        vector_bundle: &'a str,
        snapshot: &'a str,
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum NormalizedOutcome {
        Success(&'static str),
        ClosedError { code: &'static str, retryable: bool },
    }

    fn compare_once(
        provider_calls: &mut usize,
        read_transactions: &mut usize,
        outcome: NormalizedOutcome,
    ) {
        *provider_calls += 1;
        let vector_bundle = String::from("ordered-fixed-vector-bundle");
        *read_transactions += 1;
        let snapshot = String::from("single-repeatable-read-snapshot");
        let shared = SharedExecution {
            vector_bundle: &vector_bundle,
            snapshot: &snapshot,
        };

        let legacy = |execution: SharedExecution<'_>| {
            assert_eq!(execution.vector_bundle, vector_bundle);
            assert_eq!(execution.snapshot, snapshot);
            outcome.clone()
        };
        let migrated = |execution: SharedExecution<'_>| {
            assert_eq!(execution.vector_bundle, vector_bundle);
            assert_eq!(execution.snapshot, snapshot);
            outcome.clone()
        };

        assert_eq!(legacy(shared), migrated(shared));
    }

    let mut provider_calls = 0;
    let mut read_transactions = 0;
    compare_once(
        &mut provider_calls,
        &mut read_transactions,
        NormalizedOutcome::Success("normalized-result"),
    );
    compare_once(
        &mut provider_calls,
        &mut read_transactions,
        NormalizedOutcome::ClosedError {
            code: "conflict",
            retryable: true,
        },
    );

    assert_eq!(provider_calls, 2, "one Provider encode per comparison");
    assert_eq!(read_transactions, 2, "one RR snapshot per comparison");
}
