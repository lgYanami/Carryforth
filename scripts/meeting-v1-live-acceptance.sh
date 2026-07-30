#!/usr/bin/env bash
#
# Run one real-Codex Meeting V1 scale or Moderator qualification sample.
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
#   scripts/meeting-v1-live-acceptance.sh R-MOD-01 [artifact-root]
#   scripts/meeting-v1-live-acceptance.sh R-MOD-04-withdraw [artifact-root]
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
suite="scale"
scenario=""
scenario_variant=""

normalized_tier="$(printf '%s' "$tier" | tr '[:lower:]' '[:upper:]')"
case "$normalized_tier" in
  C6)
    tier="C6"
    meeting_agent_counts="3 3"
    redis_db=11
    ;;
  C10)
    tier="C10"
    meeting_agent_counts="4 3 3"
    redis_db=12
    ;;
  C12)
    tier="C12"
    meeting_agent_counts="4 4 4"
    redis_db=13
    ;;
  R-MOD-01|R-MOD-02|R-MOD-05|R-MOD-06)
    suite="moderator"
    scenario="$normalized_tier"
    tier="$normalized_tier"
    meeting_agent_counts="4"
    redis_db=14
    ;;
  R-MOD-03|R-MOD-03-REFRESH|R-MOD-03-WITHDRAW)
    suite="moderator"
    scenario="R-MOD-03"
    scenario_variant="$(
      printf '%s' "$normalized_tier" | awk -F- '{print tolower($NF)}'
    )"
    [[ "$scenario_variant" == 03 ]] && scenario_variant=refresh
    tier="${scenario}-${scenario_variant}"
    meeting_agent_counts="4"
    redis_db=14
    ;;
  R-MOD-04|R-MOD-04-REFRESH|R-MOD-04-WITHDRAW)
    suite="moderator"
    scenario="R-MOD-04"
    scenario_variant="$(
      printf '%s' "$normalized_tier" | awk -F- '{print tolower($NF)}'
    )"
    [[ "$scenario_variant" == 04 ]] && scenario_variant=withdraw
    tier="${scenario}-${scenario_variant}"
    meeting_agent_counts="4"
    redis_db=14
    ;;
  R-MOD-07)
    suite="moderator"
    scenario="R-MOD-07"
    tier="R-MOD-07"
    meeting_agent_counts="4 4 4"
    redis_db=15
    ;;
  *)
    echo "usage: $0 C6|C10|C12|R-MOD-01..07[-refresh|-withdraw] [artifact-root]" >&2
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
database_label="$(printf '%s' "$tier_lower" | tr -c '[:alnum:]' '_')"
database_name="buzz_meeting_live_${database_label}_${run_stamp//[^0-9]/}_$$"
community_id=""
relay_url="ws://localhost:3000"
database_url="postgres://buzz:buzz_dev@localhost:5432/$database_name"
redis_url="redis://localhost:6379/$redis_db"
cleanup_done=false

mkdir -p "$run_dir"/{agents,barriers,gates,meetings,observers,preflight}
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

for required in codex codex-acp curl docker jq npm pgrep ps rg shasum; do
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
if [[ "$suite" == moderator ]]; then
  [[ -x target/release/buzz-meeting-v1-acceptance-barrier ]] ||
    fail "missing acceptance binary: target/release/buzz-meeting-v1-acceptance-barrier"
fi

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
  scripts/meeting-v1-moderator-acceptance.sh \
  scripts/meeting-v1-moderator-gates.jq \
  >"$run_dir/preflight/executable-sha256.txt"
if [[ "$suite" == moderator ]]; then
  shasum -a 256 target/release/buzz-meeting-v1-acceptance-barrier \
    >>"$run_dir/preflight/executable-sha256.txt"
fi
runner_sha256="$(shasum -a 256 "$0" | awk '{print $1}')"
moderator_orchestrator_sha256="$(
  shasum -a 256 scripts/meeting-v1-moderator-acceptance.sh | awk '{print $1}'
)"
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

submit_fixture_intent() {
  local meeting_index="$1"
  local meeting_id="$2"
  local agent_index="$3"
  local role="m${meeting_index}-agent${agent_index}"
  local artifact="$run_dir/meetings/${role}-fixture-intent.json"
  local intent_id

  buzz_as "$role" meetings intents submit \
    --meeting "$meeting_id" \
    --summary "验收候选 ${role}：基于只读代码和权威 State，提供一个与当前会议目标直接相关、可被主持人立即选择的独立证据。" \
    >"$artifact"
  intent_id="$(jq -r '.intent_id // empty' "$artifact")"
  [[ "$intent_id" =~ ^[0-9a-f]{64}$ ]] ||
    fail "$role fixture Intent did not return a canonical intent_id"
  printf '%s\t%s\t%s\t%s\t%s\n' \
    "$meeting_index" "$meeting_id" "$role" "$(identity_public_key "$role")" "$intent_id" \
    >>"$run_dir/fixture-intents.tsv"
}

seed_moderator_scenario_intents() {
  : >"$run_dir/fixture-intents.tsv"
  while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
    case "$scenario" in
      R-MOD-01)
        submit_fixture_intent "$meeting_index" "$meeting_id" 2
        ;;
      R-MOD-02)
        submit_fixture_intent "$meeting_index" "$meeting_id" 3
        submit_fixture_intent "$meeting_index" "$meeting_id" 4
        ;;
      R-MOD-03|R-MOD-04|R-MOD-05)
        submit_fixture_intent "$meeting_index" "$meeting_id" 2
        submit_fixture_intent "$meeting_index" "$meeting_id" 3
        submit_fixture_intent "$meeting_index" "$meeting_id" 4
        ;;
      R-MOD-06)
        submit_fixture_intent "$meeting_index" "$meeting_id" 2
        submit_fixture_intent "$meeting_index" "$meeting_id" 3
        ;;
      R-MOD-07)
        case "$meeting_index" in
          1)
            submit_fixture_intent "$meeting_index" "$meeting_id" 3
            submit_fixture_intent "$meeting_index" "$meeting_id" 4
            ;;
          2|3)
            submit_fixture_intent "$meeting_index" "$meeting_id" 2
            submit_fixture_intent "$meeting_index" "$meeting_id" 3
            submit_fixture_intent "$meeting_index" "$meeting_id" 4
            ;;
        esac
        ;;
    esac
  done <"$run_dir/meetings.tsv"
}

if [[ "$suite" == moderator ]]; then
  log "seeding Relay-authoritative Candidate Cohort fixtures for $tier"
  seed_moderator_scenario_intents
fi

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
    if [[ "$suite" == moderator ]]; then
      role_focus="$role_focus 本场是主持协议专项验收：不要提交主持人自己的 SpeechIntent；Control Token 到手后，从 Relay 提供的有效 Candidate Cohort 中选择一个能直接推进讨论的参会者。"
    fi
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
  local barrier_socket=""

  if [[ "$agent_index" -eq 1 ]]; then
    effort=max
  fi
  private_key="$(identity_private_key "$role")"
  team_instructions="$(agent_team_instructions "$role" "$meeting_index" "$agent_index")"
  if [[ "$agent_index" -eq 1 ]] && meeting_uses_barrier "$meeting_index"; then
    barrier_socket="$secret_dir/m${meeting_index}.sock"
  fi

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
    BUZZ_ACP_MEETING_V1_ACCEPTANCE_EVENTS_PATH="$run_dir/agents/$role-events.ndjson" \
    BUZZ_ACP_MEETING_V1_PRE_SUBMIT_BARRIER_SOCKET="$barrier_socket" \
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

meeting_uses_barrier() {
  local meeting_index="$1"
  case "$scenario" in
    R-MOD-03|R-MOD-04|R-MOD-06)
      return 0
      ;;
    R-MOD-07)
      [[ "$meeting_index" -eq 2 ]]
      return
      ;;
    *)
      return 1
      ;;
  esac
}

start_acceptance_barrier() {
  local meeting_index="$1"
  local socket_path="$secret_dir/m${meeting_index}.sock"
  local evidence_path="$run_dir/barriers/m${meeting_index}.ndjson"

  target/release/buzz-meeting-v1-acceptance-barrier serve \
    --socket "$socket_path" \
    --events "$evidence_path" \
    >"$run_dir/barriers/m${meeting_index}.log" 2>&1 &
  local barrier_pid=$!
  printf '%s\t%s\t%s\t%s\n' \
    "$meeting_index" "$barrier_pid" "$socket_path" "$evidence_path" \
    >>"$run_dir/barrier-processes.tsv"

  barrier_ready=false
  for ignored_attempt in $(seq 1 100); do
    if [[ -S "$socket_path" ]]; then
      barrier_ready=true
      break
    fi
    if ! kill -0 "$barrier_pid" 2>/dev/null; then
      fail "Meeting $meeting_index acceptance barrier exited during startup"
    fi
    sleep 0.05
  done
  [[ "$barrier_ready" == true ]] ||
    fail "Meeting $meeting_index acceptance barrier did not bind its socket"
}

log "starting authenticated State subscriptions and all real Codex ACP runtimes"
: >"$run_dir/agent-processes.tsv"
: >"$run_dir/barrier-processes.tsv"
while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
  start_observer "$meeting_index" "$meeting_id" "$observer_role"
  if meeting_uses_barrier "$meeting_index"; then
    start_acceptance_barrier "$meeting_index"
  fi
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

capture_process_node() {
  local phase="$1"
  local role="$2"
  local process_id="$3"
  local depth="$4"
  local process_row
  local observed_pid
  local parent_pid
  local elapsed
  local command_name
  local child_pid

  (( depth <= 8 )) || return
  process_row="$(ps -p "$process_id" -o pid= -o ppid= -o etime= -o comm= 2>/dev/null || true)"
  [[ -n "$process_row" ]] || return
  read -r observed_pid parent_pid elapsed command_name <<EOF
$process_row
EOF
  jq -cn \
    --arg timestamp "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg phase "$phase" \
    --arg role "$role" \
    --argjson pid "$observed_pid" \
    --argjson ppid "$parent_pid" \
    --arg elapsed "$elapsed" \
    --arg command "$command_name" \
    --argjson depth "$depth" \
    '{
      timestamp: $timestamp,
      phase: $phase,
      role: $role,
      pid: $pid,
      ppid: $ppid,
      elapsed: $elapsed,
      command: $command,
      depth: $depth
    }' >>"$run_dir/process-tree.ndjson"
  for child_pid in $(pgrep -P "$process_id" 2>/dev/null || true); do
    capture_process_node "$phase" "$role" "$child_pid" $((depth + 1))
  done
}

capture_process_tree() {
  local phase="$1"
  local role
  local agent_pid
  local effort

  while IFS=$'\t' read -r role agent_pid effort; do
    capture_process_node "$phase" "$role" "$agent_pid" 0
  done <"$run_dir/agent-processes.tsv"
}

: >"$run_dir/process-tree.ndjson"
capture_process_tree start

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

event_count() {
  local role="$1"
  local kind="$2"
  local path="$run_dir/agents/$role-events.ndjson"
  if [[ ! -s "$path" ]]; then
    printf '0\n'
    return
  fi
  jq -sc --arg kind "$kind" '[.[] | select(.kind == $kind)] | length' \
    "$path" 2>/dev/null || printf '0\n'
}

record_scenario_pass() {
  local gate_name="$1"
  local observed="$2"
  local expected="$3"
  local gate_path="$run_dir/gates/scenario-${gate_name}.json"
  jq -cn \
    --arg gate "$gate_name" \
    --arg observed "$observed" \
    --arg expected "$expected" \
    '{gate: $gate, pass: true, observed: $observed, expected: $expected}' \
    >"$gate_path"
}

wait_for_decision_start() {
  local meeting_index="$1"
  local minimum_count="${2:-1}"
  local role="m${meeting_index}-agent1"
  local path="$run_dir/agents/$role-events.ndjson"
  local poll_count=0
  local count

  while true; do
    count="$(event_count "$role" meeting_v1_moderator_decision_started)"
    if [[ "$count" -ge "$minimum_count" ]]; then
      LAST_EVENT_JSON="$(
        jq -sc '
          [.[] | select(.kind == "meeting_v1_moderator_decision_started")] | last
        ' "$path"
      )"
      return
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 1900 )); then
      fail "Meeting $meeting_index did not dispatch a Moderator Decision before its window"
    fi
    sleep 0.1
  done
}

wait_for_local_intent_revision() {
  local meeting_index="$1"
  local minimum_revision="$2"
  local role="m${meeting_index}-agent1"
  local path="$run_dir/agents/$role-events.ndjson"
  local poll_count=0

  while true; do
    if [[ -s "$path" ]] && jq -se --argjson revision "$minimum_revision" '
      any(
        .[];
        (.kind == "meeting_v1_state_applied" or .kind == "meeting_v1_sync_completed")
        and (.payload.intent_revision // 0) >= $revision
      )
    ' "$path" >/dev/null 2>&1; then
      return
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 300 )); then
      fail "Meeting $meeting_index moderator did not observe intent_revision $minimum_revision"
    fi
    sleep 0.1
  done
}

wait_for_attempt_event() {
  local meeting_index="$1"
  local kind="$2"
  local attempt_id="$3"
  local role="m${meeting_index}-agent1"
  local path="$run_dir/agents/$role-events.ndjson"
  local poll_count=0

  while true; do
    if [[ -s "$path" ]] && jq -se \
      --arg kind "$kind" \
      --arg attempt "$attempt_id" '
        any(.[]; .kind == $kind and .payload.attempt_id == $attempt)
      ' "$path" >/dev/null 2>&1; then
      return
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 2400 )); then
      fail "Meeting $meeting_index did not emit $kind for attempt $attempt_id"
    fi
    sleep 0.1
  done
}

wait_for_candidate_in_later_attempt() {
  local meeting_index="$1"
  local source_id="$2"
  local excluded_attempt="$3"
  local role="m${meeting_index}-agent1"
  local path="$run_dir/agents/$role-events.ndjson"
  local poll_count=0

  while true; do
    if [[ -s "$path" ]] && jq -se \
      --arg source "$source_id" \
      --arg excluded "$excluded_attempt" '
        any(
          .[];
          .kind == "meeting_v1_moderator_attempt_registered"
          and .payload.attempt_id != $excluded
          and any(.payload.candidate_sources[]?; .source_id == $source)
        )
      ' "$path" >/dev/null 2>&1; then
      return
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 6000 )); then
      fail "Meeting $meeting_index late Intent never entered a later Candidate Cohort"
    fi
    sleep 0.1
  done
}

wait_for_barrier() {
  local meeting_index="$1"
  local role="m${meeting_index}-agent1"
  local path="$run_dir/barriers/m${meeting_index}.ndjson"
  local event_path="$run_dir/agents/$role-events.ndjson"
  local poll_count=0
  local latest_turn=""
  local deadline_seconds=0
  local now_seconds

  while true; do
    if [[ -s "$path" ]]; then
      LAST_BARRIER_JSON="$(
        jq -sc '[.[] | select(.frame_type == "meeting_v1_pre_submit")] | last' "$path"
      )"
      if [[ "$(jq -r '.action_kind // empty' <<<"$LAST_BARRIER_JSON")" != select_intent ]]; then
        inconclusive "Meeting $meeting_index model did not exercise select_intent"
      fi
      return
    fi
    if [[ -s "$event_path" ]]; then
      latest_turn="$(
        jq -sr '
          [.[] | select(.kind == "meeting_v1_moderator_decision_started")]
          | last
          | .turnId // empty
        ' "$event_path"
      )"
      deadline_seconds="$(
        jq -sr '
          [.[] | select(.kind == "meeting_v1_moderator_decision_started")]
          | last
          | ((.payload.attempt_deadline_ms // 0) / 1000 | floor)
        ' "$event_path"
      )"
      if [[ -n "$latest_turn" ]] && jq -se --arg turn "$latest_turn" '
        any(.[]; .kind == "prompt_terminal" and .turnId == $turn)
      ' "$event_path" >/dev/null 2>&1; then
        inconclusive "Meeting $meeting_index provider terminal did not exercise a primary Select"
      fi
      now_seconds="$(date -u +%s)"
      if [[ "$deadline_seconds" -gt 0 && "$now_seconds" -ge "$deadline_seconds" ]]; then
        inconclusive "Meeting $meeting_index reached its authoritative attempt deadline without a primary Select"
      fi
    fi
    poll_count=$((poll_count + 1))
    if (( poll_count > 1900 )); then
      inconclusive "Meeting $meeting_index model produced no primary Select before deadline"
    fi
    sleep 0.1
  done
}

release_barrier() {
  local meeting_index="$1"
  local token="$2"
  local socket_path="$secret_dir/m${meeting_index}.sock"
  target/release/buzz-meeting-v1-acceptance-barrier release \
    --socket "$socket_path" \
    --token "$token" \
    >"$run_dir/barriers/m${meeting_index}-release.json"
}

inconclusive() {
  log "INCONCLUSIVE: $*"
  printf '%s\n' "$*" >"$run_dir/inconclusive.txt"
  exit 3
}

role_for_pubkey() {
  local public_key="$1"
  awk -F '\t' -v wanted="$public_key" '$4 == wanted { print $1; exit }' "$identity_file"
}

mutate_intent() {
  local meeting_index="$1"
  local meeting_id="$2"
  local role="$3"
  local intent_id="$4"
  local mutation="$5"
  local label="$6"
  local before_revision
  local current_revision

  before_revision="$(
    db_scalar "SELECT intent_revision FROM meeting_baton_state WHERE session_id='$meeting_id';"
  )"
  case "$mutation" in
    refresh)
      buzz_as "$role" meetings intents refresh \
        --meeting "$meeting_id" \
        --intent "$intent_id" \
        --summary "Acceptance refresh $label：保留稳定 intent_id，但以新的权威 source version 提供同一独立证据。" \
        >"$run_dir/meetings/$label-refresh.json"
      ;;
    withdraw)
      buzz_as "$role" meetings intents withdraw \
        --meeting "$meeting_id" \
        --intent "$intent_id" \
        >"$run_dir/meetings/$label-withdraw.json"
      ;;
    *)
      fail "unknown Intent mutation: $mutation"
      ;;
  esac

  current_revision="$before_revision"
  for ignored_attempt in $(seq 1 200); do
    current_revision="$(
      db_scalar "SELECT intent_revision FROM meeting_baton_state WHERE session_id='$meeting_id';"
    )"
    [[ "$current_revision" -gt "$before_revision" ]] && break
    sleep 0.05
  done
  [[ "$current_revision" -gt "$before_revision" ]] ||
    fail "$label did not advance canonical intent_revision"
  wait_for_local_intent_revision "$meeting_index" "$current_revision"
  LAST_MUTATION_REVISION="$current_revision"
}

wait_for_agent_speech() {
  local meeting_id="$1"
  local role="$2"
  local public_key
  local poll_count=0

  public_key="$(identity_public_key "$role")"
  while [[ "$(agent_speech_count "$meeting_id" "$public_key")" -lt 1 ]]; do
    poll_count=$((poll_count + 1))
    if (( poll_count > 6000 )); then
      fail "$role did not complete a canonical speech"
    fi
    sleep 0.1
  done
}

submit_human_request_only() {
  local role="$1"
  local meeting_id="$2"
  local label="$3"
  buzz_as "$role" meetings floor request --meeting "$meeting_id" \
    >"$run_dir/meetings/$label-request.json"
  LAST_REQUEST_ID="$(jq -r '.request_id // empty' "$run_dir/meetings/$label-request.json")"
  [[ "$LAST_REQUEST_ID" =~ ^[0-9a-f]{64}$ ]] ||
    fail "$label Human Request did not return a canonical request_id"
}

complete_human_request() {
  local role="$1"
  local meeting_id="$2"
  local label="$3"
  local request_id="$4"
  local request_row
  local request_state
  local offer_id
  local poll_count=0

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
    [[ "$request_state" == queued ]] ||
      fail "$label Human Request became $request_state before ACK"
    poll_count=$((poll_count + 1))
    if (( poll_count > 2400 )); then
      fail "$label Human Offer did not arrive"
    fi
    sleep 0.1
  done
  buzz_as "$role" meetings offer ack --meeting "$meeting_id" --offer "$offer_id" \
    >"$run_dir/meetings/$label-ack.json"
  buzz_as "$role" meetings say \
    --meeting "$meeting_id" \
    --content "Human priority acceptance speech：旧主持判断不得形成 canonical action；控制权归还后才允许建立新快照。" \
    >"$run_dir/meetings/$label-say.json"
}

run_rmod_01() {
  local meeting_index="$1"
  local meeting_id="$2"
  local holder_role="m${meeting_index}-agent2"
  local holder_public_key
  local active_holder
  local before_count
  local poll_count=0

  holder_public_key="$(identity_public_key "$holder_role")"
  while true; do
    active_holder="$(
      db_scalar "
        SELECT COALESCE(encode(holder_pubkey, 'hex'), '')
        FROM meeting_baton_grants
        WHERE session_id='$meeting_id' AND state='active'
        LIMIT 1;
      "
    )"
    [[ "$active_holder" == "$holder_public_key" ]] && break
    poll_count=$((poll_count + 1))
    if (( poll_count > 6000 )); then
      inconclusive "R-MOD-01 did not acquire the required Participant A Grant"
    fi
    sleep 0.1
  done
  before_count="$(event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_started)"
  submit_fixture_intent "$meeting_index" "$meeting_id" 3
  submit_fixture_intent "$meeting_index" "$meeting_id" 4
  active_holder="$(
    db_scalar "
      SELECT COALESCE(encode(holder_pubkey, 'hex'), '')
      FROM meeting_baton_grants
      WHERE session_id='$meeting_id' AND state='active'
      LIMIT 1;
    "
  )"
  [[ "$active_holder" == "$holder_public_key" ]] ||
    inconclusive "R-MOD-01 fixture Intents did not become canonical before the Grant ended"
  [[ "$(event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_started)" -eq "$before_count" ]] ||
    fail "Moderator dispatched while another participant held the Grant"
  record_scenario_pass \
    "r_mod_01_no_dispatch_while_granted" "$before_count" "unchanged while Grant active"
  wait_for_agent_speech "$meeting_id" "$holder_role"
  wait_for_decision_start "$meeting_index" $((before_count + 1))
  record_scenario_pass \
    "r_mod_01_dispatch_after_control_return" "$((before_count + 1))" ">=1 later dispatch"
}

run_rmod_02() {
  local meeting_index="$1"
  local meeting_id="$2"
  local moderator_role="m${meeting_index}-agent1"
  local start
  local attempt_id
  local turn_id
  local decision_epoch
  local late_intent_id
  local late_revision
  local eligible_epoch
  local late_state
  local late_selection_count

  wait_for_decision_start "$meeting_index" 1
  start="$LAST_EVENT_JSON"
  attempt_id="$(jq -r '.payload.attempt_id' <<<"$start")"
  turn_id="$(jq -r '.turnId' <<<"$start")"
  decision_epoch="$(jq -r '.payload.decision_epoch' <<<"$start")"
  submit_fixture_intent "$meeting_index" "$meeting_id" 2
  late_intent_id="$(
    awk -F '\t' -v role="m${meeting_index}-agent2" '$3 == role { value=$5 } END { print value }' \
      "$run_dir/fixture-intents.tsv"
  )"
  late_revision="$(
    db_scalar "SELECT intent_revision FROM meeting_baton_state WHERE session_id='$meeting_id';"
  )"
  if jq -e --arg source "$late_intent_id" '
    any(.payload.candidate_sources[]?; .source_id == $source)
  ' <<<"$start" >/dev/null; then
    fail "R-MOD-02 late Intent appeared in the frozen first Candidate Cohort"
  fi
  wait_for_local_intent_revision "$meeting_index" "$late_revision"
  if jq -se --arg turn "$turn_id" '
    any(.[]; .kind == "prompt_terminal" and .turnId == $turn)
  ' "$run_dir/agents/$moderator_role-events.ndjson" >/dev/null 2>&1; then
    inconclusive "R-MOD-02 late Intent was not observed before the provider terminal"
  fi
  eligible_epoch="$(
    db_scalar "
      SELECT eligible_decision_epoch
      FROM meeting_speech_intents
      WHERE intent_id=decode('$late_intent_id', 'hex');
    "
  )"
  [[ "$eligible_epoch" -gt "$decision_epoch" ]] ||
    fail "R-MOD-02 late Intent was admitted to the running decision epoch"
  record_scenario_pass \
    "r_mod_02_late_intent_next_epoch" "$eligible_epoch" ">$decision_epoch"
  wait_for_attempt_event \
    "$meeting_index" meeting_v1_moderator_decision_completed "$attempt_id"
  IFS='|' read -r late_state late_selection_count <<EOF
$(
  db_scalar "
    SELECT state, selection_attempt_count
    FROM meeting_speech_intents
    WHERE intent_id=decode('$late_intent_id', 'hex');
  "
)
EOF
  [[ "$late_state" == pending && "$late_selection_count" -eq 0 ]] ||
    fail "R-MOD-02 late Intent was not pending with selection_attempt_count=0 after the first Turn"
  record_scenario_pass \
    "r_mod_02_late_intent_unselected_in_first_turn" \
    "$late_state/$late_selection_count" "pending/0"
  wait_for_candidate_in_later_attempt "$meeting_index" "$late_intent_id" "$attempt_id"
  record_scenario_pass \
    "r_mod_02_late_intent_enters_later_cohort" "$late_intent_id" "present in a later attempt"
}

run_barrier_source_mutation() {
  local meeting_index="$1"
  local meeting_id="$2"
  local mutate_selected="$3"
  local mutation="$4"
  local selected_id
  local target_id
  local target_pubkey
  local target_role
  local token
  local attempt_id
  local retry_count
  local poll_count=0
  local selected_offer_count
  local first_completed_seq
  local retry_json
  local retry_attempt_id
  local retry_attempt_number
  local retry_started_seq
  local old_selected_event_id
  local retry_ticket_row
  local retry_ticket_id
  local failed_action_event_id
  local reuse_status

  wait_for_barrier "$meeting_index"
  attempt_id="$(jq -r '.attempt_id' <<<"$LAST_BARRIER_JSON")"
  selected_id="$(jq -r '.selected_source_id' <<<"$LAST_BARRIER_JSON")"
  old_selected_event_id="$(jq -r '.selected_source_event_id // empty' <<<"$LAST_BARRIER_JSON")"
  if [[ "$mutate_selected" == true ]]; then
    target_id="$selected_id"
  else
    target_id="$(
      jq -r --arg selected "$selected_id" '
        [
          .candidate_cohort[]
          | select(.source_type == "intent" and .source_id != $selected)
          | .source_id
        ][0] // empty
      ' <<<"$LAST_BARRIER_JSON"
    )"
  fi
  [[ "$target_id" =~ ^[0-9a-f]{64}$ ]] ||
    inconclusive "Barrier Cohort did not expose the required Intent mutation target"
  target_pubkey="$(
    jq -r --arg target "$target_id" '
      [.candidate_cohort[] | select(.source_id == $target) | .author_pubkey][0] // empty
    ' <<<"$LAST_BARRIER_JSON"
  )"
  target_role="$(role_for_pubkey "$target_pubkey")"
  [[ -n "$target_role" ]] || fail "could not map barrier candidate author to a fixture role"
  mutate_intent \
    "$meeting_index" "$meeting_id" "$target_role" "$target_id" "$mutation" \
    "m${meeting_index}-barrier-${mutation}"
  token="$(jq -r '.token' <<<"$LAST_BARRIER_JSON")"
  release_barrier "$meeting_index" "$token"

  if [[ "$mutate_selected" == true ]]; then
    wait_for_attempt_event \
      "$meeting_index" meeting_v1_moderator_decision_retry_requested "$attempt_id"
    retry_count="$(
      event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_retry_started
    )"
    while [[ "$retry_count" -lt 1 ]]; do
      poll_count=$((poll_count + 1))
      if (( poll_count > 3000 )); then
        fail "selected-source conflict did not start its bounded retry within 5 minutes"
      fi
      sleep 0.1
      retry_count="$(
        event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_retry_started
      )"
    done
    [[ "$retry_count" -eq 1 ]] ||
      fail "selected-source conflict did not converge to exactly one retry"
    retry_json="$(
      jq -sc '
        [.[] | select(.kind == "meeting_v1_moderator_decision_retry_started")]
        | last
      ' "$run_dir/agents/m${meeting_index}-agent1-events.ndjson"
    )"
    retry_attempt_id="$(jq -r '.payload.attempt_id' <<<"$retry_json")"
    retry_attempt_number="$(jq -r '.payload.attempt_number' <<<"$retry_json")"
    retry_started_seq="$(jq -r '.seq' <<<"$retry_json")"
    first_completed_seq="$(
      jq -sr --arg attempt "$attempt_id" '
        [
          .[]
          | select(
              .kind == "meeting_v1_moderator_decision_completed"
              and .payload.attempt_id == $attempt
            )
        ]
        | last
        | .seq // 0
      ' "$run_dir/agents/m${meeting_index}-agent1-events.ndjson"
    )"
    [[ "$retry_started_seq" -gt "$first_completed_seq" ]] ||
      fail "selected-source retry started before the first provider Turn naturally completed"
    case "$mutation" in
      refresh)
        jq -e \
          --arg source "$selected_id" \
          --arg old_event "$old_selected_event_id" '
            any(
              .payload.candidate_sources[];
              .source_id == $source
              and .current_event_id != $old_event
            )
          ' <<<"$retry_json" >/dev/null ||
          fail "selected-source Refresh retry did not use the refreshed source version"
        ;;
      withdraw)
        if jq -e --arg source "$selected_id" '
          any(.payload.candidate_sources[]; .source_id == $source)
        ' <<<"$retry_json" >/dev/null; then
          fail "selected-source Withdraw retry reused the withdrawn source"
        fi
        ;;
    esac
    retry_ticket_row="$(
      db_scalar "
        SELECT
          encode(retry_ticket_id, 'hex'),
          encode(failed_action_event_id, 'hex')
        FROM meeting_moderator_retry_tickets
        WHERE session_id='$meeting_id'
          AND attempt_id=decode('$attempt_id', 'hex');
      "
    )"
    retry_ticket_id="${retry_ticket_row%%|*}"
    failed_action_event_id="${retry_ticket_row#*|}"
    [[ "$retry_ticket_id" =~ ^[0-9a-f]{64}$ &&
      "$failed_action_event_id" =~ ^[0-9a-f]{64}$ ]] ||
      fail "selected-source retry evidence is missing its authoritative ticket binding"
    if buzz_as "m${meeting_index}-agent1" meetings moderator retry \
      --meeting "$meeting_id" \
      --attempt "$retry_attempt_id" \
      --ticket "$retry_ticket_id" \
      --failed-action "$failed_action_event_id" \
      --attempt-number "$retry_attempt_number" \
      >"$run_dir/meetings/m${meeting_index}-retry-ticket-reuse.txt" 2>&1; then
      fail "Relay accepted a second consumption of one Moderator retry ticket"
    else
      reuse_status=$?
    fi
    [[ "$reuse_status" -eq 5 ]] ||
      fail "retry ticket reuse did not return the expected write-conflict exit code"
    rg -q 'retry_ticket_already_consumed' \
      "$run_dir/meetings/m${meeting_index}-retry-ticket-reuse.txt" ||
      fail "retry ticket reuse was not rejected as retry_ticket_already_consumed"
    wait_for_attempt_event \
      "$meeting_index" meeting_v1_moderator_decision_completed "$retry_attempt_id"
    record_scenario_pass \
      "selected_source_conflict_exactly_one_retry" \
      "retry=$retry_count,new_attempt=$retry_attempt_id" \
      "one later natural retry using the current source version"
  else
    wait_for_attempt_event \
      "$meeting_index" meeting_v1_moderator_decision_committed "$attempt_id"
    selected_offer_count="$(
      db_scalar "
        SELECT count(*)
        FROM meeting_baton_offers
        WHERE session_id='$meeting_id'
          AND source_intent_id=decode('$selected_id', 'hex');
      "
    )"
    [[ "$selected_offer_count" -ge 1 ]] ||
      fail "R-MOD-03 selected source did not form a canonical Offer"
    [[ "$(event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_retry_started)" -eq 0 ]] ||
      fail "unselected-source mutation incorrectly started a new model attempt"
    record_scenario_pass \
      "unselected_source_change_no_model_retry" \
      "retry=0,offer=$selected_offer_count" "retry=0 and offer>=1"
  fi
}

run_rmod_05() {
  local meeting_index="$1"
  local meeting_id="$2"
  local host_role="$3"
  local start
  local attempt_id
  local turn_id
  local before_revision
  local request_revision

  wait_for_decision_start "$meeting_index" 1
  start="$LAST_EVENT_JSON"
  attempt_id="$(jq -r '.payload.attempt_id' <<<"$start")"
  turn_id="$(jq -r '.turnId' <<<"$start")"
  before_revision="$(
    db_scalar "SELECT intent_revision FROM meeting_baton_state WHERE session_id='$meeting_id';"
  )"
  submit_human_request_only "$host_role" "$meeting_id" "m${meeting_index}-human-priority"
  request_revision="$(
    db_scalar "SELECT intent_revision FROM meeting_baton_state WHERE session_id='$meeting_id';"
  )"
  [[ "$request_revision" -gt "$before_revision" ]] ||
    fail "Human Request did not advance canonical intent_revision"
  wait_for_local_intent_revision "$meeting_index" "$request_revision"
  if jq -se --arg turn "$turn_id" '
    any(.[]; .kind == "prompt_terminal" and .turnId == $turn)
  ' "$run_dir/agents/m${meeting_index}-agent1-events.ndjson" >/dev/null 2>&1; then
    inconclusive "R-MOD-05 Human Request was not observed before the provider terminal"
  fi
  complete_human_request \
    "$host_role" "$meeting_id" "m${meeting_index}-human-priority" "$LAST_REQUEST_ID"
  wait_for_attempt_event \
    "$meeting_index" meeting_v1_moderator_decision_discarded "$attempt_id"
  wait_for_decision_start "$meeting_index" 2
  [[ "$(jq -r '.payload.attempt_id' <<<"$LAST_EVENT_JSON")" != "$attempt_id" ]] ||
    fail "Human speech did not produce a fresh Moderator DecisionAttempt"
  record_scenario_pass \
    "r_mod_05_human_priority_fences_old_result" "$attempt_id" "discarded before fresh attempt"
}

run_rmod_06() {
  local meeting_index="$1"
  local meeting_id="$2"
  local late_id

  wait_for_decision_start "$meeting_index" 1
  submit_fixture_intent "$meeting_index" "$meeting_id" 4
  late_id="$(
    awk -F '\t' -v role="m${meeting_index}-agent4" '$3 == role { value=$5 } END { print value }' \
      "$run_dir/fixture-intents.tsv"
  )"
  mutate_intent \
    "$meeting_index" "$meeting_id" "m${meeting_index}-agent4" "$late_id" refresh \
    "m${meeting_index}-late-burst-1"
  mutate_intent \
    "$meeting_index" "$meeting_id" "m${meeting_index}-agent4" "$late_id" refresh \
    "m${meeting_index}-late-burst-2"
  run_barrier_source_mutation "$meeting_index" "$meeting_id" true withdraw
  [[ "$(event_count "m${meeting_index}-agent1" meeting_v1_moderator_decision_retry_started)" -eq 1 ]] ||
    fail "R-MOD-06 State burst did not coalesce to exactly one retry"
  record_scenario_pass \
    "r_mod_06_state_burst_coalesces" "1" "exactly one retry"
}

run_moderator_scenario() {
  local first_turn_id
  local first_turn_ids
  local meeting_turn
  local scenario_pid
  local scenario_pids

  case "$scenario" in
    R-MOD-01)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_rmod_01 "$meeting_index" "$meeting_id"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-02)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_rmod_02 "$meeting_index" "$meeting_id"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-03)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_barrier_source_mutation \
          "$meeting_index" "$meeting_id" false "$scenario_variant"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-04)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_barrier_source_mutation \
          "$meeting_index" "$meeting_id" true "$scenario_variant"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-05)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_rmod_05 "$meeting_index" "$meeting_id" "$host_role"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-06)
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        run_rmod_06 "$meeting_index" "$meeting_id"
      done <"$run_dir/meetings.tsv"
      ;;
    R-MOD-07)
      first_turn_ids=""
      for meeting_index in 1 2 3; do
        wait_for_decision_start "$meeting_index" 1
        first_turn_id="$(jq -r '.turnId' <<<"$LAST_EVENT_JSON")"
        first_turn_ids="$first_turn_ids $meeting_index:$first_turn_id"
      done
      for meeting_turn in $first_turn_ids; do
        meeting_index="${meeting_turn%%:*}"
        first_turn_id="${meeting_turn#*:}"
        if jq -se --arg turn "$first_turn_id" '
          any(.[]; .kind == "prompt_terminal" and .turnId == $turn)
        ' "$run_dir/agents/m${meeting_index}-agent1-events.ndjson" >/dev/null 2>&1; then
          inconclusive "R-MOD-07 did not acquire three overlapping Moderator provider Turns"
        fi
      done
      record_scenario_pass \
        "r_mod_07_three_moderator_turns_overlap" \
        "meetings=1,2,3 all non-terminal at synchronization point" \
        "three simultaneous in-flight provider Turns"

      scenario_pids=""
      while IFS=$'\t' read -r meeting_index meeting_id host_role observer_role agent_count; do
        case "$meeting_index" in
          1)
            run_rmod_02 "$meeting_index" "$meeting_id" \
              >"$run_dir/meetings/m${meeting_index}-rmod-driver.log" 2>&1 &
            ;;
          2)
            run_barrier_source_mutation "$meeting_index" "$meeting_id" true withdraw \
              >"$run_dir/meetings/m${meeting_index}-rmod-driver.log" 2>&1 &
            ;;
          3)
            run_rmod_05 "$meeting_index" "$meeting_id" "$host_role" \
              >"$run_dir/meetings/m${meeting_index}-rmod-driver.log" 2>&1 &
            ;;
        esac
        scenario_pids="$scenario_pids $!"
      done <"$run_dir/meetings.tsv"
      for scenario_pid in $scenario_pids; do
        wait "$scenario_pid" || fail "an R-MOD-07 scenario driver failed"
      done
      ;;
  esac
}

if [[ "$suite" == scale ]]; then
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
else
  log "running $tier structured Moderator scenario"
  run_moderator_scenario
  log "$tier structured Moderator scenario completed"
fi

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

if [[ "$suite" == scale ]]; then
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
fi

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

wait_for_moderator_turns_to_settle() {
  local pending=0
  local role
  local event_path
  local started
  local completed
  local prompt_terminals
  local dispositions
  local all_started
  local all_terminals
  local agent_pid
  local effort
  local meeting_index

  for ignored_attempt in $(seq 1 3600); do
    pending=0
    meeting_index=1
    for ignored_count in $meeting_agent_counts; do
      role="m${meeting_index}-agent1"
      event_path="$run_dir/agents/$role-events.ndjson"
      if [[ ! -s "$event_path" ]]; then
        pending=$((pending + 1))
        meeting_index=$((meeting_index + 1))
        continue
      fi
      started="$(event_count "$role" meeting_v1_moderator_decision_started)"
      completed="$(event_count "$role" meeting_v1_moderator_decision_completed)"
      prompt_terminals="$(
        jq -sr '
          [.[]
            | select(.kind == "meeting_v1_moderator_decision_started")
            | .turnId] as $turns
          | [.[]
              | select(
                  .kind == "prompt_terminal"
                  and (.turnId as $turn | $turns | index($turn)) != null
                )]
          | length
        ' "$event_path"
      )"
      dispositions="$(
        jq -sr '
          [.[]
            | select(
                .kind == "meeting_v1_moderator_decision_committed"
                or .kind == "meeting_v1_moderator_decision_discarded"
                or .kind == "meeting_v1_moderator_decision_retry_requested"
              )]
          | length
        ' "$event_path"
      )"
      if [[ "$started" -ne "$completed" ||
        "$started" -ne "$prompt_terminals" ||
        "$started" -ne "$dispositions" ]]; then
        pending=$((pending + 1))
      fi
      meeting_index=$((meeting_index + 1))
    done
    while IFS=$'\t' read -r role agent_pid effort; do
      event_path="$run_dir/agents/$role-events.ndjson"
      if [[ ! -s "$event_path" ]]; then
        pending=$((pending + 1))
        continue
      fi
      all_started="$(event_count "$role" turn_started)"
      all_terminals="$(event_count "$role" prompt_terminal)"
      if [[ "$all_started" -ne "$all_terminals" ]]; then
        pending=$((pending + 1))
      fi
    done <"$run_dir/agent-processes.tsv"
    [[ "$pending" -eq 0 ]] && return
    sleep 0.1
  done
  fail "$pending Moderator provider Turn(s) did not reach a terminal within 6 minutes"
}

verify_final_retry_count() {
  local expected="$1"
  local observed=0
  local count
  local meeting_index
  local role
  local ticket_count
  local unconsumed_tickets
  local invalid_ticket_code

  meeting_index=1
  for ignored_count in $meeting_agent_counts; do
    role="m${meeting_index}-agent1"
    count="$(event_count "$role" meeting_v1_moderator_decision_retry_started)"
    observed=$((observed + count))
    meeting_index=$((meeting_index + 1))
  done
  [[ "$observed" -eq "$expected" ]] ||
    fail "$tier finished with $observed Moderator retries; expected exactly $expected"
  ticket_count="$(
    db_scalar "SELECT count(*) FROM meeting_moderator_retry_tickets;"
  )"
  unconsumed_tickets="$(
    db_scalar "
      SELECT count(*)
      FROM meeting_moderator_retry_tickets
      WHERE consumed_at IS NULL OR consumed_by_event_id IS NULL;
    "
  )"
  invalid_ticket_code="$(
    db_scalar "
      SELECT count(*)
      FROM meeting_moderator_retry_tickets
      WHERE conflict_code <> 'selected_source_changed';
    "
  )"
  [[ "$ticket_count" -eq "$expected" ]] ||
    fail "$tier created $ticket_count retry tickets; expected exactly $expected"
  [[ "$unconsumed_tickets" -eq 0 && "$invalid_ticket_code" -eq 0 ]] ||
    fail "$tier retained an unconsumed or non-selected-source retry ticket"
  record_scenario_pass \
    "final_moderator_retry_count" \
    "retries=$observed,tickets=$ticket_count,unconsumed=$unconsumed_tickets" \
    "exactly $expected selected-source retry/retry-ticket pair(s)"
}

if [[ "$suite" == moderator ]]; then
  log "waiting for every ACP Turn and Moderator disposition to settle"
  wait_for_moderator_turns_to_settle
  case "$scenario" in
    R-MOD-03) verify_final_retry_count 0 ;;
    R-MOD-04|R-MOD-06|R-MOD-07) verify_final_retry_count 1 ;;
    *) verify_final_retry_count 0 ;;
  esac
fi

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

post_end_before="$(
  db_scalar "
    SELECT string_agg(
      session_id::text || ':' || state_revision::text || ':' || speech_revision::text,
      ',' ORDER BY session_id
    )
    FROM meeting_baton_state;
  "
)"
sleep 2
post_end_after="$(
  db_scalar "
    SELECT string_agg(
      session_id::text || ':' || state_revision::text || ':' || speech_revision::text,
      ',' ORDER BY session_id
    )
    FROM meeting_baton_state;
  "
)"
printf 'before=%s\nafter=%s\n' "$post_end_before" "$post_end_after" \
  >"$run_dir/post-end-stability.txt"
post_end_state_changed=false
[[ "$post_end_before" == "$post_end_after" ]] || post_end_state_changed=true

capture_process_tree end

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

if [[ "$post_end_state_changed" == true ]]; then
  log "canonical State or Speech revision changed after all Meetings ended"
  protocol_failures=$((protocol_failures + 1))
fi

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
    (SELECT count(*) FROM meeting_event_outbox WHERE delivered_at IS NULL) +
    (SELECT count(*) FROM meeting_event_outbox WHERE last_error IS NOT NULL);
"
)" || fail "failed to query terminal reservations"
if [[ "$terminal_reservations" -ne 0 ]]; then
  log "terminal reservations or outbox rows remain"
  protocol_failures=$((protocol_failures + 1))
fi

if [[ "$suite" == scale ]]; then
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

database_protocol_violations="$(
  db_scalar "
    SELECT
      (SELECT count(*)
       FROM meeting_moderator_decision_attempts
       WHERE state='running')
      +
      (SELECT count(*)
       FROM meeting_moderator_decision_attempts
       WHERE abs(
         EXTRACT(EPOCH FROM (deadline_at-started_at)) * 1000 - 180000
       ) > 1)
      +
      (SELECT count(*)
       FROM meeting_baton_offers first_offer
       JOIN meeting_baton_offers second_offer
         ON second_offer.community_id=first_offer.community_id
         AND second_offer.session_id=first_offer.session_id
         AND second_offer.offer_id > first_offer.offer_id
         AND first_offer.created_at < second_offer.resolved_at
         AND second_offer.created_at < first_offer.resolved_at)
      +
      (SELECT count(*)
       FROM meeting_baton_grants first_grant
       JOIN meeting_baton_grants second_grant
         ON second_grant.community_id=first_grant.community_id
         AND second_grant.session_id=first_grant.session_id
         AND second_grant.grant_id > first_grant.grant_id
         AND first_grant.created_at < second_grant.terminal_at
         AND second_grant.created_at < first_grant.terminal_at)
      +
      (SELECT count(*)
       FROM meeting_moderator_decision_attempts first_attempt
       JOIN meeting_moderator_decision_attempts second_attempt
         ON second_attempt.community_id=first_attempt.community_id
         AND second_attempt.session_id=first_attempt.session_id
         AND second_attempt.attempt_id > first_attempt.attempt_id
         AND first_attempt.started_at < second_attempt.terminal_at
         AND second_attempt.started_at < first_attempt.terminal_at);
  "
)" || fail "failed to query Meeting protocol overlap/deadline invariants"
if [[ "$database_protocol_violations" -ne 0 ]]; then
  log "$database_protocol_violations Meeting protocol overlap/deadline violation(s) detected"
  protocol_failures=$((protocol_failures + database_protocol_violations))
fi

if [[ "$suite" == scale ]]; then
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
fi

if rg -n '\"status\":429|status=429|rate.?limited' "$run_dir/relay.log" \
  >"$run_dir/relay-429.txt"; then
  log "unexpected Relay 429 detected"
  protocol_failures=$((protocol_failures + 1))
fi

: >"$run_dir/model-proof.txt"
while IFS=$'\t' read -r role agent_pid effort; do
  agent_log="$run_dir/agents/$role.log"
  event_path="$run_dir/agents/$role-events.ndjson"
  if ! rg -q 'agent_pool_ready agents=1' "$agent_log"; then
    log "$role missing agent_pool_ready"
    protocol_failures=$((protocol_failures + 1))
  fi
  if rg -n \
    'unsupported_model|desired model .* not found|failed to set model|authentication failed|agent pool initialization failed' \
    "$agent_log" >>"$run_dir/model-proof-errors.txt"; then
    log "$role has a model/auth startup error"
    protocol_failures=$((protocol_failures + 1))
  fi
  printf '%s requested=gpt-5.6-sol[%s]\n' "$role" "$effort" \
    >>"$run_dir/model-proof.txt"
  if [[ "$suite" == scale ]] ||
    { [[ -s "$event_path" ]] && jq -se '
      any(.[]; .kind == "turn_started")
    ' "$event_path" >/dev/null 2>&1; }; then
    if ! rg -q "applied model gpt-5\\.6-sol\\[$effort\\]" "$agent_log"; then
      log "$role started a real Prompt without model gpt-5.6-sol[$effort] evidence"
      protocol_failures=$((protocol_failures + 1))
    fi
    rg 'applied model gpt-5\.6-sol' "$agent_log" \
      >>"$run_dir/model-proof.txt" || true
  else
    printf '%s\n' "$role no ACP Session was exercised by this scenario" \
      >>"$run_dir/model-proof.txt"
  fi
done <"$run_dir/agent-processes.tsv"

if [[ -f "$run_dir/model-proof-errors.txt" && ! -s "$run_dir/model-proof-errors.txt" ]]; then
  rm -- "$run_dir/model-proof-errors.txt"
fi

: >"$run_dir/runtime-anomalies.txt"
while IFS=$'\t' read -r role agent_pid effort; do
  agent_log="$run_dir/agents/$role.log"
  if [[ "$suite" == moderator ]]; then
    rg -nH \
      'agent_returned — respawning|agent_returned \(application error| ERROR ' \
      "$agent_log" >>"$run_dir/runtime-anomalies.txt" || true
  else
    rg -nH \
      'agent_returned — respawning|agent_returned \(application error|Meeting V1 .* was not confirmed| ERROR ' \
      "$agent_log" >>"$run_dir/runtime-anomalies.txt" || true
  fi
done <"$run_dir/agent-processes.tsv"
runtime_anomalies="$(wc -l <"$run_dir/runtime-anomalies.txt" | tr -d ' ')"
if [[ "$runtime_anomalies" -ne 0 ]]; then
  log "$runtime_anomalies unexpected ACP runtime anomaly/anomalies detected"
  protocol_failures=$((protocol_failures + runtime_anomalies))
else
  rm -- "$run_dir/runtime-anomalies.txt"
fi

moderator_gate_failures=0
scenario_gate_failures=0
if [[ "$suite" == moderator ]]; then
  expected_agent_count="$(wc -l <"$run_dir/agent-processes.tsv" | tr -d ' ')"
  : >"$run_dir/acceptance-events.ndjson"
  while IFS=$'\t' read -r role agent_pid effort; do
    event_path="$run_dir/agents/$role-events.ndjson"
    if [[ ! -s "$event_path" ]]; then
      log "$role produced no acceptance observer evidence"
      moderator_gate_failures=$((moderator_gate_failures + 1))
      continue
    fi
    if ! jq -c --arg role "$role" '. + {acceptanceRole: $role}' \
      "$event_path" >>"$run_dir/acceptance-events.ndjson"; then
      fail "$role acceptance observer evidence is not valid NDJSON"
    fi
  done <"$run_dir/agent-processes.tsv"

  if ! jq -s \
    --argjson expected_agents "$expected_agent_count" \
    -f scripts/meeting-v1-moderator-gates.jq \
    "$run_dir/acceptance-events.ndjson" \
    >"$run_dir/gates/moderator.json"; then
    fail "could not evaluate structured Moderator hard gates"
  fi
  structured_failures="$(
    jq -r '.failed_gates | length' "$run_dir/gates/moderator.json"
  )"
  moderator_gate_failures=$((moderator_gate_failures + structured_failures))

  case "$scenario" in
    R-MOD-01) expected_scenario_gates=3 ;;
    R-MOD-02) expected_scenario_gates=4 ;;
    R-MOD-03) expected_scenario_gates=2 ;;
    R-MOD-04) expected_scenario_gates=2 ;;
    R-MOD-05) expected_scenario_gates=2 ;;
    R-MOD-06) expected_scenario_gates=3 ;;
    R-MOD-07) expected_scenario_gates=7 ;;
    *) fail "unknown Moderator scenario while evaluating gates: $scenario" ;;
  esac
  : >"$run_dir/gates/scenario.ndjson"
  for scenario_gate_path in "$run_dir"/gates/scenario-*.json; do
    [[ -e "$scenario_gate_path" ]] || continue
    jq -c . "$scenario_gate_path" >>"$run_dir/gates/scenario.ndjson"
  done
  if ! jq -s \
    --arg scenario "$scenario" \
    --arg variant "$scenario_variant" \
    --argjson expected "$expected_scenario_gates" '
      {
        scenario: $scenario,
        variant: (if $variant == "" then null else $variant end),
        passed: (
          length == $expected
          and all(.[]; .pass == true)
        ),
        expected_gate_count: $expected,
        observed_gate_count: length,
        failed_gates: [.[] | select(.pass != true) | .gate],
        gates: .
      }
    ' "$run_dir/gates/scenario.ndjson" >"$run_dir/gates/scenario.json"; then
    fail "could not evaluate $scenario scenario gates"
  fi
  if [[ "$(jq -r '.passed' "$run_dir/gates/scenario.json")" != true ]]; then
    scenario_gate_failures=1
  fi
  if [[ "$moderator_gate_failures" -ne 0 || "$scenario_gate_failures" -ne 0 ]]; then
    log "$tier has $moderator_gate_failures structured and $scenario_gate_failures scenario gate failure(s)"
    protocol_failures=$((protocol_failures + moderator_gate_failures + scenario_gate_failures))
  fi
fi

jq -n \
  --arg run_id "$run_id" \
  --arg tier "$tier" \
  --arg suite "$suite" \
  --arg scenario "$scenario" \
  --arg scenario_variant "$scenario_variant" \
  --arg database "$database_name" \
  --arg relay "$relay_url" \
  --arg model "gpt-5.6-sol" \
  --arg adapter_version "$adapter_version" \
  --arg commit "$(git rev-parse HEAD)" \
  --arg runner_sha256 "$runner_sha256" \
  --arg moderator_orchestrator_sha256 "$moderator_orchestrator_sha256" \
  --arg workspace_diff_sha256 "$workspace_diff_sha256" \
  --argjson meeting_count "$meeting_count" \
  --argjson speech_target "$speech_target" \
  --argjson runtime_anomalies "$runtime_anomalies" \
  --argjson moderator_gate_failures "$moderator_gate_failures" \
  --argjson scenario_gate_failures "$scenario_gate_failures" \
  --argjson protocol_failures "$protocol_failures" \
  '{
    run_id: $run_id,
    tier: $tier,
    suite: $suite,
    scenario: (if $scenario == "" then null else $scenario end),
    scenario_variant: (
      if $scenario_variant == "" then null else $scenario_variant end
    ),
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
    moderator_orchestrator_sha256: $moderator_orchestrator_sha256,
    tracked_workspace_diff_sha256: $workspace_diff_sha256,
    meeting_count: $meeting_count,
    speeches_per_agent_target: $speech_target,
    runtime_anomalies: $runtime_anomalies,
    meeting_v1_acceptance_feature: ($suite == "moderator"),
    pre_submit_barrier: (
      if $suite == "moderator"
      then "acceptance-only-one-shot-before-protocol-submit-timeout"
      else null
      end
    ),
    moderator_gate_failures: $moderator_gate_failures,
    scenario_gate_failures: $scenario_gate_failures,
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
