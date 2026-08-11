#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RELEASE_SOURCE=0

if [[ "${1:-}" == "--release-source" ]]; then
  RELEASE_SOURCE=1
elif [[ $# -ne 0 ]]; then
  echo "Usage: $0 [--release-source]" >&2
  exit 2
fi

failures=()

fail() {
  failures+=("$1")
}

require_file() {
  local relative="$1"
  if [[ ! -f "$REPO_ROOT/$relative" ]]; then
    fail "required file is missing: $relative"
  fi
}

reject_pattern() {
  local pattern="$1"
  local label="$2"
  shift 2
  local existing=()
  local relative
  for relative in "$@"; do
    if [[ -e "$REPO_ROOT/$relative" ]]; then
      existing+=("$REPO_ROOT/$relative")
    fi
  done
  if [[ ${#existing[@]} -eq 0 ]]; then
    return
  fi
  if rg --line-number --ignore-case -- "$pattern" "${existing[@]}" >/dev/null; then
    fail "$label"
  fi
}

require_file "LICENSE"
require_file "NOTICE"
require_file "UPSTREAM.md"
require_file "docs/lora/stage/carryforth/open-source-release-surface-plan.md"
require_file "docs/release/THIRD_PARTY_ASSETS.md"
require_file "docs/release/packaged-assets.json"
require_file "scripts/check-public-package-metadata.py"
require_file "scripts/check-release-asset-inventory.sh"
require_file "scripts/test-retired-compose-entrypoint.sh"

if ! jq -e '
  .bundle.licenseFile == "../../LICENSE"
  and .bundle.resources["../../LICENSE"] == "licenses/LICENSE"
  and .bundle.resources["../../NOTICE"] == "licenses/NOTICE"
  and .bundle.resources["../../UPSTREAM.md"] == "licenses/UPSTREAM.md"
  and .bundle.resources["../../docs/release/THIRD_PARTY_ASSETS.md"] == "licenses/THIRD_PARTY_ASSETS.md"
' "$REPO_ROOT/desktop/src-tauri/tauri.conf.json" >/dev/null; then
  fail "Desktop packages do not embed the required license and attribution files"
fi

reject_pattern \
  'global\.block-artifacts\.com|block-pypi|blox\.sqprod|\.sqprod\.co' \
  "public dependency locks still contain Block-internal package coordinates" \
  "benchmarks/harbor-buzz-orchestra/uv.lock" \
  "benchmarks/harbor-buzz-orchestra/testbed/uv.lock"

reject_pattern \
  'global\.block-artifacts\.com|\.sqprod\.co|sprout-oss\.stage|buzz-oss\.stage|block(-lakehouse-production)?\.cloud\.databricks\.com' \
  "current source, test fixtures, or public examples still disclose internal service coordinates" \
  "scripts/cutover/1321_backfill_default_community.sql" \
  "crates/buzz-acp/src/config.rs" \
  "desktop/src-tauri/src/commands/pairing.rs" \
  "desktop/src-tauri/src/managed_agents/readiness.rs" \
  "desktop/src-tauri/src/managed_agents/config_bridge/goose.rs" \
  "desktop/src-tauri/src/managed_agents/agent_env.rs" \
  "desktop/src/shared/lib/mediaUrl.ts" \
  "desktop/src/features/communities/communityStorage.ts" \
  "desktop/src/features/agents/ui/agentSessionToolSummary.test.mjs" \
  "desktop/tests/e2e/scrollback-buzzbugs.perf.ts" \
  "desktop/tests/e2e/scroll-history.spec.ts"

reject_pattern \
  'Buzz\.app|buzz-desktop-latest|github\.com/block/buzz/releases|block/apple-codesign-action' \
  "public release workflows still contain the retired Buzz/Block Desktop release path" \
  ".github/workflows/release.yml" \
  ".github/workflows/signed-macos-canary.yml" \
  "desktop/scripts/build-release-config.mjs"

reject_pattern \
  'https://github\.com/block/sprout' \
  "workspace package metadata still points at the retired block/sprout repository" \
  "Cargo.toml"

reject_pattern \
  'https://push\.buzz\.xyz' \
  "the Carryforth Local Relay production configuration still contains the Buzz Push endpoint" \
  "crates/buzz-relay/src/config.rs"

reject_pattern \
  'Buzz Relay|github\.com/block/buzz' \
  "the public Relay NIP-11 document still exposes the retired Buzz product identity" \
  "crates/buzz-relay/src/nip11.rs"

reject_pattern \
  'web-builder|/srv/buzz/(web|admin-web)|BUZZ_(WEB|ADMIN_WEB)_DIR' \
  "the public Relay image still bundles a source-only Web or Admin product surface" \
  "Dockerfile"

if ! rg -q 'COPY LICENSE NOTICE UPSTREAM\.md /usr/share/licenses/carryforth/' "$REPO_ROOT/Dockerfile" ||
  ! rg -q 'COPY docs/release/THIRD_PARTY_ASSETS\.md /usr/share/licenses/carryforth/' "$REPO_ROOT/Dockerfile"; then
  fail "the public Relay image does not embed license and attribution files"
fi
if ! rg -q '^!docs/release/THIRD_PARTY_ASSETS\.md$' "$REPO_ROOT/.dockerignore"; then
  fail "the Relay Docker build context excludes its required attribution file"
fi
if ! rg -q 'COMPOSE_ENTRYPOINT_RETIRED=1' "$REPO_ROOT/deploy/compose/run.sh" ||
  ! rg -q '^services:[[:space:]]*\{\}[[:space:]]*$' "$REPO_ROOT/deploy/compose/compose.yml"; then
  fail "the retired legacy Compose entrypoint is still runnable"
fi

reject_pattern \
  'github\.com/block/(buzz|sprout)|ghcr\.io/block|buzz-desktop-latest|builderlab|buzz\.xyz|Buzz\.app' \
  "a current release or local deployment surface still references the retired Buzz/Block product path" \
  ".github/workflows/release.yml" \
  ".github/workflows/docker.yml" \
  ".github/workflows/signed-macos-canary.yml" \
  "Dockerfile" \
  "deploy/local/compose.yml" \
  "deploy/local/.env.example" \
  "deploy/compose/compose.yml" \
  "deploy/compose/.env.example" \
  "deploy/compose/README.md" \
  "desktop/scripts/build-release-config.mjs"

reject_pattern \
  'image:[[:space:]]*[^#[:space:]]+:(main|latest|master|edge|nightly|dev)([[:space:]#]|$)|^[A-Za-z_][A-Za-z0-9_]*_IMAGE=[^#[:space:]]+:(main|latest|master|edge|nightly|dev)([[:space:]#]|$)' \
  "an active deployment surface still defaults to a floating container tag" \
  "deploy/local/compose.yml" \
  "deploy/local/.env.example" \
  "deploy/compose/compose.yml" \
  "deploy/compose/.env.example"

reject_pattern \
  'push_runtime' \
  "the Carryforth Relay binary still starts the retired Push runtime" \
  "crates/buzz-relay/src/main.rs" \
  "crates/buzz-relay/src/lib.rs"

reject_pattern \
  'fn push_descriptor|push_descriptor\(' \
  "the Carryforth Relay still builds a Push descriptor for NIP-11" \
  "crates/buzz-relay/src/nip11.rs"

mapfile -t migration_files < <(
  git -C "$REPO_ROOT" ls-files 'migrations/[0-9][0-9][0-9][0-9]_*.sql' | sort
)
if [[ ${#migration_files[@]} -eq 0 ]]; then
  fail "no tracked SQL migrations were found"
else
  expected=1
  declare -A seen_prefixes=()
  for relative in "${migration_files[@]}"; do
    filename="${relative##*/}"
    prefix="${filename%%_*}"
    numeric=$((10#$prefix))
    if [[ -n "${seen_prefixes[$prefix]:-}" ]]; then
      fail "duplicate migration prefix $prefix: ${seen_prefixes[$prefix]} and $relative"
    fi
    seen_prefixes[$prefix]="$relative"
    if [[ $numeric -ne $expected ]]; then
      fail "migration sequence is not contiguous: expected $(printf '%04d' "$expected"), found $prefix"
      expected=$numeric
    fi
    expected=$((expected + 1))
  done
fi

if [[ $RELEASE_SOURCE -eq 1 ]]; then
  if [[ -n "$(git -C "$REPO_ROOT" status --porcelain=v1 --untracked-files=all)" ]]; then
    fail "release source worktree is not clean"
  fi

  head_commit="$(git -C "$REPO_ROOT" rev-parse HEAD)"
  exact_tag="$(git -C "$REPO_ROOT" tag --points-at "$head_commit" | head -n 1)"
  if [[ -z "$exact_tag" ]]; then
    fail "release source HEAD is not tagged"
  fi

  while IFS= read -r relative; do
    if [[ "$relative" == migrations/*.sql ]]; then
      fail "untracked migration is present in release source: $relative"
    fi
  done < <(git -C "$REPO_ROOT" ls-files --others --exclude-standard)
fi

if [[ ${#failures[@]} -ne 0 ]]; then
  echo "Carryforth open-source release surface check failed:" >&2
  for failure in "${failures[@]}"; do
    echo "- $failure" >&2
  done
  exit 1
fi

"$REPO_ROOT/scripts/check-public-package-metadata.py"
"$REPO_ROOT/scripts/test-retired-compose-entrypoint.sh"

if [[ $RELEASE_SOURCE -eq 1 ]]; then
  "$REPO_ROOT/scripts/check-release-asset-inventory.sh" --release
else
  "$REPO_ROOT/scripts/check-release-asset-inventory.sh"
fi

echo "Carryforth open-source release surface check passed."
