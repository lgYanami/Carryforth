#!/usr/bin/env bash
# Run the complete Meeting backend gate against a locally managed Relay.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RELAY_PID_FILE="/tmp/buzz-meeting-gate-relay-$$.pid"
RELAY_LOG_FILE="/tmp/buzz-meeting-gate-relay-$$.log"
ROLLOUT_FIXTURE_FILE="/tmp/buzz-meeting-v1-rollout-$$.json"
MEETING_CONTRACT_DB="buzz_meeting_gate_$$_contracts"
MEETING_RELAY_DB="buzz_meeting_gate_$$_relay"
MEETING_CONTRACT_DB_CREATED=false
MEETING_RELAY_DB_CREATED=false
# secp256k1 generator x-coordinate; startup only needs a stable deployment
# owner while the revocation E2E seeds its own actor identities.
MEETING_RELAY_OWNER_PUBKEY="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
MEETING_RELAY_PRIVATE_KEY="0000000000000000000000000000000000000000000000000000000000000001"

cd "${REPO_ROOT}"

if curl --silent --fail --max-time 1 http://127.0.0.1:3000/_readiness >/dev/null 2>&1; then
  echo "A Relay is already listening on port 3000; stop it before running this gate." >&2
  exit 1
fi

# This gate owns a local Docker/Relay topology. Do not inherit developer or CI
# endpoints: doing so could send signed test events to an unrelated service.
export REDIS_URL="redis://localhost:6379"
export RELAY_URL="ws://localhost:3000"
export BUZZ_MEETING_V1_CREATE_ENABLED=true
export BUZZ_MEETING_ROLLOUT_FIXTURE="${ROLLOUT_FIXTURE_FILE}"
export BUZZ_TEST_RELAY_PID_FILE="${RELAY_PID_FILE}"
export BUZZ_TEST_RELAY_LOG_FILE="${RELAY_LOG_FILE}"
export BUZZ_RELAY_PRIVATE_KEY="${MEETING_RELAY_PRIVATE_KEY}"

stop_relay() {
  if [[ ! -f "${RELAY_PID_FILE}" ]]; then
    return
  fi
  local relay_pid
  relay_pid="$(<"${RELAY_PID_FILE}")"
  if kill -0 "${relay_pid}" 2>/dev/null; then
    local relay_command
    relay_command="$(ps -p "${relay_pid}" -o command= 2>/dev/null || true)"
    if [[ "${relay_command}" != *"target/${CARGO_PROFILE:-ci}/buzz-relay"* ]]; then
      echo "Refusing to stop unexpected process ${relay_pid}: ${relay_command}" >&2
      rm -f "${RELAY_PID_FILE}"
      return
    fi
    kill "${relay_pid}" 2>/dev/null || true
    for _ in $(seq 1 30); do
      if ! kill -0 "${relay_pid}" 2>/dev/null; then
        break
      fi
      sleep 0.2
    done
    if kill -0 "${relay_pid}" 2>/dev/null; then
      kill -9 "${relay_pid}" 2>/dev/null || true
    fi
  fi
  rm -f "${RELAY_PID_FILE}"
}

drop_test_databases() {
  local database
  local created
  for database in "${MEETING_CONTRACT_DB}" "${MEETING_RELAY_DB}"; do
    if [[ "${database}" == "${MEETING_CONTRACT_DB}" ]]; then
      created="${MEETING_CONTRACT_DB_CREATED}"
    else
      created="${MEETING_RELAY_DB_CREATED}"
    fi
    if [[ "${created}" != "true" ]]; then
      continue
    fi
    if ! docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS \"${database}\" WITH (FORCE)" >/dev/null; then
      echo "Warning: could not remove temporary database ${database}." >&2
    fi
  done
  MEETING_CONTRACT_DB_CREATED=false
  MEETING_RELAY_DB_CREATED=false
}

cleanup() {
  stop_relay
  drop_test_databases
  rm -f "${ROLLOUT_FIXTURE_FILE}" "${RELAY_LOG_FILE}"
}

trap cleanup EXIT

echo "Running infrastructure-free ACP and Relay Meeting contracts..."
cargo test -p buzz-acp --lib -- --nocapture
cargo test -p buzz-relay --lib meeting -- --nocapture

echo "Creating isolated Meeting contract and Relay databases..."
docker compose up -d postgres redis minio minio-init
for _ in $(seq 1 60); do
  if [[ "$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)" = "healthy" ]]; then
    break
  fi
  sleep 2
done
if [[ "$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)" != "healthy" ]]; then
  echo "Postgres did not become healthy." >&2
  exit 1
fi
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"${MEETING_CONTRACT_DB}\" OWNER buzz" >/dev/null
MEETING_CONTRACT_DB_CREATED=true
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"${MEETING_RELAY_DB}\" OWNER buzz" >/dev/null
MEETING_RELAY_DB_CREATED=true

export PGDATABASE="${MEETING_CONTRACT_DB}"
export DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/${MEETING_CONTRACT_DB}"
export BUZZ_TEST_PGDATABASE="${MEETING_CONTRACT_DB}"
export BUZZ_TEST_DATABASE_URL="${DATABASE_URL}"

echo "Running Postgres-backed Meeting state-machine contracts serially..."
cargo test -p buzz-db --lib meeting -- --ignored --test-threads=1 --nocapture

export PGDATABASE="${MEETING_RELAY_DB}"
export DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/${MEETING_RELAY_DB}"
export BUZZ_TEST_PGDATABASE="${MEETING_RELAY_DB}"
export BUZZ_TEST_DATABASE_URL="${DATABASE_URL}"

echo "Starting Relay with Meeting V1 creation enabled..."
BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  "${SCRIPT_DIR}/start-relay-for-tests.sh"

echo "Running Meeting V0/V1 Relay end-to-end tests serially..."
cargo test -p buzz-test-client --test e2e_meeting -- --ignored --test-threads=1 --nocapture
cargo test -p buzz-test-client --test e2e_meeting_floor -- --ignored --test-threads=1 --nocapture
cargo test -p buzz-test-client --test e2e_meeting_baton -- \
  --ignored \
  --test-threads=1 \
  --skip relay_member_removal_disconnects_live_meeting_reader_and_blocks_reentry \
  --nocapture

echo "Creating a Meeting V1 fixture before closing the rollout gate..."
cargo test -p buzz-test-client --test e2e_meeting_rollout \
  create_rollout_fixture_before_gate_closes -- \
  --ignored \
  --test-threads=1 \
  --nocapture

echo "Restarting Relay with Meeting V1 creation disabled..."
stop_relay
BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  BUZZ_MEETING_V1_CREATE_ENABLED=false \
  "${SCRIPT_DIR}/start-relay-for-tests.sh" --no-build --no-schema

cargo test -p buzz-test-client --test e2e_meeting_rollout \
  existing_v1_survives_closed_gate_and_v0_still_works -- \
  --ignored \
  --test-threads=1 \
  --nocapture

echo "Restarting Relay with membership enforcement for revocation coverage..."
stop_relay
BUZZ_REQUIRE_RELAY_MEMBERSHIP=true \
  BUZZ_MEETING_V1_CREATE_ENABLED=true \
  RELAY_OWNER_PUBKEY="${MEETING_RELAY_OWNER_PUBKEY}" \
  "${SCRIPT_DIR}/start-relay-for-tests.sh" --no-build --no-schema

cargo test -p buzz-test-client --test e2e_meeting_baton \
  relay_member_removal_disconnects_live_meeting_reader_and_blocks_reentry -- \
  --ignored \
  --test-threads=1 \
  --nocapture

echo "Meeting backend gate passed."
