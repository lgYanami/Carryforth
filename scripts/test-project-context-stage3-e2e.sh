#!/usr/bin/env bash
# Exercise Project Context Stage 3 against a direct Project View v3 Community:
# controlled bootstrap, real Relay writes, private reads/fan-out, disable
# semantics, managed authority, and final canonical-state preservation.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

export CARGO_INCREMENTAL=0

docker compose up -d postgres redis minio minio-init >/dev/null
for container in buzz-postgres buzz-redis buzz-minio; do
  status=""
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' "${container}" 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  if [[ "${status}" != "healthy" ]]; then
    docker logs "${container}" || true
    echo "Project Context Stage 3 E2E: ${container} did not become healthy" >&2
    exit 1
  fi
done

database_name="buzz_pc_e2e_$$_${RANDOM}"
if [[ ! "${database_name}" =~ ^buzz_pc_e2e_[0-9_]+$ ]]; then
  echo "Refusing unsafe scratch database name: ${database_name}" >&2
  exit 1
fi

profile="${PROJECT_CONTEXT_E2E_PROFILE:-dev}"
if [[ "${profile}" == "dev" ]]; then
  bin_dir="${REPO_ROOT}/target/debug"
else
  bin_dir="${REPO_ROOT}/target/${profile}"
fi

port_is_in_use() {
  local candidate="$1"
  if command -v ss >/dev/null 2>&1; then
    [[ -n "$(ss -H -tan "sport = :${candidate}" 2>/dev/null)" ]]
  elif command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"${candidate}" >/dev/null 2>&1
  else
    nc -z 127.0.0.1 "${candidate}" >/dev/null 2>&1
  fi
}

if [[ -n "${PROJECT_CONTEXT_E2E_PORT:-}" ]]; then
  port="${PROJECT_CONTEXT_E2E_PORT}"
else
  # Relay opens its database and Redis connections before binding its main
  # listener. Keep the test triplet below the host's ephemeral range so one of
  # those outbound connections cannot claim the selected port during startup.
  ephemeral_port_low=32768
  if [[ -r /proc/sys/net/ipv4/ip_local_port_range ]]; then
    read -r ephemeral_port_low _ </proc/sys/net/ipv4/ip_local_port_range
  elif command -v sysctl >/dev/null 2>&1; then
    detected_ephemeral_port_low="$(sysctl -n net.inet.ip.portrange.first 2>/dev/null || true)"
    if [[ "${detected_ephemeral_port_low}" =~ ^[0-9]+$ ]]; then
      ephemeral_port_low="${detected_ephemeral_port_low}"
    fi
  fi
  candidate_port_min=10000
  candidate_port_max="$((ephemeral_port_low - 3))"
  if (( candidate_port_max > 29997 )); then
    candidate_port_max=29997
  fi
  if (( candidate_port_max < candidate_port_min )); then
    candidate_port_min=1024
  fi
  if (( candidate_port_max < candidate_port_min )); then
    echo "Project Context Stage 3 E2E: no safe non-ephemeral port range found" >&2
    exit 1
  fi

  port=""
  for _ in $(seq 1 50); do
    candidate_port="$((candidate_port_min + RANDOM % (candidate_port_max - candidate_port_min + 1)))"
    if ! port_is_in_use "${candidate_port}" \
      && ! port_is_in_use "$((candidate_port + 1))" \
      && ! port_is_in_use "$((candidate_port + 2))"; then
      port="${candidate_port}"
      break
    fi
  done
  if [[ -z "${port}" ]]; then
    echo "Project Context Stage 3 E2E: no free Relay port triplet found" >&2
    exit 1
  fi
fi
if [[ ! "${port}" =~ ^[0-9]+$ ]] || (( port < 1024 || port > 65533 )); then
  echo "Project Context Stage 3 E2E: invalid base port ${port}" >&2
  exit 1
fi
if port_is_in_use "${port}" \
  || port_is_in_use "$((port + 1))" \
  || port_is_in_use "$((port + 2))"; then
  echo "Project Context Stage 3 E2E: Relay port triplet ${port}-$((port + 2)) is unavailable" >&2
  exit 1
fi
health_port="$((port + 1))"
metrics_port="$((port + 2))"
test_host="localhost:${port}"
community_id="00000000-0000-4000-8000-00000000c003"
relay_pid=""
relay_log="$(mktemp)"
command_file="$(mktemp)"
temporary_files=("${relay_log}" "${command_file}")

cleanup() {
  cleanup_status=$?
  if [[ "${cleanup_status}" != "0" && -s "${relay_log}" ]]; then
    echo "Project Context Stage 3 E2E Relay log (failure tail):" >&2
    tail -200 "${relay_log}" >&2 || true
  fi
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  rm -f "${temporary_files[@]}"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
}
trap cleanup EXIT

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null

export PGHOST=localhost
export PGPORT=5432
export PGUSER=buzz
export PGPASSWORD=buzz_dev
export PGDATABASE="${database_name}"
export PGSCHEMA_PLAN_HOST=localhost
export PGSCHEMA_PLAN_PORT=5432
export PGSCHEMA_PLAN_DB=postgres
export PGSCHEMA_PLAN_USER=buzz
export PGSCHEMA_PLAN_PASSWORD=buzz_dev

./bin/pgschema apply --file schema/schema.sql --auto-approve >/dev/null
docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  < scripts/attach-schema-partitions.sql

relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
relay_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
member_private_key=0000000000000000000000000000000000000000000000000000000000000002
member_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
writer_private_key=0000000000000000000000000000000000000000000000000000000000000003
writer_pubkey=f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
outsider_private_key=0000000000000000000000000000000000000000000000000000000000000004
agent_private_key=0000000000000000000000000000000000000000000000000000000000000005

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host)
      VALUES ('${community_id}', '${test_host}');
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES
        ('${community_id}', '${member_pubkey}', 'owner'),
        ('${community_id}', '${writer_pubkey}', 'member');
      INSERT INTO users (community_id, pubkey, agent_owner_pubkey)
      VALUES ('${community_id}', decode('${member_pubkey}', 'hex'), NULL);" >/dev/null

if [[ "${PROJECT_CONTEXT_E2E_NO_BUILD:-0}" != "1" ]]; then
  if [[ "${profile}" == "dev" ]]; then
    cargo build -p buzz-relay -p buzz-cli -p buzz-admin
  else
    cargo build --profile "${profile}" -p buzz-relay -p buzz-cli -p buzz-admin
  fi
fi
for binary in buzz-relay buzz buzz-admin; do
  if [[ ! -x "${bin_dir}/${binary}" ]]; then
    echo "Project Context Stage 3 E2E: missing executable ${bin_dir}/${binary}" >&2
    exit 1
  fi
done

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
relay_url="ws://${test_host}"

start_relay() {
  : >"${relay_log}"
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    RELAY_URL="${relay_url}" \
    BUZZ_BIND_ADDR="0.0.0.0:${port}" \
    BUZZ_HEALTH_PORT="${health_port}" \
    BUZZ_METRICS_PORT="${metrics_port}" \
    BUZZ_AUTO_MIGRATE=false \
    BUZZ_REQUIRE_AUTH_TOKEN=false \
    BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    RELAY_OWNER_PUBKEY="${member_pubkey}" \
    RELAY_OPERATOR_API_ORIGIN="http://127.0.0.1:${port}" \
    RELAY_OPERATOR_PUBKEYS="${member_pubkey}" \
    "${bin_dir}/buzz-relay" >"${relay_log}" 2>&1 &
  relay_pid=$!

  local status_code=""
  for _ in $(seq 1 60); do
    if ! kill -0 "${relay_pid}" 2>/dev/null; then
      cat "${relay_log}" >&2
      echo "Project Context Stage 3 E2E: Relay exited before readiness" >&2
      exit 1
    fi
    status_code="$(curl --noproxy '*' -s -o /dev/null -w '%{http_code}' \
      "http://127.0.0.1:${port}/_readiness" || true)"
    [[ "${status_code}" == "200" ]] && break
    sleep 1
  done
  if [[ "${status_code}" != "200" ]]; then
    cat "${relay_log}" >&2
    echo "Project Context Stage 3 E2E: Relay did not become ready" >&2
    exit 1
  fi
}

stop_relay() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  relay_pid=""
}

run_e2e_binary() {
  local test_binary="$1"
  if [[ -n "${PROJECT_CONTEXT_TEST_ARCHIVE:-}" ]]; then
    cargo nextest run \
      --archive-file "${PROJECT_CONTEXT_TEST_ARCHIVE}" \
      -E "binary(${test_binary})" \
      --run-ignored ignored-only \
      --test-threads 1
  elif command -v cargo-nextest >/dev/null 2>&1; then
    cargo nextest run \
      -p buzz-test-client \
      --test "${test_binary}" \
      --run-ignored ignored-only \
      --test-threads 1
  else
    cargo test -p buzz-test-client --test "${test_binary}" -- \
      --ignored \
      --nocapture \
      --test-threads=1
  fi
}

buzz_as_member() {
  env \
    BUZZ_RELAY_URL="http://${test_host}" \
    BUZZ_PRIVATE_KEY="${member_private_key}" \
    "${bin_dir}/buzz" "$@"
}

project_view_admin() {
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    BUZZ_PRIVATE_KEY="${member_private_key}" \
    "${bin_dir}/buzz-admin" project-view "$@"
}

project_document_admin() {
  env \
    DATABASE_URL="${database_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    "${bin_dir}/buzz-admin" project-document "$@"
}

project_context_admin() {
  env \
    DATABASE_URL="${database_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    "${bin_dir}/buzz-admin" project-context "$@"
}

export DATABASE_URL="${database_url}"
export REDIS_URL=redis://localhost:6379
export PROJECT_CONTEXT_E2E_RELAY_URL="${relay_url}"
export PROJECT_CONTEXT_E2E_MEMBER_PRIVATE_KEY="${member_private_key}"
export PROJECT_CONTEXT_E2E_WRITER_PRIVATE_KEY="${writer_private_key}"
export PROJECT_CONTEXT_E2E_OUTSIDER_PRIVATE_KEY="${outsider_private_key}"
export PROJECT_CONTEXT_E2E_AGENT_PRIVATE_KEY="${agent_private_key}"
export PROJECT_CONTEXT_E2E_RELAY_PRIVATE_KEY="${relay_private_key}"

initial_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].project_view_schema_version == 1
  and .[0].context_revision == null
  and .[0].edge_row_count == 0
  and .[0].binding_row_count == 0
' <<<"${initial_status}" >/dev/null

# Prepare and commit a real direct-v3 Project View initialization. Context
# remains disabled until its own signed empty catalog is present and verified.
prepare_v3="$(project_view_admin prepare-v3 \
  --community "${test_host}" \
  --idempotency-key "project-context-stage3-${database_name}" \
  --operator-pubkey "${member_pubkey}")"
preparation_operation_id="$(jq -er '.operation_id' <<<"${prepare_v3}")"
jq -n \
  --arg operation "${preparation_operation_id}" \
  --arg owner "${member_pubkey}" '{
    schema_version: 3,
    expected_project_revision: 0,
    request: {
      type: "initialize",
      preparation_operation_id: $operation,
      profile: {
        name: "Project Context Stage 3",
        positioning: "A direct Project View v3 integration fixture",
        purpose: "Exercise atomic Project Context Edge commands",
        problem: "Cross-coordinate knowledge needs explicit Context Documents",
        scope: "One isolated backend Community"
      },
      goals: [{
        id: "10000000-0000-4000-8000-00000000c003",
        title: "Deliver Project Context Stage 3",
        desired_outcome: "Verified private atomic Edge writes",
        directions: ["Keep Context owned by acting Humans and Agents"]
      }],
      initial_roles: [{
        role_id: "20000000-0000-4000-8000-00000000c003",
        name: "Community owner",
        purpose: "Own initial Project governance",
        responsibilities: ["Administer the Project"],
        boundaries: ["Human governance only"],
        level: "admin",
        active: true,
        context_references: []
      }],
      initial_governance_assignments: [{
        member_pubkey: $owner,
        role_id: "20000000-0000-4000-8000-00000000c003",
        proposal_id: "30000000-0000-4000-8000-00000000c003",
        assignment_id: "40000000-0000-4000-8000-00000000c003"
      }]
    }
  }' >"${command_file}"

start_relay
init_v3="$(buzz_as_member --format compact project-view init-v3 --command "${command_file}")"
jq -e '.accepted == true' <<<"${init_v3}" >/dev/null
stop_relay
project_view_admin enable --community "${test_host}" >/dev/null

project_document_admin bootstrap \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
project_document_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
project_document_admin enable \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null

bootstrap="$(project_context_admin bootstrap \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}")"
jq -e '.bootstrapped == true and .replayed == false and .projection_generation == 1' \
  <<<"${bootstrap}" >/dev/null
bootstrap_replay="$(project_context_admin bootstrap \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}")"
jq -e '.bootstrapped == true and .replayed == true and .projection_generation == 1' \
  <<<"${bootstrap_replay}" >/dev/null
project_context_admin preflight \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
project_context_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
project_context_admin enable \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null

ready_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == true
  and .[0].context_revision == 0
  and .[0].active_edge_count == 0
  and .[0].bound_document_count == 0
  and .[0].projection_generation == 1
  and .[0].projection_parity == true
  and .[0].structural_read_ready == true
  and .[0].advertised_ready == true
  and .[0].orphan_projection_count == 0
  and .[0].pointer_mismatch_count == 0
' <<<"${ready_status}" >/dev/null

start_relay
run_e2e_binary e2e_project_context_stage3
stop_relay

project_context_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
final_enabled_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == true
  and .[0].context_revision == 5
  and .[0].active_edge_count == 1
  and .[0].bound_document_count == 1
  and .[0].edge_row_count == 1
  and .[0].binding_row_count == 3
  and .[0].change_count == 5
  and .[0].projection_parity == true
  and .[0].advertised_ready == true
' <<<"${final_enabled_status}" >/dev/null

canonical_rows_before_disable="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM project_context_edges WHERE community_id = '${community_id}';
   SELECT count(*) FROM project_context_document_bindings WHERE community_id = '${community_id}';
   SELECT count(*) FROM project_context_edge_changes WHERE community_id = '${community_id}';")"
project_context_admin disable --community "${test_host}" >/dev/null
canonical_rows_after_disable="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM project_context_edges WHERE community_id = '${community_id}';
   SELECT count(*) FROM project_context_document_bindings WHERE community_id = '${community_id}';
   SELECT count(*) FROM project_context_edge_changes WHERE community_id = '${community_id}';")"
[[ "${canonical_rows_before_disable}" == "${canonical_rows_after_disable}" ]]

project_context_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
disabled_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].context_revision == 5
  and .[0].structural_read_ready == true
  and .[0].advertised_ready == false
  and .[0].projection_parity == true
' <<<"${disabled_status}" >/dev/null

control_audits="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM audit_log
   WHERE community_id = '${community_id}'
     AND action = 'project_context_edge_control'")"
if (( control_audits < 5 )); then
  echo "Project Context Stage 3 E2E: expected bootstrap/enable/disable audit records" >&2
  exit 1
fi

echo "Project Context Stage 3 Relay, privacy, authority, and operation-gate E2E passed."
