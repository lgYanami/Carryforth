#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE_DIR="${REPO_ROOT}/crates/buzz-semantic-query/tests/fixtures"
MANIFEST="${FIXTURE_DIR}/semantic_retrieval_compatibility_manifest.json"
MANIFEST_SHA="${FIXTURE_DIR}/semantic_retrieval_compatibility_manifest.sha256"
QUALIFICATION="${REPO_ROOT}/docs/stage/semantic/ unified-engine/project-context-semantic-retrieval-compatibility-baseline.md"
BASELINE_COMMIT="e8f26d6e65"
SCOPE="${1:-all}"

fail() {
  printf 'semantic retrieval compatibility baseline failed: %s\n' "$*" >&2
  exit 1
}

case "$SCOPE" in
  manifest-only | deterministic | all) ;;
  *) fail "scope must be manifest-only, deterministic, or all" ;;
esac

cd "$REPO_ROOT"
. ./bin/activate-hermit

test -f "$MANIFEST" || fail "missing deterministic manifest"
test -f "$MANIFEST_SHA" || fail "missing deterministic manifest digest"
test -f "$QUALIFICATION" || fail "missing compatibility baseline record"

(
  cd "$FIXTURE_DIR"
  sha256sum -c "$(basename "$MANIFEST_SHA")"
)

jq -e '
  .schema_version == 1
  and .fixture_id == "carryforth.semantic-retrieval-compatibility.v1"
  and (.operations | length == 4)
  and ([.operations[].logical_operation] | unique | length == 4)
  and ([.operations[].surface] | unique | sort
       == ["coordinate_search", "one_hop_tagged_family", "semantic_graph_query"])
  and ([.operations[].result_kind] | unique | sort == [40912, 40913, 40914])
  and .operations[0].input_count == 1
  and .operations[1].input_count == 1
  and .operations[2].input_count == 1
  and .operations[3].input_count == 2
  and .operations[0].canonical_inputs[0] != .operations[1].canonical_inputs[0]
  and .operations[1].canonical_inputs[0] == .operations[2].canonical_inputs[0]
  and .operations[1].canonical_inputs[0] == .operations[3].canonical_inputs[0]
  and .operations[1].query_vector_digests[0]
      == .operations[2].query_vector_digests[0]
  and .operations[1].query_vector_digests[0]
      == .operations[3].query_vector_digests[0]
  and ([.. | objects | keys[]] | index("embedding") == null)
  and ([.operations[].query_vector_digests[]]
       | all(test("^[0-9a-f]{64}$")))
' "$MANIFEST" >/dev/null || fail "manifest structural gate failed"

if jq -r '.. | strings' "$MANIFEST" |
  rg -n -i 'nsec1|private[_ -]?key|authorization:[[:space:]]*bearer|api[_ -]?key' >/dev/null; then
  fail "manifest contains a credential-shaped value"
fi

rg -Fq '> 状态：兼容基线已冻结；真实 Provider canary 未运行；统一引擎尚未实现' "$QUALIFICATION" ||
  fail "qualification status is missing"
rg -Fq '四个逻辑 operation / 三个公开 surface' "$QUALIFICATION" ||
  fail "qualification operation matrix marker is missing"
rg -Fq 'e7b18cdba9c40fa941a6a70fd8beb2629ecc4232dcc5d94316edbaf4fdae097e' \
  "$QUALIFICATION" || fail "qualification manifest digest is missing"

cargo test -p buzz-semantic-query \
  --test compatibility_baseline compatibility_manifest_matches_golden

if [[ "$SCOPE" == "manifest-only" ]]; then
  printf 'semantic retrieval compatibility manifest gate passed\n'
  exit 0
fi

cargo test -p buzz-semantic-query --lib
cargo test -p buzz-core --lib
cargo test -p buzz-sdk --lib
cargo test -p buzz-db --lib semantic_
cargo test -p buzz-db --lib coordinate_search
cargo test -p buzz-db --lib one_hop
cargo test -p buzz-relay --lib semantic_
cargo test -p buzz-relay --lib coordinate_search
cargo test -p buzz-relay --lib one_hop
cargo test -p carryforth-cli

if [[ "$SCOPE" == "all" ]]; then
  cargo check \
    -p buzz-semantic-query \
    -p buzz-core \
    -p buzz-sdk \
    -p buzz-db \
    -p buzz-relay \
    -p carryforth-cli
fi

if [[ "${SEMANTIC_COMPATIBILITY_ENFORCE_FREEZE_DIFF:-0}" == "1" ]]; then
  changed_paths="$(
    {
      git diff --name-only "$BASELINE_COMMIT" --
      git ls-files --others --exclude-standard
    } | LC_ALL=C sort -u
  )"
  unexpected_paths="$(
    while IFS= read -r path; do
      [[ -z "$path" ]] && continue
      case "$path" in
        Justfile | \
          docs/stage/TODO.md | \
          "docs/stage/semantic/ unified-engine/"* | \
          crates/buzz-semantic-query/tests/* | \
          scripts/check-semantic-retrieval-compatibility-baseline.sh)
          ;;
        *)
          printf '%s\n' "$path"
          ;;
      esac
    done <<<"$changed_paths"
  )"
  [[ -z "$unexpected_paths" ]] ||
    fail "production-path freeze diff contains unexpected files: ${unexpected_paths//$'\n'/, }"
fi

printf 'semantic retrieval compatibility baseline passed (%s)\n' "$SCOPE"
