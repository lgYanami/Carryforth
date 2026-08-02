#!/usr/bin/env bash
# Run the Stage 6 Context/Role Brief acceptance path against real local
# PostgreSQL, Redis, Relay, CLI, admin, ACP supervisor, and Agent child
# processes. The Stage 5 canary prepares the exact schema-v3 prerequisite.

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
legacy_host="localhost:${port}"
legacy_community_id="00000000-0000-4000-8000-000000005005"
database_name="buzz_pv_stage5_canary_$$_${RANDOM}"
artifact_root="${PROJECT_VIEW_STAGE6_ARTIFACT_ROOT:-${REPO_ROOT}/test-results/stage6-canary}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
artifact_dir="${artifact_root}/${run_id}"
mkdir -p "${artifact_dir}"
artifact_dir="$(cd "${artifact_dir}" && pwd)"
temporary_dir="$(mktemp -d)"

relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
owner_private_key=0000000000000000000000000000000000000000000000000000000000000002
owner_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
agent_private_key=0000000000000000000000000000000000000000000000000000000000000003
supervisor_private_key=0000000000000000000000000000000000000000000000000000000000000004
supplemental_document_id=60000000-0000-4000-8000-000000006006

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

buzz_as() {
  local private_key="$1"
  shift
  env \
    BUZZ_RELAY_URL="http://${legacy_host}" \
    BUZZ_PRIVATE_KEY="${private_key}" \
    "${bin_dir}/buzz" "$@"
}

buzz_managed() {
  local fence_path="$1"
  shift
  env \
    BUZZ_RELAY_URL="http://${legacy_host}" \
    BUZZ_PRIVATE_KEY="${agent_private_key}" \
    BUZZ_MANAGED_AGENT=1 \
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
  curl --noproxy '*' -fsS "http://${legacy_host}/info"
}

echo "[1/6] Preparing the exact Stage 5 schema-v3 prerequisite"
stage5_root="${artifact_dir}/stage5-prerequisite"
mkdir -p "${stage5_root}"
database_created=1
PROJECT_VIEW_STAGE5_KEEP_DB=1 \
PROJECT_VIEW_STAGE5_DATABASE_NAME="${database_name}" \
PROJECT_VIEW_STAGE5_ARTIFACT_ROOT="${stage5_root}" \
PROJECT_VIEW_STAGE5_PORT="${port}" \
PROJECT_VIEW_STAGE5_PROFILE="${profile}" \
CARGO_INCREMENTAL=0 \
  "${REPO_ROOT}/scripts/test-project-view-stage5-canary.sh" \
  >"${artifact_dir}/stage5-prerequisite.stdout" \
  2>"${artifact_dir}/stage5-prerequisite.stderr"
stage5_summary="$(find "${stage5_root}" -type f -name acceptance-summary.json -print | sort | tail -1)"
[[ -n "${stage5_summary}" ]] || fail "Stage 5 prerequisite produced no acceptance summary"
stage5_run_dir="$(dirname "${stage5_summary}")"
resource_id="$(jq -er '.legacy_cutover.resource_id' "${stage5_summary}")"
guide_id="$(jq -er '.legacy_cutover.guide_document_id' "${stage5_summary}")"
old_assignment_id="$(jq -er '.legacy_cutover.assignment_id' "${stage5_summary}")"
role_id="$(psql_query "SELECT role_id FROM project_role_assignments WHERE community_id = '${legacy_community_id}' AND assignment_id = '${old_assignment_id}'")"
[[ -n "${role_id}" ]] || fail "Stage 5 prerequisite has no assigned Role"

echo "[2/6] Starting the real Relay and atomically enabling Context"
relay_log="${artifact_dir}/relay.log"
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

before_status="$(pv_admin context status --community "${legacy_host}" \
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

enable_receipt="$(pv_admin context enable --community "${legacy_host}" \
  --idempotency-key "stage6-enable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  | tee "${artifact_dir}/context-enable.json")"
enable_operation_id="$(jq -er '.operation_id' <<<"${enable_receipt}")"
jq -e '.enabled == true and .replayed == false and .closure_protocol_version == 1' \
  <<<"${enable_receipt}" >/dev/null
pv_admin context enable --community "${legacy_host}" \
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
buzz_managed "${current_fence_path}" --format compact project-view context add \
  "${role_id}" --resource "${resource_id}" >"${artifact_dir}/context-add-resource.json"
first_runtime_id="${current_runtime_id}"
first_runtime_epoch="${current_runtime_epoch}"
first_fence="${temporary_dir}/first-runtime-fence.json"
cp "${current_fence_path}" "${first_fence}"

stop_acp
start_acp "${first_runtime_id}"
second_runtime_id="${current_runtime_id}"
second_runtime_epoch="${current_runtime_epoch}"
set +e
buzz_managed "${first_fence}" project-view context add "${role_id}" \
  --document "${supplemental_document_id}" >"${artifact_dir}/stale-runtime.stdout" \
  2>"${artifact_dir}/stale-runtime.stderr"
stale_runtime_status=$?
set -e
[[ "${stale_runtime_status}" != "0" ]] || fail "retired Runtime fence mutated Context"
rg -qi "runtime.fence|runtime_fence|conflict:project_view" \
  "${artifact_dir}/stale-runtime.stdout" "${artifact_dir}/stale-runtime.stderr"
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

stale_assignment_command="${temporary_dir}/stale-assignment-command.json"
stale_assignment_event="${temporary_dir}/stale-assignment-event.json"
stale_assignment_revision="$(project_revision)"
jq -n \
  --argjson revision "${stale_assignment_revision}" \
  --arg assignment "${old_assignment_id}" \
  --arg runtime "${second_runtime_id}" \
  --argjson epoch "${second_runtime_epoch}" \
  --arg role "${role_id}" \
  --arg resource "${resource_id}" \
  --arg document "${supplemental_document_id}" '{
    schema_version: 3,
    expected_project_revision: $revision,
    acting_assignment_id: $assignment,
    runtime_fence: {runtime_id: $runtime, runtime_epoch: $epoch},
    request: {
      type: "update",
      object_type: "role",
      object_id: $role,
      patch: {context_references: [
        {type: "resource", resource_id: $resource},
        {type: "document", document_id: $document, mode: "live"},
        {type: "document", document_id: $document, mode: "pinned", document_revision: 1}
      ]}
    }
  }' >"${stale_assignment_command}"
env BUZZ_PRIVATE_KEY="${agent_private_key}" node \
  desktop/scripts/stage6-canary-sign-project-view-event.mjs \
  "${stale_assignment_command}" "${stale_assignment_event}"

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
cmp "${stage5_run_dir}/legacy-resource-guide.md" "${artifact_dir}/agent-explicit-guide.md"
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
pv_admin context disable --community "${legacy_host}" \
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

pv_admin context enable --community "${legacy_host}" \
  --idempotency-key "stage6-reenable-${database_name}" --operator-pubkey "${owner_pubkey}" \
  >"${artifact_dir}/context-reenable.json"
context_status_final="$(pv_admin context status --community "${legacy_host}" \
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

operation_count="$(psql_query "SELECT count(*) FROM project_view_context_operations WHERE community_id = '${legacy_community_id}'")"
[[ "${operation_count}" == "3" ]] || fail "Context idempotency replay appended another ledger row"
audit_count="$(psql_query "SELECT count(*) FROM audit_log WHERE community_id = '${legacy_community_id}' AND action = 'project_context_control'")"
[[ "${audit_count}" == "3" ]] || fail "Context control audit count is incomplete"

stop_acp
revision="$(project_revision)"
buzz_as "${owner_private_key}" --format compact roles assignment end \
  "${old_assignment_id}" --expected-project-revision "${revision}" \
  --reason "Stage 6 stale Assignment fence canary" \
  >"${artifact_dir}/old-assignment-end.json"
set +e
env BUZZ_PRIVATE_KEY="${agent_private_key}" node \
  desktop/scripts/stage5-canary-nip98-post.mjs \
  "http://${legacy_host}/events" "${stale_assignment_event}" \
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
  --arg host "${legacy_host}" \
  --arg role_id "${role_id}" \
  --arg resource_id "${resource_id}" \
  --arg guide_id "${guide_id}" \
  --arg document_id "${supplemental_document_id}" \
  --arg old_assignment_id "${old_assignment_id}" \
  --arg first_runtime_id "${first_runtime_id}" \
  --argjson first_runtime_epoch "${first_runtime_epoch}" \
  --arg second_runtime_id "${second_runtime_id}" \
  --argjson second_runtime_epoch "${second_runtime_epoch}" \
  --argjson stable_project_revision "${pv_revision_after_document_edit}" \
  --argjson resource_revision "${resource_revision}" '{
    accepted_at: $accepted_at,
    execution: "real_local",
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
      ended_assignment_rejected: $old_assignment_id,
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
