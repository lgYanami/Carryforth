#!/usr/bin/env bash
# Run the Meeting V2 real-Codex backend qualification matrix.
#
# The runner uses four isolated V2 Meetings in one disposable Community:
#   mixed, all_agent, moderator_abort, and admin_abort.
# It never stores private keys in the evidence directory.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${repo_root}"
if [[ -f "${repo_root}/bin/activate-hermit" ]]; then
  # shellcheck disable=SC1091
  . "${repo_root}/bin/activate-hermit" >/dev/null
fi

artifact_root="${1:-${MEETING_V2_QUALIFICATION_ARTIFACT_ROOT:-${TMPDIR:-/tmp}/buzz-meeting-v2-qualification}}"
model="${MEETING_V2_QUALIFICATION_MODEL:-gpt-5.6-sol}"
relay_port="${MEETING_V2_QUALIFICATION_RELAY_PORT:-3300}"
health_port="${MEETING_V2_QUALIFICATION_HEALTH_PORT:-9301}"
metrics_port="${MEETING_V2_QUALIFICATION_METRICS_PORT:-9302}"
scenario_timeout_seconds="${MEETING_V2_QUALIFICATION_TIMEOUT_SECONDS:-2400}"
postgres_container="${MEETING_V2_QUALIFICATION_POSTGRES_CONTAINER:-buzz-postgres}"
redis_image="${MEETING_V2_QUALIFICATION_REDIS_IMAGE:-redis:7-alpine}"
keep_database="${MEETING_V2_QUALIFICATION_KEEP_DATABASE:-false}"
run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
run_id="meeting-v2-${run_stamp}-$$"
run_dir="${artifact_root}/${run_id}"
secret_dir="$(mktemp -d "${TMPDIR:-/tmp}/buzz-meeting-v2-secrets.XXXXXX")"
identity_file="${secret_dir}/identities.tsv"
database_name="buzz_meeting_v2_qualification_${run_stamp//[^0-9]/}_$$"
redis_container="buzz-meeting-v2-redis-${run_stamp//[^0-9]/}-$$"
relay_host="localhost:${relay_port}"
relay_url="ws://${relay_host}"
database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
redis_url=""
relay_pid=""
community_id=""
cleanup_done=false
database_created=false
qualification_passed=false
mixed_preempted=false
mixed_late_board_landed=false

mkdir -p \
  "${run_dir}/logs/agents" \
  "${run_dir}/logs/meetings" \
  "${run_dir}/logs/security" \
  "${run_dir}/preflight"
chmod 700 "${secret_dir}"
: >"${identity_file}"
chmod 600 "${identity_file}"

log() {
  printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"
}

fail() {
  log "FAIL: $*"
  printf '%s\n' "$*" >"${run_dir}/failure.txt"
  exit 1
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

  running_pids="$(jobs -pr || true)"
  for child_pid in ${running_pids}; do
    if kill -0 "${child_pid}" 2>/dev/null; then
      kill -TERM "${child_pid}" 2>/dev/null || true
    fi
  done
  for child_pid in ${running_pids}; do
    wait "${child_pid}" 2>/dev/null || true
  done
  stop_relay

  if [[ "${redis_container}" =~ ^buzz-meeting-v2-redis-[0-9]+-[0-9]+$ ]]; then
    docker rm -f "${redis_container}" >/dev/null 2>&1 || true
  fi
  case "${secret_dir}" in
    "${TMPDIR:-/tmp}"/buzz-meeting-v2-secrets.*)
      rm -rf -- "${secret_dir}"
      ;;
    *)
      echo "refusing to remove unexpected secret directory: ${secret_dir}" >&2
      ;;
  esac
}

trap 'exit_status=$?;
  if [[ "${exit_status}" -ne 0 && "${exit_status}" -ne 130 &&
    ! -f "${run_dir}/failure.txt" ]]; then
    printf "qualification runner exited unexpectedly with status %s\n" "${exit_status}" \
      >"${run_dir}/failure.txt"
  fi
  cleanup
  exit "${exit_status}"' EXIT
trap 'exit 130' INT TERM

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is missing: $1"
}

for required in awk codex curl docker git jq rg sed shasum ss; do
  require_command "${required}"
done
for numeric in "${relay_port}" "${health_port}" "${metrics_port}" "${scenario_timeout_seconds}"; do
  [[ "${numeric}" =~ ^[0-9]+$ ]] || fail "port and timeout values must be non-negative integers"
done
[[ "${relay_port}" -gt 0 && "${relay_port}" -le 65535 ]] || fail "invalid Relay port"
[[ "${health_port}" -gt 0 && "${health_port}" -le 65535 ]] || fail "invalid health port"
[[ "${metrics_port}" -gt 0 && "${metrics_port}" -le 65535 ]] || fail "invalid metrics port"
[[ "${relay_port}" != "${health_port}" && "${relay_port}" != "${metrics_port}" && "${health_port}" != "${metrics_port}" ]] \
  || fail "Relay, health, and metrics ports must be distinct"
[[ "${scenario_timeout_seconds}" -ge 60 ]] || fail "qualification timeout must be at least 60 seconds"
[[ "${database_name}" =~ ^[a-z0-9_]+$ ]] || fail "unsafe generated database name"

if ss -ltn | awk '{print $4}' | rg -q ":${relay_port}$"; then
  fail "Relay port ${relay_port} is already in use"
fi
if ss -ltn | awk '{print $4}' | rg -q ":${health_port}$"; then
  fail "health port ${health_port} is already in use"
fi
if ss -ltn | awk '{print $4}' | rg -q ":${metrics_port}$"; then
  fail "metrics port ${metrics_port} is already in use"
fi

for required_binary in \
  target/release/cf \
  target/release/buzz-acp \
  target/release/buzz-admin \
  target/release/buzz-relay; do
  [[ -x "${required_binary}" ]] || fail "missing release binary: ${required_binary}"
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
  candidates+=(
    "${HOME}/.local/share/Buzz/node-tools/bin/codex-acp"
    "${HOME}/.npm/_npx/4877722a062902ce/node_modules/.bin/codex-acp"
  )
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
  "@agentclientprotocol/codex-acp 1.1.7 is not installed in PATH or Buzz node-tools"
codex_acp_package="$(cd "$(dirname "${codex_acp_bin}")/.." && pwd)/package.json"
[[ -f "${codex_acp_package}" ]] || fail "could not locate codex-acp package.json"
jq -e '.name == "@agentclientprotocol/codex-acp" and .version == "1.1.7"' \
  "${codex_acp_package}" >/dev/null || fail "codex-acp package identity is not pinned to 1.1.7"

log "preflight: authenticated Codex, exact adapter, model catalog, and ACP capability"
codex --version >"${run_dir}/preflight/codex-version.txt"
codex login status >"${run_dir}/preflight/codex-login-status.txt" 2>&1
rg -q '^Logged in using (ChatGPT|an API key)$' \
  "${run_dir}/preflight/codex-login-status.txt" || fail "Codex is not authenticated"
"${codex_acp_bin}" --version >"${run_dir}/preflight/codex-acp-version.txt"
jq '{name, version, bin}' "${codex_acp_package}" \
  >"${run_dir}/preflight/codex-acp-package.json"
target/release/buzz-acp capabilities --json \
  >"${run_dir}/preflight/acp-capabilities.json"
jq -e '.meeting.qualificationEvidenceCompiled == true' \
  "${run_dir}/preflight/acp-capabilities.json" >/dev/null \
  || fail "buzz-acp was not built with --features meeting-acceptance"
BUZZ_ACP_AGENT_COMMAND="${codex_acp_bin}" \
  target/release/buzz-acp models --json \
  >"${run_dir}/preflight/codex-acp-models.json"
jq -e --arg high "${model}[high]" --arg max "${model}[max]" '
  ([.stable.configOptions[]?.options[]?.value, .unstable.availableModels[]?.modelId]
    | any(. == $high))
  and
  ([.stable.configOptions[]?.options[]?.value, .unstable.availableModels[]?.modelId]
    | any(. == $max))
' "${run_dir}/preflight/codex-acp-models.json" >/dev/null \
  || fail "codex-acp did not expose ${model}[high] and ${model}[max]"

git status --porcelain=v1 >"${run_dir}/workspace-before.status"
git diff --binary HEAD >"${secret_dir}/workspace-before.diff"
workspace_status_sha256="$(shasum -a 256 "${run_dir}/workspace-before.status" | awk '{print $1}')"
workspace_diff_sha256="$(shasum -a 256 "${secret_dir}/workspace-before.diff" | awk '{print $1}')"
printf '%s\n' "${workspace_status_sha256}" >"${run_dir}/workspace-before.status.sha256"
printf '%s\n' "${workspace_diff_sha256}" >"${run_dir}/workspace-before.diff.sha256"
shasum -a 256 \
  "$0" \
  target/release/cf \
  target/release/buzz-acp \
  target/release/buzz-admin \
  target/release/buzz-relay \
  "${codex_acp_bin}" \
  >"${run_dir}/preflight/executable-sha256.txt"

generate_identity() {
  local scenario="$1"
  local role="$2"
  local meeting_role="$3"
  local participant_type="$4"
  local output="${secret_dir}/${role}.key-output"
  local public_key
  local private_key
  target/release/buzz-admin generate-key >"${output}"
  public_key="$(awk '/^Public key:/ {print $3}' "${output}")"
  private_key="$(awk '/^Secret key:/ {print $3}' "${output}")"
  [[ "${public_key}" =~ ^[0-9a-f]{64}$ ]] || fail "invalid generated public key for ${role}"
  [[ "${private_key}" =~ ^[0-9a-f]{64}$ ]] || fail "invalid generated private key for ${role}"
  printf '%s\t%s\t%s\t%s\t%s\t%s\n' \
    "${scenario}" "${role}" "${meeting_role}" "${participant_type}" \
    "${public_key}" "${private_key}" >>"${identity_file}"
  rm -- "${output}"
}

identity_field() {
  local role="$1"
  local column="$2"
  awk -F '\t' -v wanted="${role}" -v column="${column}" \
    '$2 == wanted { print $column; exit }' "${identity_file}"
}
identity_public_key() { identity_field "$1" 5; }
identity_private_key() { identity_field "$1" 6; }

log "generating isolated identities"
generate_identity infrastructure supervisor none human
generate_identity infrastructure outsider none human
generate_identity mixed mixed-moderator moderator agent
generate_identity mixed mixed-agent participant agent
generate_identity mixed mixed-human-a participant human
generate_identity mixed mixed-human-b participant human
generate_identity all_agent all-moderator moderator agent
generate_identity all_agent all-agent-a participant agent
generate_identity all_agent all-agent-b participant agent
generate_identity moderator_abort abort-moderator moderator agent
generate_identity moderator_abort abort-agent participant agent
generate_identity admin_abort admin-moderator moderator agent
generate_identity admin_abort admin-human participant human

printf 'scenario\trole\tmeeting_role\tparticipant_type\tpubkey\n' >"${run_dir}/roster.tsv"
awk -F '\t' 'BEGIN { OFS="\t" } $1 != "infrastructure" { print $1, $2, $3, $4, $5 }' \
  "${identity_file}" >>"${run_dir}/roster.tsv"

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

docker exec "${postgres_container}" psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"${database_name}\"" >/dev/null
database_created=true
DATABASE_URL="${database_url}" target/release/buzz-admin migrate \
  >"${run_dir}/logs/migrate.log" 2>&1

start_relay() {
  local create_enabled="$1"
  local log_path="$2"
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL="${redis_url}" \
    BUZZ_BIND_ADDR="127.0.0.1:${relay_port}" \
    BUZZ_HEALTH_PORT="${health_port}" \
    BUZZ_METRICS_PORT="${metrics_port}" \
    RELAY_URL="${relay_url}" \
    BUZZ_AUTO_MIGRATE=false \
    BUZZ_MEETING_V1_CREATE_ENABLED=true \
    BUZZ_MEETING_V2_CREATE_ENABLED="${create_enabled}" \
    BUZZ_REQUIRE_RELAY_MEMBERSHIP=true \
    RELAY_OWNER_PUBKEY="$(identity_public_key supervisor)" \
    BUZZ_RELAY_PRIVATE_KEY="0000000000000000000000000000000000000000000000000000000000000001" \
    RUST_LOG="buzz_relay=info,buzz_db=info" \
    target/release/buzz-relay >"${log_path}" 2>&1 &
  relay_pid=$!

  local ready=false
  for ignored_attempt in $(seq 1 180); do
    if curl -fsS -H "Host: ${relay_host}" "http://127.0.0.1:${relay_port}/" \
      >/dev/null 2>&1; then
      ready=true
      break
    fi
    if ! kill -0 "${relay_pid}" 2>/dev/null; then
      fail "Relay exited during startup; see ${log_path}"
    fi
    sleep 0.5
  done
  [[ "${ready}" == true ]] || fail "Relay did not become ready within 90 seconds"
}

seed_identity() {
  local role="$1"
  local participant_type="$2"
  local public_key="$3"
  local owner_sql="NULL"
  if [[ "${participant_type}" == agent ]]; then
    owner_sql="decode('$(identity_public_key supervisor)', 'hex')"
  fi
  docker exec "${postgres_container}" psql -U buzz -d "${database_name}" \
    -v ON_ERROR_STOP=1 -c "
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES ('${community_id}'::uuid, '${public_key}', 'member')
      ON CONFLICT (community_id, pubkey)
      DO UPDATE SET
        role = CASE WHEN relay_members.role = 'owner' THEN relay_members.role ELSE EXCLUDED.role END,
        updated_at = clock_timestamp();

      INSERT INTO users (
        community_id, pubkey, display_name, agent_type,
        agent_owner_pubkey, channel_add_policy
      )
      VALUES (
        '${community_id}'::uuid,
        decode('${public_key}', 'hex'),
        'Meeting V2 qualification ${role}',
        $(if [[ "${participant_type}" == agent ]]; then printf "'codex'"; else printf 'NULL'; fi),
        ${owner_sql},
        'anyone'
      )
      ON CONFLICT (community_id, pubkey)
      DO UPDATE SET
        display_name = EXCLUDED.display_name,
        agent_type = EXCLUDED.agent_type,
        agent_owner_pubkey = EXCLUDED.agent_owner_pubkey,
        channel_add_policy = 'anyone',
        deactivated_at = NULL;
    " >/dev/null
}

log "starting create-enabled Relay and seeding the disposable Community"
start_relay true "${run_dir}/logs/relay-create-enabled.log"
community_id="$(
  docker exec "${postgres_container}" psql -U buzz -d "${database_name}" -qtA \
    -c "SELECT id FROM communities WHERE lower(host)=lower('${relay_host}');"
)"
[[ "${community_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "Relay did not provision ${relay_host}"
while IFS=$'\t' read -r scenario role meeting_role participant_type public_key private_key; do
  seed_identity "${role}" "${participant_type}" "${public_key}"
done <"${identity_file}"

curl -fsS -H 'Accept: application/nostr+json' -H "Host: ${relay_host}" \
  "http://127.0.0.1:${relay_port}/" >"${run_dir}/preflight/relay-create-enabled.json"
jq -e '
  (.supported_extensions | index("buzz-meeting-v2") != null)
  and (.supported_extensions | index("buzz-meeting-v2-create") != null)
' "${run_dir}/preflight/relay-create-enabled.json" >/dev/null \
  || fail "create-enabled Relay did not advertise both V2 extensions"

cf_as() {
  local role="$1"
  shift
  CARRYFORTH_RELAY_URL="${relay_url}" \
    CARRYFORTH_PRIVATE_KEY="$(identity_private_key "${role}")" \
    target/release/cf --format compact "$@"
}

db_scalar() {
  local sql="$1"
  docker exec "${postgres_container}" psql -U buzz -d "${database_name}" -qtA -c "${sql}"
}

initial_board() {
  local scenario="$1"
  case "${scenario}" in
    mixed)
      printf '%s\n' \
        '# Goal' \
        'Complete a bounded mixed Human/Agent Meeting V2 qualification with an effective conclusion.' \
        '' \
        '# Agenda' \
        '1. Observe a Human request preempting a moderator Board window.' \
        '2. Resolve one directed handoff to the participant Agent.' \
        '3. Hear the moderator as an ordinary granted speaker.' \
        '4. Record the evidence-backed conclusion and close normally.' \
        '' \
        '# Progress' \
        'Waiting for both Humans and both Agents. Do not close before every agenda item is evidenced.'
      ;;
    all_agent)
      printf '%s\n' \
        '# Goal' \
        'Complete a bounded all-Agent Meeting V2 qualification with an effective conclusion.' \
        '' \
        '# Agenda' \
        '1. Participant A contributes and directs a handoff to Participant B.' \
        '2. Participant B answers the handoff.' \
        '3. The moderator speaks once through self Intent, Offer, Grant, and Speech.' \
        '4. Maintain the Board at least twice, record the final conclusion, and close normally.' \
        '' \
        '# Progress' \
        'No required contribution has completed.'
      ;;
    moderator_abort)
      printf '%s\n' \
        '# Goal' \
        'Determine whether a conclusion can be formed without the required evidence.' \
        '' \
        '# Blocking fact' \
        'The required evidence does not exist in this isolated qualification scenario.' \
        '' \
        '# Required outcome' \
        'The moderator must explicitly abort with unable_to_form_conclusion; normal close would be false.'
      ;;
    admin_abort)
      printf '%s\n' \
        '# Goal' \
        'Remain active until the qualification operator exercises the security revocation path.' \
        '' \
        '# Progress' \
        'Wait. Do not close or abort from the moderator model.'
      ;;
  esac
}

create_meeting() {
  local scenario="$1"
  local moderator="$2"
  shift 2
  local output="${run_dir}/logs/meetings/${scenario}-create.json"
  local -a args=(
    meetings create
    --policy moderated-board-v1
    --title "Meeting V2 ${scenario} real-provider qualification"
    --description "Disposable backend qualification; discussion only, no Project View or external effects."
    --board -
  )
  local participant
  for participant in "$@"; do
    args+=(--participant "$(identity_public_key "${participant}")")
  done
  initial_board "${scenario}" | cf_as "${moderator}" "${args[@]}" >"${output}"
  local session_id
  session_id="$(jq -r '.meeting_id // empty' "${output}")"
  [[ "${session_id}" =~ ^[0-9a-f-]{36}$ ]] || fail "${scenario} Create did not return a UUID"
  printf '%s\t%s\n' "${scenario}" "${session_id}" >>"${run_dir}/meetings.tsv"
}

printf 'scenario\tsession_id\n' >"${run_dir}/meetings.tsv"
log "creating the exact four-scenario V2 matrix"
create_meeting mixed mixed-moderator mixed-agent mixed-human-a mixed-human-b
create_meeting all_agent all-moderator all-agent-a all-agent-b
create_meeting moderator_abort abort-moderator abort-agent
create_meeting admin_abort admin-moderator admin-human

meeting_id_for() {
  awk -F '\t' -v scenario="$1" '$1 == scenario { print $2; exit }' "${run_dir}/meetings.tsv"
}

mixed_session="$(meeting_id_for mixed)"
all_agent_session="$(meeting_id_for all_agent)"
moderator_abort_session="$(meeting_id_for moderator_abort)"
admin_abort_session="$(meeting_id_for admin_abort)"

set +e
cf_as outsider meetings board get --meeting "${mixed_session}" \
  >"${run_dir}/logs/security/outsider-read.log" 2>&1
outsider_read_status=$?
cf_as outsider meetings board unchanged --meeting "${mixed_session}" \
  >"${run_dir}/logs/security/outsider-write.log" 2>&1
outsider_write_status=$?
set -e
[[ "${outsider_read_status}" -ne 0 ]] || fail "non-participant read unexpectedly succeeded"
[[ "${outsider_write_status}" -ne 0 ]] || fail "non-participant Board write unexpectedly succeeded"

log "restarting Relay with V2 Create disabled while four V2 Sessions remain active"
stop_relay
start_relay false "${run_dir}/logs/relay.log"
curl -fsS -H 'Accept: application/nostr+json' -H "Host: ${relay_host}" \
  "http://127.0.0.1:${relay_port}/" >"${run_dir}/preflight/relay-create-disabled.json"
jq -e '
  (.supported_extensions | index("buzz-meeting-v2") != null)
  and (.supported_extensions | index("buzz-meeting-v2-create") == null)
' "${run_dir}/preflight/relay-create-disabled.json" >/dev/null \
  || fail "create-disabled Relay did not retain runtime-only V2 capability"
curl -fsS -H "Host: ${relay_host}" "http://127.0.0.1:${relay_port}/_readiness" \
  >"${run_dir}/preflight/relay-create-disabled-readiness.json"

set +e
initial_board mixed | cf_as mixed-moderator meetings create \
  --policy moderated-board-v1 \
  --title 'Create-disabled rejection probe' \
  --board - \
  --participant "$(identity_public_key mixed-agent)" \
  >"${run_dir}/logs/security/create-disabled.log" 2>&1
create_disabled_status=$?
set -e
[[ "${create_disabled_status}" -ne 0 ]] || fail "V2 Create succeeded after the Create gate was disabled"

team_instructions() {
  local scenario="$1"
  local role="$2"
  case "${scenario}:${role}" in
    mixed:mixed-moderator)
      printf '%s\n' \
        '这是 Meeting V2 mixed 真实 Provider 验收。你是主持 Agent。' \
        '先提交一个主持人 self Intent，并在后续通过正常 Offer/Grant 至少发言一次；不得直接发布协议事件。' \
        '等待两名 Human 介入；每次 Control Token 返回后归纳进度并更新看板。至少形成两次被 Relay 接受的 Board UPDATE。' \
        '获得主持人 self Grant 时，Speech 必须明确写出：MIXED_QUALIFICATION_COMPLETE；并说明 Human 抢占、directed handoff 回答、两名 Human、普通 Agent 与主持人发言均已完成。' \
        '每次 Board Maintenance 都以 recent_shared_conversation 为权威事实对账；若其中已有 MIXED_QUALIFICATION_COMPLETE，不得让旧 Board 继续声称主持人尚未发言，必须 UPDATE 完整 Board，把 Goal 标为 reached 并写入 Effective conclusion。' \
        '上述完成 Board 被 Relay 接受后，下一个无候选 Floor Decision 必须 CLOSE，不得继续 IDLE；条件未满足前才可 IDLE 或选择有效候选。' \
        '只讨论本场协议证据，不修改文件、Git、Project View、任务或任何外部系统。'
      ;;
    mixed:mixed-agent)
      printf '%s\n' \
        '这是 Meeting V2 mixed 真实 Provider 验收。你是普通参会 Agent。' \
        '首次 Intent Turn 提交一个简短且具体的贡献；获得 Grant 或 directed handoff 后给出有界回答。' \
        '不得调用 Buzz 命令，不修改文件、Git、Project View、任务或任何外部系统。'
      ;;
    all_agent:all-moderator)
      printf '%s\n' \
        '这是 Meeting V2 all-agent 真实 Provider 验收。你是主持 Agent。' \
        '先提交一个 moderator self Intent，但依次让 Participant A、Participant B 和你自己都通过正常 Grant 发言。' \
        'Participant A 到 B 的 directed handoff 必须被回答。Control 返回后持续归纳，至少提交两次被接受的 Board UPDATE。' \
        '获得 moderator self Grant 时，Speech 必须明确写出：ALL_AGENT_QUALIFICATION_COMPLETE；并说明 A 的 handoff、B 的回答、三名 Agent 发言和至少两次 Board Maintenance 均已完成。' \
        '每次 Board Maintenance 都以 recent_shared_conversation 为权威事实对账；若其中已有 ALL_AGENT_QUALIFICATION_COMPLETE，不得让旧 Board 继续声称主持人尚未发言，必须 UPDATE 完整 Board，把 Goal 明确标为 reached并写入 Effective conclusion。' \
        '上述完成 Board 被 Relay 接受后，下一个无候选 Floor Decision 必须 CLOSE，不得继续 IDLE。' \
        '只讨论本场协议证据，不修改文件、Git、Project View、任务或任何外部系统。'
      ;;
    all_agent:all-agent-a)
      printf '%s\n' \
        '这是 Meeting V2 all-agent 真实 Provider 验收。首次 Intent Turn 必须 SUBMIT 一个具体贡献。' \
        "获得 Grant 后 SAY，并把 directed handoff 指向 Participant B 公钥 $(identity_public_key all-agent-b)，要求其补充独立证据。" \
        '不得执行任务或修改任何持久状态。'
      ;;
    all_agent:all-agent-b)
      printf '%s\n' \
        '这是 Meeting V2 all-agent 真实 Provider 验收。首次 Intent Turn 提交互补贡献。' \
        '获得 directed handoff 的 Grant 后明确回答 A 的问题，保持发言有界，不再创建循环 handoff。不得执行任务或修改任何持久状态。'
      ;;
    moderator_abort:abort-moderator)
      printf '%s\n' \
        '这是 Meeting V2 moderator-abort 真实 Provider 验收。你是主持 Agent。' \
        '完成一次 Board Maintenance 后，不选择 speaker、不正常 close；在 Floor Decision 返回 ABORT，reason_code 必须是 unable_to_form_conclusion。' \
        '不得发布协议事件或修改任何外部状态。'
      ;;
    moderator_abort:abort-agent)
      printf '%s\n' \
        '这是 Meeting V2 moderator-abort 真实 Provider 验收。Intent Turn 返回 PASS；不要制造可形成结论的新证据。不得修改任何持久状态。'
      ;;
    admin_abort:admin-moderator)
      printf '%s\n' \
        '这是 Meeting V2 admin/security-abort 真实 Provider 验收。你是主持 Agent。' \
        '完成语义 Turn 以证明真实 Provider 已连接，但保持 Board UNCHANGED 且 Floor IDLE；不要自行 close 或 abort，等待 Relay 安全撤权。不得修改任何持久状态。'
      ;;
  esac
}

start_agent() {
  local scenario="$1"
  local role="$2"
  local effort=high
  local log_path="logs/agents/${role}.log"
  if [[ "$(identity_field "${role}" 3)" == moderator ]]; then
    effort=max
  fi
  env \
    CODEX_CONFIG="{\"model_reasoning_effort\":\"${effort}\",\"features\":{\"multi_agent\":false}}" \
    BUZZ_ACP_MODEL="${model}[${effort}]" \
    BUZZ_ACP_AGENT_COMMAND="${codex_acp_bin}" \
    BUZZ_ACP_AGENT_ARGS="" \
    BUZZ_ACP_AGENTS=1 \
    BUZZ_ACP_LAZY_POOL=false \
    BUZZ_ACP_PERMISSION_MODE="bypass-permissions" \
    BUZZ_ACP_IDLE_TIMEOUT=620 \
    BUZZ_ACP_MAX_TURN_DURATION=7200 \
    BUZZ_ACP_MAX_TURNS_PER_SESSION=0 \
    BUZZ_ACP_MEETING_V1_AUTO_ACCEPT=true \
    BUZZ_ACP_MEETING_ACCEPTANCE_EVENTS_PATH="${run_dir}/logs/agents/${role}-events.ndjson" \
    BUZZ_ACP_MEETING_V1_LEDGER_PATH="${secret_dir}/${role}-ledger.json" \
    BUZZ_ACP_NO_MEMORY=true \
    BUZZ_ACP_RESPOND_TO=anyone \
    BUZZ_ACP_SUBSCRIBE=mentions \
    BUZZ_ACP_CONTEXT_MESSAGE_LIMIT=12 \
    BUZZ_ACP_MULTIPLE_EVENT_HANDLING=steer \
    BUZZ_ACP_TEAM_INSTRUCTIONS="$(team_instructions "${scenario}" "${role}")" \
    BUZZ_PRIVATE_KEY="$(identity_private_key "${role}")" \
    BUZZ_RELAY_URL="${relay_url}" \
    RUST_LOG="buzz_acp=info,acp=info,pool=info" \
    target/release/buzz-acp >"${run_dir}/${log_path}" 2>&1 &
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "${scenario}" "${role}" "$!" "${effort}" "${log_path}" >>"${run_dir}/processes.tsv"
}

printf 'scenario\trole\tpid\teffort\tlog_path\n' >"${run_dir}/processes.tsv"
log "starting eight isolated real Codex ACP runtimes"
start_agent mixed mixed-moderator
start_agent mixed mixed-agent
start_agent all_agent all-moderator
start_agent all_agent all-agent-a
start_agent all_agent all-agent-b
start_agent moderator_abort abort-moderator
start_agent moderator_abort abort-agent
start_agent admin_abort admin-moderator

event_count() {
  local role="$1"
  local kind="$2"
  local turn_type="${3:-}"
  local event_path="${run_dir}/logs/agents/${role}-events.ndjson"
  if [[ ! -s "${event_path}" ]]; then
    printf '0\n'
    return
  fi
  jq -sc --arg kind "${kind}" --arg turn_type "${turn_type}" '
    [.[]
      | select(.kind == $kind)
      | select($turn_type == "" or .payload.turn_type == $turn_type)]
    | length
  ' "${event_path}" 2>/dev/null || printf '0\n'
}

wait_for_mixed_board_window() {
  local deadline=$((SECONDS + scenario_timeout_seconds))
  local load_count
  local phase
  while (( SECONDS < deadline )); do
    load_count="$(event_count mixed-moderator meeting_v2_board_load_completed moderator_board)"
    phase="$(db_scalar "
      SELECT runtime_phase FROM meeting_v2_bootstrap_state
      WHERE session_id='${mixed_session}';
    ")"
    if [[ "${load_count}" -ge 1 && "${phase}" == board_pending ]]; then
      return
    fi
    sleep 0.05
  done
  fail "mixed scenario did not expose an active provider-backed Board window"
}

submit_human_request() {
  local role="$1"
  local session_id="$2"
  local label="$3"
  cf_as "${role}" meetings floor request --meeting "${session_id}" \
    >"${run_dir}/logs/meetings/${label}-request.json"
  LAST_REQUEST_ID="$(jq -r '.request_id // empty' "${run_dir}/logs/meetings/${label}-request.json")"
  [[ "${LAST_REQUEST_ID}" =~ ^[0-9a-f]{64}$ ]] || fail "${label} did not return a request ID"
}

complete_human_request() {
  local role="$1"
  local session_id="$2"
  local label="$3"
  local content="$4"
  local handoff_target="${5:-}"
  local offer_id=""
  local state=""
  local request_row=""
  local deadline=$((SECONDS + scenario_timeout_seconds))
  while (( SECONDS < deadline )); do
    request_row="$(db_scalar "
      SELECT state, COALESCE(encode(offer_id, 'hex'), '')
      FROM meeting_human_floor_requests
      WHERE request_id=decode('${LAST_REQUEST_ID}', 'hex');
    ")"
    state="${request_row%%|*}"
    offer_id="${request_row#*|}"
    if [[ "${state}" == offered && "${offer_id}" =~ ^[0-9a-f]{64}$ ]]; then
      break
    fi
    [[ "${state}" == queued ]] || fail "${label} Human Request became ${state:-missing} before ACK"
    sleep 0.1
  done
  [[ "${offer_id}" =~ ^[0-9a-f]{64}$ ]] || fail "${label} Human Offer timed out"
  cf_as "${role}" meetings offer ack --meeting "${session_id}" --offer "${offer_id}" \
    >"${run_dir}/logs/meetings/${label}-ack.json"
  local -a say_args=(meetings say --meeting "${session_id}" --content "${content}")
  if [[ -n "${handoff_target}" ]]; then
    say_args+=(
      --handoff-to "${handoff_target}"
      --handoff-type review
      --handoff-reason 'Please answer this qualification question with one independent protocol observation.'
    )
  fi
  cf_as "${role}" "${say_args[@]}" >"${run_dir}/logs/meetings/${label}-say.json"
  LAST_SPEECH_ID="$(jq -r '.speech_event_id // empty' "${run_dir}/logs/meetings/${label}-say.json")"
  [[ "${LAST_SPEECH_ID}" =~ ^[0-9a-f]{64}$ ]] || fail "${label} did not return a speech ID"
}

wait_handoff_answered() {
  local handoff_id="$1"
  local deadline=$((SECONDS + scenario_timeout_seconds))
  local state=""
  while (( SECONDS < deadline )); do
    state="$(db_scalar "
      SELECT question_state FROM meeting_directed_handoffs
      WHERE handoff_id=decode('${handoff_id}', 'hex');
    ")"
    [[ "${state}" == answered ]] && return
    [[ -z "${state}" || "${state}" == open ]] \
      || fail "directed handoff became ${state} instead of answered"
    sleep 0.25
  done
  fail "directed handoff was not answered before the qualification timeout"
}

run_mixed_driver() {
  wait_for_mixed_board_window
  local board_before
  local board_after
  local board_outcome
  board_before="$(db_scalar "
    SELECT encode(board_event_id, 'hex') FROM meeting_current_boards
    WHERE session_id='${mixed_session}';
  ")"
  submit_human_request mixed-human-a "${mixed_session}" mixed-preemption
  board_outcome="$(db_scalar "
    SELECT board_outcome FROM meeting_v2_bootstrap_state
    WHERE session_id='${mixed_session}';
  ")"
  [[ "${board_outcome}" == preempted ]] || fail "Human Request did not preempt the active Board window"
  mixed_preempted=true

  local deadline=$((SECONDS + 60))
  while (( SECONDS < deadline )); do
    if [[ "$(event_count mixed-moderator meeting_v2_host_turn_discarded)" -ge 1 ]]; then
      break
    fi
    sleep 0.05
  done
  [[ "$(event_count mixed-moderator meeting_v2_host_turn_discarded)" -ge 1 ]] \
    || fail "ACP did not discard the preempted moderator host Turn"
  sleep 1
  board_after="$(db_scalar "
    SELECT encode(board_event_id, 'hex') FROM meeting_current_boards
    WHERE session_id='${mixed_session}';
  ")"
  if [[ "${board_after}" != "${board_before}" ]]; then
    mixed_late_board_landed=true
    fail "a preempted Board result changed the current Board"
  fi

  complete_human_request \
    mixed-human-a \
    "${mixed_session}" \
    mixed-preemption \
    'Human A confirms that Board work and Floor transfer have independent authority. Participant Agent: identify one additional observable invariant.' \
    "$(identity_public_key mixed-agent)"
  local handoff_id="${LAST_SPEECH_ID}"
  wait_handoff_answered "${handoff_id}"

  submit_human_request mixed-human-b "${mixed_session}" mixed-human-b
  complete_human_request \
    mixed-human-b \
    "${mixed_session}" \
    mixed-human-b \
    'Human B confirms the directed handoff completed and asks the moderator to record the effective conclusion after its own granted Speech.'
}

log "driving the mixed Human/Agent preemption and directed-handoff path"
run_mixed_driver >"${run_dir}/logs/meetings/mixed-driver.log" 2>&1

log "waiting for every ACP pool and real model session"
all_agents_exercised=false
for ignored_attempt in $(seq 1 3600); do
  ready=0
  exercised=0
  total=0
  while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
    [[ "${scenario}" == scenario ]] && continue
    total=$((total + 1))
    if rg -q 'agent_pool_ready agents=1' "${run_dir}/${log_path}" 2>/dev/null; then
      ready=$((ready + 1))
    elif ! kill -0 "${agent_pid}" 2>/dev/null; then
      fail "${role} exited before its ACP pool became ready"
    fi
    if rg -Fq "applied model ${model}[${effort}]" "${run_dir}/${log_path}" 2>/dev/null; then
      exercised=$((exercised + 1))
    fi
  done <"${run_dir}/processes.tsv"
  if [[ "${ready}" -eq "${total}" && "${exercised}" -eq "${total}" ]]; then
    all_agents_exercised=true
    break
  fi
  sleep 0.5
done
[[ "${all_agents_exercised}" == true ]] \
  || fail "not every real ACP Agent applied its requested model within 30 minutes"
log "all eight Agent identities exercised ${model} through codex-acp 1.1.7"

log "triggering the independent admin/security abort"
cf_as supervisor moderation ban \
  --pubkey "$(identity_public_key admin-human)" \
  --reason 'Meeting V2 qualification security revocation' \
  >"${run_dir}/logs/security/admin-ban.json"

log "waiting for all four V2 Sessions to reach their required terminal outcome"
deadline=$((SECONDS + scenario_timeout_seconds))
while (( SECONDS < deadline )); do
  terminal_count="$(db_scalar "
    SELECT count(*) FROM meeting_sessions
    WHERE session_id IN (
      '${mixed_session}', '${all_agent_session}',
      '${moderator_abort_session}', '${admin_abort_session}'
    ) AND status='ended';
  ")"
  [[ "${terminal_count}" -eq 4 ]] && break
  sleep 0.5
done
[[ "${terminal_count:-0}" -eq 4 ]] || fail "not all qualification Meetings reached an End"

mixed_outcome="$(db_scalar "SELECT terminal_outcome FROM meeting_sessions WHERE session_id='${mixed_session}';")"
all_agent_outcome="$(db_scalar "SELECT terminal_outcome FROM meeting_sessions WHERE session_id='${all_agent_session}';")"
moderator_abort_outcome="$(db_scalar "SELECT terminal_outcome FROM meeting_sessions WHERE session_id='${moderator_abort_session}';")"
admin_abort_reason="$(db_scalar "SELECT terminal_reason_code FROM meeting_sessions WHERE session_id='${admin_abort_session}';")"
[[ "${mixed_outcome}" == closed ]] || fail "mixed scenario ended as ${mixed_outcome}, not closed"
[[ "${all_agent_outcome}" == closed ]] || fail "all-agent scenario ended as ${all_agent_outcome}, not closed"
[[ "${moderator_abort_outcome}" == aborted ]] || fail "moderator abort scenario did not abort"
[[ "${admin_abort_reason}" == participant_revoked ]] || fail "security scenario did not end by participant_revoked"

post_end_before="$(db_scalar "
  SELECT string_agg(session_id::text || ':' || state_revision || ':' || speech_revision, ',' ORDER BY session_id)
  FROM meeting_baton_state
  WHERE session_id IN (
    '${mixed_session}', '${all_agent_session}',
    '${moderator_abort_session}', '${admin_abort_session}'
  );
")"
sleep 2
post_end_after="$(db_scalar "
  SELECT string_agg(session_id::text || ':' || state_revision || ':' || speech_revision, ',' ORDER BY session_id)
  FROM meeting_baton_state
  WHERE session_id IN (
    '${mixed_session}', '${all_agent_session}',
    '${moderator_abort_session}', '${admin_abort_session}'
  );
")"
post_end_revision_change=0
[[ "${post_end_before}" == "${post_end_after}" ]] || post_end_revision_change=1

set +e
cf_as mixed-moderator meetings board unchanged --meeting "${mixed_session}" \
  >"${run_dir}/logs/security/post-end-write.log" 2>&1
post_end_write_status=$?
set -e
[[ "${post_end_write_status}" -ne 0 ]] || fail "post-End Board mutation unexpectedly succeeded"

log "stopping ACP runtimes and freezing privacy-filtered observer evidence"
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  kill -TERM "${agent_pid}" 2>/dev/null || true
done <"${run_dir}/processes.tsv"
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  wait "${agent_pid}" 2>/dev/null || true
done <"${run_dir}/processes.tsv"

: >"${run_dir}/acceptance-events.ndjson"
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  event_path="${run_dir}/logs/agents/${role}-events.ndjson"
  [[ -s "${event_path}" ]] || fail "${role} produced no acceptance observer evidence"
  jq -c --arg scenario "${scenario}" --arg role "${role}" \
    '. + {qualificationScenario: $scenario, acceptanceRole: $role}' \
    "${event_path}" >>"${run_dir}/acceptance-events.ndjson"
done <"${run_dir}/processes.tsv"

event_count_combined() {
  local scenario="$1"
  local role="$2"
  local kind="$3"
  local turn_type="${4:-}"
  jq -sc \
    --arg scenario "${scenario}" \
    --arg role "${role}" \
    --arg kind "${kind}" \
    --arg turn_type "${turn_type}" '
      [.[]
        | select(.qualificationScenario == $scenario and .acceptanceRole == $role)
        | select(.kind == $kind)
        | select($turn_type == "" or .payload.turn_type == $turn_type)]
      | length
    ' "${run_dir}/acceptance-events.ndjson"
}

board_changed_between_intent_and_grant() {
  local scenario="$1"
  jq -se --arg scenario "${scenario}" '
    [.[]
      | select(
          .qualificationScenario == $scenario
          and .kind == "meeting_v2_board_load_completed"
          and .payload.turn_type == "participant_intent"
        )] as $intents
    | [.[]
        | select(
            .qualificationScenario == $scenario
            and .kind == "meeting_v2_board_load_completed"
            and .payload.turn_type == "granted_speech"
          )] as $grants
    | any($intents[];
        . as $intent
        | any($grants[];
            .acceptanceRole == $intent.acceptanceRole
            and .timestamp > $intent.timestamp
            and .payload.board_event_id != $intent.payload.board_event_id))
  ' "${run_dir}/acceptance-events.ndjson" >/dev/null
}

missing_board_reads=0
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  intent_turns="$(event_count_combined "${scenario}" "${role}" meeting_v1_intent_completed)"
  intent_reads="$(event_count_combined "${scenario}" "${role}" meeting_v2_board_load_completed participant_intent)"
  speech_turns="$(event_count_combined "${scenario}" "${role}" meeting_v1_speech_submitted)"
  speech_reads="$(event_count_combined "${scenario}" "${role}" meeting_v2_board_load_completed granted_speech)"
  (( intent_turns > intent_reads )) && missing_board_reads=$((missing_board_reads + intent_turns - intent_reads))
  (( speech_turns > speech_reads )) && missing_board_reads=$((missing_board_reads + speech_turns - speech_reads))
  if [[ "$(identity_field "${role}" 3)" == moderator ]]; then
    board_turns="$(event_count_combined "${scenario}" "${role}" meeting_v2_board_turn_completed)"
    board_reads="$(event_count_combined "${scenario}" "${role}" meeting_v2_board_load_completed moderator_board)"
    floor_v2="$(event_count_combined "${scenario}" "${role}" meeting_v2_floor_turn_completed)"
    floor_candidates="$(event_count_combined "${scenario}" "${role}" meeting_v1_moderator_decision_completed)"
    floor_reads="$(event_count_combined "${scenario}" "${role}" meeting_v2_board_load_completed moderator_floor)"
    floor_turns=$((floor_v2 + floor_candidates))
    (( board_turns > board_reads )) && missing_board_reads=$((missing_board_reads + board_turns - board_reads))
    (( floor_turns > floor_reads )) && missing_board_reads=$((missing_board_reads + floor_turns - floor_reads))
  fi
done <"${run_dir}/processes.tsv"

board_floor_sequence_violations="$(jq -s '
  sort_by(.timestamp) as $events
  | [
      range(0; ($events | length)) as $floor_index
      | $events[$floor_index] as $floor
      | select(
          $floor.kind == "meeting_v2_floor_turn_queued"
          or $floor.kind == "meeting_v1_moderator_decision_started"
        )
      | ([
          range(0; $floor_index) as $board_index
          | select(
              $events[$board_index].qualificationScenario == $floor.qualificationScenario
              and $events[$board_index].acceptanceRole == $floor.acceptanceRole
              and $events[$board_index].kind == "meeting_v2_board_turn_queued"
            )
          | $board_index
        ] | last // -1) as $board_start
      | select($board_start >= 0)
      | ([
          range(($board_start + 1); $floor_index) as $terminal_index
          | $events[$terminal_index]
          | select(
              .qualificationScenario == $floor.qualificationScenario
              and .acceptanceRole == $floor.acceptanceRole
              and (
                .kind == "meeting_v2_board_turn_completed"
                or .kind == "meeting_v2_host_turn_discarded"
              )
            )
        ] | length) as $terminal_count
      | select($terminal_count == 0)
    ]
  | length
' "${run_dir}/acceptance-events.ndjson")"

board_accepted_during_offer_or_grant="$(db_scalar "
  SELECT count(*)
  FROM meeting_v2_board_command_receipts receipt
  WHERE receipt.accepted
    AND (
      EXISTS (
        SELECT 1 FROM meeting_baton_offers offer_row
        WHERE offer_row.community_id=receipt.community_id
          AND offer_row.session_id=receipt.session_id
          AND offer_row.created_at <= receipt.recorded_at
          AND COALESCE(offer_row.resolved_at, 'infinity') >= receipt.recorded_at
      )
      OR EXISTS (
        SELECT 1 FROM meeting_baton_grants grant_row
        WHERE grant_row.community_id=receipt.community_id
          AND grant_row.session_id=receipt.session_id
          AND grant_row.created_at <= receipt.recorded_at
          AND COALESCE(grant_row.terminal_at, 'infinity') >= receipt.recorded_at
      )
    );
")"
board_changed_speech_revision="$(db_scalar "
  WITH history AS (
    SELECT
      transition_primary_type,
      speech_revision,
      lag(speech_revision) OVER (PARTITION BY community_id, session_id ORDER BY state_revision) AS previous_speech_revision
    FROM meeting_baton_state_history
    WHERE session_id IN (
      '${mixed_session}', '${all_agent_session}',
      '${moderator_abort_session}', '${admin_abort_session}'
    )
  )
  SELECT count(*) FROM history
  WHERE transition_primary_type IN ('board_updated', 'board_unchanged', 'board_timed_out')
    AND previous_speech_revision IS NOT NULL
    AND speech_revision <> previous_speech_revision;
")"
pending_runtime_reservations="$(db_scalar "
  SELECT
    (SELECT count(*) FROM meeting_baton_offers WHERE state='pending') +
    (SELECT count(*) FROM meeting_baton_grants WHERE state='active') +
    (SELECT count(*) FROM meeting_moderator_decision_attempts WHERE state='running') +
    (SELECT count(*) FROM meeting_event_outbox WHERE delivered_at IS NULL OR last_error IS NOT NULL) +
    (SELECT count(*) FROM meeting_v2_bootstrap_state WHERE runtime_phase <> 'ended');
")"

scenario_summary() {
  local scenario="$1"
  local session_id="$2"
  local moderator_role="$3"
  local floor_decisions
  local board_changed=false
  floor_decisions=$((
    $(event_count_combined "${scenario}" "${moderator_role}" meeting_v2_floor_turn_completed) +
    $(event_count_combined "${scenario}" "${moderator_role}" meeting_v1_moderator_decision_completed)
  ))
  if board_changed_between_intent_and_grant "${scenario}"; then
    board_changed=true
  fi
  db_json="$(db_scalar "
    SELECT json_build_object(
      'sessionId', '${session_id}',
      'humans', (SELECT count(*) FROM meeting_participants WHERE session_id='${session_id}' AND participant_type='human'),
      'agents', (SELECT count(*) FROM meeting_participants WHERE session_id='${session_id}' AND participant_type='agent'),
      'boardUpdates', (SELECT count(*) FROM meeting_v2_board_command_receipts WHERE session_id='${session_id}' AND accepted AND action='update'),
      'distinctSpeakers', (SELECT count(DISTINCT holder_pubkey) FROM meeting_baton_grants WHERE session_id='${session_id}' AND state='spoken'),
      'resolvedHandoffs', (SELECT count(*) FROM meeting_directed_handoffs WHERE session_id='${session_id}' AND question_state='answered'),
      'moderatorSelfSpeeches', (
        SELECT count(*) FROM meeting_baton_grants grant_row
        JOIN meeting_sessions session_row USING (community_id, session_id)
        WHERE grant_row.session_id='${session_id}'
          AND grant_row.state='spoken'
          AND grant_row.holder_pubkey=session_row.moderator_pubkey
      ),
      'terminalOutcome', (SELECT terminal_outcome FROM meeting_sessions WHERE session_id='${session_id}')
    )::text;
  ")"
  jq \
    --argjson floor_decisions "${floor_decisions}" \
    --argjson board_changed "${board_changed}" \
    '. + {
      floorDecisions: $floor_decisions,
      boardChangedBetweenIntentAndGrant: $board_changed
    }' <<<"${db_json}"
}

mixed_json="$(scenario_summary mixed "${mixed_session}" mixed-moderator | jq \
  --argjson preemptions "$([[ "${mixed_preempted}" == true ]] && printf 1 || printf 0)" \
  '. + {humanBoardPreemptions: $preemptions}')"
all_agent_json="$(scenario_summary all_agent "${all_agent_session}" all-moderator)"
moderator_abort_json="$(db_scalar "
  SELECT json_build_object(
    'sessionId', session_id,
    'terminalOutcome', terminal_outcome,
    'initiator', CASE WHEN ended_by=moderator_pubkey THEN 'moderator_agent' ELSE 'unknown' END,
    'reasonCode', terminal_reason_code
  )::text
  FROM meeting_sessions WHERE session_id='${moderator_abort_session}';
")"
admin_abort_json="$(db_scalar "
  SELECT json_build_object(
    'sessionId', session_id,
    'terminalOutcome', terminal_outcome,
    'initiator', CASE WHEN ended_by=moderator_pubkey THEN 'moderator_agent' ELSE 'security' END,
    'reasonCode', terminal_reason_code
  )::text
  FROM meeting_sessions WHERE session_id='${admin_abort_session}';
")"

workspace_after_diff="${secret_dir}/workspace-after.diff"
git status --porcelain=v1 >"${run_dir}/workspace-after.status"
git diff --binary HEAD >"${workspace_after_diff}"
workspace_after_status_sha256="$(shasum -a 256 "${run_dir}/workspace-after.status" | awk '{print $1}')"
workspace_after_diff_sha256="$(shasum -a 256 "${workspace_after_diff}" | awk '{print $1}')"
printf '%s\n' "${workspace_after_status_sha256}" >"${run_dir}/workspace-after.status.sha256"
printf '%s\n' "${workspace_after_diff_sha256}" >"${run_dir}/workspace-after.diff.sha256"
workspace_changed=false
if [[ "${workspace_status_sha256}" != "${workspace_after_status_sha256}" \
  || "${workspace_diff_sha256}" != "${workspace_after_diff_sha256}" ]]; then
  workspace_changed=true
fi

unauthorized_access=0
[[ "${outsider_read_status}" -ne 0 ]] || unauthorized_access=$((unauthorized_access + 1))
[[ "${outsider_write_status}" -ne 0 ]] || unauthorized_access=$((unauthorized_access + 1))
[[ "${post_end_write_status}" -ne 0 ]] || unauthorized_access=$((unauthorized_access + 1))
late_board_landed=0
[[ "${mixed_late_board_landed}" == false ]] || late_board_landed=1
external_writes=0
[[ "${workspace_changed}" == false ]] || external_writes=1

jq -n \
  --argjson mixed "${mixed_json}" \
  --argjson all_agent "${all_agent_json}" \
  --argjson moderator_abort "${moderator_abort_json}" \
  --argjson admin_abort "${admin_abort_json}" \
  --argjson board_floor_overlap "${board_floor_sequence_violations}" \
  --argjson floor_before_board "${board_floor_sequence_violations}" \
  --argjson board_during_floor "${board_accepted_during_offer_or_grant}" \
  --argjson missing_reads "${missing_board_reads}" \
  --argjson late_board "${late_board_landed}" \
  --argjson board_speech "${board_changed_speech_revision}" \
  --argjson post_end "${post_end_revision_change}" \
  --argjson pending "${pending_runtime_reservations}" \
  --argjson unauthorized "${unauthorized_access}" \
  --argjson external "${external_writes}" '
  {
    scenarios: {
      mixed: $mixed,
      all_agent: $all_agent,
      moderator_abort: $moderator_abort,
      admin_abort: $admin_abort
    },
    zero: {
      boardFloorOverlap: $board_floor_overlap,
      floorBeforeBoardTerminal: $floor_before_board,
      boardAcceptedDuringOfferOrGrant: $board_during_floor,
      turnWithoutBoardRead: $missing_reads,
      lateBoardLanded: $late_board,
      boardChangedSpeechRevision: $board_speech,
      postEndRevisionChange: $post_end,
      pendingRuntimeReservations: $pending,
      unauthorizedBoardAccess: $unauthorized,
      externalWrites: $external
    }
  }' >"${run_dir}/protocol-invariants.json"

jq -n \
  --argjson outsider_read "$([[ "${outsider_read_status}" -ne 0 ]] && printf true || printf false)" \
  --argjson outsider_write "$([[ "${outsider_write_status}" -ne 0 ]] && printf true || printf false)" \
  --argjson create_disabled "$([[ "${create_disabled_status}" -ne 0 ]] && printf true || printf false)" \
  --argjson post_end "$([[ "${post_end_write_status}" -ne 0 ]] && printf true || printf false)" '
  {
    outsiderReadDenied: $outsider_read,
    outsiderBoardWriteDenied: $outsider_write,
    createDisabledDenied: $create_disabled,
    postEndWriteDenied: $post_end
  }' >"${run_dir}/security-probes.json"

curl -fsS "http://127.0.0.1:${metrics_port}/metrics" >"${run_dir}/metrics.prom"

# Freeze Relay output before hashing the immutable evidence package.
stop_relay

: >"${run_dir}/runtime-anomalies.txt"
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  rg -nH \
    'agent_returned — respawning|respawn_failed|agent_panic|unsupported_model|authentication failed|agent pool initialization failed' \
    "${run_dir}/${log_path}" >>"${run_dir}/runtime-anomalies.txt" || true
done <"${run_dir}/processes.tsv"
runtime_anomalies="$(wc -l <"${run_dir}/runtime-anomalies.txt" | tr -d ' ')"
agent_sessions_exercised=0
while IFS=$'\t' read -r scenario role agent_pid effort log_path; do
  [[ "${scenario}" == scenario ]] && continue
  if rg -Fq "applied model ${model}[${effort}]" "${run_dir}/${log_path}"; then
    agent_sessions_exercised=$((agent_sessions_exercised + 1))
  fi
done <"${run_dir}/processes.tsv"

agent_logs_json="$(jq -Rn --arg model "${model}" '
  [inputs | split("\t")]
  | .[1:]
  | map({
      scenario: .[0],
      role: .[1],
      path: .[4],
      model: ($model + "[" + .[3] + "]")
    })
' <"${run_dir}/processes.tsv")"
security_json="$(jq -c . "${run_dir}/security-probes.json")"

jq -n \
  --arg run_id "${run_id}" \
  --arg started_at "${run_stamp}" \
  --arg finished_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg status_sha "${workspace_status_sha256}" \
  --arg diff_sha "${workspace_diff_sha256}" \
  --arg after_status_sha "${workspace_after_status_sha256}" \
  --arg after_diff_sha "${workspace_after_diff_sha256}" \
  --arg model "${model}" \
  --arg mixed "${mixed_session}" \
  --arg all_agent "${all_agent_session}" \
  --arg moderator_abort "${moderator_abort_session}" \
  --arg admin_abort "${admin_abort_session}" \
  --argjson exercised "${agent_sessions_exercised}" \
  --argjson workspace_changed "${workspace_changed}" \
  --argjson runtime_anomalies "${runtime_anomalies}" \
  --argjson security "${security_json}" \
  --argjson agent_logs "${agent_logs_json}" '
  {
    evidenceSchema: "buzz-meeting-v2-qualification-v1",
    runId: $run_id,
    startedAt: $started_at,
    finishedAt: $finished_at,
    buzzCommit: $commit,
    sourceTree: {
      statusSha256: $status_sha,
      diffSha256: $diff_sha,
      afterStatusSha256: $after_status_sha,
      afterDiffSha256: $after_diff_sha
    },
    protocol: {schemaVersion: "3", policy: "moderated-board-v1"},
    provider: {
      real: true,
      authenticated: true,
      catalogSupported: true,
      model: $model,
      moderatorReasoning: "max",
      participantReasoning: "high",
      adapter: "@agentclientprotocol/codex-acp",
      adapterVersion: "1.1.7",
      agentSessionsExercised: $exercised
    },
    capabilities: {
      relayRuntime: true,
      createEnabledObserved: true,
      createDisabledDrainObserved: true,
      acpV2Participant: true,
      acpV2Moderator: true
    },
    scenarios: {
      mixed: {sessionId: $mixed},
      all_agent: {sessionId: $all_agent},
      moderator_abort: {sessionId: $moderator_abort},
      admin_abort: {sessionId: $admin_abort}
    },
    securityProbes: $security,
    workspaceChanged: $workspace_changed,
    projectViewDependencies: 0,
    externalWrites: (if $workspace_changed then 1 else 0 end),
    runtimeAnomalies: $runtime_anomalies,
    artifacts: {
      relayLogs: ["logs/relay-create-enabled.log", "logs/relay.log"],
      agentLogs: $agent_logs
    },
    result: "candidate"
  }' >"${run_dir}/manifest.json"

(
  cd "${run_dir}"
  find . -type f \
    ! -name sha256.txt \
    ! -name qualification-gates.json \
    ! -name failure.txt \
    -print \
    | sed 's#^./##' \
    | LC_ALL=C sort \
    | while IFS= read -r path; do
        shasum -a 256 "${path}"
      done >sha256.txt
)

log "running independent qualification evidence verifier"
"${repo_root}/scripts/verify-meeting-v2-qualification.sh" "${run_dir}"
qualification_passed=true

if [[ "${keep_database}" != true ]]; then
  docker exec "${postgres_container}" psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE \"${database_name}\"" >/dev/null
  database_created=false
fi

log "PASS: Meeting V2 real-provider qualification completed"
log "artifacts: ${run_dir}"
