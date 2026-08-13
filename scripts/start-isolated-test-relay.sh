#!/usr/bin/env bash
# =============================================================================
# start-isolated-test-relay.sh — singleton disposable test Relay harness
# =============================================================================
# Starts a Relay from the current source with a dedicated `buzz-harness`
# Compose project and alternate loopback ports. The project name, ports, volume
# names, and tmux session are fixed, so this is a singleton harness: it is not
# safe to run concurrently from another checkout. The script fails before
# changing state when that singleton is already active.
#
# Topology:
#   compose project : buzz-harness
#   postgres        : 127.0.0.1:5471  (db=buzz, user=buzz, pass=buzz_dev)
#   redis           : 127.0.0.1:6471
#   minio           : 127.0.0.1:9471 (console 9472)
#   Relay main      : 127.0.0.1:3030
#   Relay health    : 127.0.0.1:8088
#   Relay metrics   : 127.0.0.1:9202
#   client URL      : http://localhost:3030 (matches the seeded Community host)
#
# Usage:
#   ./scripts/start-isolated-test-relay.sh [--profile <cargo-profile>]
#   ./scripts/start-isolated-test-relay.sh --reset-database
#
# Ordinary teardown preserves harness data:
#   tmux kill-session -t buzz-harness-relay
#   docker compose -p buzz-harness -f docker-compose.harness.yml down
# Delete the dedicated harness volumes only when their data is disposable:
#   docker compose -p buzz-harness -f docker-compose.harness.yml down -v
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

CARGO_PROFILE="${CARGO_PROFILE:-ci}"
RESET_DATABASE=false
while [[ $# -gt 0 ]]; do
  case "$1" in
    --profile)
      if [[ $# -lt 2 || -z "$2" ]]; then
        echo "--profile requires a non-empty Cargo profile" >&2
        exit 2
      fi
      CARGO_PROFILE="$2"
      shift 2
      ;;
    --reset-database)
      RESET_DATABASE=true
      shift
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 2
      ;;
  esac
done

if [[ ! "${CARGO_PROFILE}" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "Invalid Cargo profile: ${CARGO_PROFILE}" >&2
  exit 2
fi

# Cargo calls the development profile `dev`, but writes its binaries under
# target/debug. Accept `debug` as the user-facing spelling too.
case "${CARGO_PROFILE}" in
  dev|debug)
    CARGO_BUILD_PROFILE="dev"
    CARGO_TARGET_PROFILE="debug"
    ;;
  *)
    CARGO_BUILD_PROFILE="${CARGO_PROFILE}"
    CARGO_TARGET_PROFILE="${CARGO_PROFILE}"
    ;;
esac

PROJECT="buzz-harness"
COMPOSE_FILE="docker-compose.harness.yml"
TMUX_SESSION="buzz-harness-relay"

# Fixed singleton ports, distinct from the ordinary source-development stack.
PG_PORT=5471
REDIS_PORT=6471
MINIO_PORT=9471
MINIO_CONSOLE_PORT=9472
RELAY_MAIN=3030
RELAY_HEALTH=8088
RELAY_METRICS=9202
COMMUNITY_HOST="localhost:${RELAY_MAIN}"
RELAY_LOG="${REPO_ROOT}/target/test-harness/buzz-relay.log"
RELAY_BIN="${REPO_ROOT}/target/${CARGO_TARGET_PROFILE}/buzz-relay"

BLUE='\033[0;34m'
GREEN='\033[0;32m'
RED='\033[0;31m'
NC='\033[0m'
log() { echo -e "${BLUE}[isolated-relay]${NC} $*"; }
ok() { echo -e "${GREEN}[isolated-relay]${NC} $*"; }
err() { echo -e "${RED}[isolated-relay]${NC} $*" >&2; }

# Put the repository-pinned Hermit shims first. Resolving a shim may download
# its pinned tool on first use, but all such checks happen before Docker or
# database state is changed.
export PATH="${REPO_ROOT}/bin:${PATH}"

preflight_failed=false
require_command() {
  local command_name="$1"
  if ! command -v "${command_name}" >/dev/null 2>&1; then
    err "Required command not found: ${command_name}"
    preflight_failed=true
  fi
}

for command_name in bash cargo curl docker grep lsof python3 tmux; do
  require_command "${command_name}"
done

for executable_path in \
  "${REPO_ROOT}/bin/pgschema" \
  "${REPO_ROOT}/scripts/setup-desktop-test-data.sh"; do
  if [[ ! -x "${executable_path}" ]]; then
    err "Required executable not found or not executable: ${executable_path}"
    preflight_failed=true
  fi
done

for required_path in \
  "${REPO_ROOT}/${COMPOSE_FILE}" \
  "${REPO_ROOT}/schema/schema.sql" \
  "${REPO_ROOT}/scripts/attach-schema-partitions.sql"; do
  if [[ ! -f "${required_path}" ]]; then
    err "Required harness input not found: ${required_path}"
    preflight_failed=true
  fi
done

if [[ "${preflight_failed}" == true ]]; then
  exit 1
fi

# Exercise every external tool needed later before Compose startup or an
# explicitly requested database reset.
if ! docker compose version >/dev/null 2>&1; then
  err "Docker Compose v2 is not available through 'docker compose'"
  exit 1
fi
if ! docker info >/dev/null 2>&1; then
  err "Docker daemon is not available"
  exit 1
fi
cargo --version >/dev/null
tmux -V >/dev/null
curl --version >/dev/null
python3 --version >/dev/null
"${REPO_ROOT}/bin/pgschema" --help >/dev/null
docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" config --quiet

if tmux list-sessions -F '#{session_name}' 2>/dev/null | grep -Fxq "${TMUX_SESSION}"; then
  err "The singleton tmux session '${TMUX_SESSION}' is already active."
  err "Stop and verify that harness before starting another: tmux kill-session -t ${TMUX_SESSION}"
  exit 1
fi

active_containers="$(docker ps \
  --filter "label=com.docker.compose.project=${PROJECT}" \
  --format '{{.Names}}')"
if [[ -n "${active_containers}" ]]; then
  err "The singleton Compose project '${PROJECT}' already has active containers:"
  printf '%s\n' "${active_containers}" >&2
  err "This fixed-port harness does not support parallel checkouts."
  exit 1
fi

port_conflict=false
for port_spec in \
  "postgres:${PG_PORT}" \
  "redis:${REDIS_PORT}" \
  "minio:${MINIO_PORT}" \
  "minio-console:${MINIO_CONSOLE_PORT}" \
  "relay:${RELAY_MAIN}" \
  "health:${RELAY_HEALTH}" \
  "metrics:${RELAY_METRICS}"; do
  port_name="${port_spec%%:*}"
  port_number="${port_spec##*:}"
  if ! python3 - "${port_number}" <<'PY'
import socket
import sys

with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
    listener.bind(("127.0.0.1", int(sys.argv[1])))
PY
  then
    err "${port_name} port ${port_number} is already in use:"
    lsof -nP -iTCP:"${port_number}" -sTCP:LISTEN >&2 || true
    port_conflict=true
  fi
done
if [[ "${port_conflict}" == true ]]; then
  err "No services were started. Stop only a verified conflicting process or use a different workflow."
  exit 1
fi

# Build first so an invalid profile or source/toolchain failure cannot leave a
# newly started Compose project behind.
log "Building Relay (profile=${CARGO_BUILD_PROFILE}, cargo=$(command -v cargo), $(cargo --version))..."
cargo build --profile "${CARGO_BUILD_PROFILE}" -p buzz-relay
ok "Relay built"

compose_started=false
report_incomplete_start() {
  local exit_status=$?
  if [[ ${exit_status} -ne 0 && "${compose_started:-false}" == true ]]; then
    err "Harness startup stopped after Compose was started; no automatic kill, reset, or volume deletion was performed."
    err "Inspect it, then stop while preserving data with: docker compose -p ${PROJECT} -f ${COMPOSE_FILE} down"
  fi
  return "${exit_status}"
}
trap report_incomplete_start EXIT

log "Bringing up backing services (project=${PROJECT})..."
compose_started=true
docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" up -d

wait_healthy() {
  local service="$1"
  local label="$2"
  local container_id
  local status
  container_id="$(docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" ps -q "${service}")"
  if [[ -z "${container_id}" ]]; then
    err "${label} container was not created"
    return 1
  fi
  for _ in $(seq 1 60); do
    status="$(docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "${container_id}" 2>/dev/null || true)"
    case "${status}" in
      healthy)
        ok "${label} ready"
        return 0
        ;;
      exited|dead|unhealthy)
        err "${label} entered state '${status}'"
        return 1
        ;;
    esac
    sleep 2
  done
  err "${label} did not become healthy"
  return 1
}

wait_healthy postgres "Postgres"
wait_healthy redis "Redis"
wait_healthy minio "MinIO"

# Ensure the one-shot bucket initializer has completed successfully.
minio_init_id="$(docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" ps -a -q minio-init)"
if [[ -z "${minio_init_id}" ]]; then
  err "MinIO initializer container was not created"
  exit 1
fi
for _ in $(seq 1 30); do
  minio_init_status="$(docker inspect --format '{{.State.Status}}' "${minio_init_id}")"
  if [[ "${minio_init_status}" == "exited" ]]; then
    minio_init_code="$(docker inspect --format '{{.State.ExitCode}}' "${minio_init_id}")"
    if [[ "${minio_init_code}" != "0" ]]; then
      err "MinIO initializer failed with exit code ${minio_init_code}"
      exit 1
    fi
    ok "MinIO bucket ready"
    break
  fi
  sleep 1
done
if [[ "${minio_init_status:-}" != "exited" ]]; then
  err "MinIO initializer did not complete"
  exit 1
fi

# Schema and partition setup runs only after all preflight and singleton checks.
export PGPASSWORD=buzz_dev
psql_h() {
  docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" exec -T postgres \
    psql -U buzz -d buzz -v ON_ERROR_STOP=1 "$@"
}

if [[ "${RESET_DATABASE}" == true ]]; then
  log "Resetting the dedicated harness database because --reset-database was provided..."
  psql_h -c "DROP SCHEMA public CASCADE; CREATE SCHEMA public;"
else
  log "Preserving existing harness data; applying the current schema without a reset..."
fi
export PGSCHEMA_PLAN_HOST=127.0.0.1 PGSCHEMA_PLAN_PORT="${PG_PORT}"
export PGSCHEMA_PLAN_DB=buzz PGSCHEMA_PLAN_USER=buzz PGSCHEMA_PLAN_PASSWORD=buzz_dev
export PGHOST=127.0.0.1 PGPORT="${PG_PORT}" PGUSER=buzz PGDATABASE=buzz
./bin/pgschema apply --file schema/schema.sql --auto-approve
psql_h < scripts/attach-schema-partitions.sql
ok "Schema applied"

postgres_container_id="$(docker compose -p "${PROJECT}" -f "${COMPOSE_FILE}" ps -q postgres)"
postgres_container_name="$(docker inspect --format '{{.Name}}' "${postgres_container_id}")"
postgres_container_name="${postgres_container_name#/}"

log "Seeding community (host=${COMMUNITY_HOST}), channels, and members..."
BUZZ_COMMUNITY_HOST="${COMMUNITY_HOST}" \
  BUZZ_DB_HOST=127.0.0.1 BUZZ_DB_PORT="${PG_PORT}" BUZZ_DB_USER=buzz \
  BUZZ_DB_PASS=buzz_dev BUZZ_DB_NAME=buzz \
  BUZZ_DB_DOCKER_CONTAINER="${postgres_container_name}" \
  ./scripts/setup-desktop-test-data.sh
ok "Community + channels + members seeded"

mkdir -p "$(dirname "${RELAY_LOG}")"
: > "${RELAY_LOG}"

# tmux accepts one shell-command string. Quote every value before constructing
# it so checkout paths cannot be interpreted as shell syntax.
# Start from a minimal environment so stale caller credentials, Provider gates,
# or deployment switches cannot silently alter this local test Relay.
printf -v relay_inner_command 'exec env -i PATH=%q DATABASE_URL=%q REDIS_URL=%q RELAY_URL=%q BUZZ_BIND_ADDR=%q BUZZ_HEALTH_PORT=%q BUZZ_METRICS_PORT=%q BUZZ_S3_ENDPOINT=%q BUZZ_S3_ACCESS_KEY=%q BUZZ_S3_SECRET_KEY=%q BUZZ_S3_BUCKET=%q BUZZ_REQUIRE_AUTH_TOKEN=%q BUZZ_REQUIRE_RELAY_MEMBERSHIP=%q BUZZ_RECONCILE_CHANNELS=%q BUZZ_SEMANTIC_WORKER_ENABLED=%q BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=%q %q >> %q 2>&1' \
  "${PATH}" \
  "postgres://buzz:buzz_dev@127.0.0.1:${PG_PORT}/buzz" \
  "redis://127.0.0.1:${REDIS_PORT}" \
  "ws://localhost:${RELAY_MAIN}" \
  "127.0.0.1:${RELAY_MAIN}" \
  "${RELAY_HEALTH}" \
  "${RELAY_METRICS}" \
  "http://127.0.0.1:${MINIO_PORT}" \
  "buzz_dev" \
  "buzz_dev_secret" \
  "buzz-media" \
  "false" \
  "false" \
  "true" \
  "false" \
  "false" \
  "${RELAY_BIN}" \
  "${RELAY_LOG}"
printf -v relay_tmux_command '%q -c %q' "$(command -v bash)" "${relay_inner_command}"

log "Starting Relay in singleton tmux session '${TMUX_SESSION}' on loopback :${RELAY_MAIN} (health :${RELAY_HEALTH}, metrics :${RELAY_METRICS})..."
tmux new-session -d -s "${TMUX_SESSION}" -c "${REPO_ROOT}" "${relay_tmux_command}"

for _ in $(seq 1 30); do
  if curl --fail --silent --show-error --max-time 2 -o /dev/null \
    "http://127.0.0.1:${RELAY_MAIN}/health"; then
    ok "Relay live — BUZZ_E2E_RELAY_URL=http://localhost:${RELAY_MAIN}"
    ok "Logs: ${RELAY_LOG}   Attach: tmux attach -t ${TMUX_SESSION}"
    ok "Stop Relay: tmux kill-session -t ${TMUX_SESSION}"
    ok "Stop services, preserve data: docker compose -p ${PROJECT} -f ${COMPOSE_FILE} down"
    ok "Delete dedicated data: docker compose -p ${PROJECT} -f ${COMPOSE_FILE} down -v"
    compose_started=false
    exit 0
  fi
  if ! tmux list-sessions -F '#{session_name}' 2>/dev/null | grep -Fxq "${TMUX_SESSION}"; then
    err "Relay process exited before readiness; check ${RELAY_LOG}"
    exit 1
  fi
  sleep 1
done

err "Relay did not become ready on loopback :${RELAY_MAIN} within 30s; check ${RELAY_LOG}"
exit 1
