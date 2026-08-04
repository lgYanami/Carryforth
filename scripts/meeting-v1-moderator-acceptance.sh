#!/usr/bin/env bash
#
# Acquire and run real-Codex Meeting V1 Moderator qualification scenarios.
#
# Each scenario gets at most three fresh Meeting acquisitions. Exit status 3
# from the single-run driver means the real model did not exercise the required
# race path; it is retained as INCONCLUSIVE evidence and retried without
# relaxing the production prompt or action schema.

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if [[ -f "$repo_root/bin/activate-hermit" ]]; then
  # shellcheck disable=SC1091
  . "$repo_root/bin/activate-hermit" >/dev/null
fi

selection="${1:-}"
artifact_root="${2:-${TMPDIR:-/tmp}/buzz-meeting-v1-moderator}"
run_stamp="$(date -u +%Y%m%dT%H%M%SZ)"
orchestration_dir="$artifact_root/orchestration-$run_stamp-$$"
mkdir -p "$orchestration_dir"
: >"$orchestration_dir/attempts.tsv"

usage() {
  cat >&2 <<'EOF'
usage:
  scripts/meeting-v1-moderator-acceptance.sh qualification [artifact-root]
  scripts/meeting-v1-moderator-acceptance.sh R-MOD-01 [artifact-root]
  scripts/meeting-v1-moderator-acceptance.sh R-MOD-03-refresh [artifact-root]
  scripts/meeting-v1-moderator-acceptance.sh R-MOD-04-withdraw [artifact-root]
EOF
}

normalize_scenario() {
  printf '%s' "$1" | tr '[:lower:]' '[:upper:]'
}

run_scenario() {
  local requested="$1"
  local normalized
  local acquisition
  local status
  local log_path
  local scenario_slug
  local sample_dir

  normalized="$(normalize_scenario "$requested")"
  scenario_slug="$(printf '%s' "$normalized" | tr '[:upper:]' '[:lower:]')"
  case "$normalized" in
    R-MOD-01|R-MOD-02|R-MOD-03|R-MOD-03-REFRESH|R-MOD-03-WITHDRAW|\
      R-MOD-04|R-MOD-04-REFRESH|R-MOD-04-WITHDRAW|R-MOD-05|R-MOD-06|R-MOD-07)
      ;;
    *)
      echo "unknown Moderator scenario: $requested" >&2
      return 2
      ;;
  esac

  for acquisition in 1 2 3; do
    log_path="$orchestration_dir/${scenario_slug}-acquisition-${acquisition}.log"
    printf '[%s] %s acquisition %s/3\n' \
      "$(date -u +%H:%M:%S)" "$normalized" "$acquisition"
    set +e
    scripts/meeting-v1-live-acceptance.sh "$normalized" "$artifact_root" \
      2>&1 | tee "$log_path"
    status="${PIPESTATUS[0]}"
    set -e
    if [[ "$status" -eq 0 ]]; then
      sample_dir="$(
        LC_ALL=C sed -n 's/^\[[^]]*\] artifacts: //p' "$log_path" | tail -1
      )"
      case "$sample_dir" in
        "$artifact_root"/*)
          ;;
        *)
          echo "$normalized returned success without an in-scope artifact path" >&2
          status=1
          ;;
      esac
      if [[ "$status" -eq 0 ]] &&
        { ! rg -q '^\[[^]]+\] PASS: .* qualification sample completed$' "$log_path" ||
          [[ ! -f "$sample_dir/manifest.json" ]] ||
          ! jq -e '
            .result == "pass"
            and .protocol_failures == 0
            and .runtime_anomalies == 0
            and .moderator_gate_failures == 0
            and .scenario_gate_failures == 0
          ' "$sample_dir/manifest.json" >/dev/null; }; then
        echo "$normalized returned success without a complete passing sample manifest" >&2
        status=1
      fi
    fi
    printf '%s\t%s\t%s\t%s\n' \
      "$normalized" "$acquisition" "$status" "$log_path" \
      >>"$orchestration_dir/attempts.tsv"
    case "$status" in
      0)
        return 0
        ;;
      3)
        ;;
      *)
        echo "$normalized failed a hard gate on acquisition $acquisition" >&2
        return "$status"
        ;;
    esac
  done

  echo "$normalized failed to acquire its required real-model path in three fresh Meetings" >&2
  return 1
}

case "$(normalize_scenario "$selection")" in
  QUALIFICATION)
    scenarios=(
      R-MOD-01
      R-MOD-02
      R-MOD-03-refresh
      R-MOD-04-refresh
      R-MOD-04-withdraw
      R-MOD-05
      R-MOD-06
      R-MOD-07
    )
    for scenario in "${scenarios[@]}"; do
      run_scenario "$scenario"
    done
    ;;
  R-MOD-*)
    run_scenario "$selection"
    ;;
  *)
    usage
    exit 2
    ;;
esac

jq -Rn '
  [inputs | split("\t") | {
    scenario: .[0],
    acquisition: (.[1] | tonumber),
    exit_status: (.[2] | tonumber),
    log: .[3]
  }] as $attempts
  | ([ $attempts[].scenario ] | unique) as $scenarios
  | {
      passed: (
        all($attempts[]; .exit_status == 0 or .exit_status == 3)
        and
        all(
          $scenarios[];
          . as $scenario
          | any(
              $attempts[];
              .scenario == $scenario and .exit_status == 0
            )
        )
      ),
      attempts: $attempts
    }
' <"$orchestration_dir/attempts.tsv" >"$orchestration_dir/manifest.json"

printf 'PASS: Moderator qualification orchestration completed\n'
printf 'artifacts: %s\n' "$orchestration_dir"
