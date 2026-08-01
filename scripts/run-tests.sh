#!/usr/bin/env bash
# =============================================================================
# run-tests.sh — Run Buzz test suite
# =============================================================================
# Usage:
#   ./scripts/run-tests.sh              # run all tests (default)
#   ./scripts/run-tests.sh unit         # unit tests only (no infra needed)
#   ./scripts/run-tests.sh integration  # integration tests only
#   ./scripts/run-tests.sh all          # explicit all
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
MODE="${1:-all}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

log()    { echo -e "${BLUE}[run-tests]${NC} $*"; }
success(){ echo -e "${GREEN}[run-tests]${NC} $*"; }
warn()   { echo -e "${YELLOW}[run-tests]${NC} $*"; }
error()  { echo -e "${RED}[run-tests]${NC} $*" >&2; }
section(){ echo -e "\n${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; echo -e "${CYAN}  $*${NC}"; echo -e "${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"; }

cd "${REPO_ROOT}"

# ---- Load .env if present ---------------------------------------------------

# Explicit caller values must win over repository defaults, including when a
# local .env exists. This lets CI and isolated integration runs select a fresh
# database without mutating the developer's configuration.
CALLER_DATABASE_URL_SET="${DATABASE_URL+x}"
CALLER_DATABASE_URL="${DATABASE_URL-}"
CALLER_PGHOST_SET="${PGHOST+x}"
CALLER_PGHOST="${PGHOST-}"
CALLER_PGPORT_SET="${PGPORT+x}"
CALLER_PGPORT="${PGPORT-}"
CALLER_PGUSER_SET="${PGUSER+x}"
CALLER_PGUSER="${PGUSER-}"
CALLER_PGPASSWORD_SET="${PGPASSWORD+x}"
CALLER_PGPASSWORD="${PGPASSWORD-}"
CALLER_PGDATABASE_SET="${PGDATABASE+x}"
CALLER_PGDATABASE="${PGDATABASE-}"
CALLER_REDIS_URL_SET="${REDIS_URL+x}"
CALLER_REDIS_URL="${REDIS_URL-}"

if [[ -f ".env" ]]; then
  log "Loading .env..."
  set -o allexport
  # shellcheck disable=SC1091
  source .env
  set +o allexport
else
  # Use defaults matching docker-compose.yml
  export DATABASE_URL="${DATABASE_URL:-postgres://buzz:buzz_dev@localhost:5432/buzz}" # sadscan:disable np.postgres.1
  export PGHOST="${PGHOST:-localhost}"
  export PGPORT="${PGPORT:-5432}"
  export PGUSER="${PGUSER:-buzz}"
  export PGPASSWORD="${PGPASSWORD:-buzz_dev}"
  export PGDATABASE="${PGDATABASE:-buzz}"
  export REDIS_URL="${REDIS_URL:-redis://localhost:6379}"
fi

if [[ -n "${CALLER_DATABASE_URL_SET}" ]]; then export DATABASE_URL="${CALLER_DATABASE_URL}"; fi
if [[ -n "${CALLER_PGHOST_SET}" ]]; then export PGHOST="${CALLER_PGHOST}"; fi
if [[ -n "${CALLER_PGPORT_SET}" ]]; then export PGPORT="${CALLER_PGPORT}"; fi
if [[ -n "${CALLER_PGUSER_SET}" ]]; then export PGUSER="${CALLER_PGUSER}"; fi
if [[ -n "${CALLER_PGPASSWORD_SET}" ]]; then export PGPASSWORD="${CALLER_PGPASSWORD}"; fi
if [[ -n "${CALLER_PGDATABASE_SET}" ]]; then export PGDATABASE="${CALLER_PGDATABASE}"; fi
if [[ -n "${CALLER_REDIS_URL_SET}" ]]; then export REDIS_URL="${CALLER_REDIS_URL}"; fi

# ---- Track results ----------------------------------------------------------

declare -a PASSED=()
declare -a FAILED=()

run_test_step() {
  local name="$1"
  shift
  log "Running: ${name}"
  if "$@"; then
    success "${name} passed"
    PASSED+=("${name}")
  else
    error "${name} FAILED"
    FAILED+=("${name}")
  fi
}

# ---- Check / start infra (for integration tests) ----------------------------

ensure_infra() {
  # This process has already resolved .env plus any explicit caller
  # overrides. Prevent the nested seed helper from sourcing .env a second
  # time and splitting migration/test traffic across databases.
  BUZZ_SKIP_ENV_FILE=true "${REPO_ROOT}/bin/just" _ensure-migrations
}

# ---- Unit tests (no infra needed) -------------------------------------------

run_unit_tests() {
  section "Unit Tests (no infra required)"

  run_test_step "buzz-core tests" \
    cargo test -p buzz-core --lib -- --nocapture

  run_test_step "buzz-auth unit tests" \
    cargo test -p buzz-auth --lib -- --nocapture

  # buzz-db migrator/lint unit tests (no infra): guard the embedded-migrator
  # invariant (exactly the consolidated 0001; cutover/backfill stays an operator
  # script, not startup state) and the tenant-scoping lints. The Postgres-backed
  # buzz-db tests are #[ignore]d; nothing here (or in integration mode below,
  # which runs `cargo test -p buzz-db` without --ignored) runs them — they need a
  # separate isolated-DB gate, so --lib keeps this step infra-free.
  run_test_step "buzz-db unit tests" \
    cargo test -p buzz-db --lib -- --nocapture

  # Multi-tenant conformance gate: independent replay checker + golden
  # fixtures (buzz-conformance). Pure in-process trace replay, no infra.
  run_test_step "buzz-conformance tests" \
    cargo test -p buzz-conformance -- --nocapture

  # Keep the fallback path equivalent to `just project-view-test-unit`.
  # `just test-unit` uses this function only when cargo-nextest is unavailable.
  run_test_step "Project View domain tests" \
    cargo test -p buzz-project-view -- --nocapture

  run_test_step "Project View core protocol tests" \
    cargo test -p buzz-core --lib project_view -- --nocapture

  run_test_step "Project View SDK tests" \
    cargo test -p buzz-sdk --lib project_view -- --nocapture

  run_test_step "Project View Relay adapter tests" \
    cargo test -p buzz-relay --lib project_view -- --nocapture

  run_test_step "Project View CLI tests" \
    cargo test -p buzz-cli --lib project_view -- --nocapture

  run_test_step "buzz-push-gateway tests" \
    cargo test -p buzz-push-gateway -- --nocapture

  # ACP owns the Meeting runtime and its privacy-safe wire/log boundary. Run
  # the complete lib suite so cross-cutting tests are not orphaned by a name
  # filter.
  run_test_step "buzz-acp lib tests" \
    cargo test -p buzz-acp --lib -- --nocapture

  run_test_step "buzz-relay Meeting unit tests" \
    cargo test -p buzz-relay --lib meeting -- --nocapture
}

# ---- DB / integration tests (infra required) --------------------------------

run_integration_tests() {
  section "Integration Tests (requires running services)"

  ensure_infra

  run_test_step "buzz-db tests" \
    cargo test -p buzz-db -- --nocapture

  if find crates/buzz-auth/tests -maxdepth 1 -name '*.rs' -print -quit 2>/dev/null | grep -q .; then
    run_test_step "buzz-auth integration tests" \
      cargo test -p buzz-auth --test '*' -- --nocapture
  else
    run_test_step "buzz-auth (no integration tests found)" true
  fi

  run_test_step "workspace integration tests" \
    cargo test --test '*' -- --nocapture 2>/dev/null || \
    run_test_step "workspace integration tests (none found)" true
}

# ---- Main -------------------------------------------------------------------

START_TIME=$(date +%s)

case "${MODE}" in
  unit)
    run_unit_tests
    ;;
  integration)
    run_integration_tests
    ;;
  all|*)
    run_unit_tests
    run_integration_tests
    ;;
esac

END_TIME=$(date +%s)
ELAPSED=$((END_TIME - START_TIME))

# ---- Summary ----------------------------------------------------------------

section "Test Summary"
echo ""
echo -e "  Duration: ${ELAPSED}s"
echo ""

if [[ ${#PASSED[@]} -gt 0 ]]; then
  echo -e "  ${GREEN}Passed (${#PASSED[@]}):${NC}"
  for t in "${PASSED[@]}"; do
    echo -e "    ${GREEN}pass${NC} ${t}"
  done
fi

if [[ ${#FAILED[@]} -gt 0 ]]; then
  echo ""
  echo -e "  ${RED}Failed (${#FAILED[@]}):${NC}"
  for t in "${FAILED[@]}"; do
    echo -e "    ${RED}fail${NC} ${t}"
  done
  echo ""
  exit 1
fi

echo ""
success "All tests passed!"
exit 0
