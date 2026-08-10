#!/usr/bin/env bash
# Deterministic positive/negative fixtures for the Meeting V2 evidence verifier.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMP_DIR="$(mktemp -d)"
bash -n "${SCRIPT_DIR}/meeting-v2-live-qualification.sh"
cleanup() {
  rm -rf "${TEMP_DIR}"
}
trap cleanup EXIT

hash_fixture() {
  local directory="$1"
  (
    cd "${directory}"
    find . -type f \
      ! -name sha256.txt \
      ! -name qualification-gates.json \
      -print \
      | sed 's#^./##' \
      | LC_ALL=C sort \
      | while IFS= read -r path; do
          shasum -a 256 "${path}"
        done >sha256.txt
  )
}

write_fixture() {
  local directory="$1"
  mkdir -p "${directory}/logs/agents" "${directory}/preflight"
  cat >"${directory}/manifest.json" <<'JSON'
{
  "evidenceSchema": "buzz-meeting-v2-qualification-v1",
  "buzzCommit": "1111111111111111111111111111111111111111",
  "sourceTree": {
    "statusSha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "diffSha256": "3333333333333333333333333333333333333333333333333333333333333333",
    "afterStatusSha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    "afterDiffSha256": "3333333333333333333333333333333333333333333333333333333333333333"
  },
  "protocol": {"schemaVersion": "3", "policy": "moderated-board-v1"},
  "provider": {
    "real": true,
    "authenticated": true,
    "catalogSupported": true,
    "model": "gpt-5.6-sol",
    "moderatorReasoning": "max",
    "participantReasoning": "high",
    "adapter": "@agentclientprotocol/codex-acp",
    "adapterVersion": "1.1.7",
    "agentSessionsExercised": 8
  },
  "capabilities": {
    "relayRuntime": true,
    "createEnabledObserved": true,
    "createDisabledDrainObserved": true,
    "acpV2Participant": true,
    "acpV2Moderator": true
  },
  "scenarios": {
    "mixed": {"sessionId": "00000000-0000-4000-8000-000000000001"},
    "all_agent": {"sessionId": "00000000-0000-4000-8000-000000000002"},
    "moderator_abort": {"sessionId": "00000000-0000-4000-8000-000000000003"},
    "admin_abort": {"sessionId": "00000000-0000-4000-8000-000000000004"}
  },
  "securityProbes": {
    "outsiderReadDenied": true,
    "outsiderBoardWriteDenied": true,
    "createDisabledDenied": true,
    "postEndWriteDenied": true
  },
  "workspaceChanged": false,
  "projectViewDependencies": 0,
  "externalWrites": 0,
  "runtimeAnomalies": 0,
  "artifacts": {
    "relayLogs": ["logs/relay-create-enabled.log", "logs/relay.log"],
    "agentLogs": [
      {"scenario":"mixed","role":"mixed-moderator","path":"logs/agents/mixed-moderator.log","model":"gpt-5.6-sol[max]"},
      {"scenario":"mixed","role":"mixed-agent","path":"logs/agents/mixed-agent.log","model":"gpt-5.6-sol[high]"},
      {"scenario":"all_agent","role":"all-moderator","path":"logs/agents/all-moderator.log","model":"gpt-5.6-sol[max]"},
      {"scenario":"all_agent","role":"all-agent-a","path":"logs/agents/all-agent-a.log","model":"gpt-5.6-sol[high]"},
      {"scenario":"all_agent","role":"all-agent-b","path":"logs/agents/all-agent-b.log","model":"gpt-5.6-sol[high]"},
      {"scenario":"moderator_abort","role":"abort-moderator","path":"logs/agents/abort-moderator.log","model":"gpt-5.6-sol[max]"},
      {"scenario":"moderator_abort","role":"abort-agent","path":"logs/agents/abort-agent.log","model":"gpt-5.6-sol[high]"},
      {"scenario":"admin_abort","role":"admin-moderator","path":"logs/agents/admin-moderator.log","model":"gpt-5.6-sol[max]"}
    ]
  }
}
JSON
  cat >"${directory}/protocol-invariants.json" <<'JSON'
{
  "scenarios": {
    "mixed": {
      "sessionId": "00000000-0000-4000-8000-000000000001",
      "humans": 2, "agents": 2, "boardUpdates": 2, "floorDecisions": 3,
      "distinctSpeakers": 4, "boardChangedBetweenIntentAndGrant": false,
      "humanBoardPreemptions": 1, "resolvedHandoffs": 1,
      "moderatorSelfSpeeches": 1, "terminalOutcome": "closed"
    },
    "all_agent": {
      "sessionId": "00000000-0000-4000-8000-000000000002",
      "humans": 0, "agents": 3, "boardUpdates": 3, "floorDecisions": 4,
      "distinctSpeakers": 3, "boardChangedBetweenIntentAndGrant": true,
      "resolvedHandoffs": 2, "moderatorSelfSpeeches": 1,
      "terminalOutcome": "closed"
    },
    "moderator_abort": {
      "sessionId": "00000000-0000-4000-8000-000000000003",
      "terminalOutcome": "aborted", "initiator": "moderator_agent",
      "reasonCode": "unable_to_form_conclusion"
    },
    "admin_abort": {
      "sessionId": "00000000-0000-4000-8000-000000000004",
      "terminalOutcome": "aborted", "initiator": "security",
      "reasonCode": "participant_revoked"
    }
  },
  "zero": {
    "boardFloorOverlap": 0,
    "floorBeforeBoardTerminal": 0,
    "boardAcceptedDuringOfferOrGrant": 0,
    "turnWithoutBoardRead": 0,
    "lateBoardLanded": 0,
    "boardChangedSpeechRevision": 0,
    "postEndRevisionChange": 0,
    "pendingRuntimeReservations": 0,
    "unauthorizedBoardAccess": 0,
    "externalWrites": 0
  }
}
JSON
  cat >"${directory}/acceptance-events.ndjson" <<'NDJSON'
{"timestamp":"2026-08-03T00:00:01Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"turn_type":"participant_intent","board_event_id":"board-a"},"qualificationScenario":"mixed","acceptanceRole":"mixed-agent"}
{"timestamp":"2026-08-03T00:00:02Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"turn_type":"moderator_board","board_event_id":"board-a"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:03Z","kind":"meeting_v2_board_turn_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"action":"UPDATE"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:04Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"turn_type":"moderator_floor","board_event_id":"board-b"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:05Z","kind":"meeting_v2_floor_turn_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"action":"IDLE","reason_code":null},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:06Z","kind":"meeting_v2_host_turn_discarded","channelId":"00000000-0000-4000-8000-000000000001","payload":{"reason":"board_or_floor_authority_changed"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:07Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000001","payload":{"turn_type":"granted_speech","board_event_id":"board-a"},"qualificationScenario":"mixed","acceptanceRole":"mixed-agent"}
{"timestamp":"2026-08-03T00:00:08Z","kind":"meeting_v1_speech_submitted","channelId":"00000000-0000-4000-8000-000000000001","payload":{"outcome":"accepted"},"qualificationScenario":"mixed","acceptanceRole":"mixed-agent"}
{"timestamp":"2026-08-03T00:00:09Z","kind":"meeting_v1_speech_submitted","channelId":"00000000-0000-4000-8000-000000000001","payload":{"outcome":"accepted"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:00:10Z","kind":"meeting_v1_state_applied","channelId":"00000000-0000-4000-8000-000000000001","payload":{"phase":"ended"},"qualificationScenario":"mixed","acceptanceRole":"mixed-moderator"}
{"timestamp":"2026-08-03T00:01:01Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"turn_type":"participant_intent","board_event_id":"board-c"},"qualificationScenario":"all_agent","acceptanceRole":"all-agent-a"}
{"timestamp":"2026-08-03T00:01:02Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"turn_type":"moderator_board","board_event_id":"board-c"},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:03Z","kind":"meeting_v2_board_turn_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"action":"UPDATE"},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:04Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"turn_type":"moderator_floor","board_event_id":"board-d"},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:05Z","kind":"meeting_v2_floor_turn_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"action":"IDLE","reason_code":null},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:06Z","kind":"meeting_v2_board_load_completed","channelId":"00000000-0000-4000-8000-000000000002","payload":{"turn_type":"granted_speech","board_event_id":"board-d"},"qualificationScenario":"all_agent","acceptanceRole":"all-agent-a"}
{"timestamp":"2026-08-03T00:01:07Z","kind":"meeting_v1_speech_submitted","channelId":"00000000-0000-4000-8000-000000000002","payload":{"outcome":"accepted"},"qualificationScenario":"all_agent","acceptanceRole":"all-agent-a"}
{"timestamp":"2026-08-03T00:01:08Z","kind":"meeting_v1_speech_submitted","channelId":"00000000-0000-4000-8000-000000000002","payload":{"outcome":"accepted"},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:09Z","kind":"meeting_v1_state_applied","channelId":"00000000-0000-4000-8000-000000000002","payload":{"phase":"ended"},"qualificationScenario":"all_agent","acceptanceRole":"all-moderator"}
{"timestamp":"2026-08-03T00:01:10Z","kind":"turn_started","channelId":"00000000-0000-4000-8000-000000000002","payload":{"source":"meeting"},"qualificationScenario":"all_agent","acceptanceRole":"all-agent-b"}
{"timestamp":"2026-08-03T00:02:01Z","kind":"meeting_v2_floor_turn_completed","channelId":"00000000-0000-4000-8000-000000000003","payload":{"action":"ABORT","reason_code":"unable_to_form_conclusion"},"qualificationScenario":"moderator_abort","acceptanceRole":"abort-moderator"}
{"timestamp":"2026-08-03T00:02:02Z","kind":"turn_started","channelId":"00000000-0000-4000-8000-000000000003","payload":{"source":"meeting"},"qualificationScenario":"moderator_abort","acceptanceRole":"abort-agent"}
{"timestamp":"2026-08-03T00:03:01Z","kind":"turn_started","channelId":"00000000-0000-4000-8000-000000000004","payload":{"source":"meeting"},"qualificationScenario":"admin_abort","acceptanceRole":"admin-moderator"}
NDJSON
  cat >"${directory}/roster.tsv" <<'TSV'
scenario	role	meeting_role	participant_type	pubkey
mixed	mixed-moderator	moderator	agent	1111111111111111111111111111111111111111111111111111111111111111
mixed	mixed-agent	participant	agent	1212121212121212121212121212121212121212121212121212121212121212
mixed	mixed-human-a	participant	human	1313131313131313131313131313131313131313131313131313131313131313
mixed	mixed-human-b	participant	human	1414141414141414141414141414141414141414141414141414141414141414
all_agent	all-moderator	moderator	agent	2121212121212121212121212121212121212121212121212121212121212121
all_agent	all-agent-a	participant	agent	2222222222222222222222222222222222222222222222222222222222222222
all_agent	all-agent-b	participant	agent	2323232323232323232323232323232323232323232323232323232323232323
moderator_abort	abort-moderator	moderator	agent	3131313131313131313131313131313131313131313131313131313131313131
moderator_abort	abort-agent	participant	agent	3232323232323232323232323232323232323232323232323232323232323232
admin_abort	admin-moderator	moderator	agent	4141414141414141414141414141414141414141414141414141414141414141
admin_abort	admin-human	participant	human	4242424242424242424242424242424242424242424242424242424242424242
TSV
  cat >"${directory}/meetings.tsv" <<'TSV'
scenario	session_id
mixed	00000000-0000-4000-8000-000000000001
all_agent	00000000-0000-4000-8000-000000000002
moderator_abort	00000000-0000-4000-8000-000000000003
admin_abort	00000000-0000-4000-8000-000000000004
TSV
  cat >"${directory}/metrics.prom" <<'PROM'
meeting_v2_board_command_total{action="update",outcome="accepted",duplicate="false"} 4
meeting_v2_board_read_total{transport="http",outcome="success"} 12
meeting_v2_end_total{outcome="closed",reason_code="none",duplicate="false"} 2
PROM
  cat >"${directory}/processes.tsv" <<'TSV'
scenario	role	pid	effort	log_path
mixed	mixed-moderator	101	max	logs/agents/mixed-moderator.log
mixed	mixed-agent	102	high	logs/agents/mixed-agent.log
all_agent	all-moderator	103	max	logs/agents/all-moderator.log
all_agent	all-agent-a	104	high	logs/agents/all-agent-a.log
all_agent	all-agent-b	105	high	logs/agents/all-agent-b.log
moderator_abort	abort-moderator	106	max	logs/agents/abort-moderator.log
moderator_abort	abort-agent	107	high	logs/agents/abort-agent.log
admin_abort	admin-moderator	108	max	logs/agents/admin-moderator.log
TSV
  cat >"${directory}/security-probes.json" <<'JSON'
{"outsiderReadDenied":true,"outsiderBoardWriteDenied":true,"createDisabledDenied":true,"postEndWriteDenied":true}
JSON
  cat >"${directory}/preflight/codex-version.txt" <<'TEXT'
codex-cli 0.144.4
TEXT
  cat >"${directory}/preflight/codex-login-status.txt" <<'TEXT'
Logged in using ChatGPT
TEXT
  cat >"${directory}/preflight/codex-acp-version.txt" <<'TEXT'
@agentclientprotocol/codex-acp 1.1.7
TEXT
  cat >"${directory}/preflight/codex-acp-package.json" <<'JSON'
{"name":"@agentclientprotocol/codex-acp","version":"1.1.7","bin":{"codex-acp":"dist/index.js"}}
JSON
  cat >"${directory}/preflight/codex-acp-models.json" <<'JSON'
{"stable":{"configOptions":[{"options":[{"value":"gpt-5.6-sol[high]"},{"value":"gpt-5.6-sol[max]"}]}]},"unstable":{"availableModels":[]}}
JSON
  cat >"${directory}/preflight/acp-capabilities.json" <<'JSON'
{"meeting":{"qualificationEvidenceCompiled":true,"protocols":[{"schemaVersion":"3","policy":"moderated-board-v1","roles":["participant","moderator"],"turns":["intent","granted_speech","board_maintenance","floor_decision"]}]}}
JSON
  cat >"${directory}/preflight/relay-create-enabled.json" <<'JSON'
{"supported_extensions":["buzz-meeting-v2","buzz-meeting-v2-create"]}
JSON
  cat >"${directory}/preflight/relay-create-disabled.json" <<'JSON'
{"supported_extensions":["buzz-meeting-v2"]}
JSON
  cat >"${directory}/preflight/executable-sha256.txt" <<'TEXT'
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  target/release/cf
bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  target/release/buzz-acp
cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  target/release/buzz-relay
dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd  @agentclientprotocol/codex-acp
TEXT
  printf 'Relay create-enabled fixture log\n' >"${directory}/logs/relay-create-enabled.log"
  printf 'Relay create-disabled fixture log\n' >"${directory}/logs/relay.log"
  while IFS=$'\t' read -r scenario role path model; do
    printf 'agent_pool_ready agents=1\napplied model %s\n' "${model}" \
      >"${directory}/${path}"
  done < <(jq -r '.artifacts.agentLogs[] | [.scenario,.role,.path,.model] | @tsv' \
    "${directory}/manifest.json")
  : >"${directory}/workspace-before.status"
  : >"${directory}/workspace-after.status"
  printf '%s\n' e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 \
    >"${directory}/workspace-before.status.sha256"
  printf '%s\n' 3333333333333333333333333333333333333333333333333333333333333333 \
    >"${directory}/workspace-before.diff.sha256"
  printf '%s\n' e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 \
    >"${directory}/workspace-after.status.sha256"
  printf '%s\n' 3333333333333333333333333333333333333333333333333333333333333333 \
    >"${directory}/workspace-after.diff.sha256"
  hash_fixture "${directory}"
}

expect_failure_gate() {
  local directory="$1"
  local expected_gate="$2"
  if "${SCRIPT_DIR}/verify-meeting-v2-qualification.sh" "${directory}" >/dev/null 2>&1; then
    echo "negative qualification fixture unexpectedly passed: ${expected_gate}" >&2
    exit 1
  fi
  jq -e --arg gate "${expected_gate}" \
    '.passed == false and (.failedGates | index($gate) != null)' \
    "${directory}/qualification-gates.json" >/dev/null
}

passing="${TEMP_DIR}/passing"
write_fixture "${passing}"
"${SCRIPT_DIR}/verify-meeting-v2-qualification.sh" "${passing}" >/dev/null
jq -e '.passed == true and (.failedGates | length) == 0' \
  "${passing}/qualification-gates.json" >/dev/null

failing="${TEMP_DIR}/failing"
write_fixture "${failing}"
jq '.scenarios.mixed.humanBoardPreemptions = 0' \
  "${failing}/protocol-invariants.json" >"${failing}/protocol-invariants.changed"
mv "${failing}/protocol-invariants.changed" "${failing}/protocol-invariants.json"
hash_fixture "${failing}"
expect_failure_gate "${failing}" mixed_lifecycle

board_refresh_failing="${TEMP_DIR}/board-refresh-failing"
write_fixture "${board_refresh_failing}"
jq '.scenarios.all_agent.boardChangedBetweenIntentAndGrant = false' \
  "${board_refresh_failing}/protocol-invariants.json" \
  >"${board_refresh_failing}/protocol-invariants.changed"
mv "${board_refresh_failing}/protocol-invariants.changed" \
  "${board_refresh_failing}/protocol-invariants.json"
sed -i 's/"turn_type":"granted_speech","board_event_id":"board-d"/"turn_type":"granted_speech","board_event_id":"board-c"/' \
  "${board_refresh_failing}/acceptance-events.ndjson"
hash_fixture "${board_refresh_failing}"
expect_failure_gate "${board_refresh_failing}" participant_current_board_refresh

workspace_failing="${TEMP_DIR}/workspace-failing"
write_fixture "${workspace_failing}"
printf ' M protected-worktree-file\n' >"${workspace_failing}/workspace-after.status"
hash_fixture "${workspace_failing}"
expect_failure_gate "${workspace_failing}" workspace_and_external_effects

workspace_diff_failing="${TEMP_DIR}/workspace-diff-failing"
write_fixture "${workspace_diff_failing}"
printf '%s\n' 4444444444444444444444444444444444444444444444444444444444444444 \
  >"${workspace_diff_failing}/workspace-after.diff.sha256"
jq '.sourceTree.afterDiffSha256 = "4444444444444444444444444444444444444444444444444444444444444444"' \
  "${workspace_diff_failing}/manifest.json" >"${workspace_diff_failing}/manifest.changed"
mv "${workspace_diff_failing}/manifest.changed" "${workspace_diff_failing}/manifest.json"
hash_fixture "${workspace_diff_failing}"
expect_failure_gate "${workspace_diff_failing}" workspace_and_external_effects

unhashed="${TEMP_DIR}/unhashed"
write_fixture "${unhashed}"
grep -v 'metrics.prom$' "${unhashed}/sha256.txt" >"${unhashed}/sha256.changed"
mv "${unhashed}/sha256.changed" "${unhashed}/sha256.txt"
if "${SCRIPT_DIR}/verify-meeting-v2-qualification.sh" "${unhashed}" >/dev/null 2>&1; then
  echo "qualification fixture with unhashed core evidence unexpectedly passed" >&2
  exit 1
fi

incomplete_zero="${TEMP_DIR}/incomplete-zero"
write_fixture "${incomplete_zero}"
jq 'del(.zero.turnWithoutBoardRead)' \
  "${incomplete_zero}/protocol-invariants.json" \
  >"${incomplete_zero}/protocol-invariants.changed"
mv "${incomplete_zero}/protocol-invariants.changed" \
  "${incomplete_zero}/protocol-invariants.json"
hash_fixture "${incomplete_zero}"
expect_failure_gate "${incomplete_zero}" zero_invariants

provider_failing="${TEMP_DIR}/provider-failing"
write_fixture "${provider_failing}"
sed -i 's/applied model gpt-5.6-sol\[max\]/applied model other-model/' \
  "${provider_failing}/logs/agents/mixed-moderator.log"
hash_fixture "${provider_failing}"
expect_failure_gate "${provider_failing}" artifact_integrity

catalog_failing="${TEMP_DIR}/catalog-failing"
write_fixture "${catalog_failing}"
jq '(.stable.configOptions[0].options) |= map(select(.value != "gpt-5.6-sol[max]"))' \
  "${catalog_failing}/preflight/codex-acp-models.json" \
  >"${catalog_failing}/preflight/codex-acp-models.changed"
mv "${catalog_failing}/preflight/codex-acp-models.changed" \
  "${catalog_failing}/preflight/codex-acp-models.json"
hash_fixture "${catalog_failing}"
expect_failure_gate "${catalog_failing}" real_provider

process_failing="${TEMP_DIR}/process-failing"
write_fixture "${process_failing}"
sed -i '/all-agent-b/d' "${process_failing}/processes.tsv"
hash_fixture "${process_failing}"
expect_failure_gate "${process_failing}" artifact_integrity

topology_failing="${TEMP_DIR}/topology-failing"
write_fixture "${topology_failing}"
jq '.scenarios.mixed.sessionId = "00000000-0000-4000-8000-000000000099"' \
  "${topology_failing}/manifest.json" >"${topology_failing}/manifest.changed"
mv "${topology_failing}/manifest.changed" "${topology_failing}/manifest.json"
hash_fixture "${topology_failing}"
expect_failure_gate "${topology_failing}" scenario_and_roster_topology

echo "Meeting V2 qualification gate fixtures passed."
