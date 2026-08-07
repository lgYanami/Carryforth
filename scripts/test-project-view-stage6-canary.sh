#!/usr/bin/env bash
# Run the Stage 6 Context/Role Brief acceptance path against a self-contained
# schema-v3 greenfield scratch Community. No legacy migration fixture or old
# Project View runtime participates in this canary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# shellcheck source=/dev/null
. ./bin/activate-hermit

umask 077
export CARGO_INCREMENTAL=0

fail() {
  echo "Project View Stage 6 canary: $*" >&2
  exit 1
}

for command in cargo curl docker jq node rg sha256sum; do
  command -v "${command}" >/dev/null || fail "missing required command: ${command}"
done

profile="${PROJECT_VIEW_STAGE6_PROFILE:-dev}"
if [[ "${profile}" == "dev" ]]; then
  bin_dir="${REPO_ROOT}/target/debug"
else
  bin_dir="${REPO_ROOT}/target/${profile}"
fi

port="${PROJECT_VIEW_STAGE6_PORT:-$((30000 + ($$ % 4000)))}"
health_port="$((port + 1))"
metrics_port="$((port + 2))"
test_host="localhost:${port}"
community_id="00000000-0000-4000-8000-0000000060c0"
database_name="buzz_pv_stage6_canary_$$_${RANDOM}"
[[ "${database_name}" =~ ^buzz_pv_stage6_canary_[0-9_]+$ ]] \
  || fail "refusing unsafe scratch database name: ${database_name}"
artifact_root="${PROJECT_VIEW_STAGE6_ARTIFACT_ROOT:-${REPO_ROOT}/test-results/stage6-canary}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
artifact_dir="${artifact_root}/${run_id}"
mkdir -p "${artifact_dir}"
artifact_dir="$(cd "${artifact_dir}" && pwd)"
temporary_dir="$(mktemp -d)"

relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
relay_signer_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
owner_private_key=0000000000000000000000000000000000000000000000000000000000000002
owner_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
agent_private_key=0000000000000000000000000000000000000000000000000000000000000003
agent_pubkey=f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
supervisor_private_key=0000000000000000000000000000000000000000000000000000000000000004
supervisor_pubkey=e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13
owner_role_id=60000000-0000-4000-8000-000000006001
owner_proposal_id=60000000-0000-4000-8000-000000006002
owner_assignment_id=60000000-0000-4000-8000-000000006003
goal_id=60000000-0000-4000-8000-000000006004
role_id=60000000-0000-4000-8000-000000006005
resource_id=60000000-0000-4000-8000-000000006006
guide_id=60000000-0000-4000-8000-000000006007
supplemental_document_id=60000000-0000-4000-8000-000000006008

relay_pid=""
acp_pid=""
acp_generation=0
database_created=0
runtime_state="${temporary_dir}/runtime-supervisor-state.json"
runtime_registry="${runtime_state}.children.json"
current_fence_path=""
current_runtime_id=""
current_runtime_epoch=""

stop_acp() {
  if [[ -n "${acp_pid}" ]] && kill -0 "${acp_pid}" 2>/dev/null; then
    kill -INT "${acp_pid}" 2>/dev/null || true
    for _ in $(seq 1 30); do
      kill -0 "${acp_pid}" 2>/dev/null || break
      sleep 1
    done
    if kill -0 "${acp_pid}" 2>/dev/null; then
      kill -KILL "${acp_pid}" 2>/dev/null || true
    fi
    wait "${acp_pid}" 2>/dev/null || true
  fi
  acp_pid=""
}

stop_relay() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  relay_pid=""
}

clean_incremental_artifacts() {
  for root in "${REPO_ROOT}/target" "${REPO_ROOT}/desktop/src-tauri/target"; do
    [[ -d "${root}" ]] || continue
    find "${root}" -type d -name incremental -prune -exec rm -rf -- {} +
  done
}

cleanup() {
  stop_acp
  stop_relay
  if [[ "${database_created}" == "1" ]] && [[ "${PROJECT_VIEW_STAGE6_KEEP_DB:-0}" != "1" ]]; then
    [[ "${database_name}" =~ ^buzz_pv_stage6_canary_[0-9_]+$ ]] \
      || fail "refusing unsafe scratch database cleanup: ${database_name}"
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
  elif [[ "${database_created}" == "1" ]]; then
    echo "Kept scratch database ${database_name}" >&2
  fi
  if [[ "${temporary_dir}" == /tmp/tmp.* ]]; then
    rm -rf -- "${temporary_dir}"
  fi
  clean_incremental_artifacts
}
trap cleanup EXIT

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"

psql_query() {
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 -Atc "$1"
}

pv_admin() {
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    BUZZ_PRIVATE_KEY="${owner_private_key}" \
    "${bin_dir}/buzz-admin" project-view "$@"
}

runtime_status() {
  env \
    DATABASE_URL="${database_url}" \
    BUZZ_PRIVATE_KEY="${owner_private_key}" \
    "${bin_dir}/buzz-admin" project-runtime status \
    --host "${test_host}" --assignment "${assignment_id}"
}

wait_for_current_runtime_status() {
  local runtime_id="$1"
  local runtime_epoch="$2"
  local retired_runtime_id="$3"
  local output_path="$4"
  for _ in $(seq 1 60); do
    if runtime_status >"${output_path}" 2>/dev/null \
      && jq -e --arg supervisor "${supervisor_pubkey}" \
        --arg runtime "${runtime_id}" --argjson epoch "${runtime_epoch}" \
        --arg retired "${retired_runtime_id}" '
          .status.managed == true
          and .status.availability == "available"
          and .status.binding.supervisor_pubkey == $supervisor
          and ($retired == "" or all(.status.runtimes[]; .runtime_id != $retired))
          and any(.status.runtimes[];
            .runtime_id == $runtime and .runtime_epoch == $epoch
            and .availability == "available")
        ' "${output_path}" >/dev/null; then
      return 0
    fi
    sleep 1
  done
  fail "Runtime ${runtime_id}:${runtime_epoch} did not become the only expected current lease"
}

buzz_as() {
  local private_key="$1"
  shift
  env \
    BUZZ_RELAY_URL="http://${test_host}" \
    BUZZ_PRIVATE_KEY="${private_key}" \
    "${bin_dir}/buzz" "$@"
}

buzz_managed() {
  local fence_path="$1"
  shift
  env \
    BUZZ_RELAY_URL="http://${test_host}" \
    BUZZ_PRIVATE_KEY="${agent_private_key}" \
    BUZZ_MANAGED_RUNTIME=1 \
    BUZZ_RUNTIME_FENCE_PATH="${fence_path}" \
    "${bin_dir}/buzz" "$@"
}

project_revision() {
  buzz_as "${owner_private_key}" --format compact project-view get \
    | jq -er '.project_revision'
}

proc_env_value() {
  local process_id="$1"
  local name="$2"
  local entry=""
  while IFS= read -r -d '' entry; do
    case "${entry}" in
      "${name}="*) printf '%s' "${entry#*=}"; return 0 ;;
    esac
  done <"/proc/${process_id}/environ"
  return 1
}

start_acp() {
  local prior_runtime="${1:-}"
  acp_generation=$((acp_generation + 1))
  local acp_log="${artifact_dir}/acp-generation-${acp_generation}.log"
  env \
    BUZZ_RELAY_URL="ws://${test_host}" \
    BUZZ_PRIVATE_KEY="${agent_private_key}" \
    BUZZ_ACP_AGENT_OWNER="${owner_pubkey}" \
    BUZZ_ACP_AGENT_COMMAND="${REPO_ROOT}/desktop/tests/e2e/fixtures/fake-acp-agent.mjs" \
    BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY="${supervisor_private_key}" \
    BUZZ_RUNTIME_SUPERVISION_STATE_PATH="${runtime_state}" \
    RUST_LOG=info \
    "${bin_dir}/buzz-acp" --respond-to nobody --no-presence --no-typing --no-memory \
    >"${acp_log}" 2>&1 &
  acp_pid=$!

  for _ in $(seq 1 60); do
    kill -0 "${acp_pid}" 2>/dev/null || {
      tail -200 "${acp_log}" >&2
      fail "ACP exited before establishing Runtime generation ${acp_generation}"
    }
    if jq -e --arg prior "${prior_runtime}" '
      .runtime_id != null and .runtime_epoch > 0
      and ($prior == "" or .runtime_id != $prior)
    ' "${runtime_state}" >/dev/null 2>&1 \
      && jq -e '.process_groups | length == 1' "${runtime_registry}" >/dev/null 2>&1; then
      break
    fi
    sleep 1
  done
  current_runtime_id="$(jq -er '.runtime_id' "${runtime_state}")"
  current_runtime_epoch="$(jq -er '.runtime_epoch' "${runtime_state}")"
  local child_pid
  child_pid="$(jq -er '.process_groups | keys[0]' "${runtime_registry}")"
  current_fence_path="$(proc_env_value "${child_pid}" BUZZ_RUNTIME_FENCE_PATH)"
  [[ -f "${current_fence_path}" ]] || fail "Agent child has no current Runtime fence"
  jq -e --arg runtime "${current_runtime_id}" --argjson epoch "${current_runtime_epoch}" '
    .runtime_id == $runtime and .runtime_epoch == $epoch
  ' "${current_fence_path}" >/dev/null
}

info_for() {
  curl --noproxy '*' -fsS "http://${test_host}/info"
}

echo "[1/6] Creating the isolated schema-v3 greenfield prerequisite"
docker compose up -d postgres redis >/dev/null
for container in buzz-postgres buzz-redis; do
  status=""
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' "${container}" 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  [[ "${status}" == "healthy" ]] || fail "${container} did not become healthy"
done

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null
database_created=1

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

psql_query "
  INSERT INTO communities (id, host, project_view_schema_version)
  VALUES ('${community_id}'::uuid, '${test_host}', 3);
  INSERT INTO relay_members (community_id, pubkey, role)
  VALUES
    ('${community_id}'::uuid, '${owner_pubkey}', 'owner'),
    ('${community_id}'::uuid, '${agent_pubkey}', 'member');
  INSERT INTO users (
    community_id, pubkey, display_name, agent_type,
    agent_owner_pubkey, channel_add_policy
  ) VALUES
    ('${community_id}'::uuid, decode('${owner_pubkey}', 'hex'),
     'Stage 6 owner', NULL, NULL, 'anyone'),
    ('${community_id}'::uuid, decode('${agent_pubkey}', 'hex'),
     'Stage 6 managed Agent', 'codex', decode('${owner_pubkey}', 'hex'), 'anyone');
" >/dev/null

if [[ "${PROJECT_VIEW_STAGE6_NO_BUILD:-0}" != "1" ]]; then
  if [[ "${profile}" == "dev" ]]; then
    cargo build -p buzz-relay -p buzz-cli -p buzz-admin -p buzz-acp
  else
    cargo build --profile "${profile}" -p buzz-relay -p buzz-cli -p buzz-admin -p buzz-acp
  fi
fi
for binary in buzz-relay buzz buzz-admin buzz-acp; do
  [[ -x "${bin_dir}/${binary}" ]] || fail "missing executable ${bin_dir}/${binary}"
done

relay_log="${artifact_dir}/relay.log"
env \
  DATABASE_URL="${database_url}" \
  REDIS_URL=redis://localhost:6379 \
  RELAY_URL="ws://${test_host}" \
  BUZZ_BIND_ADDR="0.0.0.0:${port}" \
  BUZZ_HEALTH_PORT="${health_port}" \
  BUZZ_METRICS_PORT="${metrics_port}" \
  BUZZ_AUTO_MIGRATE=false \
  BUZZ_REQUIRE_AUTH_TOKEN=false \
  BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
  BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  RELAY_OWNER_PUBKEY="${owner_pubkey}" \
  RELAY_OPERATOR_API_ORIGIN="http://127.0.0.1:${port}" \
  RELAY_OPERATOR_PUBKEYS="${owner_pubkey}" \
  "${bin_dir}/buzz-relay" >"${relay_log}" 2>&1 &
relay_pid=$!
status_code=""
for _ in $(seq 1 60); do
  kill -0 "${relay_pid}" 2>/dev/null || {
    tail -200 "${relay_log}" >&2
    fail "Relay exited before readiness"
  }
  status_code="$(curl --noproxy '*' -s -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${port}/_readiness" || true)"
  [[ "${status_code}" == "200" ]] && break
  sleep 1
done
[[ "${status_code}" == "200" ]] || fail "Relay did not become ready"

pre_initialize_info="$(info_for | tee "${artifact_dir}/info-v3-bootstrap.json")"
jq -e '
  (.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v3-bootstrap")) != null
    and ($extensions | index("buzz-project-view-v3")) == null
' <<<"${pre_initialize_info}" >/dev/null

prepare_receipt="$(pv_admin prepare-v3 \
  --community "${test_host}" \
  --idempotency-key "stage6-prepare-${database_name}" \
  --operator-pubkey "${owner_pubkey}" \
  | tee "${artifact_dir}/project-view-prepare-v3.json")"
preparation_operation_id="$(jq -er '.operation_id' <<<"${prepare_receipt}")"
initialize_file="${temporary_dir}/initialize-v3.json"
jq -n \
  --arg preparation_operation_id "${preparation_operation_id}" \
  --arg owner_pubkey "${owner_pubkey}" \
  --arg goal_id "${goal_id}" \
  --arg owner_role_id "${owner_role_id}" \
  --arg owner_proposal_id "${owner_proposal_id}" \
  --arg owner_assignment_id "${owner_assignment_id}" '{
    schema_version: 3,
    expected_project_revision: 0,
    request: {
      type: "initialize",
      preparation_operation_id: $preparation_operation_id,
      profile: {
        name: "Stage 6 Context canary",
        positioning: "Independent schema-v3 greenfield fixture",
        purpose: "Exercise Context and Role Brief closure",
        problem: "Context governance needs a repeatable current-runtime acceptance path",
        scope: "Isolated local scratch Community only"
      },
      goals: [{
        id: $goal_id,
        title: "Accept Stage 6 Context governance",
        desired_outcome: "Verified Context, Role Brief, and Runtime fences",
        directions: ["Keep every ordinary Project View operation on schema v3"]
      }],
      initial_roles: [{
        role_id: $owner_role_id,
        name: "Stage 6 owner",
        purpose: "Govern the isolated canary",
        responsibilities: ["Create the member Role and offer"],
        boundaries: ["Scratch Community only"],
        level: "admin",
        active: true,
        context_references: []
      }],
      initial_governance_assignments: [{
        member_pubkey: $owner_pubkey,
        role_id: $owner_role_id,
        proposal_id: $owner_proposal_id,
        assignment_id: $owner_assignment_id
      }]
    }
  }' >"${initialize_file}"
buzz_as "${owner_private_key}" --format compact project-view init-v3 \
  --command "${initialize_file}" >"${artifact_dir}/project-view-init-v3.json"
pv_admin enable --community "${test_host}" >"${artifact_dir}/project-view-enable-v3.json"
info_for | tee "${artifact_dir}/info-project-view-v3.json" \
  | jq -e '
      (.supported_extensions // []) as $extensions
      | ($extensions | index("buzz-project-view-v3")) != null
        and ($extensions | index("buzz-project-view-v3-bootstrap")) == null
        and ($extensions | all(
          (startswith("buzz-project-view-") | not)
          or . == "buzz-project-view-v3"
        ))
    ' >/dev/null

env DATABASE_URL="${database_url}" BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  "${bin_dir}/buzz-admin" project-document bootstrap \
  --community "${test_host}" --expected-pubkey "${relay_signer_pubkey}" \
  >"${artifact_dir}/document-bootstrap.json"
env DATABASE_URL="${database_url}" BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  "${bin_dir}/buzz-admin" project-document verify \
  --community "${test_host}" --expected-pubkey "${relay_signer_pubkey}" \
  >"${artifact_dir}/document-verify.json"
env DATABASE_URL="${database_url}" BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  "${bin_dir}/buzz-admin" project-document enable \
  --community "${test_host}" --expected-pubkey "${relay_signer_pubkey}" \
  >"${artifact_dir}/document-enable.json"

guide_body="${temporary_dir}/resource-guide.md"
printf '%s\n' '# Stage 6 Resource Guide' '' \
  'STAGE6_GUIDE_BODY_MUST_REQUIRE_EXPLICIT_FETCH' >"${guide_body}"
buzz_as "${owner_private_key}" --format compact documents create \
  --document-id "${guide_id}" --title "Stage 6 Resource Guide" \
  --summary "Mandatory Guide for the isolated Resource" \
  --content-file "${guide_body}" >"${artifact_dir}/guide-create.json"

resource_file="${temporary_dir}/resource.json"
jq -n --arg guide "${guide_id}" '{
  name: "Stage 6 managed context resource",
  resource_kind: "local_canary",
  summary: "Resource created entirely inside the schema-v3 Stage 6 fixture",
  guide_document_id: $guide,
  context_references: []
}' >"${resource_file}"
buzz_as "${owner_private_key}" --format compact project-view create resource \
  --expected-project-revision "$(project_revision)" --id "${resource_id}" \
  --data "${resource_file}" >"${artifact_dir}/resource-create-v3.json"

role_file="${temporary_dir}/role.json"
jq -n '{
  name: "Stage 6 Context operator",
  purpose: "Exercise verified Role Context and Runtime fences",
  responsibilities: ["Maintain the isolated Context set"],
  boundaries: ["Do not operate outside the scratch Community"],
  active: true,
  context_references: []
}' >"${role_file}"
buzz_as "${owner_private_key}" --format compact project-view create role \
  --expected-project-revision "$(project_revision)" --id "${role_id}" \
  --role-level member --data "${role_file}" >"${artifact_dir}/role-create-v3.json"

buzz_as "${owner_private_key}" --format compact roles offer \
  --role "${role_id}" --member "${agent_pubkey}" \
  --expected-project-revision "$(project_revision)" \
  --reason "Assign the isolated Stage 6 Context Role" \
  >"${artifact_dir}/role-offer-v3.json"
agent_proposals="$(buzz_as "${agent_private_key}" --format compact roles proposals \
  --status open --limit 20 | tee "${artifact_dir}/agent-open-proposals-v3.json")"
proposal_id="$(jq -er --arg role "${role_id}" --arg member "${agent_pubkey}" '
  [.proposals[]
   | select(.role_id == $role and .candidate_pubkey == $member
            and .effective_status == "open")]
  | if length == 1 then .[0].proposal_id else error("expected one open Stage 6 proposal") end
' <<<"${agent_proposals}")"
proposal_revision="$(jq -er '.project_revision' <<<"${agent_proposals}")"
buzz_as "${agent_private_key}" --format compact roles proposal accept "${proposal_id}" \
  --expected-project-revision "${proposal_revision}" \
  >"${artifact_dir}/role-accept-v3.json"
assignment_id="$(psql_query "
  SELECT assignment_id FROM project_role_assignments
  WHERE community_id = '${community_id}'::uuid
    AND member_pubkey = '${agent_pubkey}'
    AND role_id = '${role_id}'::uuid AND ended_at IS NULL
")"
[[ "${assignment_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "managed Agent has no active v3 Assignment"

env DATABASE_URL="${database_url}" BUZZ_PRIVATE_KEY="${owner_private_key}" \
  "${bin_dir}/buzz-admin" project-runtime bind \
  --host "${test_host}" --assignment "${assignment_id}" \
  --supervisor-pubkey "${supervisor_pubkey}" --operator-pubkey "${owner_pubkey}" \
  >"${artifact_dir}/runtime-supervisor-binding.json"

echo "[2/6] Atomically enabling Context on the current v3 fixture"

before_status="$(pv_admin context status --community "${test_host}" \
  | tee "${artifact_dir}/context-status-before.json")"
jq -e '
  .project_view_schema_version == 3
  and .project_view_enabled == true
  and .context_enabled == false
  and .document_enabled == true
  and .project_view_ready == true
  and .document_ready == true
  and .advertised_ready == false
' <<<"${before_status}" >/dev/null
set +e
buzz_as "${owner_private_key}" project-view context add "${role_id}" \
  --resource "${resource_id}" >"${artifact_dir}/context-add-disabled.stdout" \
  2>"${artifact_dir}/context-add-disabled.stderr"
disabled_add_status=$?
set -e
[[ "${disabled_add_status}" != "0" ]] || fail "Context add succeeded before capability enable"
rg -q "unavailable:project_view:context_capability" \
  "${artifact_dir}/context-add-disabled.stderr"

enable_receipt="$(pv_admin context enable --community "${test_host}" \
  --idempotency-key "stage6-enable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  | tee "${artifact_dir}/context-enable.json")"
enable_operation_id="$(jq -er '.operation_id' <<<"${enable_receipt}")"
jq -e '.enabled == true and .replayed == false and .closure_protocol_version == 1' \
  <<<"${enable_receipt}" >/dev/null
pv_admin context enable --community "${test_host}" \
  --idempotency-key "stage6-enable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  >"${artifact_dir}/context-enable-replay.json"
jq -e --arg operation "${enable_operation_id}" '
  .operation_id == $operation and .enabled == true and .replayed == true
' "${artifact_dir}/context-enable-replay.json" >/dev/null
info_for | tee "${artifact_dir}/info-context-enabled.json" \
  | jq -e '(.supported_extensions // []) | index("buzz-project-context-v1") != null' >/dev/null

echo "[3/6] Exercising managed-Agent Runtime and Assignment fences"
document_body_v1="${temporary_dir}/supplemental-v1.md"
printf '%s\n' '# Explicit Stage 6 task input' '' \
  'STAGE6_DOCUMENT_BODY_MUST_REQUIRE_EXPLICIT_FETCH_V1' >"${document_body_v1}"
buzz_as "${owner_private_key}" --format compact documents create \
  --document-id "${supplemental_document_id}" \
  --title "Stage 6 current runbook" --summary "Explicit local task context" \
  --content-file "${document_body_v1}" >"${artifact_dir}/supplemental-create.json"
jq -e --arg document "${supplemental_document_id}" '
  .accepted == true and .document_id == $document and .document_revision == 1
' "${artifact_dir}/supplemental-create.json" >/dev/null

start_acp
first_runtime_id="${current_runtime_id}"
first_runtime_epoch="${current_runtime_epoch}"
wait_for_current_runtime_status "${first_runtime_id}" "${first_runtime_epoch}" "" \
  "${artifact_dir}/runtime-status-generation-1.json"
buzz_managed "${current_fence_path}" --format compact project-view context add \
  "${role_id}" --resource "${resource_id}" >"${artifact_dir}/context-add-resource.json"

stop_acp
start_acp "${first_runtime_id}"
second_runtime_id="${current_runtime_id}"
second_runtime_epoch="${current_runtime_epoch}"
wait_for_current_runtime_status "${second_runtime_id}" "${second_runtime_epoch}" \
  "${first_runtime_id}" "${artifact_dir}/runtime-status-generation-2.json"
# Context is governed by Community/Role authority, not by the operational
# supervisor lease. The current fence stays in the harness environment, but
# Context mutation authorization depends on the active Assignment below.
buzz_managed "${current_fence_path}" --format compact project-view context add \
  "${role_id}" --document "${supplemental_document_id}" \
  >"${artifact_dir}/context-add-live.json"
buzz_managed "${current_fence_path}" --format compact project-view context add \
  "${role_id}" --document "${supplemental_document_id}" --revision 1 \
  >"${artifact_dir}/context-add-pinned.json"

context_before_assignment_change="$(buzz_managed "${current_fence_path}" --format compact \
  project-view context list "${role_id}" \
  | tee "${artifact_dir}/context-before-assignment-change.json")"
jq -e --arg resource "${resource_id}" --arg document "${supplemental_document_id}" '
  (.context_references | length) == 3
  and any(.context_references[]; .type == "resource" and .resource_id == $resource)
  and any(.context_references[]; .type == "document" and .document_id == $document and .mode == "live")
  and any(.context_references[]; .type == "document" and .document_id == $document and .mode == "pinned" and .document_revision == 1)
' <<<"${context_before_assignment_change}" >/dev/null

echo "[4/6] Verifying Role closure, body-free metadata, and explicit Guide fetch"
brief_v1="$(buzz_managed "${current_fence_path}" --format compact roles brief \
  | tee "${artifact_dir}/role-brief-context-v1.json")"
jq -e --arg resource "${resource_id}" --arg guide "${guide_id}" \
  --arg document "${supplemental_document_id}" '
  .project_view_schema_version == 3
  and .state.status == "assigned"
  and .context.availability.state == "ready"
  and .source_revisions.document_metadata.state == "verified"
  and any(.context.resources[]; .resource_id == $resource and .guide_document_id == $guide and .guide_document_revision == 1)
  and any(.context.live_documents[]; .document_id == $document and .document_revision == 1 and .title == "Stage 6 current runbook")
  and any(.context.pinned_documents[]; .document_id == $document and .document_revision == 1)
  and ((tostring | contains("STAGE6_DOCUMENT_BODY_MUST_REQUIRE_EXPLICIT_FETCH")) | not)
' <<<"${brief_v1}" >/dev/null

buzz_managed "${current_fence_path}" resources guide "${resource_id}" --content-only \
  >"${artifact_dir}/agent-explicit-guide.md"
cmp "${guide_body}" "${artifact_dir}/agent-explicit-guide.md"
! rg -q "STAGE6_DOCUMENT_BODY_MUST_REQUIRE_EXPLICIT_FETCH" \
  "${artifact_dir}"/acp-generation-*.log

pv_revision_before_document_edit="$(project_revision)"
document_body_v2="${temporary_dir}/supplemental-v2.md"
printf '%s\n' '# Updated explicit Stage 6 task input' '' \
  'STAGE6_DOCUMENT_BODY_MUST_REQUIRE_EXPLICIT_FETCH_V2' >"${document_body_v2}"
buzz_as "${owner_private_key}" --format compact documents update \
  "${supplemental_document_id}" --expected-revision 1 \
  --title "Updated Stage 6 current runbook" --summary "Updated explicit local task context" \
  --content-file "${document_body_v2}" >"${artifact_dir}/supplemental-update.json"
pv_revision_after_document_edit="$(project_revision)"
[[ "${pv_revision_before_document_edit}" == "${pv_revision_after_document_edit}" ]] \
  || fail "Document edit advanced the Project View revision"
brief_v2="$(buzz_managed "${current_fence_path}" --format compact roles brief \
  | tee "${artifact_dir}/role-brief-context-v2.json")"
jq -e --arg document "${supplemental_document_id}" '
  any(.context.live_documents[]; .document_id == $document and .document_revision == 2 and .title == "Updated Stage 6 current runbook")
  and any(.context.pinned_documents[]; .document_id == $document and .document_revision == 1)
  and ((tostring | contains("STAGE6_DOCUMENT_BODY_MUST_REQUIRE_EXPLICIT_FETCH")) | not)
' <<<"${brief_v2}" >/dev/null

set +e
buzz_as "${owner_private_key}" documents delete "${supplemental_document_id}" \
  --expected-revision 2 >"${artifact_dir}/live-delete-protected.stdout" \
  2>"${artifact_dir}/live-delete-protected.stderr"
live_delete_status=$?
set -e
[[ "${live_delete_status}" != "0" ]] || fail "Live-referenced Document was deleted"
rg -qi "referenc|conflict|protected" \
  "${artifact_dir}/live-delete-protected.stdout" "${artifact_dir}/live-delete-protected.stderr"

echo "[5/6] Verifying disable preservation, subset cleanup, and re-enable"
pv_admin context disable --community "${test_host}" \
  --idempotency-key "stage6-disable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  >"${artifact_dir}/context-disable.json"
info_for | tee "${artifact_dir}/info-context-disabled.json" \
  | jq -e '((.supported_extensions // []) | index("buzz-project-context-v1")) == null' >/dev/null
disabled_brief="$(buzz_as "${agent_private_key}" --format compact roles brief \
  | tee "${artifact_dir}/role-brief-context-disabled.json")"
jq -e '
  .context.availability.state == "unavailable_preserved"
  and (.context.resources | length) == 0
  and (.context.live_documents | length) == 0
  and (.context.pinned_documents | length) == 0
  and .source_revisions.document_metadata.state == "not_required"
' <<<"${disabled_brief}" >/dev/null
disabled_list="$(buzz_as "${agent_private_key}" --format compact project-view context list \
  "${role_id}" | tee "${artifact_dir}/context-disabled-preserved.json")"
jq -e '(.context_references | length) == 3 and .context_capability == false' \
  <<<"${disabled_list}" >/dev/null
set +e
buzz_as "${owner_private_key}" project-view context add "${role_id}" \
  --document "${guide_id}" >"${artifact_dir}/disabled-retarget.stdout" \
  2>"${artifact_dir}/disabled-retarget.stderr"
disabled_retarget_status=$?
set -e
[[ "${disabled_retarget_status}" != "0" ]] || fail "disabled Context accepted a new target"
rg -q "unavailable:project_view:context_capability" \
  "${artifact_dir}/disabled-retarget.stderr"
buzz_as "${owner_private_key}" --format compact project-view context remove \
  "${role_id}" --document "${supplemental_document_id}" \
  >"${artifact_dir}/disabled-remove-live.json"

pv_admin context enable --community "${test_host}" \
  --idempotency-key "stage6-reenable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  >"${artifact_dir}/context-reenable.json"
context_status_final="$(pv_admin context status --community "${test_host}" \
  | tee "${artifact_dir}/context-status-reenabled.json")"
jq -e '
  .context_enabled == true and .advertised_ready == true
  and .resource_reference_count == 1 and .document_reference_count == 1
' <<<"${context_status_final}" >/dev/null
reenabled_brief="$(buzz_managed "${current_fence_path}" --format compact roles brief \
  | tee "${artifact_dir}/role-brief-context-reenabled.json")"
jq -e --arg document "${supplemental_document_id}" '
  .context.availability.state == "ready"
  and (.context.live_documents | length) == 0
  and any(.context.pinned_documents[]; .document_id == $document and .document_revision == 1)
' <<<"${reenabled_brief}" >/dev/null

echo "[6/6] Verifying delete protection, pinned history, and normalized cleanup"
buzz_as "${owner_private_key}" --format compact documents delete \
  "${supplemental_document_id}" --expected-revision 2 \
  >"${artifact_dir}/pinned-document-delete.json"
buzz_as "${agent_private_key}" documents get "${supplemental_document_id}" \
  --revision 1 --content-only >"${artifact_dir}/pinned-document-v1.md"
cmp "${document_body_v1}" "${artifact_dir}/pinned-document-v1.md"
buzz_as "${agent_private_key}" --format compact project-view context list "${role_id}" \
  >"${artifact_dir}/context-after-document-delete.json"
jq -e --arg document "${supplemental_document_id}" '
  any(.context_references[]; .type == "document" and .document_id == $document and .mode == "pinned" and .document_revision == 1)
' "${artifact_dir}/context-after-document-delete.json" >/dev/null

set +e
buzz_as "${owner_private_key}" documents delete "${guide_id}" --expected-revision 1 \
  >"${artifact_dir}/guide-delete-protected.stdout" \
  2>"${artifact_dir}/guide-delete-protected.stderr"
guide_delete_status=$?
set -e
[[ "${guide_delete_status}" != "0" ]] || fail "mandatory Resource Guide was deleted"
rg -qi "guide|referenc|conflict|protected" \
  "${artifact_dir}/guide-delete-protected.stdout" "${artifact_dir}/guide-delete-protected.stderr"

resource_revision="$(buzz_as "${owner_private_key}" \
  project-view get-object resource "${resource_id}" | jq -er '.object.object_revision')"
revision="$(project_revision)"
set +e
buzz_as "${owner_private_key}" project-view delete resource "${resource_id}" \
  --expected-project-revision "${revision}" >"${artifact_dir}/resource-delete-protected.stdout" \
  2>"${artifact_dir}/resource-delete-protected.stderr"
resource_delete_status=$?
set -e
[[ "${resource_delete_status}" != "0" ]] || fail "Context-referenced Resource was deleted"
rg -qi "referenc|conflict|protected" \
  "${artifact_dir}/resource-delete-protected.stdout" "${artifact_dir}/resource-delete-protected.stderr"

buzz_as "${owner_private_key}" --format compact project-view context remove \
  "${role_id}" --resource "${resource_id}" >"${artifact_dir}/context-remove-resource.json"
buzz_as "${owner_private_key}" --format compact project-view context remove \
  "${role_id}" --document "${supplemental_document_id}" --revision 1 \
  >"${artifact_dir}/context-remove-pinned.json"
final_context="$(buzz_as "${owner_private_key}" --format compact project-view context list \
  "${role_id}" | tee "${artifact_dir}/context-final-empty.json")"
jq -e '.context_capability == true and (.context_references | length) == 0' \
  <<<"${final_context}" >/dev/null

operation_count="$(psql_query "SELECT count(*) FROM project_view_context_operations WHERE community_id = '${community_id}'")"
[[ "${operation_count}" == "3" ]] || fail "Context idempotency replay appended another ledger row"
audit_count="$(psql_query "SELECT count(*) FROM audit_log WHERE community_id = '${community_id}' AND action = 'project_context_control'")"
[[ "${audit_count}" == "3" ]] || fail "Context control audit count is incomplete"

revision="$(project_revision)"
stale_assignment_command="${temporary_dir}/stale-assignment-command.json"
stale_assignment_event="${temporary_dir}/stale-assignment-event.json"
expected_revision_after_assignment_end="$((revision + 1))"
jq -n \
  --argjson revision "${expected_revision_after_assignment_end}" \
  --arg assignment "${assignment_id}" \
  --arg role "${role_id}" \
  --arg resource "${resource_id}" '{
    schema_version: 3,
    expected_project_revision: $revision,
    acting_assignment_id: $assignment,
    request: {
      type: "update",
      object_type: "role",
      object_id: $role,
      patch: {context_references: [
        {type: "resource", resource_id: $resource}
      ]}
    }
  }' >"${stale_assignment_command}"
env BUZZ_PRIVATE_KEY="${agent_private_key}" node \
  desktop/scripts/stage6-canary-sign-project-view-event.mjs \
  "${stale_assignment_command}" "${stale_assignment_event}"
stop_acp
buzz_as "${owner_private_key}" --format compact roles assignment end \
  "${assignment_id}" --expected-project-revision "${revision}" \
  --reason "Stage 6 stale Assignment fence canary" \
  >"${artifact_dir}/old-assignment-end.json"
ended_runtime_status="$(runtime_status | tee "${artifact_dir}/runtime-status-assignment-ended.json")"
jq -e '
  .status.managed == false
  and (.status | has("binding") | not)
  and (.status | has("availability") | not)
  and (.status.runtimes | length) == 0
' <<<"${ended_runtime_status}" >/dev/null
set +e
env BUZZ_PRIVATE_KEY="${agent_private_key}" node \
  desktop/scripts/project-view-canary-nip98-post.mjs \
  "http://${test_host}/events" "${stale_assignment_event}" \
  >"${artifact_dir}/stale-assignment-response.json" \
  2>"${artifact_dir}/stale-assignment-request.stderr"
stale_assignment_request=$?
set -e
[[ "${stale_assignment_request}" != "0" ]] || fail "ended Assignment mutated Context"
rg -q "HTTP (400|409)" "${artifact_dir}/stale-assignment-request.stderr"
rg -qi "assignment|restricted|conflict" "${artifact_dir}/stale-assignment-response.json"
stop_relay

jq -n \
  --arg accepted_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg database "${database_name}" \
  --arg host "${test_host}" \
  --arg role_id "${role_id}" \
  --arg resource_id "${resource_id}" \
  --arg guide_id "${guide_id}" \
  --arg document_id "${supplemental_document_id}" \
  --arg assignment_id "${assignment_id}" \
  --arg first_runtime_id "${first_runtime_id}" \
  --argjson first_runtime_epoch "${first_runtime_epoch}" \
  --arg second_runtime_id "${second_runtime_id}" \
  --argjson second_runtime_epoch "${second_runtime_epoch}" \
  --argjson stable_project_revision "${pv_revision_after_document_edit}" \
  --argjson resource_revision "${resource_revision}" '{
    accepted_at: $accepted_at,
    execution: "real_local",
    fixture_origin: "greenfield_v3",
    project_view: {schema_version: 3, ordinary_runtime: "v3_only"},
    services: ["PostgreSQL", "Redis", "Relay", "buzz-cli", "buzz-admin", "buzz-acp", "ACP child"],
    context_control: {status: "passed", closure_protocol_version: 1, replay_first: true, audit_rows: 3},
    role_context: {
      status: "passed",
      role_id: $role_id,
      resource_id: $resource_id,
      guide_document_id: $guide_id,
      supplemental_document_id: $document_id,
      resource_object_revision: $resource_revision,
      body_injected: false,
      explicit_guide_fetch: true
    },
    fences: {
      status: "passed",
      purpose: "operational_supervision_not_context_acl",
      first_runtime_retired: true,
      binding_revoked_with_assignment: true,
      ended_assignment_rejected: $assignment_id,
      runtimes: [
        {runtime_id: $first_runtime_id, runtime_epoch: $first_runtime_epoch},
        {runtime_id: $second_runtime_id, runtime_epoch: $second_runtime_epoch}
      ]
    },
    document_metadata: {
      status: "passed",
      project_revision_unchanged_at: $stable_project_revision,
      live_revision_refreshed_to: 2,
      pinned_revision_survived_delete: 1
    },
    scratch_database: $database,
    host: $host
  }' >"${artifact_dir}/acceptance-summary.json"

(
  cd "${artifact_dir}"
  find . -type f ! -name artifact-digests.sha256 -print0 \
    | sort -z \
    | xargs -0 sha256sum
) >"${artifact_dir}/artifact-digests.sha256"

echo "Project View Stage 6 local-real canary passed."
echo "Evidence: ${artifact_dir}"
