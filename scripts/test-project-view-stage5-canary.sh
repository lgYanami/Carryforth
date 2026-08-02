#!/usr/bin/env bash
# Run the two Stage 5 acceptance paths against one real local Relay process:
# a bounded schema-v2 Resource/Guide maintenance cutover with a supervised ACP,
# and an independent empty-state direct-v3 initialization.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# shellcheck source=/dev/null
. ./bin/activate-hermit

umask 077
export CARGO_INCREMENTAL=0

fail() {
  echo "Project View Stage 5 canary: $*" >&2
  exit 1
}

for command in cargo curl docker jq node sha256sum; do
  command -v "${command}" >/dev/null || fail "missing required command: ${command}"
done

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
    fail "${container} did not become healthy"
  fi
done

database_name="${PROJECT_VIEW_STAGE5_DATABASE_NAME:-buzz_pv_stage5_canary_$$_${RANDOM}}"
if [[ ! "${database_name}" =~ ^buzz_pv_stage5_canary_[0-9_]+$ ]]; then
  fail "refusing unsafe scratch database name: ${database_name}"
fi

profile="${PROJECT_VIEW_STAGE5_PROFILE:-dev}"
if [[ "${profile}" == "dev" ]]; then
  bin_dir="${REPO_ROOT}/target/debug"
else
  bin_dir="${REPO_ROOT}/target/${profile}"
fi

port="${PROJECT_VIEW_STAGE5_PORT:-$((25000 + ($$ % 5000)))}"
health_port="$((port + 1))"
metrics_port="$((port + 2))"
legacy_host="localhost:${port}"
empty_host="127.0.0.1:${port}"
legacy_community_id="00000000-0000-4000-8000-000000005005"
empty_community_id="00000000-0000-4000-8000-000000005006"

artifact_root="${PROJECT_VIEW_STAGE5_ARTIFACT_ROOT:-${REPO_ROOT}/test-results/stage5-canary}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
artifact_dir="${artifact_root}/${run_id}"
mkdir -p "${artifact_dir}"
artifact_dir="$(cd "${artifact_dir}" && pwd)"
temporary_dir="$(mktemp -d)"

relay_log="${artifact_dir}/relay.log"
acp_log="${artifact_dir}/legacy-acp.log"
relay_pid=""
acp_pid=""

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
  if [[ "${PROJECT_VIEW_STAGE5_KEEP_DB:-0}" != "1" ]]; then
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
  else
    echo "Kept scratch database ${database_name}" >&2
  fi
  if [[ "${temporary_dir}" == /tmp/tmp.* ]]; then
    rm -rf -- "${temporary_dir}"
  fi
  clean_incremental_artifacts
}
trap cleanup EXIT

relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
relay_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
owner_private_key=0000000000000000000000000000000000000000000000000000000000000002
owner_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
agent_private_key=0000000000000000000000000000000000000000000000000000000000000003
agent_pubkey=f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
supervisor_private_key=0000000000000000000000000000000000000000000000000000000000000004
supervisor_pubkey=e493dbf1c10d80f3581e4904930b1404cc6c13900ee0758474fa94abe8c4cd13

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

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host) VALUES
        ('${legacy_community_id}', '${legacy_host}'),
        ('${empty_community_id}', '${empty_host}');
      INSERT INTO relay_members (community_id, pubkey, role) VALUES
        ('${legacy_community_id}', '${owner_pubkey}', 'owner'),
        ('${empty_community_id}', '${owner_pubkey}', 'owner');
      INSERT INTO users (community_id, pubkey, agent_owner_pubkey) VALUES
        ('${legacy_community_id}', decode('${owner_pubkey}', 'hex'), NULL),
        ('${legacy_community_id}', decode('${agent_pubkey}', 'hex'), decode('${owner_pubkey}', 'hex')),
        ('${empty_community_id}', decode('${owner_pubkey}', 'hex'), NULL);" >/dev/null

if [[ "${PROJECT_VIEW_STAGE5_NO_BUILD:-0}" != "1" ]]; then
  if [[ "${profile}" == "dev" ]]; then
    cargo build -p buzz-relay -p buzz-cli -p buzz-admin -p buzz-acp
  else
    cargo build --profile "${profile}" -p buzz-relay -p buzz-cli -p buzz-admin -p buzz-acp
  fi
fi
for binary in buzz-relay buzz buzz-admin buzz-acp; do
  [[ -x "${bin_dir}/${binary}" ]] || fail "missing executable ${bin_dir}/${binary}"
done

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

pd_admin() {
  env \
    DATABASE_URL="${database_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    "${bin_dir}/buzz-admin" project-document "$@"
}

buzz_as() {
  local private_key="$1"
  local host="$2"
  shift 2
  env \
    BUZZ_RELAY_URL="http://${host}" \
    BUZZ_PRIVATE_KEY="${private_key}" \
    "${bin_dir}/buzz" "$@"
}

write_response_result() {
  jq -er '.message | sub("^response:"; "") | fromjson' <<<"$1"
}

write_project_revision() {
  write_response_result "$1" | jq -er '.project_revision'
}

info_for() {
  curl --noproxy '*' -fsS "http://$1/info"
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

env \
  DATABASE_URL="${database_url}" \
  REDIS_URL=redis://localhost:6379 \
  RELAY_URL="ws://${legacy_host}" \
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
  if ! kill -0 "${relay_pid}" 2>/dev/null; then
    tail -200 "${relay_log}" >&2
    fail "Relay exited before readiness"
  fi
  status_code="$(curl --noproxy '*' -s -o /dev/null -w '%{http_code}' \
    "http://127.0.0.1:${port}/_readiness" || true)"
  [[ "${status_code}" == "200" ]] && break
  sleep 1
done
[[ "${status_code}" == "200" ]] || fail "Relay did not become ready"

echo "[1/2] Running bounded local schema-v2 to schema-v3 canary"

profile_file="${temporary_dir}/legacy-profile.json"
goal_file="${temporary_dir}/legacy-goal.json"
role_file="${temporary_dir}/legacy-role.json"
resource_file="${temporary_dir}/legacy-resource.json"
jq -n '{
  name: "Stage 5 local legacy canary",
  positioning: "A bounded real-process cutover fixture",
  purpose: "Accept the Project View v3 maintenance path",
  problem: "Resource locators must move into reviewed Guides",
  scope: "One isolated local Community"
}' >"${profile_file}"
jq -n '{
  id: "10000000-0000-4000-8000-000000005005",
  title: "Complete the Stage 5 cutover",
  desired_outcome: "A verified locator-free v3 Resource and fresh Runtime fence",
  directions: ["Keep Context disabled", "Require ordered maintenance ACKs"]
}' >"${goal_file}"
jq -n '{
  name: "Managed canary maintainer",
  purpose: "Exercise Role and Runtime continuity during cutover",
  responsibilities: ["Maintain the local canary"],
  boundaries: ["No external deployment"],
  active: true
}' >"${role_file}"
jq -n '{
  name: "Stage 5 source repository",
  resource_type: "repository",
  locator: {
    locator_type: "url",
    value: "https://example.invalid/stage5-canary.git"
  },
  description: "Synthetic local locator used only by the Stage 5 canary"
}' >"${resource_file}"

pv_admin enable --community "${legacy_host}" >"${artifact_dir}/legacy-enable-v1.txt"
legacy_init="$(buzz_as "${owner_private_key}" "${legacy_host}" --format compact \
  project-view init --profile "${profile_file}" --goal "${goal_file}" \
  | tee "${artifact_dir}/legacy-init-v1.json")"
legacy_revision="$(write_project_revision "${legacy_init}")"
[[ "${legacy_revision}" == "1" ]] || fail "unexpected v1 initialization revision"

pv_admin disable --community "${legacy_host}" >"${artifact_dir}/legacy-disable-v1.txt"
cutover_v2="$(pv_admin cutover-v2 \
  --community "${legacy_host}" \
  --idempotency-key "stage5-v2-${database_name}" \
  --expected-pubkey "${relay_pubkey}" \
  | tee "${artifact_dir}/legacy-cutover-v2.json")"
legacy_revision="$(jq -er '.project_revision' <<<"${cutover_v2}")"
[[ "$(psql_query "SELECT project_view_schema_version FROM communities WHERE id = '${legacy_community_id}'")" == "2" ]]

pd_admin bootstrap --community "${legacy_host}" --expected-pubkey "${relay_pubkey}" \
  >"${artifact_dir}/legacy-document-bootstrap.json"
pd_admin verify --community "${legacy_host}" --expected-pubkey "${relay_pubkey}" \
  >"${artifact_dir}/legacy-document-verify.json"
pd_admin enable --community "${legacy_host}" --expected-pubkey "${relay_pubkey}" \
  >"${artifact_dir}/legacy-document-enable.txt"
pv_admin enable --community "${legacy_host}" >"${artifact_dir}/legacy-enable-v2.txt"

legacy_info_v2="$(info_for "${legacy_host}" | tee "${artifact_dir}/legacy-info-v2.json")"
jq -e '(.supported_extensions // []) | index("buzz-project-view-v2") != null' \
  <<<"${legacy_info_v2}" >/dev/null

role_created="$(buzz_as "${owner_private_key}" "${legacy_host}" --format compact \
  project-view create role --expected-project-revision "${legacy_revision}" --data "${role_file}" \
  | tee "${artifact_dir}/legacy-role-create.json")"
role_id="$(jq -er '.object_id' <<<"${role_created}")"
legacy_revision="$(write_project_revision "${role_created}")"

role_offer="$(buzz_as "${owner_private_key}" "${legacy_host}" --format compact \
  roles offer --role "${role_id}" --member "${agent_pubkey}" \
  --expected-project-revision "${legacy_revision}" \
  | tee "${artifact_dir}/legacy-role-offer.json")"
legacy_revision="$(write_project_revision "${role_offer}")"
proposals="$(buzz_as "${agent_private_key}" "${legacy_host}" --format compact \
  roles proposals --status open | tee "${artifact_dir}/legacy-agent-proposals.json")"
proposal_id="$(jq -er --arg role "${role_id}" \
  '.proposals[] | select(.role_id == $role) | .proposal_id' <<<"${proposals}")"
role_accept="$(buzz_as "${agent_private_key}" "${legacy_host}" --format compact \
  roles proposal accept "${proposal_id}" --expected-project-revision "${legacy_revision}" \
  | tee "${artifact_dir}/legacy-role-accept.json")"
legacy_revision="$(write_project_revision "${role_accept}")"
agent_current="$(buzz_as "${agent_private_key}" "${legacy_host}" --format compact \
  roles current | tee "${artifact_dir}/legacy-agent-current-v2.json")"
assignment_id="$(jq -er '.assignment.assignment_id' <<<"${agent_current}")"
jq -e --arg assignment "${assignment_id}" \
  '.assigned == true and .assignment.assignment_id == $assignment' \
  <<<"${agent_current}" >/dev/null

resource_created="$(buzz_as "${owner_private_key}" "${legacy_host}" --format compact \
  project-view create resource --expected-project-revision "${legacy_revision}" \
  --data "${resource_file}" | tee "${artifact_dir}/legacy-resource-create.json")"
resource_id="$(jq -er '.object_id' <<<"${resource_created}")"
legacy_revision="$(write_project_revision "${resource_created}")"
legacy_resource="$(buzz_as "${owner_private_key}" "${legacy_host}" \
  project-view get-object resource "${resource_id}" \
  | tee "${artifact_dir}/legacy-resource-v2.json")"
legacy_resource_revision="$(jq -er '.object.object_revision' <<<"${legacy_resource}")"
jq -e '.object.data.data.locator.value == "https://example.invalid/stage5-canary.git"' \
  <<<"${legacy_resource}" >/dev/null

review_dir="${artifact_dir}/resource-review"
mkdir -p "${review_dir}"
pv_admin v3 resources export --community "${legacy_host}" --out "${review_dir}" \
  --operator-pubkey "${owner_pubkey}" >"${artifact_dir}/legacy-resource-export.txt"
draft_manifest="${review_dir}/resource-mapping-draft.json"
guide_id="$(jq -er --arg resource "${resource_id}" \
  '.entries[] | select(.resource_id == $resource) | .guide_document_id' "${draft_manifest}")"
guide_content="${temporary_dir}/guide.md"
jq -r --arg resource "${resource_id}" \
  '.entries[] | select(.resource_id == $resource) | .suggested_guide_markdown' \
  "${draft_manifest}" >"${guide_content}"

guide_create="$(buzz_as "${owner_private_key}" "${legacy_host}" --format compact \
  documents create --document-id "${guide_id}" --title "Stage 5 source repository Guide" \
  --summary "Reviewed local canary access instructions" --content-file "${guide_content}" \
  | tee "${artifact_dir}/legacy-guide-create.json")"
jq -e --arg guide "${guide_id}" \
  '.accepted == true and .document_id == $guide and .document_revision == 1' \
  <<<"${guide_create}" >/dev/null
guide_get="$(buzz_as "${owner_private_key}" "${legacy_host}" \
  documents get "${guide_id}" | tee "${artifact_dir}/legacy-guide-get.json")"
guide_revision="$(jq -er '.document_revision' <<<"${guide_get}")"
guide_head_event_id="$(jq -er '.head_event_id' <<<"${guide_get}")"
guide_revision_event_id="$(jq -er '.revision_event_id' <<<"${guide_get}")"

completed_draft="${review_dir}/resource-mapping-completed.json"
jq \
  --arg resource "${resource_id}" \
  --arg head "${guide_head_event_id}" \
  --arg revision_event "${guide_revision_event_id}" \
  --argjson guide_revision "${guide_revision}" '
    .entries |= map(
      if .resource_id == $resource then
        .reviewed_v3_payload = {
          resource_data: {
            name: .legacy_resource.name,
            resource_kind: "vendor.custom_repository",
            summary: "Human-reviewed local canary repository",
            guide_document_id: .guide_document_id
          },
          context_references: []
        }
        | .guide_document_revision = $guide_revision
        | .guide_head_event_id = $head
        | .guide_revision_event_id = $revision_event
      else . end
    )
  ' "${draft_manifest}" >"${completed_draft}"
reviewed_manifest="${review_dir}/resource-mapping-reviewed.json"
buzz_as "${owner_private_key}" "${legacy_host}" project-view v3 resources approve \
  --manifest "${completed_draft}" --out "${reviewed_manifest}" \
  >"${artifact_dir}/legacy-resource-approve.txt"
manifest_digest="$(sha256sum "${reviewed_manifest}" | awk '{print $1}')"
pv_admin v3 resources validate --community "${legacy_host}" --manifest "${reviewed_manifest}" \
  | tee "${artifact_dir}/legacy-resource-validate-before.json" >/dev/null

binding_body="${temporary_dir}/binding.json"
jq -n \
  --arg host "${legacy_host}" \
  --arg assignment "${assignment_id}" \
  --arg supervisor "${supervisor_pubkey}" '{
    host: $host,
    assignment_id: $assignment,
    supervisor_pubkey: $supervisor
  }' >"${binding_body}"
binding_url="http://127.0.0.1:${port}/operator/project-runtime/bindings"
binding_result="$(env BUZZ_PRIVATE_KEY="${owner_private_key}" \
  node desktop/scripts/stage5-canary-nip98-post.mjs "${binding_url}" "${binding_body}" \
  | tee "${artifact_dir}/legacy-runtime-binding.json")"
jq -e --arg assignment "${assignment_id}" --arg supervisor "${supervisor_pubkey}" \
  '.assignment_id == $assignment and .supervisor_pubkey == $supervisor' \
  <<<"${binding_result}" >/dev/null

runtime_state="${artifact_dir}/runtime-supervisor-state.json"
runtime_registry="${runtime_state}.children.json"
env \
  BUZZ_RELAY_URL="ws://${legacy_host}" \
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
    fail "ACP exited before establishing its v2 Runtime"
  }
  if jq -e --arg assignment "${assignment_id}" \
    '.assignment_id == $assignment and .runtime_id != null and .runtime_epoch > 0' \
    "${runtime_state}" >/dev/null 2>&1 \
    && jq -e '.process_groups | length == 1' "${runtime_registry}" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
jq -e '.runtime_id != null' "${runtime_state}" >/dev/null 2>&1 \
  || fail "ACP did not persist a Runtime state"
jq -e '.process_groups | length == 1' "${runtime_registry}" >/dev/null 2>&1 \
  || fail "ACP did not register its real Agent child"
cp "${runtime_state}" "${artifact_dir}/legacy-runtime-before.json"
old_runtime_id="$(jq -er '.runtime_id' "${runtime_state}")"
old_runtime_epoch="$(jq -er '.runtime_epoch' "${runtime_state}")"
old_child_pid="$(jq -er '.process_groups | keys[0]' "${runtime_registry}")"
old_fence_path="$(proc_env_value "${old_child_pid}" BUZZ_RUNTIME_FENCE_PATH)"
[[ -f "${old_fence_path}" ]] || fail "real Agent child did not inherit a Runtime fence"
cp "${old_fence_path}" "${artifact_dir}/legacy-fence-before.json"
jq -e --arg runtime "${old_runtime_id}" --argjson epoch "${old_runtime_epoch}" \
  '.runtime_id == $runtime and .runtime_epoch == $epoch' \
  "${artifact_dir}/legacy-fence-before.json" >/dev/null
[[ "$(psql_query "SELECT availability FROM project_runtime_leases WHERE community_id = '${legacy_community_id}' AND assignment_id = '${assignment_id}' AND runtime_id = '${old_runtime_id}' AND ended_at IS NULL")" == "available" ]]

maintenance_begin="$(pv_admin maintenance begin \
  --community "${legacy_host}" \
  --required-client-protocol-version 1 \
  --expected-pubkey "${relay_pubkey}" \
  --idempotency-key "stage5-begin-${database_name}" \
  | tee "${artifact_dir}/legacy-maintenance-begin.json")"
maintenance_epoch="$(jq -er '.maintenance_epoch' <<<"${maintenance_begin}")"
jq -e '.result.assignment_baseline_count == 1 and .state == "draining"' \
  <<<"${maintenance_begin}" >/dev/null
legacy_info_draining="$(info_for "${legacy_host}" | tee "${artifact_dir}/legacy-info-draining.json")"
jq -e '(.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v2")) == null
  and ($extensions | index("buzz-project-view-v3")) == null' \
  <<<"${legacy_info_draining}" >/dev/null

ack_ready=0
for attempt in $(seq 1 90); do
  if pv_admin maintenance ack-probe --community "${legacy_host}" \
    --epoch "${maintenance_epoch}" --max-poll-age-seconds 30 \
    >"${artifact_dir}/legacy-maintenance-readiness.json" 2>"${temporary_dir}/ack-probe.err"; then
    ack_ready=1
    break
  fi
  kill -0 "${acp_pid}" 2>/dev/null || {
    tail -200 "${acp_log}" >&2
    fail "ACP exited while draining maintenance epoch ${maintenance_epoch}"
  }
  sleep 1
done
if [[ "${ack_ready}" != "1" ]]; then
  cat "${temporary_dir}/ack-probe.err" >&2
  pv_admin maintenance readiness --community "${legacy_host}" \
    --epoch "${maintenance_epoch}" --max-poll-age-seconds 30 >&2 || true
  fail "maintenance did not become ready to freeze"
fi
jq -e '
  .ready_to_freeze == true
  and .fleet_protocol_ready == true
  and .fleet_poll_ready == true
  and .durable_acks_complete == true
  and .runtime_retirement_complete == true
  and (.assignments | length) == 1
  and (.runtimes | length) == 1
' "${artifact_dir}/legacy-maintenance-readiness.json" >/dev/null
jq -e '.process_groups | length == 0' "${runtime_registry}" >/dev/null
kill -0 "${old_child_pid}" 2>/dev/null \
  && fail "pre-maintenance Agent child survived its ordered ACK" \
  || true
[[ ! -e "${old_fence_path}" ]] || fail "pre-maintenance Runtime fence remained published"
ack_ordered="$(psql_query "SELECT
    (SELECT max(acked_at) FROM project_view_maintenance_acks
      WHERE community_id = '${legacy_community_id}' AND maintenance_epoch = ${maintenance_epoch})
    <
    (SELECT min(acked_at) FROM project_view_maintenance_assignment_acks
      WHERE community_id = '${legacy_community_id}' AND maintenance_epoch = ${maintenance_epoch})")"
[[ "${ack_ordered}" == "t" ]] || fail "Assignment ACK was not durably ordered after Runtime ACK"
psql_query "SELECT jsonb_pretty(jsonb_build_object(
    'runtime_ack', (SELECT to_jsonb(a) FROM project_view_maintenance_acks a
      WHERE community_id = '${legacy_community_id}' AND maintenance_epoch = ${maintenance_epoch}),
    'assignment_ack', (SELECT to_jsonb(a) FROM project_view_maintenance_assignment_acks a
      WHERE community_id = '${legacy_community_id}' AND maintenance_epoch = ${maintenance_epoch})
  ))" >"${artifact_dir}/legacy-maintenance-ack-order.json"

pv_admin maintenance freeze --community "${legacy_host}" --epoch "${maintenance_epoch}" \
  --idempotency-key "stage5-freeze-${database_name}" \
  | tee "${artifact_dir}/legacy-maintenance-freeze.json" >/dev/null
pv_admin v3 resources validate --community "${legacy_host}" --manifest "${reviewed_manifest}" \
  | tee "${artifact_dir}/legacy-resource-validate-frozen.json" >/dev/null
cutover_v3="$(pv_admin v3 cutover \
  --community "${legacy_host}" \
  --manifest "${reviewed_manifest}" \
  --maintenance-epoch "${maintenance_epoch}" \
  --idempotency-key "stage5-cutover-${database_name}" \
  --expected-pubkey "${relay_pubkey}" \
  | tee "${artifact_dir}/legacy-cutover-v3.json")"
jq -e '.target_schema_version == 3 or .result.target_schema_version == 3' \
  <<<"${cutover_v3}" >/dev/null 2>&1 || true
pv_admin maintenance verify --community "${legacy_host}" --epoch "${maintenance_epoch}" \
  --idempotency-key "stage5-verify-${database_name}" --expected-pubkey "${relay_pubkey}" \
  | tee "${artifact_dir}/legacy-maintenance-verify.json" >/dev/null
pv_admin maintenance resume --community "${legacy_host}" --epoch "${maintenance_epoch}" \
  --idempotency-key "stage5-resume-${database_name}" --expected-pubkey "${relay_pubkey}" \
  | tee "${artifact_dir}/legacy-maintenance-resume.json" >/dev/null

fresh_runtime=0
for _ in $(seq 1 90); do
  kill -0 "${acp_pid}" 2>/dev/null || {
    tail -200 "${acp_log}" >&2
    fail "ACP exited before establishing its post-v3 Runtime"
  }
  if jq -e --arg old "${old_runtime_id}" \
    '.runtime_id != $old and .runtime_epoch > 0' "${runtime_state}" >/dev/null 2>&1 \
    && jq -e '.process_groups | length == 1' "${runtime_registry}" >/dev/null 2>&1; then
    fresh_runtime=1
    break
  fi
  sleep 1
done
[[ "${fresh_runtime}" == "1" ]] || fail "ACP did not establish a fresh v3 Runtime generation"
cp "${runtime_state}" "${artifact_dir}/legacy-runtime-after.json"
new_runtime_id="$(jq -er '.runtime_id' "${runtime_state}")"
new_runtime_epoch="$(jq -er '.runtime_epoch' "${runtime_state}")"
new_child_pid="$(jq -er '.process_groups | keys[0]' "${runtime_registry}")"
new_fence_path="$(proc_env_value "${new_child_pid}" BUZZ_RUNTIME_FENCE_PATH)"
[[ "${new_fence_path}" == "${old_fence_path}" ]] \
  || fail "ACP changed its generation-scoped fence path unexpectedly"
cp "${new_fence_path}" "${artifact_dir}/legacy-fence-after.json"
jq -e --arg runtime "${new_runtime_id}" --argjson epoch "${new_runtime_epoch}" \
  '.runtime_id == $runtime and .runtime_epoch == $epoch' \
  "${artifact_dir}/legacy-fence-after.json" >/dev/null
[[ "${new_runtime_id}:${new_runtime_epoch}" != "${old_runtime_id}:${old_runtime_epoch}" ]]
! rg -q --fixed-strings "${old_runtime_id}" "${artifact_dir}/legacy-fence-after.json"
[[ "$(psql_query "SELECT availability FROM project_runtime_leases WHERE community_id = '${legacy_community_id}' AND assignment_id = '${assignment_id}' AND runtime_id = '${new_runtime_id}' AND ended_at IS NULL")" == "available" ]]

legacy_info_v3="$(info_for "${legacy_host}" | tee "${artifact_dir}/legacy-info-v3.json")"
jq -e '(.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v3")) != null
  and ($extensions | index("buzz-project-view-v2")) == null
  and ($extensions | index("buzz-project-context-v1")) == null' \
  <<<"${legacy_info_v3}" >/dev/null

buzz_as "${owner_private_key}" "${legacy_host}" --format compact project-view get \
  >"${artifact_dir}/legacy-project-view-v3.json"
resource_v3="$(buzz_as "${owner_private_key}" "${legacy_host}" \
  project-view get-object resource "${resource_id}" \
  | tee "${artifact_dir}/legacy-resource-v3.json")"
jq -e \
  --arg kind "vendor.custom_repository" \
  --arg guide "${guide_id}" \
  --arg reviewer "${owner_pubkey}" \
  --argjson revision "$((legacy_resource_revision + 1))" '
    .project_view_schema_version == 3
    and .object.object_revision == $revision
    and .object.updated_by == $reviewer
    and .object.data.object_type == "resource"
    and .object.data.data.resource_kind == $kind
    and .object.data.data.guide_document_id == $guide
    and (.object.data.data | has("locator") | not)
    and (.object.data.data | has("resource_type") | not)
    and .source.source_type == "operator"
  ' <<<"${resource_v3}" >/dev/null
buzz_as "${owner_private_key}" "${legacy_host}" resources guide "${resource_id}" \
  >"${artifact_dir}/legacy-resource-guide.json"
buzz_as "${owner_private_key}" "${legacy_host}" resources guide "${resource_id}" --content-only \
  >"${artifact_dir}/legacy-resource-guide.md"
cmp "${guide_content}" "${artifact_dir}/legacy-resource-guide.md"

buzz_as "${owner_private_key}" "${legacy_host}" roles brief --member "${agent_pubkey}" \
  >"${artifact_dir}/legacy-agent-role-brief-v3.json"
jq -e '
  .project_view_schema_version == 3
  and .state.status == "assigned"
  and .context.availability.state == "not_advertised_empty"
  and .source_revisions.document_metadata.state == "not_required"
' "${artifact_dir}/legacy-agent-role-brief-v3.json" >/dev/null

provenance_ok="$(psql_query "SELECT
    object.schema_version = 3
    AND object.object_revision = ${legacy_resource_revision} + 1
    AND encode(object.updated_by, 'hex') = '${owner_pubkey}'
    AND object.source_type = 'operator'
    AND mapping.status = 'consumed'
    AND encode(mapping.reviewed_by_pubkey, 'hex') = '${owner_pubkey}'
    AND EXISTS (
      SELECT 1 FROM project_view_v3_committed_resource_entries committed
      WHERE committed.community_id = object.community_id
        AND committed.resource_id = object.object_id
        AND committed.legacy_object_revision = ${legacy_resource_revision}
        AND encode(committed.reviewed_by_pubkey, 'hex') = '${owner_pubkey}'
    )
  FROM project_view_objects object
  JOIN project_view_v3_resource_mappings mapping
    ON mapping.community_id = object.community_id AND mapping.resource_id = object.object_id
  WHERE object.community_id = '${legacy_community_id}' AND object.object_id = '${resource_id}'")"
[[ "${provenance_ok}" == "t" ]] || fail "v3 Resource reviewer/provenance evidence is incomplete"

set +e
buzz_as "${owner_private_key}" "${legacy_host}" project-view v3 resources approve \
  --manifest "${completed_draft}" --out "${temporary_dir}/must-not-exist.json" \
  >"${artifact_dir}/legacy-v2-only-after-v3.stdout" \
  2>"${artifact_dir}/legacy-v2-only-after-v3.stderr"
legacy_v2_status=$?
set -e
[[ "${legacy_v2_status}" != "0" ]] || fail "v2-only Resource approval remained available after v3 cutover"
rg -qi "unsupported|only valid before a v2-to-v3 cutover" \
  "${artifact_dir}/legacy-v2-only-after-v3.stderr"

pv_admin maintenance status --community "${legacy_host}" --epoch "${maintenance_epoch}" \
  >"${artifact_dir}/legacy-maintenance-final.json"
jq -e '
  .state == "normal"
  and .current_epoch == null
  and .project_view_schema_version == 3
  and .project_view_enabled == true
  and .epoch.outcome == "resumed"
  and .epoch.assignment_baseline_count == 1
  and .epoch.assignment_ack_count == 1
  and .epoch.runtime_baseline_count == 1
  and .epoch.runtime_ack_count == 1
' "${artifact_dir}/legacy-maintenance-final.json" >/dev/null

stop_acp

echo "[2/2] Running independent empty-state direct-v3 canary"

prepare_v3="$(pv_admin prepare-v3 --community "${empty_host}" \
  --idempotency-key "stage5-prepare-${database_name}" --operator-pubkey "${owner_pubkey}" \
  | tee "${artifact_dir}/empty-prepare-v3.json")"
preparation_operation_id="$(jq -er '.operation_id' <<<"${prepare_v3}")"
empty_command="${temporary_dir}/empty-initialize-v3.json"
jq -n \
  --arg operation "${preparation_operation_id}" \
  --arg owner "${owner_pubkey}" '{
    schema_version: 3,
    expected_project_revision: 0,
    request: {
      type: "initialize",
      preparation_operation_id: $operation,
      profile: {
        name: "Stage 5 empty-state canary",
        positioning: "A direct schema-v3 local fixture",
        purpose: "Accept greenfield Project View v3 initialization",
        problem: "Greenfield Communities need no legacy migration",
        scope: "One isolated local Community"
      },
      goals: [{
        id: "10000000-0000-4000-8000-000000005006",
        title: "Initialize directly on v3",
        desired_outcome: "Revision one with exact Human governance",
        directions: ["Keep Context empty"]
      }],
      initial_roles: [{
        role_id: "20000000-0000-4000-8000-000000005006",
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
        role_id: "20000000-0000-4000-8000-000000005006",
        proposal_id: "30000000-0000-4000-8000-000000005006",
        assignment_id: "40000000-0000-4000-8000-000000005006"
      }]
    }
  }' >"${empty_command}"

empty_init="$(buzz_as "${owner_private_key}" "${empty_host}" --format compact \
  project-view init-v3 --command "${empty_command}" \
  | tee "${artifact_dir}/empty-init-v3.json")"
jq -e '.accepted == true' <<<"${empty_init}" >/dev/null
[[ "$(write_project_revision "${empty_init}")" == "1" ]] \
  || fail "unexpected direct-v3 initialization revision"
empty_disabled_state="$(psql_query "SELECT json_build_object(
    'schema_version', c.project_view_schema_version,
    'enabled', c.project_view_enabled,
    'context_enabled', c.project_context_enabled,
    'project_revision', s.project_revision,
    'projection_generation', s.projection_generation,
    'active_objects', s.active_object_count,
    'active_assignments', s.active_assignment_count
  ) FROM communities c JOIN project_view_state s ON s.community_id = c.id
  WHERE c.id = '${empty_community_id}'")"
printf '%s\n' "${empty_disabled_state}" >"${artifact_dir}/empty-state-before-enable.json"
jq -e '
  .schema_version == 3
  and .enabled == false
  and .context_enabled == false
  and .project_revision == 1
  and .projection_generation == 1
  and .active_objects == 3
  and .active_assignments == 1
' <<<"${empty_disabled_state}" >/dev/null

pv_admin enable --community "${empty_host}" >"${artifact_dir}/empty-enable-v3.txt"
empty_info_v3="$(info_for "${empty_host}" | tee "${artifact_dir}/empty-info-v3.json")"
jq -e '(.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v3")) != null
  and ($extensions | index("buzz-project-view-v2")) == null
  and ($extensions | index("buzz-project-context-v1")) == null' \
  <<<"${empty_info_v3}" >/dev/null

empty_view="$(buzz_as "${owner_private_key}" "${empty_host}" \
  project-view get | tee "${artifact_dir}/empty-project-view-v3.json")"
jq -e '
  .project_view_schema_version == 3
  and .project_revision == 1
  and .projection_generation == 1
  and (.objects | length) == 3
' <<<"${empty_view}" >/dev/null
buzz_as "${owner_private_key}" "${empty_host}" roles current \
  >"${artifact_dir}/empty-owner-current-v3.json"
jq -e '
  .project_view_schema_version == 3
  and .assigned == true
  and .assignment.assignment_id == "40000000-0000-4000-8000-000000005006"
  and .role.level == "admin"
' "${artifact_dir}/empty-owner-current-v3.json" >/dev/null
buzz_as "${owner_private_key}" "${empty_host}" roles brief \
  >"${artifact_dir}/empty-owner-role-brief-v3.json"
jq -e '
  .project_view_schema_version == 3
  and .project_revision == 1
  and .context.availability.state == "not_advertised_empty"
  and .source_revisions.document_metadata.state == "not_required"
' "${artifact_dir}/empty-owner-role-brief-v3.json" >/dev/null

set +e
buzz_as "${owner_private_key}" "${empty_host}" project-view v3 resources approve \
  --manifest "${completed_draft}" --out "${temporary_dir}/empty-must-not-exist.json" \
  >"${artifact_dir}/empty-v2-only.stdout" \
  2>"${artifact_dir}/empty-v2-only.stderr"
empty_v2_status=$?
set -e
[[ "${empty_v2_status}" != "0" ]] || fail "v2-only Resource approval remained available on direct v3"
rg -qi "unsupported|only valid before a v2-to-v3 cutover" "${artifact_dir}/empty-v2-only.stderr"

empty_final_state="$(psql_query "SELECT json_build_object(
    'schema_version', c.project_view_schema_version,
    'enabled', c.project_view_enabled,
    'context_enabled', c.project_context_enabled,
    'project_revision', s.project_revision,
    'projection_generation', s.projection_generation,
    'preparation_consumed', p.consumed_by_change_id IS NOT NULL,
    'owner_assignment_count', (SELECT count(*) FROM project_role_assignments a
      WHERE a.community_id = c.id AND a.ended_at IS NULL
        AND a.member_pubkey = '${owner_pubkey}')
  ) FROM communities c
  JOIN project_view_state s ON s.community_id = c.id
  JOIN project_view_provisioning_operations p
    ON p.community_id = c.id AND p.operation_id = '${preparation_operation_id}'
  WHERE c.id = '${empty_community_id}'")"
printf '%s\n' "${empty_final_state}" >"${artifact_dir}/empty-state-final.json"
jq -e '
  .schema_version == 3
  and .enabled == true
  and .context_enabled == false
  and .project_revision == 1
  and .projection_generation == 1
  and .preparation_consumed == true
  and .owner_assignment_count == 1
' <<<"${empty_final_state}" >/dev/null

stop_relay

jq -n \
  --arg accepted_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg database "${database_name}" \
  --arg legacy_host "${legacy_host}" \
  --arg empty_host "${empty_host}" \
  --arg manifest_digest "${manifest_digest}" \
  --arg resource_id "${resource_id}" \
  --arg guide_id "${guide_id}" \
  --arg assignment_id "${assignment_id}" \
  --argjson maintenance_epoch "${maintenance_epoch}" \
  --arg old_runtime_id "${old_runtime_id}" \
  --argjson old_runtime_epoch "${old_runtime_epoch}" \
  --arg new_runtime_id "${new_runtime_id}" \
  --argjson new_runtime_epoch "${new_runtime_epoch}" \
  --arg preparation_operation_id "${preparation_operation_id}" '{
    accepted_at: $accepted_at,
    execution: "real_local",
    services: ["PostgreSQL", "Redis", "Relay", "buzz-cli", "buzz-admin", "buzz-acp", "ACP child"],
    legacy_cutover: {
      status: "passed",
      host: $legacy_host,
      resource_id: $resource_id,
      guide_document_id: $guide_id,
      reviewed_manifest_sha256: $manifest_digest,
      maintenance_epoch: $maintenance_epoch,
      assignment_id: $assignment_id,
      runtime_before: {runtime_id: $old_runtime_id, runtime_epoch: $old_runtime_epoch},
      runtime_after: {runtime_id: $new_runtime_id, runtime_epoch: $new_runtime_epoch},
      context_advertised: false
    },
    empty_direct_v3: {
      status: "passed",
      host: $empty_host,
      preparation_operation_id: $preparation_operation_id,
      project_revision: 1,
      projection_generation: 1,
      context_advertised: false
    },
    scratch_database: $database
  }' >"${artifact_dir}/acceptance-summary.json"

(
  cd "${artifact_dir}"
  find . -type f ! -name artifact-digests.sha256 -print0 \
    | sort -z \
    | xargs -0 sha256sum
) >"${artifact_dir}/artifact-digests.sha256"

echo "Project View Stage 5 local-real canaries passed."
echo "Evidence: ${artifact_dir}"
