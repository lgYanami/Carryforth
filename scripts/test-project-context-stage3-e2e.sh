#!/usr/bin/env bash
# Exercise Project Context Stages 3 through 5, plus the optional Stage 7
# reprojection and Desktop trusted-read acceptance gate, against a direct
# Project View v3 Community:
# controlled bootstrap, real Relay writes, private reads/fan-out, disable
# semantics, managed authority, cross-domain lifecycle, verified CLI queries,
# metadata-only hydration, and final canonical state preservation.

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

desktop_context_probe() {
  local mode="$1"
  local expected_revision="$2"
  env \
    PROJECT_CONTEXT_DESKTOP_E2E_RELAY_URL="http://${test_host}" \
    PROJECT_CONTEXT_DESKTOP_E2E_PRIVATE_KEY="${member_private_key}" \
    PROJECT_CONTEXT_DESKTOP_E2E_MODE="${mode}" \
    PROJECT_CONTEXT_DESKTOP_E2E_EXPECTED_REVISION="${expected_revision}" \
    cargo test --manifest-path desktop/src-tauri/Cargo.toml \
      real_relay_stage7_matches_cli_and_desktop_trusted_read -- \
      --ignored --nocapture --test-threads=1
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

context_business_fingerprint() {
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc \
    "SELECT md5(jsonb_build_object(
       'state', (SELECT to_jsonb(state) - 'projection_pubkey'
                        - 'projection_generation' - 'meta_projection_event_id'
                 FROM project_context_edge_state state
                 WHERE community_id = '${community_id}'),
       'edges', COALESCE((SELECT jsonb_agg(to_jsonb(edge) ORDER BY edge.edge_key)
                          FROM project_context_edges edge
                          WHERE community_id = '${community_id}'), '[]'::jsonb),
       'coordinates', COALESCE((SELECT jsonb_agg(to_jsonb(coordinate)
                                      ORDER BY coordinate.edge_key, coordinate.ordinal)
                                FROM project_context_edge_coordinates coordinate
                                WHERE community_id = '${community_id}'), '[]'::jsonb),
       'bindings', COALESCE((SELECT jsonb_agg(
                                      to_jsonb(binding) - 'current_projection_event_id'
                                      ORDER BY binding.context_document_id)
                             FROM project_context_document_bindings binding
                             WHERE community_id = '${community_id}'), '[]'::jsonb),
       'changes', COALESCE((SELECT jsonb_agg(to_jsonb(change)
                                   ORDER BY change.context_revision)
                            FROM project_context_edge_changes change
                            WHERE community_id = '${community_id}'), '[]'::jsonb),
       'document_state', (SELECT to_jsonb(state)
                          FROM project_document_state state
                          WHERE community_id = '${community_id}'),
       'documents', COALESCE((SELECT jsonb_agg(to_jsonb(document)
                                     ORDER BY document.document_id)
                              FROM project_documents document
                              WHERE community_id = '${community_id}'), '[]'::jsonb),
       'resource_context_references', COALESCE((SELECT jsonb_agg(to_jsonb(reference)
                                               ORDER BY reference.source_object_id,
                                                        reference.target_resource_id)
                                        FROM project_view_resource_context_references reference
                                        WHERE community_id = '${community_id}'), '[]'::jsonb),
       'document_context_references', COALESCE((SELECT jsonb_agg(to_jsonb(reference)
                                               ORDER BY reference.source_object_id,
                                                        reference.target_document_id,
                                                        reference.reference_mode,
                                                        reference.revision_key)
                                        FROM project_view_document_context_references reference
                                        WHERE community_id = '${community_id}'), '[]'::jsonb)
     )::text);"
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
  and .[0].project_view_schema_version == 3
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
      goals: [
        {
          id: "10000000-0000-4000-8000-00000000c003",
          title: "Deliver Project Context Stage 3",
          desired_outcome: "Verified private atomic Edge writes",
          directions: ["Keep Context owned by acting Humans and Agents"]
        },
        {
          id: "10000000-0000-4000-8000-00000000c004",
          title: "Preserve Project lifecycle invariants",
          desired_outcome: "Keep one active Goal while testing coordinate tombstones",
          directions: ["Do not weaken Project View lifecycle rules"]
        }
      ],
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

# Keep the pre-existing Project View Context Reference capability active in
# the lifecycle fixture. Stage 4 proves that its Live references and Resource
# Guides remain independent from the new Context Edge binding.
project_view_admin context enable \
  --community "${test_host}" \
  --idempotency-key "project-context-stage4-reference-${database_name}" \
  --operator-pubkey "${member_pubkey}" >/dev/null

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

# Stage 5 drives the public Agent CLI only. Build one binary Edge and one
# overlapping hyperedge, then prove exact/incident/contains-all, metadata-first
# output, body-on-demand, independent Document revisions, and tombstone
# hydration without reaching into canonical storage for the behavior itself.
profile_coordinate="project_profile:${community_id}"
goal_coordinate="goal:10000000-0000-4000-8000-00000000c004"
binary_context_document="60000000-0000-4000-8000-00000000c005"
hyper_context_document="60000000-0000-4000-8000-00000000c006"
coordinate_document="70000000-0000-4000-8000-00000000c005"
document_coordinate="document:${coordinate_document}"
binary_body_marker="STAGE5_BINARY_CONTEXT_BODY_MUST_BE_FETCHED_EXPLICITLY"
hyper_body_marker="STAGE5_HYPER_CONTEXT_BODY_MUST_BE_FETCHED_EXPLICITLY"
corrected_hyper_body_marker="STAGE5_CORRECTED_HYPER_BODY_MUST_BE_FETCHED_EXPLICITLY"

create_binary_document="$(buzz_as_member --format compact documents create \
  --document-id "${binary_context_document}" \
  --title "Binary Context" \
  --summary "Explains the profile and lifecycle goal" \
  --content "${binary_body_marker}")"
jq -e --arg id "${binary_context_document}" '
  .accepted == true
  and .document_id == $id
  and .document_revision == 1
' <<<"${create_binary_document}" >/dev/null

create_hyper_document="$(buzz_as_member --format compact documents create \
  --document-id "${hyper_context_document}" \
  --title "Hyper Context" \
  --summary "Explains the profile, goal, and supporting Document" \
  --content "${hyper_body_marker}")"
jq -e --arg id "${hyper_context_document}" '
  .accepted == true
  and .document_id == $id
  and .document_revision == 1
' <<<"${create_hyper_document}" >/dev/null

create_coordinate_document="$(buzz_as_member --format compact documents create \
  --document-id "${coordinate_document}" \
  --title "Supporting Document coordinate" \
  --summary "A coordinate, not the explanatory Context Document" \
  --content "Supporting coordinate body")"
jq -e --arg id "${coordinate_document}" '
  .accepted == true
  and .document_id == $id
  and .document_revision == 1
' <<<"${create_coordinate_document}" >/dev/null

attach_binary="$(buzz_as_member --format compact project-context attach \
  --context-document "${binary_context_document}" \
  --coordinate "${goal_coordinate}" \
  --coordinate "${profile_coordinate}")"
jq -e --arg id "${binary_context_document}" '
  .accepted == true
  and .confirmation == "receipt"
  and .receipt.operation == "attach"
  and .receipt.context_revision == 11
  and .receipt.edge_state == "active"
  and .receipt.edge_document_count == 1
  and .receipt.context_document_id == $id
' <<<"${attach_binary}" >/dev/null

attach_hyper="$(buzz_as_member --format compact project-context attach \
  --context-document "${hyper_context_document}" \
  --coordinate "${document_coordinate}" \
  --coordinate "${profile_coordinate}" \
  --coordinate "${goal_coordinate}")"
jq -e --arg id "${hyper_context_document}" '
  .accepted == true
  and .receipt.operation == "attach"
  and .receipt.context_revision == 12
  and .receipt.edge_state == "active"
  and .receipt.edge_document_count == 1
  and .receipt.context_document_id == $id
' <<<"${attach_hyper}" >/dev/null

exact_binary="$(buzz_as_member --format compact project-context exact \
  --coordinate "${goal_coordinate}" \
  --coordinate "${profile_coordinate}")"
jq -e --arg id "${binary_context_document}" '
  .context_revision == 12
  and .query.query_type == "exact"
  and (.edges | length) == 1
  and (.edges[0].coordinates | length) == 2
  and (.edges[0].context_documents | length) == 1
  and .edges[0].context_documents[0].document_id == $id
  and .edges[0].context_documents[0].document_revision == 1
  and .edges[0].context_documents[0].fetch_command == ("buzz documents get " + $id + " --content-only")
  and (.edges[0].context_documents[0] | has("content_markdown") | not)
' <<<"${exact_binary}" >/dev/null
if grep -Fq "${binary_body_marker}" <<<"${exact_binary}"; then
  echo "Project Context Stage 5 E2E: exact output leaked a Document body" >&2
  exit 1
fi

incident_profile="$(buzz_as_member --format compact project-context incident \
  "${profile_coordinate}")"
jq -e '
  .query.query_type == "incident"
  and (.edges | length) == 2
  and ([.edges[].coordinates | length] | sort) == [2, 3]
' <<<"${incident_profile}" >/dev/null

contains_pair="$(buzz_as_member --format compact project-context contains-all \
  --coordinate "${profile_coordinate}" \
  --coordinate "${goal_coordinate}")"
jq -e '
  .query.query_type == "contains_all"
  and (.edges | length) == 2
  and ([.edges[].coordinates | length] | sort) == [2, 3]
' <<<"${contains_pair}" >/dev/null

contains_everything="$(buzz_as_member --format compact project-context contains-all)"
jq -e '
  .query.query_type == "contains_all"
  and (.query.coordinates | length) == 0
  and (.edges | length) == 2
' <<<"${contains_everything}" >/dev/null

incident_document="$(buzz_as_member --format compact project-context incident \
  "${document_coordinate}")"
jq -e --arg id "${hyper_context_document}" '
  (.edges | length) == 1
  and (.edges[0].coordinates | length) == 3
  and .edges[0].context_documents[0].document_id == $id
' <<<"${incident_document}" >/dev/null

update_hyper="$(buzz_as_member --format compact documents update \
  "${hyper_context_document}" \
  --expected-revision 1 \
  --title "Hyper Context corrected" \
  --summary "Corrected after practical discovery" \
  --content "${corrected_hyper_body_marker}")"
jq -e --arg id "${hyper_context_document}" '
  .accepted == true
  and .document_id == $id
  and .document_revision == 2
' <<<"${update_hyper}" >/dev/null

delete_coordinate="$(buzz_as_member --format compact documents delete \
  "${coordinate_document}" --expected-revision 1)"
jq -e --arg id "${coordinate_document}" '
  .accepted == true
  and .document_id == $id
  and .document_revision == 2
' <<<"${delete_coordinate}" >/dev/null

tombstoned_hyper="$(buzz_as_member --format compact project-context incident \
  "${document_coordinate}")"
jq -e --arg coordinate_id "${coordinate_document}" --arg context_id "${hyper_context_document}" '
  .context_revision == 12
  and (.edges | length) == 1
  and any(.edges[0].coordinates[];
    .coordinate.coordinate_type == "document"
    and .coordinate.document_id == $coordinate_id
    and .state == "tombstoned"
    and .document_revision == 2)
  and .edges[0].context_documents[0].document_id == $context_id
  and .edges[0].context_documents[0].title == "Hyper Context corrected"
  and .edges[0].context_documents[0].document_revision == 2
  and (.edges[0].context_documents[0] | has("content_markdown") | not)
' <<<"${tombstoned_hyper}" >/dev/null
if grep -Fq "${corrected_hyper_body_marker}" <<<"${tombstoned_hyper}"; then
  echo "Project Context Stage 5 E2E: hydrated Edge output leaked a Document body" >&2
  exit 1
fi
fetched_hyper_body="$(buzz_as_member documents get \
  "${hyper_context_document}" --content-only)"
[[ "${fetched_hyper_body}" == "${corrected_hyper_body_marker}" ]]

set +e
protected_delete="$(buzz_as_member --format compact documents delete \
  "${binary_context_document}" --expected-revision 1 2>&1)"
protected_delete_status=$?
set -e
[[ "${protected_delete_status}" == "5" ]]
grep -Fq "conflict:project_document:still_referenced" <<<"${protected_delete}"

detach_hyper="$(buzz_as_member --format compact project-context detach \
  --context-document "${hyper_context_document}" \
  --coordinate "${profile_coordinate}" \
  --coordinate "${goal_coordinate}" \
  --coordinate "${document_coordinate}")"
jq -e '
  .accepted == true
  and .receipt.operation == "detach"
  and .receipt.context_revision == 13
  and .receipt.edge_state == "deleted"
  and .receipt.edge_document_count == 0
' <<<"${detach_hyper}" >/dev/null

post_detach_all="$(buzz_as_member --format compact project-context contains-all)"
jq -e '
  .context_revision == 13
  and (.edges | length) == 1
  and (.edges[0].coordinates | length) == 2
' <<<"${post_detach_all}" >/dev/null

stop_relay

project_context_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
final_enabled_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == true
  and .[0].context_revision == 13
  and .[0].active_edge_count == 1
  and .[0].bound_document_count == 1
  and .[0].edge_row_count == 5
  and .[0].binding_row_count == 7
  and .[0].change_count == 13
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
  and .[0].context_revision == 13
  and .[0].active_edge_count == 1
  and .[0].bound_document_count == 1
  and .[0].structural_read_ready == true
  and .[0].advertised_ready == false
  and .[0].projection_parity == true
' <<<"${disabled_status}" >/dev/null

# Capability-off removes discovery from NIP-11 but not verified structural
# reads or cleanup. The CLI must therefore read and detach the retained Edge,
# while refusing a new attach before submission.
start_relay
disabled_exact="$(buzz_as_member --format compact project-context exact \
  --coordinate "${profile_coordinate}" \
  --coordinate "${goal_coordinate}")"
jq -e --arg id "${binary_context_document}" '
  .context_revision == 13
  and (.edges | length) == 1
  and .edges[0].context_documents[0].document_id == $id
' <<<"${disabled_exact}" >/dev/null

set +e
disabled_attach="$(buzz_as_member --format compact project-context attach \
  --context-document "${hyper_context_document}" \
  --coordinate "${profile_coordinate}" \
  --coordinate "${goal_coordinate}" 2>&1)"
disabled_attach_status=$?
set -e
[[ "${disabled_attach_status}" == "4" ]]
grep -Fq "unavailable:project_context:capability_disabled" <<<"${disabled_attach}"

disabled_detach="$(buzz_as_member --format compact project-context detach \
  --context-document "${binary_context_document}" \
  --coordinate "${goal_coordinate}" \
  --coordinate "${profile_coordinate}")"
jq -e '
  .accepted == true
  and .receipt.operation == "detach"
  and .receipt.context_revision == 14
  and .receipt.edge_state == "deleted"
  and .receipt.edge_document_count == 0
' <<<"${disabled_detach}" >/dev/null

disabled_empty="$(buzz_as_member --format compact project-context contains-all)"
jq -e '
  .context_revision == 14
  and (.edges | length) == 0
' <<<"${disabled_empty}" >/dev/null
stop_relay

project_context_admin verify \
  --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
final_disabled_status="$(project_context_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].context_revision == 14
  and .[0].active_edge_count == 0
  and .[0].bound_document_count == 0
  and .[0].edge_row_count == 5
  and .[0].binding_row_count == 7
  and .[0].change_count == 14
  and .[0].structural_read_ready == true
  and .[0].advertised_ready == false
  and .[0].projection_parity == true
  and .[0].integrity_ready == true
' <<<"${final_disabled_status}" >/dev/null

if [[ "${PROJECT_CONTEXT_E2E_STAGE7:-0}" == "1" ]]; then
  # Rebuild every retained deleted binding head plus reset metadata at a new
  # generation. This real admin path complements the DB integration fixture,
  # which rotates to a distinct signer, while keeping this direct-v3 system's
  # already healthy Project View and Project Document signer unchanged.
  business_before_reproject="$(context_business_fingerprint)"
  reproject="$(project_context_admin reproject \
    --community "${test_host}" --expected-pubkey "${relay_pubkey}")"
  jq -e --arg signer "${relay_pubkey}" '
    .reprojected == true
    and .enabled == false
    and .source_projection_generation == 1
    and .projection_generation == 2
    and .source_projection_pubkey == $signer
    and .projection_pubkey == $signer
    and .context_revision == 14
    and .binding_count == 7
    and .business_state_preserved == true
    and .projection_parity == true
    and .integrity_ready == true
    and .orphan_projection_count == 0
    and .pointer_mismatch_count == 0
  ' <<<"${reproject}" >/dev/null
  business_after_reproject="$(context_business_fingerprint)"
  [[ "${business_before_reproject}" == "${business_after_reproject}" ]]

  replacement_projection_state="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc \
    "SELECT
       count(*) FILTER (WHERE kind = 40908),
       count(*) FILTER (WHERE kind = 40909),
       bool_and(pubkey = decode('${relay_pubkey}', 'hex')),
       bool_and((content::jsonb->>'projection_generation')::bigint = 2),
       bool_and(CASE WHEN kind = 40909 THEN
         (content::jsonb->>'reset')::boolean
         AND jsonb_array_length(content::jsonb->'changed_bindings') = 0
         AND NOT content::jsonb ? 'source_event_id'
       ELSE true END)
     FROM events
     WHERE community_id = '${community_id}'
       AND kind IN (40908, 40909) AND deleted_at IS NULL;")"
  [[ "${replacement_projection_state}" == "7|1|t|t|t" ]]

  recovered_status="$(project_context_admin status --community "${test_host}")"
  jq -e '
    length == 1
    and .[0].enabled == false
    and .[0].context_revision == 14
    and .[0].projection_generation == 2
    and .[0].projection_parity == true
    and .[0].integrity_ready == true
    and .[0].structural_read_ready == true
    and .[0].advertised_ready == false
    and .[0].reproject_required == false
  ' <<<"${recovered_status}" >/dev/null
  project_context_admin enable \
    --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
  start_relay
  recovered_empty="$(buzz_as_member --format compact project-context contains-all)"
  jq -e '
    .context_revision == 14
    and .projection_generation == 2
    and (.edges | length) == 0
  ' <<<"${recovered_empty}" >/dev/null

  # Build a deterministic read-only Desktop acceptance fixture. CLI remains
  # the only writer: two disjoint Edges first, then one bridging Edge. Desktop
  # reads the same signed revision through its native trusted boundary.
  stage7_context_a="80000000-0000-4000-8000-00000000c701"
  stage7_context_b="80000000-0000-4000-8000-00000000c702"
  stage7_context_bridge="80000000-0000-4000-8000-00000000c703"
  stage7_coordinate_document="80000000-0000-4000-8000-00000000c704"
  stage7_document_coordinate="document:${stage7_coordinate_document}"
  stage7_role_coordinate="role:20000000-0000-4000-8000-00000000c003"
  stage7_body_a="STAGE7_CONTEXT_BODY_A_MUST_STAY_OUT_OF_GRAPH_RESULTS"
  stage7_body_a_corrected="STAGE7_CONTEXT_BODY_A_CORRECTED_MUST_BE_LAZY"

  create_stage7_a="$(buzz_as_member --format compact documents create \
    --document-id "${stage7_context_a}" \
    --title "Stage 7 Context A" \
    --summary "Explains the profile and lifecycle goal island" \
    --content "${stage7_body_a}")"
  jq -e --arg id "${stage7_context_a}" '
    .accepted == true and .document_id == $id and .document_revision == 1
  ' <<<"${create_stage7_a}" >/dev/null

  create_stage7_b="$(buzz_as_member --format compact documents create \
    --document-id "${stage7_context_b}" \
    --title "Stage 7 Context B" \
    --summary "Explains the role and Document island" \
    --content "STAGE7_CONTEXT_BODY_B_MUST_STAY_LAZY")"
  jq -e --arg id "${stage7_context_b}" '
    .accepted == true and .document_id == $id and .document_revision == 1
  ' <<<"${create_stage7_b}" >/dev/null

  create_stage7_bridge="$(buzz_as_member --format compact documents create \
    --document-id "${stage7_context_bridge}" \
    --title "Stage 7 bridge Context" \
    --summary "Connects the two Context islands" \
    --content "STAGE7_BRIDGE_BODY_MUST_STAY_LAZY")"
  jq -e --arg id "${stage7_context_bridge}" '
    .accepted == true and .document_id == $id and .document_revision == 1
  ' <<<"${create_stage7_bridge}" >/dev/null

  create_stage7_coordinate="$(buzz_as_member --format compact documents create \
    --document-id "${stage7_coordinate_document}" \
    --title "Stage 7 Coordinate Document" \
    --summary "Acts as a graph Coordinate, not a Context binding" \
    --content "Stage 7 Coordinate body")"
  jq -e --arg id "${stage7_coordinate_document}" '
    .accepted == true and .document_id == $id and .document_revision == 1
  ' <<<"${create_stage7_coordinate}" >/dev/null

  stage7_attach_a="$(buzz_as_member --format compact project-context attach \
    --context-document "${stage7_context_a}" \
    --coordinate "${goal_coordinate}" \
    --coordinate "${profile_coordinate}")"
  jq -e '
    .accepted == true
    and .receipt.context_revision == 15
    and .receipt.edge_state == "active"
  ' <<<"${stage7_attach_a}" >/dev/null

  stage7_attach_b="$(buzz_as_member --format compact project-context attach \
    --context-document "${stage7_context_b}" \
    --coordinate "${stage7_role_coordinate}" \
    --coordinate "${stage7_document_coordinate}")"
  jq -e '
    .accepted == true
    and .receipt.context_revision == 16
    and .receipt.edge_state == "active"
  ' <<<"${stage7_attach_b}" >/dev/null

  stage7_all_split="$(buzz_as_member --format compact project-context contains-all)"
  jq -e --arg a "${stage7_context_a}" --arg b "${stage7_context_b}" '
    .context_revision == 16
    and .projection_generation == 2
    and (.edges | length) == 2
    and ([.edges[].context_documents[].document_id] | sort) == ([$a, $b] | sort)
  ' <<<"${stage7_all_split}" >/dev/null
  if grep -Fq "STAGE7_CONTEXT_BODY" <<<"${stage7_all_split}"; then
    echo "Project Context Stage 7 E2E: All leaked a Context Document body" >&2
    exit 1
  fi

  stage7_exact="$(buzz_as_member --format compact project-context exact \
    --coordinate "${profile_coordinate}" \
    --coordinate "${goal_coordinate}")"
  jq -e --arg id "${stage7_context_a}" '
    .context_revision == 16
    and .query.query_type == "exact"
    and (.edges | length) == 1
    and .edges[0].context_documents[0].document_id == $id
  ' <<<"${stage7_exact}" >/dev/null

  stage7_incident="$(buzz_as_member --format compact project-context incident \
    "${goal_coordinate}")"
  jq -e --arg id "${stage7_context_a}" '
    .context_revision == 16
    and .query.query_type == "incident"
    and (.edges | length) == 1
    and .edges[0].context_documents[0].document_id == $id
  ' <<<"${stage7_incident}" >/dev/null

  stage7_contains="$(buzz_as_member --format compact project-context contains-all \
    --coordinate "${stage7_role_coordinate}")"
  jq -e --arg id "${stage7_context_b}" '
    .context_revision == 16
    and .query.query_type == "contains_all"
    and (.edges | length) == 1
    and .edges[0].context_documents[0].document_id == $id
  ' <<<"${stage7_contains}" >/dev/null
  desktop_context_probe split 16

  stage7_attach_bridge="$(buzz_as_member --format compact project-context attach \
    --context-document "${stage7_context_bridge}" \
    --coordinate "${goal_coordinate}" \
    --coordinate "${stage7_role_coordinate}")"
  jq -e '
    .accepted == true
    and .receipt.context_revision == 17
    and .receipt.edge_state == "active"
  ' <<<"${stage7_attach_bridge}" >/dev/null
  stage7_all_merged="$(buzz_as_member --format compact project-context contains-all)"
  jq -e '
    .context_revision == 17
    and (.edges | length) == 3
  ' <<<"${stage7_all_merged}" >/dev/null
  desktop_context_probe merged 17

  stage7_update_a="$(buzz_as_member --format compact documents update \
    "${stage7_context_a}" \
    --expected-revision 1 \
    --title "Stage 7 Context A corrected" \
    --summary "Corrected without changing Edge membership" \
    --content "${stage7_body_a_corrected}")"
  jq -e --arg id "${stage7_context_a}" '
    .accepted == true and .document_id == $id and .document_revision == 2
  ' <<<"${stage7_update_a}" >/dev/null
  stage7_after_update="$(buzz_as_member --format compact project-context exact \
    --coordinate "${profile_coordinate}" \
    --coordinate "${goal_coordinate}")"
  jq -e '
    .context_revision == 17
    and .edges[0].context_documents[0].title == "Stage 7 Context A corrected"
    and .edges[0].context_documents[0].document_revision == 2
  ' <<<"${stage7_after_update}" >/dev/null
  if grep -Fq "${stage7_body_a_corrected}" <<<"${stage7_after_update}"; then
    echo "Project Context Stage 7 E2E: updated query leaked the current body" >&2
    exit 1
  fi
  desktop_context_probe updated 17

  stage7_delete_coordinate="$(buzz_as_member --format compact documents delete \
    "${stage7_coordinate_document}" --expected-revision 1)"
  jq -e --arg id "${stage7_coordinate_document}" '
    .accepted == true and .document_id == $id and .document_revision == 2
  ' <<<"${stage7_delete_coordinate}" >/dev/null
  stage7_tombstoned="$(buzz_as_member --format compact project-context incident \
    "${stage7_document_coordinate}")"
  jq -e --arg id "${stage7_coordinate_document}" '
    .context_revision == 17
    and (.edges | length) == 1
    and any(.edges[0].coordinates[];
      .coordinate.coordinate_type == "document"
      and .coordinate.document_id == $id
      and .state == "tombstoned"
      and .document_revision == 2)
  ' <<<"${stage7_tombstoned}" >/dev/null
  desktop_context_probe tombstoned 17

  set +e
  stage7_protected_delete="$(buzz_as_member --format compact documents delete \
    "${stage7_context_a}" --expected-revision 2 2>&1)"
  stage7_protected_delete_status=$?
  set -e
  [[ "${stage7_protected_delete_status}" == "5" ]]
  grep -Fq "conflict:project_document:still_referenced" <<<"${stage7_protected_delete}"

  stop_relay
  project_context_admin disable --community "${test_host}" >/dev/null
  start_relay
  stage7_capability_off="$(buzz_as_member --format compact project-context contains-all)"
  jq -e '
    .context_revision == 17
    and .projection_generation == 2
    and (.edges | length) == 3
  ' <<<"${stage7_capability_off}" >/dev/null
  desktop_context_probe capability_off 17

  stage7_detach_bridge="$(buzz_as_member --format compact project-context detach \
    --context-document "${stage7_context_bridge}" \
    --coordinate "${goal_coordinate}" \
    --coordinate "${stage7_role_coordinate}")"
  jq -e '.accepted == true and .receipt.context_revision == 18' \
    <<<"${stage7_detach_bridge}" >/dev/null
  stage7_detach_a="$(buzz_as_member --format compact project-context detach \
    --context-document "${stage7_context_a}" \
    --coordinate "${profile_coordinate}" \
    --coordinate "${goal_coordinate}")"
  jq -e '.accepted == true and .receipt.context_revision == 19' \
    <<<"${stage7_detach_a}" >/dev/null
  stage7_detach_b="$(buzz_as_member --format compact project-context detach \
    --context-document "${stage7_context_b}" \
    --coordinate "${stage7_role_coordinate}" \
    --coordinate "${stage7_document_coordinate}")"
  jq -e '.accepted == true and .receipt.context_revision == 20' \
    <<<"${stage7_detach_b}" >/dev/null
  stage7_clean="$(buzz_as_member --format compact project-context contains-all)"
  jq -e '.context_revision == 20 and (.edges | length) == 0' \
    <<<"${stage7_clean}" >/dev/null
  stop_relay
  project_context_admin verify \
    --community "${test_host}" --expected-pubkey "${relay_pubkey}" >/dev/null
  stage7_final_status="$(project_context_admin status --community "${test_host}")"
  jq -e '
    length == 1
    and .[0].enabled == false
    and .[0].context_revision == 20
    and .[0].active_edge_count == 0
    and .[0].bound_document_count == 0
    and .[0].projection_generation == 2
    and .[0].edge_row_count == 7
    and .[0].binding_row_count == 10
    and .[0].change_count == 20
    and .[0].projection_parity == true
    and .[0].integrity_ready == true
  ' <<<"${stage7_final_status}" >/dev/null
fi

control_audits="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM audit_log
   WHERE community_id = '${community_id}'
     AND action = 'project_context_edge_control'")"
if (( control_audits < 5 )); then
  echo "Project Context Stage 3 E2E: expected bootstrap/enable/disable audit records" >&2
  exit 1
fi

if [[ "${PROJECT_CONTEXT_E2E_STAGE7:-0}" == "1" ]]; then
  reproject_audits="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc \
    "SELECT count(*) FROM audit_log
     WHERE community_id = '${community_id}'
       AND action = 'project_context_edge_control'
       AND detail->>'operation' = 'reproject'")"
  [[ "${reproject_audits}" == "1" ]]
  echo "Project Context Stage 7 reprojection, Desktop read parity, lifecycle, and regression E2E passed."
fi

echo "Project Context Stage 3/4/5 Relay, privacy, authority, lifecycle, and Agent CLI E2E passed."
