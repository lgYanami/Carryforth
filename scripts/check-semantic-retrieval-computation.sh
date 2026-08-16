#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/crates/buzz-semantic-query/tests/fixtures"
MANIFEST="${FIXTURE_DIR}/semantic_retrieval_computation_differential_v1.json"
MANIFEST_SHA="${FIXTURE_DIR}/semantic_retrieval_computation_differential_v1.sha256"
PHASE_BASE_COMMIT="ab395ff6f"
PHASE_PRODUCTION_CLOSE_COMMIT="97c31fa03"
SCOPE="${1:-all}"

fail() {
  printf 'semantic retrieval computation gate failed: %s\n' "$*" >&2
  exit 1
}

case "$SCOPE" in
  manifest-only | deterministic | all) ;;
  *) fail "scope must be manifest-only, deterministic, or all" ;;
esac

cd "$REPO_ROOT"
. ./bin/activate-hermit

test -f "$MANIFEST" || fail "missing computation differential manifest"
test -f "$MANIFEST_SHA" || fail "missing computation differential manifest digest"

(
  cd "$FIXTURE_DIR"
  sha256sum -c "$(basename "$MANIFEST_SHA")"
)

jq -e --arg base "$PHASE_BASE_COMMIT" '
  .schema_version == 1
  and .fixture_id == "carryforth.semantic-retrieval-computation-differential.v1"
  and .phase_base_commit == $base
  and .historical_oracle.sha256
      == "e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e"
  and (.operations | length == 4)
  and ([.operations[].logical_operation] | unique | length == 4)
  and ([.operations[].surface] | unique | sort
       == ["coordinate_search", "one_hop_tagged_family", "semantic_graph_query"])
  and .execution_seam.provider_encoding == "once_per_attempt"
  and .execution_seam.legacy_and_migrated_provider_calls == 1
  and .execution_seam.legacy_and_migrated_read_transactions == 1
  and .execution_seam.ordered_input_bundle_shared
  and .execution_seam.ordered_vector_bundle_shared
  and .execution_seam.repeatable_read_snapshot_shared
  and .execution_seam.production_compare_mode == "not_compiled_by_default"
  and .execution_seam.production_default_route == "legacy"
  and ([.operations[].input_bundle.query_vector_digests[]]
       | all(test("^[0-9a-f]{64}$")))
' "$MANIFEST" >/dev/null || fail "computation differential manifest structural gate failed"

cargo test -p buzz-semantic-query \
  --test compatibility_baseline compatibility_manifest_matches_golden
cargo test -p buzz-semantic-query --test computation_differential

if [[ "$SCOPE" == "manifest-only" ]]; then
  printf 'semantic retrieval computation manifest gate passed\n'
  exit 0
fi

# Phase 1 intentionally changed only the internal semantic-computation owners.
# Audit its frozen production range rather than all later work on this branch.
# Current semantic behavior remains covered above by tracked manifests and
# executable differential tests.
git merge-base --is-ancestor "$PHASE_BASE_COMMIT" "$PHASE_PRODUCTION_CLOSE_COMMIT" ||
  fail "Phase 1 production close is not descended from its base"
git merge-base --is-ancestor "$PHASE_PRODUCTION_CLOSE_COMMIT" HEAD ||
  fail "current HEAD does not contain the frozen Phase 1 production close"
changed_paths="$(
  git diff --name-only "$PHASE_BASE_COMMIT" "$PHASE_PRODUCTION_CLOSE_COMMIT" -- |
    LC_ALL=C sort -u
)"
unexpected_paths="$(
  while IFS= read -r path; do
    [[ -z "$path" ]] && continue
    case "$path" in
      Justfile | \
        docs/stage/TODO.md | \
        "docs/stage/semantic/ unified-engine/"* | \
        crates/buzz-semantic-query/src/* | \
        crates/buzz-semantic-query/tests/* | \
        crates/buzz-db/src/semantic_query.rs | \
        crates/buzz-db/src/semantic_query/* | \
        crates/buzz-db/src/semantic_coordinate_search.rs | \
        crates/buzz-db/src/semantic_coordinate_search_qualification_tests.rs | \
        crates/buzz-relay/src/semantic_provider.rs | \
        crates/buzz-relay/src/semantic_one_shot.rs | \
        crates/buzz-relay/src/semantic_coordinate_search.rs | \
        crates/buzz-relay/src/semantic_one_hop_search.rs | \
        crates/buzz-relay/src/semantic_graph_query.rs | \
        crates/buzz-relay/src/semantic_graph_traversal.rs | \
        crates/buzz-relay/src/semantic_graph_response.rs | \
        crates/buzz-relay/src/semantic_fleet.rs | \
        scripts/check-semantic-retrieval-compatibility-baseline.sh | \
        scripts/check-semantic-retrieval-computation.sh)
        ;;
      *) printf '%s\n' "$path" ;;
    esac
  done <<<"$changed_paths"
)"
[[ -z "$unexpected_paths" ]] ||
  fail "Phase 1 changed a protected path: ${unexpected_paths//$'\n'/, }"

if rg -n 'AcceptanceCompareReturnLegacy' \
  crates/buzz-semantic-query/src crates/buzz-db/src crates/buzz-relay/src >/dev/null; then
  fail "acceptance compare route is reachable from the default production source set"
fi

if [[ "$SCOPE" == "deterministic" ]]; then
  printf 'semantic retrieval computation deterministic gate passed\n'
  exit 0
fi

cargo test -p buzz-semantic-query --lib
cargo test -p buzz-db --lib semantic_
cargo test -p buzz-db --lib coordinate_search
cargo test -p buzz-db --lib one_hop
cargo test -p buzz-relay --lib semantic_
cargo test -p buzz-relay --lib coordinate_search
cargo test -p buzz-relay --lib one_hop
cargo check -p buzz-semantic-query -p buzz-db -p buzz-relay

printf 'semantic retrieval computation gate passed (%s)\n' "$SCOPE"
