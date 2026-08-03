#!/usr/bin/env bash
# Run one bounded real-Codex acceptance of Meeting V2 action finalization.
#
# This intentionally runs one scenario once. It is not a retrying qualification
# framework: a provider, continuity, materializer, or lifecycle failure leaves a
# failed evidence record for review.
set -euo pipefail
umask 077

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"
if [[ -f "${repo_root}/bin/activate-hermit" ]]; then
  # shellcheck disable=SC1091
  . "${repo_root}/bin/activate-hermit" >/dev/null
fi

artifact_root="${1:-${MEETING_V2_ACTIONS_ACCEPTANCE_ARTIFACT_ROOT:-${TMPDIR:-/tmp}/buzz-meeting-v2-actions-acceptance}}"
model="${MEETING_V2_ACTIONS_ACCEPTANCE_MODEL:-gpt-5.6-sol}"
relay_port="${MEETING_V2_ACTIONS_ACCEPTANCE_RELAY_PORT:-3320}"
health_port="${MEETING_V2_ACTIONS_ACCEPTANCE_HEALTH_PORT:-9321}"
metrics_port="${MEETING_V2_ACTIONS_ACCEPTANCE_METRICS_PORT:-9322}"
scenario_timeout_seconds="${MEETING_V2_ACTIONS_ACCEPTANCE_TIMEOUT_SECONDS:-600}"
postgres_container="${MEETING_V2_ACTIONS_ACCEPTANCE_POSTGRES_CONTAINER:-buzz-postgres}"
redis_image="${MEETING_V2_ACTIONS_ACCEPTANCE_REDIS_IMAGE:-redis:7-alpine}"
run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run_id="meeting-v2-actions-${run_stamp}-$$"
run_dir="${artifact_root}/${run_id}"
secret_dir="$(mktemp -d "${TMPDIR:-/tmp}/buzz-meeting-v2-actions-secrets.XXXXXX")"
database_name="buzz_meeting_v2_actions_acceptance_${run_stamp//[^0-9]/}_$$"
redis_container="buzz-meeting-v2-actions-redis-${run_stamp//[^0-9]/}-$$"
relay_host="localhost:${relay_port}"
relay_url="ws://${relay_host}"
database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
relay_private_key="0000000000000000000000000000000000000000000000000000000000000001"
relay_pubkey="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
action_capability="meeting-v2-action-finalization-v1"
redis_url=""
relay_pid=""
agent_pid=""
database_created=false
cleanup_done=false

mkdir -p "${run_dir}/logs" "${run_dir}/preflight"

log() {
  printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"
}

fail() {
  log "FAIL: $*"
  printf '%s\n' "$*" >"${run_dir}/failure.txt"
  exit 1
}

stop_agent() {
  if [[ -n "${agent_pid}" ]] && kill -0 "${agent_pid}" 2>/dev/null; then
    kill -TERM "${agent_pid}" 2>/dev/null || true
    wait "${agent_pid}" 2>/dev/null || true
  fi
  agent_pid=""
}

stop_relay() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill -TERM "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  relay_pid=""
}

cleanup() {
  if [[ "${cleanup_done}" == true ]]; then
    return
  fi
  cleanup_done=true
  stop_agent
  stop_relay
  if [[ "${redis_container}" =~ ^buzz-meeting-v2-actions-redis-[0-9]+-[0-9]+$ ]]; then
    docker rm -f "${redis_container}" >/dev/null 2>&1 || true
  fi
  if [[ "${database_created}" == true && "${database_name}" =~ ^buzz_meeting_v2_actions_acceptance_[0-9_]+$ ]]; then
    docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS \"${database_name}\" WITH (FORCE)" >/dev/null 2>&1 || true
  fi
  case "${secret_dir}" in
    "${TMPDIR:-/tmp}"/buzz-meeting-v2-actions-secrets.*)
      rm -rf -- "${secret_dir}"
      ;;
    *)
      echo "refusing to remove unexpected secret directory: ${secret_dir}" >&2
      ;;
  esac
}

write_checksums() {
  [[ -d "${run_dir}" ]] || return
  (
    cd "${run_dir}"
    find . -type f ! -name sha256.txt -print \
      | sed 's#^./##' | LC_ALL=C sort \
      | while IFS= read -r path; do shasum -a 256 "${path}"; done \
      >sha256.txt
  ) || true
}

trap 'exit_status=$?;
  if [[ "${exit_status}" -ne 0 && "${exit_status}" -ne 130 && ! -f "${run_dir}/failure.txt" ]]; then
    printf "acceptance runner exited unexpectedly with status %s\n" "${exit_status}" >"${run_dir}/failure.txt"
  fi
  cleanup
  write_checksums
  exit "${exit_status}"' EXIT
trap 'exit 130' INT TERM

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is missing: $1"
}

for required in awk codex curl docker git jq rg sed shasum ss uuidgen; do
  require_command "${required}"
done
for numeric in "${relay_port}" "${health_port}" "${metrics_port}" "${scenario_timeout_seconds}"; do
  [[ "${numeric}" =~ ^[0-9]+$ ]] || fail "ports and timeout must be integers"
done
for port in "${relay_port}" "${health_port}" "${metrics_port}"; do
  [[ "${port}" -gt 0 && "${port}" -le 65535 ]] || fail "invalid port: ${port}"
done
[[ "${relay_port}" != "${health_port}" && "${relay_port}" != "${metrics_port}" \
  && "${health_port}" != "${metrics_port}" ]] || fail "Relay, health, and metrics ports must differ"
[[ "${scenario_timeout_seconds}" -ge 60 && "${scenario_timeout_seconds}" -le 1800 ]] \
  || fail "acceptance timeout must be between 60 and 1800 seconds"
for port in "${relay_port}" "${health_port}" "${metrics_port}"; do
  if ss -ltn | awk '{print $4}' | rg -q ":${port}$"; then
    fail "port ${port} is already in use"
  fi
done
for binary in buzz buzz-acp buzz-admin buzz-relay; do
  [[ -x "target/release/${binary}" ]] || fail "missing release binary: target/release/${binary}"
done

resolve_codex_acp() {
  local candidate
  local -a candidates=()
  if [[ -n "${MEETING_V2_CODEX_ACP_BIN:-}" ]]; then
    candidates+=("${MEETING_V2_CODEX_ACP_BIN}")
  fi
  if command -v codex-acp >/dev/null 2>&1; then
    candidates+=("$(command -v codex-acp)")
  fi
  candidates+=("${HOME}/.local/share/Buzz/node-tools/bin/codex-acp")
  for candidate in "${candidates[@]}"; do
    if [[ -x "${candidate}" ]] \
      && [[ "$("${candidate}" --version 2>/dev/null || true)" == "@agentclientprotocol/codex-acp 1.1.7" ]]; then
      readlink -f "${candidate}"
      return 0
    fi
  done
  return 1
}

codex_acp_bin="$(resolve_codex_acp)" || fail \
  "@agentclientprotocol/codex-acp 1.1.7 is not installed"
codex_acp_package="$(cd "$(dirname "${codex_acp_bin}")/.." && pwd)/package.json"
[[ -f "${codex_acp_package}" ]] || fail "could not locate codex-acp package.json"
jq -e '.name == "@agentclientprotocol/codex-acp" and .version == "1.1.7"' \
  "${codex_acp_package}" >/dev/null || fail "unexpected codex-acp package identity"

log "preflight: provider login, adapter, model catalog, and action capability"
codex --version >"${run_dir}/preflight/codex-version.txt"
codex login status >"${run_dir}/preflight/codex-login-status.txt" 2>&1
rg -q '^Logged in using (ChatGPT|an API key)$' \
  "${run_dir}/preflight/codex-login-status.txt" || fail "Codex is not authenticated"
"${codex_acp_bin}" --version >"${run_dir}/preflight/codex-acp-version.txt"
jq '{name, version, bin}' "${codex_acp_package}" \
  >"${run_dir}/preflight/codex-acp-package.json"
target/release/buzz-acp capabilities --json >"${run_dir}/preflight/acp-capabilities.json"
jq -e --arg capability "${action_capability}" '
  .meeting.qualificationEvidenceCompiled == true
  and (.meeting.capabilities | index($capability) != null)
  and any(
    .meeting.protocols[];
    .schemaVersion == "3"
      and .policy == "moderated-board-actions-v1"
      and .capability == $capability
      and .moderatorContinuity == "exact_agent_slot_and_acp_session"
      and (.turns | index("action_finalization") != null)
  )
' "${run_dir}/preflight/acp-capabilities.json" >/dev/null \
  || fail "buzz-acp does not expose the compiled action acceptance capability"
BUZZ_ACP_AGENT_COMMAND="${codex_acp_bin}" target/release/buzz-acp models --json \
  >"${run_dir}/preflight/codex-acp-models.json"
jq -e --arg requested "${model}[max]" '
  [.stable.configOptions[]?.options[]?.value, .unstable.availableModels[]?.modelId]
  | any(. == $requested)
' "${run_dir}/preflight/codex-acp-models.json" >/dev/null \
  || fail "codex-acp did not expose ${model}[max]"

git status --porcelain=v1 >"${run_dir}/workspace-before.status"
git diff --binary HEAD >"${secret_dir}/workspace-before.diff"
workspace_status_sha256="$(shasum -a 256 "${run_dir}/workspace-before.status" | awk '{print $1}')"
workspace_diff_sha256="$(shasum -a 256 "${secret_dir}/workspace-before.diff" | awk '{print $1}')"
printf '%s\n' "${workspace_status_sha256}" >"${run_dir}/workspace-before.status.sha256"
printf '%s\n' "${workspace_diff_sha256}" >"${run_dir}/workspace-before.diff.sha256"
shasum -a 256 "$0" target/release/buzz target/release/buzz-acp \
  target/release/buzz-admin target/release/buzz-relay "${codex_acp_bin}" \
  >"${run_dir}/preflight/executable-sha256.txt"

generate_identity() {
  local name="$1"
  local output="${secret_dir}/${name}.key-output"
  target/release/buzz-admin generate-key >"${output}"
  awk '/^Public key:/ {print $3}' "${output}" >"${secret_dir}/${name}.pub"
  awk '/^Secret key:/ {print $3}' "${output}" >"${secret_dir}/${name}.key"
  rm -- "${output}"
  [[ "$(<"${secret_dir}/${name}.pub")" =~ ^[0-9a-f]{64}$ ]] \
    || fail "invalid public key for ${name}"
  [[ "$(<"${secret_dir}/${name}.key")" =~ ^[0-9a-f]{64}$ ]] \
    || fail "invalid private key for ${name}"
}

generate_identity supervisor
generate_identity moderator
generate_identity participant
supervisor_pubkey="$(<"${secret_dir}/supervisor.pub")"
moderator_pubkey="$(<"${secret_dir}/moderator.pub")"
participant_pubkey="$(<"${secret_dir}/participant.pub")"
moderator_private_key="$(<"${secret_dir}/moderator.key")"

printf 'role\tmeeting_role\tparticipant_type\tpubkey\n' >"${run_dir}/roster.tsv"
printf 'moderator\tmoderator\tagent\t%s\n' "${moderator_pubkey}" >>"${run_dir}/roster.tsv"
printf 'participant\tparticipant\thuman\t%s\n' "${participant_pubkey}" >>"${run_dir}/roster.tsv"

log "starting disposable Redis and PostgreSQL database"
docker run -d --rm --name "${redis_container}" -p 127.0.0.1::6379 "${redis_image}" \
  >"${run_dir}/logs/redis-container-id.txt"
redis_port="$(docker port "${redis_container}" 6379/tcp | sed -n 's/.*://p' | head -n 1)"
[[ "${redis_port}" =~ ^[0-9]+$ ]] || fail "could not resolve disposable Redis port"
redis_url="redis://127.0.0.1:${redis_port}/0"
redis_ready=false
for ignored_attempt in $(seq 1 100); do
  if docker exec "${redis_container}" redis-cli ping 2>/dev/null | rg -q '^PONG$'; then
    redis_ready=true
    break
  fi
  sleep 0.1
done
[[ "${redis_ready}" == true ]] || fail "disposable Redis did not become ready"

docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"${database_name}\"" >/dev/null
database_created=true
DATABASE_URL="${database_url}" target/release/buzz-admin migrate \
  >"${run_dir}/logs/migrate.log" 2>&1

env \
  DATABASE_URL="${database_url}" \
  REDIS_URL="${redis_url}" \
  BUZZ_BIND_ADDR="127.0.0.1:${relay_port}" \
  BUZZ_HEALTH_PORT="${health_port}" \
  BUZZ_METRICS_PORT="${metrics_port}" \
  RELAY_URL="${relay_url}" \
  BUZZ_AUTO_MIGRATE=false \
  BUZZ_MEETING_V1_CREATE_ENABLED=true \
  BUZZ_MEETING_V2_CREATE_ENABLED=true \
  BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED=true \
  BUZZ_REQUIRE_RELAY_MEMBERSHIP=true \
  RELAY_OWNER_PUBKEY="${supervisor_pubkey}" \
  BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  RUST_LOG="buzz_relay=info,buzz_db=info" \
  target/release/buzz-relay >"${run_dir}/logs/relay.log" 2>&1 &
relay_pid=$!

relay_ready=false
for ignored_attempt in $(seq 1 180); do
  if curl -fsS -H "Host: ${relay_host}" "http://127.0.0.1:${relay_port}/_readiness" \
    >/dev/null 2>&1; then
    relay_ready=true
    break
  fi
  if ! kill -0 "${relay_pid}" 2>/dev/null; then
    fail "Relay exited during startup; see ${run_dir}/logs/relay.log"
  fi
  sleep 0.5
done
[[ "${relay_ready}" == true ]] || fail "Relay did not become ready"

community_id="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
  psql -U buzz -d "${database_name}" -qtA \
  -c "SELECT id FROM communities WHERE lower(host)=lower('${relay_host}');")"
[[ "${community_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "could not resolve disposable Community"

seed_identity() {
  local role="$1"
  local participant_type="$2"
  local public_key="$3"
  local owner_sql="NULL"
  local agent_type_sql="NULL"
  local capabilities_sql="NULL"
  if [[ "${participant_type}" == agent ]]; then
    owner_sql="decode('${supervisor_pubkey}', 'hex')"
    agent_type_sql="'codex'"
    capabilities_sql="jsonb_build_array('${action_capability}'::text)"
  fi
  docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
    psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 -c "
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES ('${community_id}'::uuid, '${public_key}', '${role}')
      ON CONFLICT (community_id, pubkey)
      DO UPDATE SET role=EXCLUDED.role, updated_at=clock_timestamp();
      INSERT INTO users (
        community_id, pubkey, display_name, agent_type,
        agent_owner_pubkey, channel_add_policy, capabilities
      ) VALUES (
        '${community_id}'::uuid, decode('${public_key}', 'hex'),
        'Meeting action acceptance ${role}', ${agent_type_sql},
        ${owner_sql}, 'anyone', ${capabilities_sql}
      )
      ON CONFLICT (community_id, pubkey)
      DO UPDATE SET
        display_name=EXCLUDED.display_name,
        agent_type=EXCLUDED.agent_type,
        agent_owner_pubkey=EXCLUDED.agent_owner_pubkey,
        channel_add_policy='anyone',
        capabilities=EXCLUDED.capabilities,
        deactivated_at=NULL;
    " >/dev/null
}

seed_identity owner human "${supervisor_pubkey}"
seed_identity admin agent "${moderator_pubkey}"
seed_identity member human "${participant_pubkey}"

curl -fsS -H 'Accept: application/nostr+json' -H "Host: ${relay_host}" \
  "http://127.0.0.1:${relay_port}/" >"${run_dir}/preflight/relay-capabilities.json"
jq -e '
  (.supported_extensions | index("buzz-meeting-v2-actions") != null)
  and (.supported_extensions | index("buzz-meeting-v2-actions-create") != null)
' "${run_dir}/preflight/relay-capabilities.json" >/dev/null \
  || fail "Relay did not advertise action runtime and Create capabilities"

buzz_as_moderator() {
  BUZZ_RELAY_URL="${relay_url}" BUZZ_PRIVATE_KEY="${moderator_private_key}" \
    target/release/buzz --format compact "$@"
}

admin_project_view() {
  DATABASE_URL="${database_url}" REDIS_URL="${redis_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    target/release/buzz-admin project-view "$@"
}

goal_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
role_id="$(uuidgen | tr '[:upper:]' '[:lower:]')"
jq -n '{
  name: "Meeting action acceptance",
  positioning: "A disposable action-finalization target",
  purpose: "Validate a Meeting-owned Project View materialization",
  problem: "Meeting conclusions need durable follow-up",
  scope: "Backend acceptance only"
}' >"${secret_dir}/profile.json"
jq -n --arg id "${goal_id}" '{
  id: $id,
  title: "Accept Meeting action finalization",
  desired_outcome: "The Meeting closes only after its frozen plan is materialized",
  directions: []
}' >"${secret_dir}/goal.json"
jq -n '{
  name: "Meeting action owner",
  purpose: "Own the materialized acceptance Work",
  responsibilities: ["Carry the action accepted by the Meeting"],
  boundaries: ["Only the frozen Meeting plan"],
  active: true
}' >"${secret_dir}/role.json"

log "initializing and cutting the disposable Project View over to v2"
admin_project_view enable --community "${relay_host}" >"${run_dir}/logs/project-view-enable-v1.log"
buzz_as_moderator project-view init \
  --profile "${secret_dir}/profile.json" \
  --goal "${secret_dir}/goal.json" >"${run_dir}/logs/project-view-init.json"
buzz_as_moderator project-view create role \
  --expected-project-revision 1 \
  --id "${role_id}" \
  --data "${secret_dir}/role.json" >"${run_dir}/logs/project-view-role.json"
admin_project_view disable --community "${relay_host}" >"${run_dir}/logs/project-view-disable.log"
admin_project_view cutover-v2 \
  --community "${relay_host}" \
  --admin-assignment "${moderator_pubkey}=${role_id}" \
  --idempotency-key "${run_id}" \
  --expected-pubkey "${relay_pubkey}" >"${run_dir}/logs/project-view-cutover.json"
admin_project_view enable --community "${relay_host}" >"${run_dir}/logs/project-view-enable-v2.log"
buzz_as_moderator project-view get >"${run_dir}/logs/project-view-v2.json"

project_schema="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
  psql -U buzz -d "${database_name}" -qtA \
  -c "SELECT project_view_schema_version FROM communities WHERE id='${community_id}'::uuid;")"
[[ "${project_schema}" == 2 ]] || fail "Project View did not reach schema v2"
assignment_id="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
  psql -U buzz -d "${database_name}" -qtA \
  -c "SELECT assignment_id FROM project_role_assignments
      WHERE community_id='${community_id}'::uuid
        AND member_pubkey='${moderator_pubkey}' AND role_id='${role_id}'::uuid
        AND ended_at IS NULL;")"
[[ "${assignment_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "moderator has no active Project View Assignment"

board_file="${secret_dir}/board.md"
printf '%s\n' \
  '# Goal' \
  'Reached: validate that a Meeting-owned action is materialized before normal close.' \
  '' \
  '# Effective conclusion' \
  'The accepted follow-up must be represented by one Requirement and one Work in this Community Project View.' \
  '' \
  '# Closing operation' \
  'Requirement title: Preserve accepted Meeting outcomes.' \
  'Work title: Implement the accepted Meeting follow-up.' \
  "Work assignee pubkey: ${moderator_pubkey}." \
  'No other participant has a follow-up action. Materialize exactly this operation, then close normally.' \
  >"${board_file}"

log "creating one action-capable Meeting"
buzz_as_moderator meetings create \
  --policy moderated-board-actions-v1 \
  --title 'Meeting V2 action finalization real-provider acceptance' \
  --description 'One bounded backend acceptance; exactly one Project View Requirement and Work.' \
  --board "${board_file}" \
  --participant "${participant_pubkey}" >"${run_dir}/logs/meeting-create.json"
meeting_id="$(jq -r '.meeting_id // empty' "${run_dir}/logs/meeting-create.json")"
[[ "${meeting_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "Meeting Create did not return a UUID"
printf 'scenario\tsession_id\nreal_provider_action\t%s\n' "${meeting_id}" \
  >"${run_dir}/meetings.tsv"

team_instructions="$(printf '%s\n' \
  '这是 Meeting V2 行动收口的一次性真实 Provider 验收。你是主持 Agent。' \
  '当前看板已经明确记录 Goal reached、Effective conclusion、一个 Requirement、一个 Work，以及该 Work 由你自己的精确 pubkey 承接。' \
  'Board Maintenance 必须保持这些事实；若无需修正则返回 UNCHANGED。' \
  '紧接着的 Floor Decision 必须选择 FINALIZE_ACTIONS，不能 CLOSE、IDLE 或 ABORT。' \
  'Action Finalization 必须只把最终看板记录的一个 Requirement 和一个 Work 转换为严格 Materialization Intent；assignee_pubkey 必须使用看板中的主持人 pubkey，不得给 Human 参会者分配行动。' \
  '不得自行发布 Meeting 或 Project View 事件，不得修改文件、Git 或看板之外的任何状态；由 Harness 编译计划并机械执行。')"

log "starting exactly one real Codex ACP moderator runtime"
env \
  CODEX_CONFIG='{"model_reasoning_effort":"max","features":{"multi_agent":false}}' \
  BUZZ_ACP_MODEL="${model}[max]" \
  BUZZ_ACP_AGENT_COMMAND="${codex_acp_bin}" \
  BUZZ_ACP_AGENT_ARGS="" \
  BUZZ_ACP_AGENTS=1 \
  BUZZ_ACP_LAZY_POOL=false \
  BUZZ_ACP_PERMISSION_MODE=bypass-permissions \
  BUZZ_ACP_IDLE_TIMEOUT=300 \
  BUZZ_ACP_MAX_TURN_DURATION=900 \
  BUZZ_ACP_MAX_TURNS_PER_SESSION=0 \
  BUZZ_ACP_MEETING_V1_AUTO_ACCEPT=true \
  BUZZ_ACP_MEETING_ACCEPTANCE_EVENTS_PATH="${run_dir}/acceptance-events.ndjson" \
  BUZZ_ACP_MEETING_V1_LEDGER_PATH="${secret_dir}/meeting-ledger.json" \
  BUZZ_ACP_NO_MEMORY=true \
  BUZZ_ACP_RESPOND_TO=anyone \
  BUZZ_ACP_SUBSCRIBE=mentions \
  BUZZ_ACP_CONTEXT_MESSAGE_LIMIT=12 \
  BUZZ_ACP_MULTIPLE_EVENT_HANDLING=steer \
  BUZZ_ACP_TEAM_INSTRUCTIONS="${team_instructions}" \
  BUZZ_PRIVATE_KEY="${moderator_private_key}" \
  BUZZ_RELAY_URL="${relay_url}" \
  RUST_LOG="buzz_acp=info,acp=info,pool=info" \
  target/release/buzz-acp >"${run_dir}/logs/moderator.log" 2>&1 &
agent_pid=$!
printf 'scenario\trole\tpid\tmodel\tlog_path\nreal_provider_action\tmoderator\t%s\t%s[max]\tlogs/moderator.log\n' \
  "${agent_pid}" "${model}" >"${run_dir}/processes.tsv"

log "waiting within the single ${scenario_timeout_seconds}s scenario budget"
deadline=$((SECONDS + scenario_timeout_seconds))
terminal_status=""
while (( SECONDS < deadline )); do
  if ! kill -0 "${agent_pid}" 2>/dev/null; then
    fail "the real ACP moderator exited before the Meeting reached terminal state"
  fi
  action_condition="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
    psql -U buzz -d "${database_name}" -qtA \
    -c "SELECT COALESCE(action_condition, '') FROM meeting_v2_action_runs
        WHERE session_id='${meeting_id}'::uuid ORDER BY created_at DESC LIMIT 1;")"
  if [[ "${action_condition}" == blocked ]]; then
    fail "the real-provider action run became blocked"
  fi
  terminal_status="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
    psql -U buzz -d "${database_name}" -qtA \
    -c "SELECT status FROM meeting_sessions WHERE session_id='${meeting_id}'::uuid;")"
  [[ "${terminal_status}" == ended ]] && break
  sleep 0.5
done
[[ "${terminal_status}" == ended ]] || fail "Meeting did not end within the bounded scenario budget"

stop_agent
[[ -s "${run_dir}/acceptance-events.ndjson" ]] || fail "ACP produced no acceptance observer evidence"

db_state="$(docker exec -e PGPASSWORD=buzz_dev "${postgres_container}" \
  psql -U buzz -d "${database_name}" -qtA -c "
    SELECT json_build_object(
      'meetingStatus', session_row.status,
      'terminalOutcome', session_row.terminal_outcome,
      'policy', session_row.floor_policy_version,
      'actionTerminalStatus', action_row.terminal_status,
      'actionPhase', action_row.action_phase,
      'actionCondition', action_row.action_condition,
      'completionProjectRevision', action_row.completion_project_revision,
      'itemCount', jsonb_array_length(action_row.plan_json->'items'),
      'stepCount', count(step_row.*),
      'appliedSteps', count(step_row.*) FILTER (WHERE step_row.status='applied'),
      'acceptedAttempts', (
        SELECT count(*) FROM meeting_v2_action_step_attempts attempt_row
        WHERE attempt_row.community_id=action_row.community_id
          AND attempt_row.session_id=action_row.session_id
          AND attempt_row.action_run_id=action_row.action_run_id
          AND attempt_row.status='accepted'
      ),
      'requirementObjects', count(object_row.*) FILTER (
        WHERE step_row.step_kind='project_view.create_requirement'
          AND object_row.object_type='requirement' AND object_row.deleted_at IS NULL
      ),
      'workObjects', count(object_row.*) FILTER (
        WHERE step_row.step_kind='project_view.create_work'
          AND object_row.object_type='work' AND object_row.deleted_at IS NULL
      ),
      'responsibilityMatches', count(object_row.*) FILTER (
        WHERE step_row.step_kind='project_view.set_work_responsibility'
          AND object_row.object_type='work'
          AND object_row.responsible_role_id='${role_id}'::uuid
      ),
      'workCommitments', (
        SELECT count(*) FROM project_work_commitments commitment
        WHERE commitment.community_id=action_row.community_id
          AND commitment.work_id IN (
            SELECT target_object_id FROM meeting_v2_action_steps
            WHERE community_id=action_row.community_id
              AND session_id=action_row.session_id
              AND action_run_id=action_row.action_run_id
              AND step_kind='project_view.create_work'
          )
      )
    )::text
    FROM meeting_sessions session_row
    JOIN meeting_v2_action_runs action_row
      ON action_row.community_id=session_row.community_id
     AND action_row.session_id=session_row.session_id
    JOIN meeting_v2_action_steps step_row
      ON step_row.community_id=action_row.community_id
     AND step_row.session_id=action_row.session_id
     AND step_row.action_run_id=action_row.action_run_id
    LEFT JOIN project_view_objects object_row
      ON object_row.community_id=step_row.community_id
     AND object_row.object_id=step_row.target_object_id
    WHERE session_row.session_id='${meeting_id}'::uuid
    GROUP BY session_row.status, session_row.terminal_outcome,
      session_row.floor_policy_version, action_row.community_id,
      action_row.session_id, action_row.action_run_id, action_row.terminal_status,
      action_row.action_phase, action_row.action_condition,
      action_row.completion_project_revision, action_row.plan_json;
  ")"
[[ -n "${db_state}" ]] || fail "could not read final Meeting action state"
jq -e '
  .meetingStatus == "ended"
  and .terminalOutcome == "closed"
  and .policy == "moderated-board-actions-v1"
  and .actionTerminalStatus == "completed_closed"
  and .actionPhase == "ready_to_close"
  and .actionCondition == "runnable"
  and (.completionProjectRevision > 0)
  and .itemCount == 1
  and .stepCount == 3
  and .appliedSteps == 3
  and .acceptedAttempts == 3
  and .requirementObjects == 1
  and .workObjects == 1
  and .responsibilityMatches == 1
  and .workCommitments == 0
' <<<"${db_state}" >/dev/null || fail "final database lifecycle invariants did not pass"
printf '%s\n' "${db_state}" | jq . >"${run_dir}/database-invariants.json"

observer_summary="$(jq -sc '
  [ .[] | select(.kind == "meeting_v2_continuity_bound") ] as $bindings
  | {
      boardTurns: ([.[] | select(.kind == "meeting_v2_board_turn_completed")] | length),
      finalizingFloors: ([.[] | select(
        .kind == "meeting_v2_floor_turn_completed"
        and .payload.action == "FINALIZE_ACTIONS"
      )] | length),
      actionTurns: ([.[] | select(
        .kind == "meeting_v1_turn_started"
        and .payload.turn_type == "action_finalization"
      )] | length),
      plansCompiled: ([.[] | select(
        .kind == "meeting_v2_action_plan_compiled"
        and .payload.item_count == 1
        and .payload.step_count == 3
      )] | length),
      formatRetries: ([.[] | select(.kind == "meeting_v2_action_format_retry")] | length),
      continuityLost: ([.[] | select(.kind == "meeting_v2_continuity_lost")] | length),
      continuityTuples: ($bindings
        | map([.payload.agent_index, .payload.acp_session_id])
        | unique),
      continuityPhases: ($bindings | map(.payload.phase) | unique | sort)
    }
' "${run_dir}/acceptance-events.ndjson")"
jq -e '
  .boardTurns >= 1
  and .finalizingFloors == 1
  and .actionTurns >= 1
  and .plansCompiled == 1
  and .formatRetries <= 1
  and .continuityLost == 0
  and (.continuityTuples | length) == 1
  and (.continuityTuples[0][0] == 0)
  and (.continuityTuples[0][1] | type == "string" and length > 0)
  and (.continuityPhases | index("final_control_cycle") != null)
  and (.continuityPhases | index("pending_action") != null)
  and (.continuityPhases | index("action") != null)
' <<<"${observer_summary}" >/dev/null || fail "observer continuity or semantic-turn evidence did not pass"
printf '%s\n' "${observer_summary}" | jq . >"${run_dir}/observer-invariants.json"

curl -fsS "http://127.0.0.1:${metrics_port}/metrics" >"${run_dir}/metrics.prom"
rg -q '^meeting_v2_action_command_total' "${run_dir}/metrics.prom" \
  || fail "action command metrics were not exposed"

git status --porcelain=v1 >"${run_dir}/workspace-after.status"
git diff --binary HEAD >"${secret_dir}/workspace-after.diff"
workspace_after_status_sha256="$(shasum -a 256 "${run_dir}/workspace-after.status" | awk '{print $1}')"
workspace_after_diff_sha256="$(shasum -a 256 "${secret_dir}/workspace-after.diff" | awk '{print $1}')"
printf '%s\n' "${workspace_after_status_sha256}" >"${run_dir}/workspace-after.status.sha256"
printf '%s\n' "${workspace_after_diff_sha256}" >"${run_dir}/workspace-after.diff.sha256"
[[ "${workspace_status_sha256}" == "${workspace_after_status_sha256}" \
  && "${workspace_diff_sha256}" == "${workspace_after_diff_sha256}" ]] \
  || fail "the real Agent changed the source workspace"

runtime_anomalies="$(
  (rg -n \
    'agent_returned — respawning|respawn_failed|agent_panic|unsupported_model|authentication failed|agent pool initialization failed' \
    "${run_dir}/logs/moderator.log" || true) \
    | wc -l | tr -d ' '
)"
[[ "${runtime_anomalies}" == 0 ]] || fail "the ACP runtime reported an anomaly"
rg -Fq "applied model ${model}[max]" "${run_dir}/logs/moderator.log" \
  || fail "the requested real model was not applied"

finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
jq -n \
  --arg run_id "${run_id}" \
  --arg started_at "${run_stamp}" \
  --arg finished_at "${finished_at}" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg status_sha "${workspace_status_sha256}" \
  --arg diff_sha "${workspace_diff_sha256}" \
  --arg model "${model}[max]" \
  --arg meeting_id "${meeting_id}" \
  --arg assignment_id "${assignment_id}" \
  --arg role_id "${role_id}" \
  --argjson database "${db_state}" \
  --argjson observer "${observer_summary}" '
  {
    evidenceSchema: "buzz-meeting-v2-actions-acceptance-v1",
    runId: $run_id,
    startedAt: $started_at,
    finishedAt: $finished_at,
    buzzCommit: $commit,
    sourceTree: {statusSha256: $status_sha, diffSha256: $diff_sha, unchanged: true},
    protocol: {
      schemaVersion: "3",
      policy: "moderated-board-actions-v1",
      capability: "meeting-v2-action-finalization-v1"
    },
    provider: {
      real: true,
      authenticated: true,
      adapter: "@agentclientprotocol/codex-acp",
      adapterVersion: "1.1.7",
      model: $model,
      agentProcesses: 1
    },
    scenario: {
      meetingId: $meeting_id,
      roster: {agents: 1, humans: 1},
      projectViewAssignmentId: $assignment_id,
      responsibleRoleId: $role_id
    },
    database: $database,
    observer: $observer,
    result: "PASS"
  }
' >"${run_dir}/manifest.json"

stop_relay
write_checksums

log "PASS: one bounded Meeting V2 action-finalization acceptance completed"
log "artifacts: ${run_dir}"
