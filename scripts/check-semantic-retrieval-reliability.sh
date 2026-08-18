#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/crates/buzz-semantic-query/tests/fixtures"
MANIFEST="${FIXTURE_DIR}/semantic_retrieval_reliability_characterization_v1.json"
MANIFEST_SHA="${FIXTURE_DIR}/semantic_retrieval_reliability_characterization_v1.sha256"
SCOPE="${1:-all}"

# R0 anchor. Every later reliability change is audited from this commit;
# override only for explicit qualification reruns against a different base.
FREEZE_BASE="${SEMANTIC_RELIABILITY_FREEZE_BASE:-db6c8c1d5}"

fail() {
  printf 'semantic retrieval reliability gate failed: %s\n' "$*" >&2
  exit 1
}

case "$SCOPE" in
  manifest-only | deterministic | all) ;;
  *) fail "scope must be manifest-only, deterministic, or all" ;;
esac

cd "$REPO_ROOT"
. ./bin/activate-hermit

test -f "$MANIFEST" || fail "missing reliability characterization manifest"
test -f "$MANIFEST_SHA" || fail "missing reliability characterization digest"

(
  cd "$FIXTURE_DIR"
  sha256sum -c "$(basename "$MANIFEST_SHA")"
)

jq -e '
  .schema_version == 1
  and .fixture_id == "carryforth.semantic-retrieval-reliability-characterization.v1"
  and .phase == "pre_phase2_reliability_runtime"
  and (.operations | length == 4)
  and ([.operations[].logical_operation] | unique | length == 4)
  and ([.operations[].result_kind] | unique | sort == [40912, 40913, 40914])
  and ([.operations[].provider_attempts_per_traversal_hop] | all(. == 0))
  and (.operations[3].logical_operation == "bounded_complete_path")
  and (.operations[3].provider_batch_calls
       == "one_ordered_q0_qi_bundle_per_root_attempt")
  and (.retry_ledger_bounds.provider_transport_retry_per_logical_request == 0)
  and (.retry_ledger_bounds.one_shot_operation_attempts == 1)
  and (.retry_ledger_bounds.complete_path_root_attempts == 2)
  and (.retry_ledger_bounds.complete_path_physical_provider_attempts == 2)
  and (.compiled_profile.routes
       == {"edge_member_coordinate": "migrated",
           "coordinate_incident_edge": "migrated",
           "whole_graph_coordinate_discovery": "migrated",
           "bounded_complete_path": "migrated"})
  and (.compiled_profile.http_runtime_digest | test("^[0-9a-f]{64}$"))
  and ((.known_gaps_frozen | map(.gap_id))
       | contains(["one_shot_release_permit_dropped",
                   "no_unified_request_cancellation",
                   "one_shot_deadline_does_not_bind_post_release_work"]))
  and ([.. | objects | keys[]] | index("embedding") == null)
' "$MANIFEST" >/dev/null || fail "reliability characterization structural gate failed"

if jq -r '.. | strings' "$MANIFEST" |
  rg -n -i 'nsec1|private[_ -]?key|authorization:[[:space:]]*bearer|api[_ -]?key' >/dev/null; then
  fail "manifest contains a credential-shaped value"
fi

cargo test -p buzz-semantic-query --test reliability_characterization

if [[ "$SCOPE" == "manifest-only" ]]; then
  printf 'semantic retrieval reliability manifest gate passed\n'
  exit 0
fi

# Phase 2 protected-surface freeze diff. The unified reliability runtime may
# only touch the closed semantic execution owners below; anything else in the
# audited range must be justified as a separate reviewed change.
changed_paths="$(git diff --name-only "${FREEZE_BASE}" -- | LC_ALL=C sort -u)"
unexpected_paths="$(
  while IFS= read -r path; do
    [[ -z "$path" ]] && continue
    case "$path" in
      Justfile | \
      docs/stage/TODO.md | \
      docs/stage/semantic/unified-engine/* | \
      docs/en/current-status.md | \
      crates/buzz-semantic-query/src/fleet.rs | \
      crates/buzz-semantic-query/src/lib.rs | \
      crates/buzz-semantic-query/tests/* | \
      crates/buzz-db/src/lib.rs | \
      crates/buzz-db/src/error.rs | \
      crates/buzz-db/src/semantic.rs | \
      crates/buzz-db/src/semantic_query.rs | \
      crates/buzz-db/src/semantic_query/* | \
      crates/buzz-db/src/semantic_coordinate_search.rs | \
      crates/buzz-relay/src/lib.rs | \
      crates/buzz-relay/src/reliability_fix_regressions.rs | \
      crates/buzz-relay/src/state.rs | \
      crates/buzz-relay/src/main.rs | \
      crates/buzz-relay/src/config.rs | \
      crates/buzz-relay/src/api/bridge.rs | \
      crates/buzz-relay/src/semantic_provider.rs | \
      crates/buzz-relay/src/semantic_one_shot.rs | \
      crates/buzz-relay/src/semantic_query_runtime.rs | \
      crates/buzz-relay/src/semantic_coordinate_search.rs | \
      crates/buzz-relay/src/semantic_one_hop_search.rs | \
      crates/buzz-relay/src/semantic_graph_query.rs | \
      crates/buzz-relay/src/semantic_graph_traversal.rs | \
      crates/buzz-relay/src/semantic_graph_response.rs | \
      crates/buzz-relay/src/semantic_graph_observability.rs | \
      crates/buzz-relay/src/semantic_fleet.rs | \
      scripts/check-semantic-retrieval-compatibility-baseline.sh | \
      scripts/check-semantic-retrieval-reliability.sh)
        ;;
      *)
        printf '%s\n' "$path"
        ;;
    esac
  done <<<"$changed_paths"
)"
[[ -z "$unexpected_paths" ]] ||
  fail "Phase 2 reliability freeze diff contains unexpected files: ${unexpected_paths//$'\n'/, }"

if [[ "$SCOPE" == "deterministic" ]]; then
  printf 'semantic retrieval reliability deterministic gate passed\n'
  exit 0
fi

cargo test -p buzz-semantic-query --lib
cargo test -p buzz-relay --lib semantic_
cargo check -p buzz-semantic-query -p buzz-db -p buzz-relay

printf 'semantic retrieval reliability gate passed (%s)\n' "$SCOPE"
