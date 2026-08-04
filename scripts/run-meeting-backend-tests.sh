#!/usr/bin/env bash
# Run the complete Meeting backend gate against a locally managed Relay.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RELAY_PID_FILE="/tmp/buzz-meeting-gate-relay-$$.pid"
RELAY_LOG_FILE="/tmp/buzz-meeting-gate-relay-$$.log"
ROLLOUT_FIXTURE_FILE="/tmp/buzz-meeting-v1-rollout-$$.json"
ACP_CAPABILITY_FILE="/tmp/buzz-meeting-v2-acp-capability-$$.json"
MEETING_CONTRACT_DB="buzz_meeting_gate_$$_contracts"
MEETING_RELAY_DB="buzz_meeting_gate_$$_relay"
MEETING_CONTRACT_DB_CREATED=false
MEETING_RELAY_DB_CREATED=false
MEETING_RELAY_PORT="${BUZZ_TEST_RELAY_PORT:-3000}"
if [[ ! "${MEETING_RELAY_PORT}" =~ ^[0-9]+$ ]] \
  || ((MEETING_RELAY_PORT < 1 || MEETING_RELAY_PORT > 65535)); then
  echo "BUZZ_TEST_RELAY_PORT must be an integer between 1 and 65535." >&2
  exit 1
fi
# secp256k1 generator x-coordinate; startup only needs a stable deployment
# owner while the revocation E2E seeds its own actor identities.
MEETING_RELAY_OWNER_PUBKEY="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
MEETING_RELAY_PRIVATE_KEY="0000000000000000000000000000000000000000000000000000000000000001"

cd "${REPO_ROOT}"

if curl --silent --fail --max-time 1 \
  "http://127.0.0.1:${MEETING_RELAY_PORT}/_readiness" >/dev/null 2>&1; then
  echo "A Relay is already listening on port ${MEETING_RELAY_PORT}; stop it or set BUZZ_TEST_RELAY_PORT before running this gate." >&2
  exit 1
fi

# This gate owns a local Docker/Relay topology. Do not inherit developer or CI
# endpoints: doing so could send signed test events to an unrelated service.
# Explicit BUZZ_TEST_* overrides exist only for an equally isolated topology.
export BUZZ_TEST_RELAY_PORT="${MEETING_RELAY_PORT}"
export REDIS_URL="${BUZZ_TEST_REDIS_URL:-redis://localhost:6379}"
export RELAY_URL="ws://localhost:${MEETING_RELAY_PORT}"
export BUZZ_MEETING_V1_CREATE_ENABLED=true
export BUZZ_MEETING_V2_CREATE_ENABLED=true
export BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=true
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
  rm -f \
    "${ROLLOUT_FIXTURE_FILE}" \
    "${RELAY_LOG_FILE}" \
    "${ACP_CAPABILITY_FILE}"
}

trap cleanup EXIT

echo "Running infrastructure-free ACP and Relay Meeting contracts..."
cargo test -p buzz-acp --lib -- --nocapture
cargo test -p buzz-acp --features meeting-acceptance --lib acceptance -- --nocapture
cargo test -p buzz-relay --lib meeting -- --nocapture
./scripts/meeting-v2-qualification-gates-test.sh

echo "Verifying the production ACP artifact declares complete Meeting V2 roles..."
cargo run --quiet -p buzz-acp --bin buzz-acp -- capabilities --json >"${ACP_CAPABILITY_FILE}"
jq -e '
  .meeting.protocols[]
  | select(.schemaVersion == "3" and .policy == "moderated-board-v1")
  | (.roles == ["participant", "moderator"])
    and (.turns | index("board_maintenance") != null)
    and (.turns | index("floor_decision") != null)
    and (.currentBoard == "authoritative_read_before_each_semantic_turn")
' "${ACP_CAPABILITY_FILE}" >/dev/null
jq -e '
  (.meeting.capabilities | index("meeting-v2-action-finalization-v2") != null)
  and any(
    .meeting.protocols[];
    .schemaVersion == "3"
      and .policy == "moderated-board-actions-v2"
      and .capability == "meeting-v2-action-finalization-v2"
      and (.turns | index("action_finalization") != null)
      and .moderatorContinuity == "exact_agent_slot_and_acp_session"
  )
' "${ACP_CAPABILITY_FILE}" >/dev/null

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

echo "Running fresh, upgrade, and concurrent migration contracts..."
cargo test -p buzz-db --lib migration::tests -- --ignored --test-threads=1 --nocapture

echo "Checking Meeting migration/schema desired-state drift..."
PGHOST=localhost \
PGPORT=5432 \
PGUSER=buzz \
PGPASSWORD=buzz_dev \
PGDATABASE="${MEETING_CONTRACT_DB}" \
  ./scripts/meeting-v2-schema-drift.sh

echo "Building the real agent-facing CLI for Meeting V2 lifecycle coverage..."
cargo build -p buzz-cli
export MEETING_E2E_BUZZ_BIN="${REPO_ROOT}/target/debug/buzz"

export PGDATABASE="${MEETING_RELAY_DB}"
export DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/${MEETING_RELAY_DB}"
export BUZZ_TEST_PGDATABASE="${MEETING_RELAY_DB}"
export BUZZ_TEST_DATABASE_URL="${DATABASE_URL}"

echo "Starting Relay with Meeting V1/V2 creation enabled..."
BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  "${SCRIPT_DIR}/start-relay-for-tests.sh"

echo "Verifying Relay Meeting V2 runtime and Create capabilities..."
curl --silent --show-error --fail \
  -H 'Accept: application/nostr+json' \
  -H "Host: localhost:${MEETING_RELAY_PORT}" \
  "http://127.0.0.1:${MEETING_RELAY_PORT}/" \
  | jq -e '
      (.supported_extensions | index("buzz-meeting-v2") != null)
      and (.supported_extensions | index("buzz-meeting-v2-create") != null)
      and (.supported_extensions | index("buzz-meeting-v2-direct-actions") != null)
      and (.supported_extensions | index("buzz-meeting-v2-direct-actions-create") != null)
    ' >/dev/null
curl --silent --show-error --fail \
  "http://127.0.0.1:${MEETING_RELAY_PORT}/_readiness" \
  | jq -e '.status == "ready" and .meeting_v2 == true' >/dev/null

echo "Running Meeting V0/V1/V2 Relay end-to-end tests serially..."
cargo test -p buzz-test-client --test e2e_meeting -- --ignored --test-threads=1 --nocapture
cargo test -p buzz-test-client --test e2e_meeting_floor -- --ignored --test-threads=1 --nocapture
cargo test -p buzz-test-client --test e2e_meeting_v2_stage1 -- \
  --ignored \
  --test-threads=1 \
  --nocapture
cargo test -p buzz-test-client --test e2e_meeting_baton -- \
  --ignored \
  --test-threads=1 \
  --skip relay_member_removal_disconnects_live_meeting_reader_and_blocks_reentry \
  --nocapture

echo "Creating Meeting V1/V2 fixtures before closing the rollout gates..."
cargo test -p buzz-test-client --test e2e_meeting_rollout \
  create_rollout_fixture_before_gate_closes -- \
  --ignored \
  --test-threads=1 \
  --nocapture

echo "Restarting Relay with Meeting V1/V2 creation disabled..."
stop_relay
BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  BUZZ_MEETING_V1_CREATE_ENABLED=false \
  BUZZ_MEETING_V2_CREATE_ENABLED=false \
  BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=false \
  "${SCRIPT_DIR}/start-relay-for-tests.sh" --no-build --no-schema

echo "Verifying closed Create retains the Meeting V2 drain capability..."
curl --silent --show-error --fail \
  -H 'Accept: application/nostr+json' \
  -H "Host: localhost:${MEETING_RELAY_PORT}" \
  "http://127.0.0.1:${MEETING_RELAY_PORT}/" \
  | jq -e '
      (.supported_extensions | index("buzz-meeting-v2") != null)
      and (.supported_extensions | index("buzz-meeting-v2-create") == null)
      and (.supported_extensions | index("buzz-meeting-v2-direct-actions") != null)
      and (.supported_extensions | index("buzz-meeting-v2-direct-actions-create") == null)
    ' >/dev/null
curl --silent --show-error --fail \
  "http://127.0.0.1:${MEETING_RELAY_PORT}/_readiness" \
  | jq -e '.status == "ready" and .meeting_v2 == true' >/dev/null

cargo test -p buzz-test-client --test e2e_meeting_rollout \
  existing_v1_and_v2_survive_closed_gates_and_v0_still_works -- \
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

# Keep the direct-action scenario last so its terminal Meeting fixtures cannot
# affect the V0/V1/V2 compatibility and membership-revocation scenarios above.
echo "Restarting Relay for the isolated Meeting action-finalization scenario..."
stop_relay
BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  BUZZ_MEETING_V1_CREATE_ENABLED=true \
  BUZZ_MEETING_V2_CREATE_ENABLED=true \
  BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=true \
  "${SCRIPT_DIR}/start-relay-for-tests.sh" --no-build --no-schema

cargo test -p buzz-test-client --test e2e_meeting_v2_actions -- \
  --ignored \
  --test-threads=1 \
  --nocapture

echo "Meeting backend gate passed."
