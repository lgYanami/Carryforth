#!/usr/bin/env bash
# =============================================================================
# start-relay-for-tests.sh — Start the Buzz relay and its backing services
# =============================================================================
# Shared script for CI jobs that need a running relay. Starts docker compose
# services, waits for health, applies the schema, builds the relay, starts it,
# and polls readiness.
#
# Usage:
#   ./scripts/start-relay-for-tests.sh [--profile <cargo-profile>] [--no-build] [--no-schema]
#
# Options:
#   --profile <profile>   Cargo build profile (default: ci)
#   --no-build            Use existing target/<profile>/ binaries (CI artifact reuse)
#   --no-schema           Reuse an already-prepared database (Relay restart tests)
#
# Exports:
#   RELAY_URL=ws://localhost:${BUZZ_TEST_RELAY_PORT:-3000}
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ── Defaults ──────────────────────────────────────────────────────────────────

CARGO_PROFILE="${CARGO_PROFILE:-ci}"
SKIP_BUILD=false
SKIP_SCHEMA=false
RELAY_PID_FILE="${BUZZ_TEST_RELAY_PID_FILE:-/tmp/buzz-relay.pid}"
RELAY_LOG_FILE="${BUZZ_TEST_RELAY_LOG_FILE:-/tmp/buzz-relay.log}"
RELAY_PORT="${BUZZ_TEST_RELAY_PORT:-3000}"
RELAY_HEALTH_PORT="${BUZZ_TEST_RELAY_HEALTH_PORT:-8080}"
RELAY_METRICS_PORT="${BUZZ_TEST_RELAY_METRICS_PORT:-9102}"
for port_setting in RELAY_PORT RELAY_HEALTH_PORT RELAY_METRICS_PORT; do
  port_value="${!port_setting}"
  if [[ ! "${port_value}" =~ ^[0-9]+$ ]] || ((port_value < 1 || port_value > 65535)); then
    echo "${port_setting} must be an integer between 1 and 65535." >&2
    exit 1
  fi
done

# ── Parse args ────────────────────────────────────────────────────────────────

while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      CARGO_PROFILE="$2"
      shift 2
      ;;
    --no-build)
      SKIP_BUILD=true
      shift
      ;;
    --no-schema)
      SKIP_SCHEMA=true
      shift
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

# ── Colors ────────────────────────────────────────────────────────────────────

BLUE='\033[0;34m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'

log()   { echo -e "${BLUE}[relay-test]${NC} $*"; }
ok()    { echo -e "${GREEN}[relay-test]${NC} $*"; }
err()   { echo -e "${RED}[relay-test]${NC} $*" >&2; }

# ── Start docker compose services ────────────────────────────────────────────

cd "${REPO_ROOT}"

log "Starting docker compose services..."
docker compose up -d postgres redis minio minio-init

# ── Wait for services to be healthy ──────────────────────────────────────────

wait_healthy() {
  local service="$1"
  local container="$2"
  log "Waiting for ${service}..."
  for attempt in $(seq 1 60); do
    status=$(docker inspect --format='{{.State.Health.Status}}' "${container}" 2>/dev/null || echo "not_found")
    if [ "${status}" = "healthy" ]; then
      ok "${service} is healthy"
      return 0
    fi
    sleep 2
  done
  err "${service} did not become healthy within 120s"
  docker logs "${container}" || true
  return 1
}

wait_healthy "Postgres" "buzz-postgres"
wait_healthy "Redis" "buzz-redis"
wait_healthy "MinIO" "buzz-minio"

# This helper owns the local docker-compose Postgres instance. Keep schema,
# partition attachment, seeding, and Relay startup on one connection tuple;
# callers may isolate a run by changing only the database name.
export PGHOST=localhost
export PGPORT=5432
export PGUSER=buzz
export PGPASSWORD=buzz_dev
export PGDATABASE="${BUZZ_TEST_PGDATABASE:-buzz}"
export DATABASE_URL="postgres://${PGUSER}:${PGPASSWORD}@${PGHOST}:${PGPORT}/${PGDATABASE}"

# ── Apply database schema ────────────────────────────────────────────────────

if [[ "${SKIP_SCHEMA}" == "true" ]]; then
  log "Skipping database schema (--no-schema); reusing ${PGDATABASE}"
else
  log "Applying database schema..."

  # Use the already-running docker postgres for desired-state planning instead of
  # downloading an embedded Postgres from Maven Central (transient-fetch flake source).
  export PGSCHEMA_PLAN_HOST="${PGHOST}"
  export PGSCHEMA_PLAN_PORT="${PGPORT}"
  export PGSCHEMA_PLAN_DB="${PGDATABASE}"
  export PGSCHEMA_PLAN_USER="${PGUSER}"
  export PGSCHEMA_PLAN_PASSWORD="${PGPASSWORD}"

  ./bin/pgschema apply --file schema/schema.sql --auto-approve
  docker exec -i -e PGPASSWORD="${PGPASSWORD}" buzz-postgres \
    psql -U "${PGUSER}" -d "${PGDATABASE}" -v ON_ERROR_STOP=1 < scripts/attach-schema-partitions.sql
  ok "Schema applied"
fi

# ── Seed the deployment community ────────────────────────────────────────────
# Multi-tenant: the relay resolves every connection's tenant from the durable
# communities host map (WHERE host = normalize_host($1)). normalize_host keeps
# non-default ports, so the host must include the selected test port verbatim
# to match RELAY_URL. The relay never auto-seeds a community
# (ensure_configured_community has no callers) and fails closed on an unmapped
# host, so without this row every e2e connection would 404 at host-binding.
# The unique index is on lower(host), so ON CONFLICT must target that expression.
# psql is not on PATH in the hermit env; postgres runs as the buzz-postgres
# docker container, so exec into it (same fallback as setup-desktop-test-data.sh).
log "Seeding deployment community (host=localhost:${RELAY_PORT})..."
if command -v psql >/dev/null 2>&1; then
  seed_psql() { PGPASSWORD="${PGPASSWORD}" psql -h "${PGHOST}" -p "${PGPORT}" -U "${PGUSER}" -d "${PGDATABASE}" -qtA "$@"; }
else
  seed_psql() { docker exec -e PGPASSWORD="${PGPASSWORD}" buzz-postgres psql -U "${PGUSER}" -d "${PGDATABASE}" -qtA "$@"; }
fi
seed_psql -c "
INSERT INTO communities (id, host)
VALUES ('00000000-0000-4000-8000-00000000c0de', 'localhost:${RELAY_PORT}')
ON CONFLICT (lower(host)) DO NOTHING
;
"
ok "Community seeded"

# ── Build relay ──────────────────────────────────────────────────────────────

if [[ "${SKIP_BUILD}" == "true" ]]; then
  for bin in buzz-relay git-credential-nostr; do
    if [[ ! -x "./target/${CARGO_PROFILE}/${bin}" ]]; then
      err "--no-build: ./target/${CARGO_PROFILE}/${bin} missing or not executable"
      exit 1
    fi
  done
  log "Skipping relay build (--no-build); using existing target/${CARGO_PROFILE}/ binaries"
else
  log "Building relay (profile: ${CARGO_PROFILE})..."
  cargo build --profile "${CARGO_PROFILE}" -p buzz-relay -p git-credential-nostr
  ok "Relay built"
fi

# ── Start relay ──────────────────────────────────────────────────────────────

log "Starting relay..."
nohup env \
  DATABASE_URL="${DATABASE_URL}" \
  REDIS_URL="${REDIS_URL:-redis://localhost:6379}" \
  RELAY_URL="ws://localhost:${RELAY_PORT}" \
  BUZZ_BIND_ADDR="0.0.0.0:${RELAY_PORT}" \
  BUZZ_HEALTH_PORT="${RELAY_HEALTH_PORT}" \
  BUZZ_METRICS_PORT="${RELAY_METRICS_PORT}" \
  BUZZ_REQUIRE_AUTH_TOKEN=false \
  BUZZ_RECONCILE_CHANNELS=true \
  BUZZ_GIT_PROBE_WRITERS=8 \
  "./target/${CARGO_PROFILE}/buzz-relay" > "${RELAY_LOG_FILE}" 2>&1 &
echo $! > "${RELAY_PID_FILE}"

# ── Poll readiness ───────────────────────────────────────────────────────────

log "Waiting for relay readiness..."
for attempt in $(seq 1 60); do
  if ! kill -0 "$(<"${RELAY_PID_FILE}")" 2>/dev/null; then
    err "Relay process died"
    cat "${RELAY_LOG_FILE}"
    exit 1
  fi
  status_code=$(curl -s -o /dev/null -w "%{http_code}" \
    "http://127.0.0.1:${RELAY_PORT}/_readiness" || true)
  if [ "${status_code}" = "200" ]; then
    ok "Relay is ready at ws://localhost:${RELAY_PORT}"
    export RELAY_URL="ws://localhost:${RELAY_PORT}"
    exit 0
  fi
  sleep 1
done

err "Relay did not become ready within 60s"
cat "${RELAY_LOG_FILE}"
exit 1
