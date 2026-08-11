#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

failures=()

fail() {
  failures+=("$1")
}

require_absent() {
  local relative="$1"
  if [[ -e "$REPO_ROOT/$relative" ]]; then
    fail "retired current-product surface is still present: $relative"
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
  if [[ ${#existing[@]} -ne 0 ]] &&
    rg --line-number --ignore-case -- "$pattern" "${existing[@]}" >/dev/null; then
    fail "$label"
  fi
}

for retired in \
  deploy/charts \
  .github/workflows/helm-chart.yml \
  ct.yaml \
  deploy/local/build-and-deploy.sh \
  deploy/local/quickstart-ha-values.yaml \
  deploy/compose/Caddyfile \
  deploy/compose/compose.caddy.yml \
  deploy/compose/compose.dev.yml \
  script/start \
  crates/buzz-push-gateway \
  Dockerfile.push-gateway; do
  require_absent "$retired"
done

for retired_asset in \
  desktop/public/runtime-icons \
  desktop/src/features/onboarding/assets/harness-logos \
  desktop/public/onboarding/starter-team \
  desktop/src-tauri/src/managed_agents/persona_avatars.rs \
  crates/buzz-agent/sprout-agent.png \
  docs/assets/sprout.png \
  docs/assets/sprout-icon.png \
  docs/assets/screenshots \
  mobile/assets/images/buzz-icon.png \
  mobile/ios/Runner/Assets.xcassets/LaunchImage.imageset; do
  require_absent "$retired_asset"
done

if compgen -G "$REPO_ROOT/desktop/public/sounds/*.mp3" >/dev/null; then
  fail "legacy notification MP3 assets are still present"
fi
if compgen -G "$REPO_ROOT/mobile/assets/fonts/Geist*.ttf" >/dev/null; then
  fail "unlicensed Geist font assets are still present"
fi

if [[ -d "$REPO_ROOT/deploy/compose" ]]; then
  actual_compose_files="$({
    shopt -s nullglob dotglob
    for compose_entry in "$REPO_ROOT/deploy/compose"/*; do
      basename "$compose_entry"
    done
  } | LC_ALL=C sort | awk 'BEGIN { separator = "" } { printf "%s%s", separator, $0; separator = " " } END { print "" }')"
  expected_compose_files=".env.example README.md compose.yml run.sh"
  if [[ "$actual_compose_files" != "$expected_compose_files" ]]; then
    fail "deploy/compose must contain only the minimal fail-closed tombstone files"
  fi
  for expected_compose_file in .env.example README.md compose.yml run.sh; do
    if [[ ! -f "$REPO_ROOT/deploy/compose/$expected_compose_file" ]]; then
      fail "deploy/compose tombstone entry must be a regular file: $expected_compose_file"
    fi
  done
  if ! rg -q 'COMPOSE_ENTRYPOINT_RETIRED=1' "$REPO_ROOT/deploy/compose/run.sh" ||
    ! rg -q '^services:[[:space:]]*\{\}[[:space:]]*$' "$REPO_ROOT/deploy/compose/compose.yml"; then
    fail "deploy/compose tombstone is not fail closed"
  fi
fi

reject_pattern \
  'buzz-push-gateway' \
  "the retired Push Gateway executable remains in the default build or test graph" \
  Cargo.toml Cargo.lock Justfile scripts/run-tests.sh .github/workflows/ci.yml \
  .github/workflows/docker.yml .github/workflows/release.yml

reject_pattern \
  'deploy/charts|helm-chart\.yml|build-and-deploy\.sh|quickstart-ha-values\.yaml' \
  "a current build, CI, or operator surface still references the retired Helm/Kubernetes path" \
  Justfile .github/workflows/ci.yml scripts/run-tests.sh \
  scripts/test-project-view-release-contract.sh README.md RELEASING.md \
  docs/project-view-operations.md docs/multi-tenant-relay.md

reject_pattern \
  'Buzz Dev|Buzz Backend|real buzz CLI|command -v buzz|Buzz dev environment|local Buzz|Buzz app' \
  "a local developer surface still presents Buzz as the current product or CLI" \
  Justfile .env.example scripts/instance-env.sh scripts/dev-start.sh \
  scripts/dev-rebuild-start.sh scripts/dev-stop.sh scripts/dev-setup.sh \
  scripts/dev-reset.sh scripts/reset-desktop-dev-state.sh scripts/grab-emoji.sh \
  scripts/build-sprig.sh deploy/local/README.md

reject_pattern \
  'REPO="block/buzz"|--repo[[:space:]]+block/buzz|repos/block/buzz' \
  "a contributor helper still writes branches, comments, or artifacts to the upstream repository" \
  scripts/post-screenshots.sh

reject_pattern \
  'sprig-latest|gh[[:space:]]+release[[:space:]]+(create|edit|upload)' \
  "Sprig still has an independent GitHub Release publication path" \
  .github/workflows/sprig.yml

reject_pattern \
  'ghcr\.io/block/buzz(:|@)|github\.com/block/buzz|"product"[[:space:]]*:[[:space:]]*"Buzz"|"organization"[[:space:]]*:[[:space:]]*"Block"' \
  "the active benchmark surface still defaults to a Block/Buzz image or public submission identity" \
  .github/workflows/benchmark-harbor.yml \
  benchmarks/harbor-buzz-orchestra/scripts/benchmark.py \
  benchmarks/harbor-buzz-orchestra/scripts/run_leaderboard.py

reject_pattern \
  'deploy/compose' \
  "the active benchmark runner still depends on the retired Compose tombstone" \
  benchmarks/harbor-buzz-orchestra/scripts/benchmark.py

benchmark_compose="$REPO_ROOT/benchmarks/harbor-buzz-orchestra/testbed/compose.benchmark.yml"
benchmark_runner="$REPO_ROOT/benchmarks/harbor-buzz-orchestra/scripts/benchmark.py"
if [[ ! -f "$benchmark_compose" ]]; then
  fail "the self-contained benchmark Compose manifest is missing"
else
  for service in relay postgres redis minio minio-init; do
    if ! rg -q "^[[:space:]]{2}${service}:$" "$benchmark_compose"; then
      fail "the benchmark Compose manifest is missing service: $service"
    fi
  done
  if ! rg -Fq 'image: ${BUZZ_IMAGE:?' "$benchmark_compose"; then
    fail "the benchmark Relay image is not explicit and fail closed"
  fi
fi
if [[ ! -f "$benchmark_runner" ]] ||
  ! rg -Fq 'PACKAGE_ROOT / "testbed" / "compose.benchmark.yml"' "$benchmark_runner"; then
  fail "the benchmark runner does not use its self-contained Compose manifest"
fi

reject_pattern \
  'ghcr\.io/block/(buzz|buzz-push-gateway)|oci://ghcr\.io/block|push\.buzz\.xyz|app\.builderlab\.xyz|buzz-desktop-latest|github\.com/block/buzz/releases' \
  "a current local, workflow, or generated-artifact surface still references the retired hosted product" \
  .github/workflows/sprig.yml .github/workflows/benchmark-harbor.yml \
  deploy/local/README.md deploy/local/compose.yml deploy/local/.env.example \
  scripts/build-sprig.sh scripts/instance-env.sh

reject_pattern \
  'api\.github\.com/repos/block/buzz|github\.com/block/buzz/releases|buzz://(join|connect)|carryforth://(join|connect)|Accept invite in Buzz|Open in Buzz|Download Buzz' \
  "the Web runtime still exposes an upstream release or unsupported Desktop handoff" \
  web/index.html web/src

if [[ -e "$REPO_ROOT/web/src/shared/lib/buzz-download.ts" ]] ||
  [[ -e "$REPO_ROOT/web/src/features/repos/ui/ConnectButton.tsx" ]] ||
  [[ -e "$REPO_ROOT/web/src/assets/app-icon@3x.png" ]]; then
  fail "a retired Web Buzz asset, download, or app-handoff implementation is still present"
fi
if [[ ! -f "$REPO_ROOT/web/src/shared/lib/carryforth-source.ts" ]] ||
  ! rg -Fq 'https://github.com/lgYanami/Carryforth#build-and-run-from-source' \
    "$REPO_ROOT/web/src/shared/lib/carryforth-source.ts"; then
  fail "the Web source-only handoff does not point to Carryforth build instructions"
fi
if [[ ! -f "$REPO_ROOT/web/src/assets/carryforth.svg" ]]; then
  fail "the Web source-only surface is missing the Carryforth visual identity"
fi

reject_pattern \
  'Buzz CLI|Buzz app|Buzz Desktop|Buzz text meeting|Buzz instance|Buzz auth|Buzz orientation|hosted instance|REPOS/buzz-nostr' \
  "a current local runtime help, prompt, or test runbook still presents Buzz/Hosted as the product" \
  crates/buzz-acp/README.md crates/buzz-acp/src/config.rs \
  crates/buzz-acp/src/setup_mode.rs crates/buzz-agent/src/auth.rs \
  crates/buzz-acp/src/meeting_v1_prompt.md \
  crates/buzz-acp/src/meeting_prompt.md \
  crates/buzz-acp/src/meeting_participant_intent_prompt.md \
  crates/buzz-acp/src/meeting_moderator_prompt.md \
  crates/buzz-acp/src/meeting_granted_speech_prompt.md \
  crates/buzz-admin/src/main.rs TESTING.md crates/carryforth-cli/TESTING.md \
  docs/buzz-shared-compute-dev.md crates/buzz-persona/PERSONA_PACK_SPEC.md

reject_pattern \
  'Buzz can expose|Buzz clients|In Buzz,|Buzz-agent supports' \
  "a current engineering document still presents Buzz as the active product" \
  docs/admin/README.md docs/MCP_DRIVEN_HOOKS.md \
  docs/bridge-channel-window.md docs/git-on-object-storage.md

reject_pattern \
  'Buzz Nest|Buzz/relay media URLs|Buzz community|across Buzz|Buzz[[:space:]]*<b>Admin' \
  "a managed-Agent or Admin Web surface still presents Buzz as the active product" \
  desktop/src-tauri/src/managed_agents/nest_agents.md \
  desktop/src-tauri/src/managed_agents/screenshot_skill.md \
  admin-web/src/App.tsx admin-web/index.html
reject_pattern \
  'BuzzMark|buzz feedback diagnostics|for the Buzz relay|running Buzz relay' \
  "an Admin fixture or test-client help surface still presents Buzz as the active product" \
  admin-web/src/App.tsx scripts/seed-admin-dashboard.sh \
  crates/buzz-test-client/src/main.rs crates/buzz-test-client/src/lib.rs
if ! rg -q 'const NEST_AGENTS_VERSION: u32 = ([5-9]|[1-9][0-9]+);' \
  "$REPO_ROOT/desktop/src-tauri/src/managed_agents/nest.rs"; then
  fail "the Carryforth Nest template change is not versioned for existing workspaces"
fi

reject_pattern \
  'building on Buzz|Buzz relay|Buzz agents|Buzz participants|Buzz UI|Start Buzz|Buzz-only|Buzz channels|# Buzz$|Buzz admin' \
  "a current source-build guide or example still presents Buzz as the active product" \
  desktop/README.md NOSTR.md examples/README.md \
  examples/countdown-bot/README.md examples/countdown-bot/src/main.rs \
  examples/meadow-core/README.md crates/buzz-pairing-cli/README.md \
  crates/git-credential-nostr/README.md admin-web/index.html

if ! rg -Fq 'Historical upstream conformance record' \
  "$REPO_ROOT/docs/multi-tenant-conformance.md"; then
  fail "the upstream multi-tenant conformance record lacks a historical boundary"
fi

if rg -n '^[[:space:]]*buzz[[:space:]]+install\b' \
  "$REPO_ROOT/crates/buzz-persona/PERSONA_PACK_SPEC.md" >/dev/null; then
  fail "the persona pack specification still publishes the retired buzz install command"
fi

for vision in \
  VISION.md VISION_ACTIVITY.md VISION_AGENT.md VISION_MESH.md \
  VISION_MODERATION.md VISION_PROJECTS.md VISION_SOVEREIGN.md; do
  if ! rg -Fq 'Historical upstream vision' "$REPO_ROOT/$vision"; then
    fail "$vision presents the upstream Buzz narrative without a historical boundary"
  fi
done

if [[ -f "$REPO_ROOT/.github/workflows/benchmark-harbor.yml" ]] &&
  ! rg -q '^name:[[:space:]]+.*Carryforth' "$REPO_ROOT/.github/workflows/benchmark-harbor.yml"; then
  fail "benchmark workflow display name is not Carryforth"
fi

check_package_name() {
  local relative="$1"
  local expected="$2"
  local actual
  actual="$(jq -r '.name // empty' "$REPO_ROOT/$relative")"
  if [[ "$actual" != "$expected" ]]; then
    fail "$relative package name must be $expected (found ${actual:-<empty>})"
  fi
}

check_package_name package.json carryforth-workspace
check_package_name web/package.json carryforth-web
check_package_name admin-web/package.json carryforth-admin-web

if ! rg -Fq 'for binary in cf buzz-acp buzz-admin buzz-relay' \
  "$REPO_ROOT/scripts/meeting-v2-actions-live-acceptance.sh"; then
  fail "Meeting Action acceptance still preflights the retired CLI binary"
fi

if ! rg -Fq '.name == "Carryforth Relay"' \
  "$REPO_ROOT/scripts/e2e-git-perms.sh"; then
  fail "git E2E readiness is not aligned with the Carryforth NIP-11 identity"
fi

if rg -n '\[Buzz events?([[:space:]:—]|$)' \
  "$REPO_ROOT/crates/buzz-acp/src" >/dev/null; then
  fail "ACP still exposes Buzz as the model-visible event product identity"
fi

if [[ ${#failures[@]} -ne 0 ]]; then
  echo "Carryforth current product surface check failed:" >&2
  for failure in "${failures[@]}"; do
    echo "- $failure" >&2
  done
  exit 1
fi

echo "Carryforth current product surface check passed."
