#!/usr/bin/env bash
# Verify one privacy-filtered Meeting V2 real-provider evidence package.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUN_DIR="${1:-}"

if [[ -z "${RUN_DIR}" ]]; then
  echo "usage: $0 <qualification-run-directory> [report-path]" >&2
  exit 2
fi
RUN_DIR="$(cd "${RUN_DIR}" 2>/dev/null && pwd)" || {
  echo "qualification directory does not exist: ${RUN_DIR}" >&2
  exit 2
}
REPORT_PATH="${2:-${RUN_DIR}/qualification-gates.json}"

nonempty_evidence=(
  manifest.json
  protocol-invariants.json
  acceptance-events.ndjson
  roster.tsv
  meetings.tsv
  metrics.prom
  processes.tsv
  security-probes.json
  preflight/codex-version.txt
  preflight/codex-login-status.txt
  preflight/codex-acp-version.txt
  preflight/codex-acp-package.json
  preflight/codex-acp-models.json
  preflight/acp-capabilities.json
  preflight/relay-create-enabled.json
  preflight/relay-create-disabled.json
  preflight/executable-sha256.txt
  workspace-before.status.sha256
  workspace-before.diff.sha256
  workspace-after.status.sha256
  workspace-after.diff.sha256
  sha256.txt
)
workspace_evidence=(workspace-before.status workspace-after.status)

safe_relative_path() {
  local evidence_path="$1"
  [[ -n "${evidence_path}" &&
    "${evidence_path}" != /* &&
    "${evidence_path}" != -* &&
    "${evidence_path}" != ../* &&
    "${evidence_path}" != */../* &&
    "${evidence_path}" != */.. &&
    "${evidence_path}" != */./* &&
    "${evidence_path}" != */. &&
    "${evidence_path}" != *//* &&
    "${evidence_path}" =~ ^[A-Za-z0-9._/-]+$ ]]
}

for required in "${nonempty_evidence[@]}"; do
  if [[ ! -s "${RUN_DIR}/${required}" || -L "${RUN_DIR}/${required}" ]]; then
    echo "qualification evidence is missing ${required}" >&2
    exit 1
  fi
done

for required in "${workspace_evidence[@]}"; do
  if [[ ! -f "${RUN_DIR}/${required}" || -L "${RUN_DIR}/${required}" ]]; then
    echo "qualification evidence is missing ${required}" >&2
    exit 1
  fi
done

hashed_paths=()
while IFS= read -r hash_line || [[ -n "${hash_line}" ]]; do
  if [[ ! "${hash_line}" =~ ^([0-9a-f]{64})[[:space:]]+([A-Za-z0-9._/-]+)$ ]]; then
    echo "qualification evidence contains an invalid sha256 entry" >&2
    exit 1
  fi

  evidence_path="${BASH_REMATCH[2]}"
  if ! safe_relative_path "${evidence_path}"; then
    echo "qualification evidence contains an unsafe sha256 path: ${evidence_path}" >&2
    exit 1
  fi

  for hashed_path in "${hashed_paths[@]}"; do
    if [[ "${hashed_path}" == "${evidence_path}" ]]; then
      echo "qualification evidence hashes ${evidence_path} more than once" >&2
      exit 1
    fi
  done

  candidate_path="${RUN_DIR}"
  IFS='/' read -r -a path_components <<<"${evidence_path}"
  for path_component in "${path_components[@]}"; do
    candidate_path="${candidate_path}/${path_component}"
    if [[ -L "${candidate_path}" ]]; then
      echo "qualification evidence contains a symlink: ${evidence_path}" >&2
      exit 1
    fi
  done
  if [[ ! -f "${candidate_path}" ]]; then
    echo "qualification evidence hash target is missing: ${evidence_path}" >&2
    exit 1
  fi

  hashed_paths+=("${evidence_path}")
done <"${RUN_DIR}/sha256.txt"

path_is_hashed() {
  local expected="$1"
  local hashed_path
  for hashed_path in "${hashed_paths[@]}"; do
    [[ "${hashed_path}" == "${expected}" ]] && return 0
  done
  return 1
}

# Every immutable regular artifact must be covered, not only the minimum list.
# The verifier output, checksum file, and a runner failure diagnostic written
# after package freeze are the only intentional exceptions.
while IFS= read -r artifact_path; do
  artifact_path="${artifact_path#./}"
  case "${artifact_path}" in
    sha256.txt | qualification-gates.json | failure.txt) continue ;;
  esac
  if ! path_is_hashed "${artifact_path}"; then
    echo "qualification evidence does not hash ${artifact_path}" >&2
    exit 1
  fi
done < <(cd "${RUN_DIR}" && find . -type f -print | LC_ALL=C sort)

if ! (cd "${RUN_DIR}" && shasum -a 256 -c sha256.txt >/dev/null); then
  echo "qualification evidence hash verification failed" >&2
  exit 1
fi

for hash_file in \
  workspace-before.status.sha256 \
  workspace-before.diff.sha256 \
  workspace-after.status.sha256 \
  workspace-after.diff.sha256; do
  if [[ "$(wc -l <"${RUN_DIR}/${hash_file}" | tr -d ' ')" != "1" ]] \
    || ! rg -q '^[0-9a-f]{64}$' "${RUN_DIR}/${hash_file}"; then
    echo "qualification workspace digest is invalid: ${hash_file}" >&2
    exit 1
  fi
done

workspace_before_status_sha="$(<"${RUN_DIR}/workspace-before.status.sha256")"
workspace_before_diff_sha="$(<"${RUN_DIR}/workspace-before.diff.sha256")"
workspace_after_status_sha="$(<"${RUN_DIR}/workspace-after.status.sha256")"
workspace_after_diff_sha="$(<"${RUN_DIR}/workspace-after.diff.sha256")"
actual_before_status_sha="$(shasum -a 256 "${RUN_DIR}/workspace-before.status" | awk '{print $1}')"
actual_after_status_sha="$(shasum -a 256 "${RUN_DIR}/workspace-after.status" | awk '{print $1}')"
workspace_verified=false
if [[ "${workspace_before_status_sha}" == "${actual_before_status_sha}" \
  && "${workspace_after_status_sha}" == "${actual_after_status_sha}" \
  && "${workspace_before_status_sha}" == "${workspace_after_status_sha}" \
  && "${workspace_before_diff_sha}" == "${workspace_after_diff_sha}" ]] \
  && jq -e \
    --arg before_status "${workspace_before_status_sha}" \
    --arg before_diff "${workspace_before_diff_sha}" \
    --arg after_status "${workspace_after_status_sha}" \
    --arg after_diff "${workspace_after_diff_sha}" '
      .sourceTree.statusSha256 == $before_status
      and .sourceTree.diffSha256 == $before_diff
      and .sourceTree.afterStatusSha256 == $after_status
      and .sourceTree.afterDiffSha256 == $after_diff
    ' "${RUN_DIR}/manifest.json" >/dev/null; then
  workspace_verified=true
fi

if [[ "$(head -n 1 "${RUN_DIR}/roster.tsv")" != $'scenario\trole\tmeeting_role\tparticipant_type\tpubkey' ]]; then
  echo "qualification roster has an invalid header" >&2
  exit 1
fi
if ! awk -F '\t' '
  NR == 1 { next }
  NF != 5 || $1 == "" || $2 == "" || ($3 != "moderator" && $3 != "participant") ||
    ($4 != "human" && $4 != "agent") || $5 !~ /^[0-9a-f]{64}$/ { exit 1 }
  END { if (NR < 2) exit 1 }
' "${RUN_DIR}/roster.tsv"; then
  echo "qualification roster contains an invalid row" >&2
  exit 1
fi

if [[ "$(head -n 1 "${RUN_DIR}/meetings.tsv")" != $'scenario\tsession_id' ]]; then
  echo "qualification meetings index has an invalid header" >&2
  exit 1
fi
if ! awk -F '\t' '
  NR == 1 { next }
  NF != 2 || $1 == "" || $2 !~ /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/ { exit 1 }
  END { if (NR != 5) exit 1 }
' "${RUN_DIR}/meetings.tsv"; then
  echo "qualification meetings index must contain four valid scenario rows" >&2
  exit 1
fi

if [[ "$(head -n 1 "${RUN_DIR}/processes.tsv")" != $'scenario\trole\tpid\teffort\tlog_path' ]]; then
  echo "qualification process index has an invalid header" >&2
  exit 1
fi
if ! awk -F '\t' '
  NR == 1 { next }
  NF != 5 || $1 == "" || $2 == "" || $3 !~ /^[0-9]+$/ ||
    ($4 != "high" && $4 != "max") || $5 == "" { exit 1 }
  END { if (NR < 2) exit 1 }
' "${RUN_DIR}/processes.tsv"; then
  echo "qualification process index contains an invalid row" >&2
  exit 1
fi

temporary_manifest="$(mktemp)"
temporary_roster="$(mktemp)"
temporary_meetings="$(mktemp)"
temporary_processes="$(mktemp)"
cleanup() {
  rm -f \
    "${temporary_manifest}" \
    "${temporary_roster}" \
    "${temporary_meetings}" \
    "${temporary_processes}"
}
trap cleanup EXIT

jq -Rn '
  [inputs | split("\t")]
  | .[1:]
  | map({
      scenario: .[0],
      role: .[1],
      meetingRole: .[2],
      participantType: .[3],
      pubkey: .[4]
    })
' <"${RUN_DIR}/roster.tsv" >"${temporary_roster}"
jq -Rn '
  [inputs | split("\t")]
  | .[1:]
  | map({scenario: .[0], sessionId: .[1]})
' <"${RUN_DIR}/meetings.tsv" >"${temporary_meetings}"
jq -Rn '
  [inputs | split("\t")]
  | .[1:]
  | map({scenario: .[0], role: .[1], pid: .[2], effort: .[3], logPath: .[4]})
' <"${RUN_DIR}/processes.tsv" >"${temporary_processes}"

if ! jq -e '
  length == 4
  and ([.[].scenario] | sort == ["admin_abort", "all_agent", "mixed", "moderator_abort"])
  and ([.[].sessionId] | unique | length == 4)
' "${temporary_meetings}" >/dev/null; then
  echo "qualification meetings index does not contain the exact scenario matrix" >&2
  exit 1
fi

provider_evidence_verified=true
if ! rg -q '^Logged in using (ChatGPT|an API key)$' \
  "${RUN_DIR}/preflight/codex-login-status.txt"; then
  provider_evidence_verified=false
fi
if ! rg -q '^@agentclientprotocol/codex-acp 1\.1\.7$' \
  "${RUN_DIR}/preflight/codex-acp-version.txt"; then
  provider_evidence_verified=false
fi
if ! jq -e '
  .name == "@agentclientprotocol/codex-acp" and .version == "1.1.7"
' "${RUN_DIR}/preflight/codex-acp-package.json" >/dev/null; then
  provider_evidence_verified=false
fi
provider_model="$(jq -r '.provider.model // ""' "${RUN_DIR}/manifest.json")"
if [[ -z "${provider_model}" ]] \
  || ! jq -e \
    --arg high "${provider_model}[high]" \
    --arg max "${provider_model}[max]" '
      ([.stable.configOptions[]?.options[]?.value, .unstable.availableModels[]?.modelId]
        | any(. == $high))
      and
      ([.stable.configOptions[]?.options[]?.value, .unstable.availableModels[]?.modelId]
        | any(. == $max))
    ' "${RUN_DIR}/preflight/codex-acp-models.json" >/dev/null; then
  provider_evidence_verified=false
fi

capability_evidence_verified=false
if jq -e '
  .meeting.qualificationEvidenceCompiled == true
  and any(.meeting.protocols[];
    .schemaVersion == "3"
    and .policy == "moderated-board-v1"
    and .roles == ["participant", "moderator"]
    and (.turns | index("intent") != null)
    and (.turns | index("granted_speech") != null)
    and (.turns | index("board_maintenance") != null)
    and (.turns | index("floor_decision") != null))
' "${RUN_DIR}/preflight/acp-capabilities.json" >/dev/null \
  && jq -e '
    (.supported_extensions | index("buzz-meeting-v2") != null)
    and (.supported_extensions | index("buzz-meeting-v2-create") != null)
  ' "${RUN_DIR}/preflight/relay-create-enabled.json" >/dev/null \
  && jq -e '
    (.supported_extensions | index("buzz-meeting-v2") != null)
    and (.supported_extensions | index("buzz-meeting-v2-create") == null)
  ' "${RUN_DIR}/preflight/relay-create-disabled.json" >/dev/null; then
  capability_evidence_verified=true
fi

metrics_evidence_verified=true
for metric in \
  meeting_v2_board_command_total \
  meeting_v2_board_read_total \
  meeting_v2_end_total; do
  if ! rg -q "^${metric}(\\{| )" "${RUN_DIR}/metrics.prom"; then
    metrics_evidence_verified=false
  fi
done

process_evidence_verified=false
if jq -e \
  --slurpfile roster "${temporary_roster}" \
  --slurpfile processes "${temporary_processes}" '
    . as $manifest
    | ($roster[0] | map(select(.participantType == "agent"))) as $agents
    | ($processes[0]) as $processes
    | ($manifest.artifacts.agentLogs // []) as $logs
    | ($processes | length) == ($agents | length)
      and ([$processes[] | [.scenario, .role] | join(":")] | unique | length)
        == ($processes | length)
      and all($agents[];
        . as $agent
        | any($processes[];
            .scenario == $agent.scenario and .role == $agent.role))
      and all($processes[];
        . as $process
        | any($agents[];
            .scenario == $process.scenario
            and .role == $process.role
            and (($process.effort == "max") == (.meetingRole == "moderator"))))
      and ($logs | length) == ($processes | length)
      and all($processes[];
        . as $process
        | any($logs[];
            .scenario == $process.scenario
            and .role == $process.role
            and .path == $process.logPath
            and .model == ($manifest.provider.model + "[" + $process.effort + "]")))
      and all($logs[];
        . as $log
        | any($processes[];
            .scenario == $log.scenario
            and .role == $log.role
            and .logPath == $log.path))
  ' "${RUN_DIR}/manifest.json" >/dev/null; then
  process_evidence_verified=true
fi
agent_log_count=0
while IFS=$'\t' read -r scenario role log_path model_id; do
  agent_log_count=$((agent_log_count + 1))
  if ! safe_relative_path "${log_path}" \
    || [[ ! -s "${RUN_DIR}/${log_path}" || -L "${RUN_DIR}/${log_path}" ]] \
    || ! path_is_hashed "${log_path}"; then
    process_evidence_verified=false
    continue
  fi
  if ! jq -e --arg scenario "${scenario}" --arg role "${role}" '
    any(.[]; .scenario == $scenario and .role == $role and .participantType == "agent")
  ' "${temporary_roster}" >/dev/null; then
    process_evidence_verified=false
  fi
  if ! rg -q 'agent_pool_ready agents=1' "${RUN_DIR}/${log_path}" \
    || ! rg -Fq "applied model ${model_id}" "${RUN_DIR}/${log_path}"; then
    process_evidence_verified=false
  fi
  if rg -q \
    'agent_returned — respawning|respawn_failed|agent_panic|unsupported_model|authentication failed|agent pool initialization failed' \
    "${RUN_DIR}/${log_path}"; then
    process_evidence_verified=false
  fi
done < <(jq -r '
  .artifacts.agentLogs[]?
  | [.scenario, .role, .path, .model] | @tsv
' "${RUN_DIR}/manifest.json")
expected_agent_log_count="$(jq '[.[] | select(.participantType == "agent")] | length' \
  "${temporary_roster}")"
if [[ "${agent_log_count}" -ne "${expected_agent_log_count}" ]]; then
  process_evidence_verified=false
fi

relay_log_count=0
while IFS= read -r relay_log; do
  relay_log_count=$((relay_log_count + 1))
  if ! safe_relative_path "${relay_log}" \
    || [[ ! -s "${RUN_DIR}/${relay_log}" || -L "${RUN_DIR}/${relay_log}" ]] \
    || ! path_is_hashed "${relay_log}"; then
    process_evidence_verified=false
  fi
done < <(jq -r '.artifacts.relayLogs[]?' "${RUN_DIR}/manifest.json")
if [[ "${relay_log_count}" -ne 2 ]] \
  || ! jq -e '
    (.artifacts.relayLogs | sort)
      == ["logs/relay-create-enabled.log", "logs/relay.log"]
  ' "${RUN_DIR}/manifest.json" >/dev/null; then
  process_evidence_verified=false
fi

jq \
  --argjson workspace_verified "${workspace_verified}" \
  --argjson provider_verified "${provider_evidence_verified}" \
  --argjson capability_verified "${capability_evidence_verified}" \
  --argjson metrics_verified "${metrics_evidence_verified}" \
  --argjson process_verified "${process_evidence_verified}" \
  '.sha256Verified = true
   | .workspaceVerified = $workspace_verified
   | .providerEvidenceVerified = $provider_verified
   | .capabilityEvidenceVerified = $capability_verified
   | .metricsEvidenceVerified = $metrics_verified
   | .processEvidenceVerified = $process_verified' \
  "${RUN_DIR}/manifest.json" >"${temporary_manifest}"

jq -n \
  --slurpfile manifest "${temporary_manifest}" \
  --slurpfile invariants "${RUN_DIR}/protocol-invariants.json" \
  --slurpfile events "${RUN_DIR}/acceptance-events.ndjson" \
  --slurpfile roster "${temporary_roster}" \
  --slurpfile meetings "${temporary_meetings}" \
  --slurpfile security "${RUN_DIR}/security-probes.json" \
  -f "${REPO_ROOT}/scripts/meeting-v2-qualification-gates.jq" \
  >"${REPORT_PATH}"

if [[ "$(jq -r '.passed' "${REPORT_PATH}")" != true ]]; then
  jq -r '.failedGates[] | "qualification gate failed: \(.)"' "${REPORT_PATH}" >&2
  exit 1
fi

echo "Meeting V2 qualification evidence passed: ${REPORT_PATH}"
