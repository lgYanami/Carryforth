#!/usr/bin/env bash
#
# Run one real-Codex Meeting V1 cross-meeting qualification tier.
#
# This runner intentionally keeps protocol observation and control separate:
#   - one authenticated WebSocket subscription records Relay-signed State per Session;
#   - read-only PostgreSQL checks drive time-sensitive Human ACK automation;
#   - all canonical actions still go through the public Buzz CLI/Relay path.
#
# Usage:
#   scripts/meeting-v1-live-acceptance.sh C6 [artifact-root]
#   scripts/meeting-v1-live-acceptance.sh C10 [artifact-root]
#   scripts/meeting-v1-live-acceptance.sh C12 [artifact-root]
#
# A run is one cold-start qualification sample. Formal acceptance still requires
# the repetitions and soak described in meeting-v1-live-acceptance-plan.md.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -f "$repo_root/bin/activate-hermit" ]]; then
  # shellcheck disable=SC1091
  . "$repo_root/bin/activate-hermit" >/dev/null
fi

tier="${1:-}"
artifact_root="${2:-${TMPDIR:-/tmp}/buzz-meeting-v1-live}"
speech_target="${MEETING_LIVE_SPEECHES_PER_AGENT:-2}"
keep_database="${MEETING_LIVE_KEEP_DATABASE:-false}"

case "$tier" in
  C6|c6)
    tier="C6"
    meeting_agent_counts="3 3"
    redis_db=11
    ;;
  C10|c10)
    tier="C10"
    meeting_agent_counts="4 3 3"
    redis_db=12
    ;;
  C12|c12)
    tier="C12"
    meeting_agent_counts="4 4 4"
    redis_db=13
    ;;
  *)
    echo "usage: $0 C6|C10|C12 [artifact-root]" >&2
    exit 2
    ;;
esac

case "$speech_target" in
  ''|*[!0-9]*|0)
    echo "MEETING_LIVE_SPEECHES_PER_AGENT must be a positive integer" >&2
    exit 2
    ;;
esac

run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
tier_lower="$(printf '%s' "$tier" | tr '[:upper:]' '[:lower:]')"
run_id="$(printf '%s-%s-%s' "$tier_lower" "$run_stamp" "$$")"
run_dir="$artifact_root/$run_id"
secret_dir="$(mktemp -d "${TMPDIR:-/tmp}/buzz-meeting-v1-secrets.XXXXXX")"
identity_file="$secret_dir/identities.tsv"
database_name="buzz_meeting_live_${tier_lower}_${run_stamp//[^0-9]/}_$$"
community_id=""
relay_url="ws://localhost:3000"
database_url="postgres://buzz:buzz_dev@localhost:5432/$database_name"
redis_url="redis://localhost:6379/$redis_db"
cleanup_done=false

mkdir -p "$run_dir"/{agents,meetings,observers,preflight}
chmod 700 "$secret_dir"
: >"$identity_file"
chmod 600 "$identity_file"

log() {
  printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*"
}

cleanup() {
  if [[ "$cleanup_done" == true ]]; then
    return
  fi
  cleanup_done=true

  running_pids="$(jobs -pr || true)"
  for child_pid in $running_pids; do
    if kill -0 "$child_pid" 2>/dev/null; then
      kill -TERM "$child_pid" 2>/dev/null || true
    fi
  done
  for child_pid in $running_pids; do
    wait "$child_pid" 2>/dev/null || true
  done

  case "$secret_dir" in
    "${TMPDIR:-/tmp}"/buzz-meeting-v1-secrets.*)
      rm -rf -- "$secret_dir"
      ;;
    *)
      echo "refusing to remove unexpected secret directory: $secret_dir" >&2
      ;;
  esac
}
trap 'exit_status=$?; cleanup; exit "$exit_status"' EXIT
trap 'exit 130' INT TERM

fail() {
  log "FAIL: $*"
  printf '%s\n' "$*" >"$run_dir/failure.txt"
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || fail "required command is missing: $1"
}

for required in codex codex-acp curl docker jq npm rg shasum; do
  require_command "$required"
done

for required_binary in \
  target/release/buzz \
  target/release/buzz-acp \
  target/release/buzz-admin \
  target/release/buzz-relay \
  target/release/buzz-test-cli; do
  [[ -x "$required_binary" ]] || fail "missing release binary: $required_binary"
done

if curl -fsS -H 'Host: localhost:3000' http://127.0.0.1:3000/ >/dev/null 2>&1; then
  fail "localhost:3000 is already serving a relay"
fi

git status --porcelain=v1 >"$run_dir/workspace-before.status"
git diff --binary HEAD >"$run_dir/workspace-before.diff"
shasum -a 256 \
  "$0" \
  target/release/buzz \
  target/release/buzz-acp \
  target/release/buzz-admin \
  target/release/buzz-relay \
  target/release/buzz-test-cli \
  >"$run_dir/preflight/executable-sha256.txt"
runner_sha256="$(shasum -a 256 "$0" | awk '{print $1}')"
workspace_diff_sha256="$(
  shasum -a 256 "$run_dir/workspace-before.diff" | awk '{print $1}'
)"

log "preflight: real Codex auth, catalog, and exact codex-acp adapter"
codex --version >"$run_dir/preflight/codex-version.txt"
codex-acp --version >"$run_dir/preflight/codex-acp-version.txt"
codex login status >"$run_dir/preflight/codex-login-status.txt" 2>&1
rg -q 'Logged in using ChatGPT|Logged in using an API key' \
  "$run_dir/preflight/codex-login-status.txt" ||
  fail "Codex is not authenticated"

npm ls -g @agentclientprotocol/codex-acp --depth=0 --json \
  >"$run_dir/preflight/codex-acp-package.json"
adapter_version="$(
  jq -r '.dependencies["@agentclientprotocol/codex-acp"].version // empty' \
    "$run_dir/preflight/codex-acp-package.json"
)"
[[ "$adapter_version" == "1.1.7" ]] ||
  fail "expected @agentclientprotocol/codex-acp 1.1.7, found ${adapter_version:-none}"

codex debug models \
  -c 'model="gpt-5.6-sol"' \
  -c 'model_reasoning_effort="max"' \
  -c 'features.multi_agent=false' \
  >"$run_dir/preflight/codex-models.json"
jq -e '
  .models[]
  | select(.slug == "gpt-5.6-sol")
  | [.supported_reasoning_levels[].effort]
  | (index("high") != null and index("max") != null)
' "$run_dir/preflight/codex-models.json" >/dev/null ||
  fail "gpt-5.6-sol does not expose both high and max reasoning"

BUZZ_ACP_AGENT_COMMAND=codex-acp \
  target/release/buzz-acp models --json \
  >"$run_dir/preflight/codex-acp-models.json"
jq -e '
  ([
    .stable.configOptions[]?.options[]?.value,
    .unstable.availableModels[]?.modelId
  ] | any(. == "gpt-5.6-sol[high]"))
  and
  ([
    .stable.configOptions[]?.options[]?.value,
    .unstable.availableModels[]?.modelId
  ] | any(. == "gpt-5.6-sol[max]"))
' "$run_dir/preflight/codex-acp-models.json" >/dev/null ||
  fail "codex-acp did not expose both gpt-5.6-sol[high] and gpt-5.6-sol[max]"

generate_identity() {
  local role="$1"
  local meeting_index="$2"
  local identity_type="$3"
  local key_output="$secret_dir/$role.key-output"
  local public_key
  local private_key

  target/release/buzz-admin generate-key >"$key_output"
  public_key="$(awk '/^Public key:/ {print $3}' "$key_output")"
  private_key="$(awk '/^Secret key:/ {print $3}' "$key_output")"
  [[ "$public_key" =~ ^[0-9a-f]{64}$ ]] || fail "invalid generated public key for $role"
  [[ "$private_key" =~ ^[0-9a-f]{64}$ ]] || fail "invalid generated private key for $role"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$role" "$meeting_index" "$identity_type" "$public_key" "$private_key" \
    >>"$identity_file"
  rm -- "$key_output"
}

identity_public_key() {
  local role="$1"
  awk -F '\t' -v wanted="$role" '$1 == wanted { print $4; exit }' "$identity_file"
}

identity_private_key() {
  local role="$1"
  awk -F '\t' -v wanted="$role" '$1 == wanted { print $5; exit }' "$identity_file"
}

meeting_count=0
for ignored_count in $meeting_agent_counts; do
  meeting_count=$((meeting_count + 1))
done

log "generating isolated identities for $tier ($meeting_count Meetings)"
meeting_index=1
for agent_count in $meeting_agent_counts; do
  generate_identity "m${meeting_index}-host" "$meeting_index" human
  generate_identity "m${meeting_index}-observer" "$meeting_index" human
  agent_index=1
  while [[ "$agent_index" -le "$agent_count" ]]; do
    generate_identity "m${meeting_index}-agent${agent_index}" "$meeting_index" agent
    agent_index=$((agent_index + 1))
  done
  meeting_index=$((meeting_index + 1))
done

awk -F '\t' 'BEGIN { OFS="\t" } { print $1, $2, $3, $4 }' \
  "$identity_file" >"$run_dir/roster.tsv"

log "creating isolated PostgreSQL database $database_name"
[[ "$database_name" =~ ^[a-z0-9_]+$ ]] || fail "unsafe generated database name"
docker exec buzz-postgres psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE \"$database_name\"" >/dev/null
DATABASE_URL="$database_url" target/release/buzz-admin migrate \
  >"$run_dir/migrate.log" 2>&1
docker exec buzz-redis redis-cli -n "$redis_db" FLUSHDB >/dev/null

relay_owner="$(identity_public_key m1-host)"
log "starting isolated Relay"
env \
  DATABASE_URL="$database_url" \
  REDIS_URL="$redis_url" \
  BUZZ_BIND_ADDR="127.0.0.1:3000" \
  RELAY_URL="$relay_url" \
  BUZZ_AUTO_MIGRATE=false \
  BUZZ_MEETING_V1_CREATE_ENABLED=true \
  BUZZ_REQUIRE_RELAY_MEMBERSHIP=true \
  RELAY_OWNER_PUBKEY="$relay_owner" \
  BUZZ_RELAY_PRIVATE_KEY="0000000000000000000000000000000000000000000000000000000000000001" \
  RUST_LOG="buzz_relay=info,buzz_db=info,buzz_acp=info" \
  target/release/buzz-relay >"$run_dir/relay.log" 2>&1 &
relay_pid=$!

relay_ready=false
for ignored_attempt in $(seq 1 120); do
  if curl -fsS -H 'Host: localhost:3000' http://127.0.0.1:3000/ >/dev/null 2>&1; then
    relay_ready=true
    break
  fi
  if ! kill -0 "$relay_pid" 2>/dev/null; then
    fail "Relay exited during startup"
  fi
  sleep 0.5
done
[[ "$relay_ready" == true ]] || fail "Relay did not become ready within 60 seconds"
community_id="$(
  docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA \
    -c "SELECT id FROM communities WHERE lower(host)=lower('localhost:3000');"
)"
[[ "$community_id" =~ ^[0-9a-f-]{36}$ ]] ||
  fail "Relay did not provision the localhost:3000 community"

seed_identity() {
  local role="$1"
  local meeting_index="$2"
  local identity_type="$3"
  local public_key="$4"
  local host_role="m${meeting_index}-host"
  local owner_public_key
  local display_name

  display_name="${tier} ${role}"
  docker exec buzz-postgres psql -U buzz -d "$database_name" \
    -v ON_ERROR_STOP=1 \
    -c "
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES ('$community_id'::uuid, '$public_key', 'member')
      ON CONFLICT (community_id, pubkey)
      DO UPDATE SET
        role = CASE
          WHEN relay_members.role = 'owner' THEN relay_members.role
          ELSE EXCLUDED.role
        END,
        updated_at = clock_timestamp();
    " >/dev/null

  if [[ "$identity_type" == agent ]]; then
    owner_public_key="$(identity_public_key "$host_role")"
    docker exec buzz-postgres psql -U buzz -d "$database_name" \
      -v ON_ERROR_STOP=1 \
      -c "
        INSERT INTO users (
          community_id, pubkey, display_name, agent_type,
          agent_owner_pubkey, channel_add_policy
        )
        VALUES (
          '$community_id'::uuid,
          decode('$public_key', 'hex'),
          '$display_name',
          'codex',
          decode('$owner_public_key', 'hex'),
          'owner_only'
        )
        ON CONFLICT (community_id, pubkey)
        DO UPDATE SET
          display_name = EXCLUDED.display_name,
          agent_type = EXCLUDED.agent_type,
          agent_owner_pubkey = EXCLUDED.agent_owner_pubkey,
          channel_add_policy = EXCLUDED.channel_add_policy,
          deactivated_at = NULL;
      " >/dev/null
  else
    docker exec buzz-postgres psql -U buzz -d "$database_name" \
      -v ON_ERROR_STOP=1 \
      -c "
        INSERT INTO users (
          community_id, pubkey, display_name, agent_type,
          agent_owner_pubkey, channel_add_policy
        )
        VALUES (
          '$community_id'::uuid,
          decode('$public_key', 'hex'),
          '$display_name',
          NULL,
          NULL,
          'anyone'
        )
        ON CONFLICT (community_id, pubkey)
        DO UPDATE SET
          display_name = EXCLUDED.display_name,
          agent_type = NULL,
          agent_owner_pubkey = NULL,
          channel_add_policy = 'anyone',
          deactivated_at = NULL;
      " >/dev/null
  fi
}

log "seeding and verifying authoritative membership/ownership"
while IFS=$'\t' read -r role meeting_index identity_type public_key private_key; do
  seed_identity "$role" "$meeting_index" "$identity_type" "$public_key"
done <"$identity_file"

expected_identity_count="$(wc -l <"$identity_file" | tr -d ' ')"
seeded_members="$(
  docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA \
    -c "SELECT count(*) FROM relay_members WHERE community_id='$community_id';"
)"
seeded_users="$(
  docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA \
    -c "SELECT count(*) FROM users WHERE community_id='$community_id';"
)"
[[ "$seeded_members" -eq "$expected_identity_count" ]] ||
  fail "membership seed mismatch: expected $expected_identity_count, found $seeded_members"
[[ "$seeded_users" -eq "$expected_identity_count" ]] ||
  fail "user seed mismatch: expected $expected_identity_count, found $seeded_users"

buzz_as() {
  local role="$1"
  shift
  BUZZ_RELAY_URL="$relay_url" \
    BUZZ_PRIVATE_KEY="$(identity_private_key "$role")" \
    target/release/buzz --format compact "$@"
}

db_scalar() {
  local sql="$1"
  docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA -c "$sql"
}

human_turn() {
  local role="$1"
  local meeting_id="$2"
  local artifact_prefix="$3"
  local content="$4"
  local handoff_target="${5:-}"
  local handoff_reason="${6:-}"
  local request_id
  local request_row
  local request_state
  local offer_id
  local poll_count=0
  local -a say_args

  log "$artifact_prefix: Human Request"
  buzz_as "$role" meetings floor request --meeting "$meeting_id" \
    >"$run_dir/meetings/$artifact_prefix-request.json"
  request_id="$(
    jq -r '.request_id // empty' "$run_dir/meetings/$artifact_prefix-request.json"
  )"
  [[ "$request_id" =~ ^[0-9a-f]{64}$ ]] ||
    fail "$artifact_prefix: request did not return a canonical request_id"

  while true; do
    request_row="$(
      db_scalar "
        SELECT state, COALESCE(encode(offer_id, 'hex'), '')
        FROM meeting_human_floor_requests
        WHERE request_id=decode('$request_id', 'hex');
      "
    )"
    request_state="${request_row%%|*}"
    offer_id="${request_row#*|}"
    if [[ "$request_state" == offered && "$offer_id" =~ ^[0-9a-f]{64}$ ]]; then
      break
    fi
    if [[ "$request_state" != queued ]]; then
      fail "$artifact_prefix: Human Request became $request_state before ACK"
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 2400 )); then
      fail "$artifact_prefix: Human Offer did not arrive within 10 minutes"
    fi
    if (( poll_count % 80 == 0 )); then
      log "$artifact_prefix: waiting for current speaker to return control"
    fi
    sleep 0.25
  done

  buzz_as "$role" meetings offer ack --meeting "$meeting_id" --offer "$offer_id" \
    >"$run_dir/meetings/$artifact_prefix-ack.json"
  jq -e '.accepted == true and .outcome == "accepted"' \
    "$run_dir/meetings/$artifact_prefix-ack.json" >/dev/null ||
    fail "$artifact_prefix: Human ACK was not accepted"

  say_args=(meetings say --meeting "$meeting_id" --content "$content")
  if [[ -n "$handoff_target" ]]; then
    say_args+=(
      --handoff-to "$handoff_target"
      --handoff-type review
      --handoff-reason "$handoff_reason"
    )
  fi
  buzz_as "$role" "${say_args[@]}" \
    >"$run_dir/meetings/$artifact_prefix-say.json"
  LAST_SPEECH_ID="$(
    jq -r '.speech_event_id // empty' "$run_dir/meetings/$artifact_prefix-say.json"
  )"
  [[ "$LAST_SPEECH_ID" =~ ^[0-9a-f]{64}$ ]] ||
    fail "$artifact_prefix: SAY did not return a canonical speech_event_id"
  log "$artifact_prefix: Human SAY accepted"
}

log "creating $meeting_count V1 Meetings and publishing opening agenda"
: >"$run_dir/meetings.tsv"
meeting_index=1
for agent_count in $meeting_agent_counts; do
  host_role="m${meeting_index}-host"
  observer_role="m${meeting_index}-observer"
  moderator_role="m${meeting_index}-agent1"
  moderator_public_key="$(identity_public_key "$moderator_role")"
  create_path="$run_dir/meetings/m${meeting_index}-create.json"
  create_args=(
    meetings create
    --policy moderated-baton-v1
    --title "$tier real Codex qualification / Meeting $meeting_index"
    --description "Cross-meeting real Codex qualification. Read-only project investigation."
    --moderator "$moderator_public_key"
    --participant "$(identity_public_key "$observer_role")"
  )
  agent_index=1
  while [[ "$agent_index" -le "$agent_count" ]]; do
    create_args+=(--participant "$(identity_public_key "m${meeting_index}-agent${agent_index}")")
    agent_index=$((agent_index + 1))
  done
  buzz_as "$host_role" "${create_args[@]}" >"$create_path"
  meeting_id="$(jq -r '.meeting_id // empty' "$create_path")"
  [[ "$meeting_id" =~ ^[0-9a-f-]{36}$ ]] ||
    fail "Meeting $meeting_index create did not return a UUID"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$meeting_index" "$meeting_id" "$host_role" "$observer_role" "$agent_count" \
    >>"$run_dir/meetings.tsv"

  human_turn \
    "$host_role" \
    "$meeting_id" \
    "m${meeting_index}-opening" \
    "这是 ${tier} 并发验收的 Meeting ${meeting_index}。请围绕 Meeting V1 在真实 Codex 并发下的协议连续性、数据库不变量、ACP 延迟和运维边界进行只读调查；所有 Agent 先提交简短且不重复的发言意图，获得 Grant 后用代码事实支撑结论。禁止修改工作区或执行外部写操作。"
  meeting_index=$((meeting_index + 1))
done

start_observer() {
  local meeting_index="$1"
  local meeting_id="$2"
  local observer_role="$3"
  local observer_key
  observer_key="$(identity_private_key "$observer_role")"
  BUZZ_PRIVATE_KEY="$observer_key" \
    target/release/buzz-test-cli \
      --url "$relay_url" \
      --channel "$meeting_id" \
      --subscribe \
      --kind 42103 \
      >"$run_dir/observers/m${meeting_index}-state-ws.log" 2>&1 &
}

agent_team_instructions() {
  local role="$1"
  local meeting_index="$2"
  local agent_index="$3"
  local role_focus

  if [[ "$agent_index" -eq 1 ]]; then
    role_focus="你是本场主持 Agent。主持时优先 Human Request、恢复未完成 Handoff，并给出有证据的选择理由；获得自己的 Grant 时也必须作为参会者发言。"
  else
    case $(((meeting_index + agent_index) % 4)) in
      0) role_focus="你的调查重点是 Meeting 协议状态机、发言权和 Handoff 连续性。" ;;
      1) role_focus="你的调查重点是 PostgreSQL 约束、事务、outbox 和恢复语义。" ;;
      2) role_focus="你的调查重点是 buzz-acp、Codex 延迟、Prompt 和工具调用。" ;;
      *) role_focus="你的调查重点是运维指标、限流、并发、故障与发布门槛。" ;;
    esac
  fi

  printf '%s\n' \
    "你正在参加 ${tier} 的真实 Meeting V1 验收（Meeting ${meeting_index}，身份 ${role}）。" \
    "$role_focus" \
    "会议只做讨论与只读调查；可以使用工具读取仓库和运行只读命令，但不要修改文件、Git、Buzz 项目状态、第三方系统或网络资源。" \
    "Intent 只概括为什么现在值得发言。获得 Grant 后再组织发言，保持结论具体、可核验且避免重复；有明确提问对象时使用 directed handoff 并说明原因。"
}

start_agent() {
  local role="$1"
  local meeting_index="$2"
  local agent_index="$3"
  local effort=high
  local private_key
  local team_instructions

  if [[ "$agent_index" -eq 1 ]]; then
    effort=max
  fi
  private_key="$(identity_private_key "$role")"
  team_instructions="$(agent_team_instructions "$role" "$meeting_index" "$agent_index")"

  env \
    CODEX_CONFIG="{\"model_reasoning_effort\":\"$effort\",\"features\":{\"multi_agent\":false}}" \
    BUZZ_ACP_MODEL="gpt-5.6-sol[$effort]" \
    BUZZ_ACP_AGENT_COMMAND="codex-acp" \
    BUZZ_ACP_AGENT_ARGS="" \
    BUZZ_ACP_AGENTS=1 \
    BUZZ_ACP_LAZY_POOL=false \
    BUZZ_ACP_PERMISSION_MODE="bypass-permissions" \
    BUZZ_ACP_IDLE_TIMEOUT=620 \
    BUZZ_ACP_MAX_TURN_DURATION=7200 \
    BUZZ_ACP_MAX_TURNS_PER_SESSION=0 \
    BUZZ_ACP_MEETING_V1_AUTO_ACCEPT=true \
    BUZZ_ACP_MEETING_V1_LEDGER_PATH="$run_dir/agents/$role-ledger.json" \
    BUZZ_ACP_NO_MEMORY=true \
    BUZZ_ACP_RESPOND_TO=anyone \
    BUZZ_ACP_SUBSCRIBE=mentions \
    BUZZ_ACP_CONTEXT_MESSAGE_LIMIT=12 \
    BUZZ_ACP_MULTIPLE_EVENT_HANDLING=steer \
    BUZZ_ACP_TEAM_INSTRUCTIONS="$team_instructions" \
    BUZZ_PRIVATE_KEY="$private_key" \
    BUZZ_RELAY_URL="$relay_url" \
    RUST_LOG="buzz_acp=info,acp=info,pool=info,engram=info" \
    target/release/buzz-acp >"$run_dir/agents/$role.log" 2>&1 &
  local agent_pid=$!
  printf '%s\t%s\t%s\n' "$role" "$agent_pid" "$effort" \
    >>"$run_dir/agent-processes.tsv"
}

log "starting authenticated State subscriptions and all real Codex ACP runtimes"
: >"$run_dir/agent-processes.tsv"
while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  start_observer "$meeting_index" "$meeting_id" "$observer_role"
done <"$run_dir/meetings.tsv"

meeting_index=1
for agent_count in $meeting_agent_counts; do
  agent_index=1
  while [[ "$agent_index" -le "$agent_count" ]]; do
    start_agent "m${meeting_index}-agent${agent_index}" "$meeting_index" "$agent_index"
    agent_index=$((agent_index + 1))
  done
  meeting_index=$((meeting_index + 1))
done

all_agents_ready=false
for ignored_attempt in $(seq 1 180); do
  ready_count=0
  while IFS=$'\t' read -r role agent_pid effort; do
    if rg -q 'agent_pool_ready agents=1' "$run_dir/agents/$role.log" 2>/dev/null; then
      ready_count=$((ready_count + 1))
    elif ! kill -0 "$agent_pid" 2>/dev/null; then
      fail "$role exited before agent_pool_ready"
    fi
  done <"$run_dir/agent-processes.tsv"
  if [[ "$ready_count" -eq "$(wc -l <"$run_dir/agent-processes.tsv" | tr -d ' ')" ]]; then
    all_agents_ready=true
    break
  fi
  sleep 0.5
done
[[ "$all_agents_ready" == true ]] || fail "not all ACP runtimes became ready within 90 seconds"
log "all real Codex ACP runtimes are ready"

all_models_applied=false
for ignored_attempt in $(seq 1 240); do
  applied_count=0
  while IFS=$'\t' read -r role agent_pid effort; do
    agent_log="$run_dir/agents/$role.log"
    if rg -q \
      'desired model .* not found|unsupported_model|failed to set model|model set .* timed out' \
      "$agent_log" 2>/dev/null; then
      fail "$role could not fail-closed apply gpt-5.6-sol[$effort]"
    fi
    if rg -q "applied model gpt-5\\.6-sol\\[$effort\\]" "$agent_log" 2>/dev/null; then
      applied_count=$((applied_count + 1))
    elif ! kill -0 "$agent_pid" 2>/dev/null; then
      fail "$role exited before applying gpt-5.6-sol[$effort]"
    fi
  done <"$run_dir/agent-processes.tsv"
  if [[ "$applied_count" -eq "$(wc -l <"$run_dir/agent-processes.tsv" | tr -d ' ')" ]]; then
    all_models_applied=true
    break
  fi
  sleep 0.5
done
[[ "$all_models_applied" == true ]] ||
  fail "not all real Meeting Sessions applied their requested model within 120 seconds"
log "all Meeting Sessions applied gpt-5.6-sol with their requested effort"

wait_for_active_agent_grant() {
  local meeting_id="$1"
  local poll_count=0
  local active_count
  while true; do
    active_count="$(
      db_scalar "
        SELECT count(*)
        FROM meeting_baton_grants grant_row
        JOIN meeting_participants participant
          USING (community_id, session_id)
        WHERE grant_row.session_id='$meeting_id'
          AND grant_row.state='active'
          AND participant.pubkey=grant_row.holder_pubkey
          AND participant.participant_type='agent';
      "
    )"
    if [[ "$active_count" -eq 1 ]]; then
      return
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 2400 )); then
      fail "$meeting_id: no active Agent Grant within 10 minutes"
    fi
    if (( poll_count % 80 == 0 )); then
      log "$meeting_id: waiting for first Agent Grant"
    fi
    sleep 0.25
  done
}

wait_handoff_resolved() {
  local handoff_id="$1"
  local label="$2"
  local allow_dismissed="${3:-false}"
  local poll_count=0
  local handoff_state
  while true; do
    handoff_state="$(
      db_scalar "
        SELECT question_state
        FROM meeting_directed_handoffs
        WHERE handoff_id=decode('$handoff_id', 'hex');
      "
    )"
    if [[ "$handoff_state" == answered ]]; then
      log "$label: directed handoff answered"
      return
    fi
    if [[ "$handoff_state" == dismissed && "$allow_dismissed" == true ]]; then
      log "$label: moderator dismissed the directed handoff; continuing with a distinct question"
      return
    fi
    if [[ -n "$handoff_state" && "$handoff_state" != open ]]; then
      fail "$label: directed handoff became $handoff_state"
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 2400 )); then
      fail "$label: directed handoff was not answered within 10 minutes"
    fi
    if (( poll_count % 80 == 0 )); then
      log "$label: waiting for real Codex speech"
    fi
    sleep 0.25
  done
}

observer_priority_flow() {
  local meeting_index="$1"
  local meeting_id="$2"
  local observer_role="$3"
  local target_role="$4"
  local target_public_key
  local label="m${meeting_index}-observer-priority"

  target_public_key="$(identity_public_key "$target_role")"
  wait_for_active_agent_grant "$meeting_id"
  human_turn \
    "$observer_role" \
    "$meeting_id" \
    "$label" \
    "Human 并发验收：这个请求是在 Agent Grant 有效时异步排队。请 $target_role 在获得下一轮定向发言权后说明：当前证据中最可能首先阻止 $tier 继续放大的一个风险是什么，以及对应的可观测停止条件是什么？" \
    "$target_public_key" \
    "请基于你的职责给出一个首要扩容风险和可观测停止条件。"
  wait_handoff_resolved "$LAST_SPEECH_ID" "$label" false
}

log "arming one Human-priority flow per Meeting while Agent Grants are active"
driver_pids=""
while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  target_role="m${meeting_index}-agent${agent_count}"
  observer_priority_flow \
    "$meeting_index" "$meeting_id" "$observer_role" "$target_role" \
    >"$run_dir/meetings/m${meeting_index}-observer-priority-driver.log" 2>&1 &
  driver_pid=$!
  driver_pids="$driver_pids $driver_pid"
done <"$run_dir/meetings.tsv"

for driver_pid in $driver_pids; do
  wait "$driver_pid" ||
    fail "a Human-priority driver failed"
done
log "all Human-priority directed handoffs completed"

agent_speech_count() {
  local meeting_id="$1"
  local agent_public_key="$2"
  db_scalar "
    SELECT count(*)
    FROM meeting_baton_grants
    WHERE session_id='$meeting_id'
      AND holder_pubkey=decode('$agent_public_key', 'hex')
      AND state='spoken';
  "
}

qualify_meeting() {
  local meeting_index="$1"
  local meeting_id="$2"
  local host_role="$3"
  local agent_count="$4"
  local agent_index=1
  local targeted_round=0
  local target_role
  local target_public_key
  local current_count
  local label
  local qualification_prompt
  local qualification_reason

  while [[ "$agent_index" -le "$agent_count" ]]; do
    target_role="m${meeting_index}-agent${agent_index}"
    target_public_key="$(identity_public_key "$target_role")"
    current_count="$(agent_speech_count "$meeting_id" "$target_public_key")"
    while [[ "$current_count" -lt "$speech_target" ]]; do
      targeted_round=$((targeted_round + 1))
      if [[ "$targeted_round" -gt $((agent_count * speech_target + agent_count)) ]]; then
        fail "Meeting $meeting_index exceeded targeted qualification turn budget"
      fi
      label="m${meeting_index}-qualify-${targeted_round}-${target_role}"
      case $((targeted_round % 3)) in
        1)
          qualification_prompt="Host 定向复核 ${targeted_round}：请 ${target_role} 对照本场已有历史，补充一个尚未被充分证明的协议或数据库不变量。回答必须读取实际代码或权威 State，并明确区分本场观察、确定性测试和仍待验证的结论。"
          qualification_reason="请补充尚未充分证明的协议或数据库证据，避免重复已有发言。"
          ;;
        2)
          qualification_prompt="Host 定向复核 ${targeted_round}：请 ${target_role} 只分析真实 Codex 并发、ACK/Grant 时延与 provider 配额，指出一个前一轮没有覆盖的失败模式，并给出可量化的停止门槛。"
          qualification_reason="请从真实 provider 并发与时延角度给出新的失败模式和门槛。"
          ;;
        *)
          qualification_prompt="Host 定向复核 ${targeted_round}：请 ${target_role} 基于本场最新 State 和历史，判断当前证据支持 PASS、CONDITIONAL PASS 还是 FAIL，并列出进入下一 Tier 前仍缺少的唯一关键证据。"
          qualification_reason="请基于最新会议状态作阶段判断，并指出唯一关键证据缺口。"
          ;;
      esac
      human_turn \
        "$host_role" \
        "$meeting_id" \
        "$label" \
        "$qualification_prompt" \
        "$target_public_key" \
        "$qualification_reason"
      wait_handoff_resolved "$LAST_SPEECH_ID" "$label" true
      current_count="$(agent_speech_count "$meeting_id" "$target_public_key")"
    done
    log "Meeting $meeting_index: $target_role reached $current_count canonical speeches"
    agent_index=$((agent_index + 1))
  done
}

log "driving each Agent to at least $speech_target canonical speeches"
qualification_pids=""
while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  qualify_meeting "$meeting_index" "$meeting_id" "$host_role" "$agent_count" \
    >"$run_dir/meetings/m${meeting_index}-qualification-driver.log" 2>&1 &
  qualification_pid=$!
  qualification_pids="$qualification_pids $qualification_pid"
done <"$run_dir/meetings.tsv"

for qualification_pid in $qualification_pids; do
  wait "$qualification_pid" ||
    fail "an Agent qualification driver failed"
done
log "all Agents reached the qualification speech target"

log "ending all Meetings through the canonical CLI path"
while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  buzz_as "$host_role" meetings history --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-history-pre-end.json"
  buzz_as "$host_role" meetings floor status --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-floor-pre-end.json"
  buzz_as "$host_role" meetings end --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-end.json"
done <"$run_dir/meetings.tsv"

all_ended=false
for ignored_attempt in $(seq 1 80); do
  ended_count="$(
    db_scalar "
      SELECT count(*)
      FROM meeting_baton_state
      WHERE phase='ended';
    "
  )"
  if [[ "$ended_count" -eq "$meeting_count" ]]; then
    all_ended=true
    break
  fi
  sleep 0.25
done
[[ "$all_ended" == true ]] || fail "not all Meetings reached ended State"

for ignored_attempt in $(seq 1 80); do
  pending_outbox="$(
    db_scalar "SELECT count(*) FROM meeting_event_outbox WHERE delivered_at IS NULL;"
  )"
  [[ "$pending_outbox" -eq 0 ]] && break
  sleep 0.25
done

while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  buzz_as "$host_role" meetings show --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-show-post-end.json"
  buzz_as "$host_role" meetings history --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-history-post-end.json"
  buzz_as "$host_role" meetings floor status --meeting "$meeting_id" \
    >"$run_dir/meetings/m${meeting_index}-floor-post-end.json"
done <"$run_dir/meetings.tsv"

docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA -F '|' -c "
  SELECT
    session_id,
    phase,
    state_revision,
    floor_revision,
    intent_revision,
    speech_revision,
    active_offer_id IS NULL,
    active_grant_id IS NULL
  FROM meeting_baton_state
  ORDER BY session_id;

  SELECT 'offer_state', state, count(*)
  FROM meeting_baton_offers
  GROUP BY state
  ORDER BY state;

  SELECT 'grant_state', state, count(*)
  FROM meeting_baton_grants
  GROUP BY state
  ORDER BY state;

  SELECT 'handoff_state', question_state, count(*)
  FROM meeting_directed_handoffs
  GROUP BY question_state
  ORDER BY question_state;

  SELECT
    'outbox',
    count(*) FILTER (WHERE delivered_at IS NULL),
    count(*) FILTER (WHERE last_error IS NOT NULL),
    COALESCE(max(attempts), 0)
  FROM meeting_event_outbox;
" >"$run_dir/protocol-invariants.txt"

docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA -F '|' -c "
  WITH ack AS (
    SELECT
      participant.participant_type,
      EXTRACT(EPOCH FROM (offer_row.resolved_at-offer_row.created_at))*1000 AS ms
    FROM meeting_baton_offers offer_row
    JOIN meeting_participants participant
      USING (community_id, session_id)
    WHERE participant.pubkey=offer_row.target_pubkey
      AND offer_row.state='acked'
  )
  SELECT
    'offer_ack_ms',
    participant_type,
    count(*),
    round(min(ms)::numeric, 1),
    round(percentile_cont(0.5) WITHIN GROUP (ORDER BY ms)::numeric, 1),
    round(max(ms)::numeric, 1)
  FROM ack
  GROUP BY participant_type
  ORDER BY participant_type;

  WITH speech AS (
    SELECT
      participant.participant_type,
      EXTRACT(EPOCH FROM (grant_row.terminal_at-grant_row.created_at))*1000 AS ms
    FROM meeting_baton_grants grant_row
    JOIN meeting_participants participant
      USING (community_id, session_id)
    WHERE participant.pubkey=grant_row.holder_pubkey
      AND grant_row.state='spoken'
  )
  SELECT
    'grant_to_speech_ms',
    participant_type,
    count(*),
    round(min(ms)::numeric, 1),
    round(percentile_cont(0.5) WITHIN GROUP (ORDER BY ms)::numeric, 1),
    round(max(ms)::numeric, 1)
  FROM speech
  GROUP BY participant_type
  ORDER BY participant_type;

  WITH delivery AS (
    SELECT EXTRACT(EPOCH FROM (delivered_at-available_at))*1000 AS ms
    FROM meeting_event_outbox
    WHERE delivered_at IS NOT NULL
  )
  SELECT
    'outbox_delivery_ms',
    count(*),
    round(min(ms)::numeric, 1),
    round(percentile_cont(0.5) WITHIN GROUP (ORDER BY ms)::numeric, 1),
    round(max(ms)::numeric, 1)
  FROM delivery;
" >"$run_dir/latency-summary.txt"

docker exec buzz-postgres psql -U buzz -d "$database_name" -qtA -F '|' -c "
  SELECT
    grant_row.session_id,
    encode(grant_row.holder_pubkey, 'hex'),
    count(*)
  FROM meeting_baton_grants grant_row
  JOIN meeting_participants participant
    USING (community_id, session_id)
  WHERE grant_row.state='spoken'
    AND participant.pubkey=grant_row.holder_pubkey
    AND participant.participant_type='agent'
  GROUP BY grant_row.session_id, grant_row.holder_pubkey
  ORDER BY grant_row.session_id, grant_row.holder_pubkey;
" >"$run_dir/agent-speech-counts.txt"

curl -fsS http://127.0.0.1:9102/metrics >"$run_dir/metrics.prom" || true
git status --porcelain=v1 >"$run_dir/workspace-after.status"
git diff --binary HEAD >"$run_dir/workspace-after.diff"

protocol_failures=0

if ! cmp -s "$run_dir/workspace-before.status" "$run_dir/workspace-after.status" ||
  ! cmp -s "$run_dir/workspace-before.diff" "$run_dir/workspace-after.diff"; then
  log "workspace audit changed during Agent execution"
  protocol_failures=$((protocol_failures + 1))
fi

non_ended_meetings="$(
  db_scalar "SELECT count(*) FROM meeting_baton_state WHERE phase <> 'ended';"
)" || fail "failed to query terminal Meeting States"
if [[ "$non_ended_meetings" -ne 0 ]]; then
  log "one or more Meeting States are not ended"
  protocol_failures=$((protocol_failures + 1))
fi

terminal_reservations="$(
  db_scalar "
  SELECT
    (SELECT count(*) FROM meeting_baton_offers WHERE state='pending') +
    (SELECT count(*) FROM meeting_baton_grants WHERE state='active') +
    (SELECT count(*) FROM meeting_directed_handoffs WHERE question_state='open') +
    (SELECT count(*) FROM meeting_event_outbox WHERE delivered_at IS NULL);
"
)" || fail "failed to query terminal reservations"
if [[ "$terminal_reservations" -ne 0 ]]; then
  log "terminal reservations or outbox rows remain"
  protocol_failures=$((protocol_failures + 1))
fi

failed_agent_offers="$(
  db_scalar "
  SELECT count(*)
  FROM meeting_baton_offers offer_row
  JOIN meeting_participants participant USING (community_id, session_id)
  WHERE participant.pubkey=offer_row.target_pubkey
    AND participant.participant_type='agent'
    AND offer_row.state <> 'acked';
"
)" || fail "failed to query Agent Offer outcomes"
if [[ "$failed_agent_offers" -ne 0 ]]; then
  log "an Agent Offer did not ACK successfully"
  protocol_failures=$((protocol_failures + 1))
fi

broken_revision_histories="$(
  db_scalar "
  SELECT count(*)
  FROM (
    SELECT
      state_row.session_id,
      count(history.state_revision) AS history_count,
      min(history.state_revision) AS min_revision,
      max(history.state_revision) AS max_revision,
      count(DISTINCT history.state_revision) AS distinct_revision_count
    FROM meeting_baton_state state_row
    JOIN meeting_baton_state_history history
      USING (community_id, session_id)
    GROUP BY state_row.session_id, state_row.state_revision
    HAVING
      min(history.state_revision) <> 1 OR
      max(history.state_revision) <> state_row.state_revision OR
      count(history.state_revision) <> state_row.state_revision OR
      count(DISTINCT history.state_revision) <> state_row.state_revision
  ) broken;
"
)" || fail "failed to query State revision continuity"
if [[ "$broken_revision_histories" -ne 0 ]]; then
  log "State revision history is not contiguous"
  protocol_failures=$((protocol_failures + 1))
fi

underqualified_agents="$(
  db_scalar "
    SELECT count(*)
    FROM (
      SELECT participant.session_id, participant.pubkey
      FROM meeting_participants participant
      LEFT JOIN meeting_baton_grants grant_row
        ON grant_row.community_id=participant.community_id
        AND grant_row.session_id=participant.session_id
        AND grant_row.holder_pubkey=participant.pubkey
        AND grant_row.state='spoken'
      WHERE participant.participant_type='agent'
      GROUP BY participant.session_id, participant.pubkey
      HAVING count(grant_row.grant_id) < $speech_target
    ) missing;
  "
)"
if [[ "$underqualified_agents" -ne 0 ]]; then
  log "$underqualified_agents Agents did not reach $speech_target speeches"
  protocol_failures=$((protocol_failures + 1))
fi

if rg -n '\"status\":429|status=429|rate.?limited' "$run_dir/relay.log" \
  >"$run_dir/relay-429.txt"; then
  log "unexpected Relay 429 detected"
  protocol_failures=$((protocol_failures + 1))
fi

: >"$run_dir/model-proof.txt"
while IFS=$'\t' read -r role agent_pid effort; do
  agent_log="$run_dir/agents/$role.log"
  if ! rg -q 'agent_pool_ready agents=1' "$agent_log"; then
    log "$role missing agent_pool_ready"
    protocol_failures=$((protocol_failures + 1))
  fi
  if ! rg -q "applied model gpt-5\\.6-sol\\[$effort\\]" "$agent_log"; then
    log "$role missing applied model gpt-5.6-sol[$effort]"
    protocol_failures=$((protocol_failures + 1))
  fi
  if rg -n \
    'unsupported_model|desired model .* not found|failed to set model|authentication failed|agent pool initialization failed' \
    "$agent_log" >>"$run_dir/model-proof-errors.txt"; then
    log "$role has a model/auth startup error"
    protocol_failures=$((protocol_failures + 1))
  fi
  rg 'agent_pool_ready agents=1|applied model gpt-5\.6-sol' "$agent_log" \
    >>"$run_dir/model-proof.txt" || true
done <"$run_dir/agent-processes.tsv"

if [[ -f "$run_dir/model-proof-errors.txt" && ! -s "$run_dir/model-proof-errors.txt" ]]; then
  rm -- "$run_dir/model-proof-errors.txt"
fi

: >"$run_dir/runtime-anomalies.txt"
while IFS=$'\t' read -r role agent_pid effort; do
  agent_log="$run_dir/agents/$role.log"
  rg -nH \
    'agent_returned — respawning|agent_returned \(application error|Meeting V1 .* was not confirmed| ERROR ' \
    "$agent_log" >>"$run_dir/runtime-anomalies.txt" || true
done <"$run_dir/agent-processes.tsv"
runtime_anomalies="$(wc -l <"$run_dir/runtime-anomalies.txt" | tr -d ' ')"
if [[ "$runtime_anomalies" -ne 0 ]]; then
  log "$runtime_anomalies unexpected ACP runtime anomaly/anomalies detected"
  protocol_failures=$((protocol_failures + runtime_anomalies))
else
  rm -- "$run_dir/runtime-anomalies.txt"
fi

jq -n \
  --arg run_id "$run_id" \
  --arg tier "$tier" \
  --arg database "$database_name" \
  --arg relay "$relay_url" \
  --arg model "gpt-5.6-sol" \
  --arg adapter_version "$adapter_version" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg runner_sha256 "$runner_sha256" \
  --arg workspace_diff_sha256 "$workspace_diff_sha256" \
  --argjson meeting_count "$meeting_count" \
  --argjson speech_target "$speech_target" \
  --argjson runtime_anomalies "$runtime_anomalies" \
  --argjson protocol_failures "$protocol_failures" \
  '{
    run_id: $run_id,
    tier: $tier,
    qualification_sample: 1,
    formal_repetitions_required: 3,
    database: $database,
    relay: $relay,
    model: $model,
    moderator_reasoning: "max",
    participant_reasoning: "high",
    effort_evidence: "requested_catalog_supported_and_adapter_session_log",
    codex_acp_version: $adapter_version,
    buzz_commit: $commit,
    runner_sha256: $runner_sha256,
    tracked_workspace_diff_sha256: $workspace_diff_sha256,
    meeting_count: $meeting_count,
    speeches_per_agent_target: $speech_target,
    runtime_anomalies: $runtime_anomalies,
    protocol_failures: $protocol_failures,
    result: (if $protocol_failures == 0 then "pass" else "fail" end)
  }' >"$run_dir/manifest.json"

if [[ "$keep_database" != true && "$protocol_failures" -eq 0 ]]; then
  cleanup
  docker exec buzz-postgres psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE \"$database_name\"" >/dev/null
  printf '%s\n' "dropped" >"$run_dir/database-disposition.txt"
else
  printf '%s\n' "retained:$database_name" >"$run_dir/database-disposition.txt"
fi

if [[ "$protocol_failures" -ne 0 ]]; then
  fail "$tier qualification completed with $protocol_failures hard-gate failure(s)"
fi

log "PASS: $tier qualification sample completed"
log "artifacts: $run_dir"
