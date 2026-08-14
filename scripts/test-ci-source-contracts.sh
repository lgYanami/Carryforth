#!/usr/bin/env bash
# Run every source-level CI contract and report all failures together. This
# script deliberately does not fail fast: one stale routing contract must not
# hide the next one.
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

run_contracts() {
  local status=0
  local entry label command

  for entry in "$@"; do
    label="${entry%%::*}"
    command="${entry#*::}"
    printf '\n==> %s\n' "${label}"
    if "${command}"; then
      printf '<== PASS: %s\n' "${label}"
    else
      local rc=$?
      printf '<== FAIL (%d): %s\n' "${rc}" "${label}" >&2
      status=1
    fi
  done

  return "${status}"
}

self_test_runner() {
  local tmp log
  tmp="$(mktemp -d)"
  log="${tmp}/executed"
  trap 'rm -rf "${tmp}"' RETURN

  for fixture in 01-fail 02-pass 03-fail; do
    local exit_code=0
    if [[ "${fixture}" != "02-pass" ]]; then
      exit_code=7
    fi
    printf '#!/usr/bin/env bash\nprintf "%%s\\n" "%s" >>"%s"\nexit %d\n' \
      "${fixture}" "${log}" "${exit_code}" >"${tmp}/${fixture}"
    chmod +x "${tmp}/${fixture}"
  done

  if run_contracts \
    "first expected failure::${tmp}/01-fail" \
    "middle expected pass::${tmp}/02-pass" \
    "last expected failure::${tmp}/03-fail" \
    >/dev/null 2>&1; then
    echo "CI source contract runner accepted a failing contract set" >&2
    return 1
  fi

  if [[ "$(tr '\n' ' ' <"${log}")" != "01-fail 02-pass 03-fail " ]]; then
    echo "CI source contract runner stopped before executing every contract" >&2
    return 1
  fi

  : >"${log}"
  if ! run_contracts "all pass::${tmp}/02-pass" >/dev/null 2>&1; then
    echo "CI source contract runner rejected an all-pass contract set" >&2
    return 1
  fi
  if [[ "$(tr '\n' ' ' <"${log}")" != "02-pass " ]]; then
    echo "CI source contract runner did not execute the all-pass fixture" >&2
    return 1
  fi
}

case "${1:-}" in
  "")
    ;;
  --self-test)
    if [[ $# -ne 1 ]]; then
      echo "Usage: $0 [--self-test]" >&2
      exit 2
    fi
    self_test_runner
    echo "CI source contract runner self-test passed."
    exit 0
    ;;
  *)
    echo "Usage: $0 [--self-test]" >&2
    exit 2
    ;;
esac

self_test_runner || exit 1

contracts=(
  "GitHub Actions workflow contract::${SCRIPT_DIR}/check-ci-workflow.sh"
  "Release reference contract::${SCRIPT_DIR}/test-release-ref-contract.sh"
  "Open-source source-surface contract::${SCRIPT_DIR}/check-open-source-release-surface.sh"
  "Local deployment contract::${SCRIPT_DIR}/test-carryforth-local-deployment.sh"
  "Source first-start contract::${SCRIPT_DIR}/test-source-dev-start.sh"
  "Carryforth CLI cutover contract::${SCRIPT_DIR}/check-cf-cli-cutover.sh"
  "Relay artifact contract::${SCRIPT_DIR}/test-ci-relay-artifact-contract.sh"
  "Playwright report contract::${SCRIPT_DIR}/test-ci-playwright-report-contract.sh"
  "Project View release contract::${SCRIPT_DIR}/test-project-view-release-contract.sh"
  "Project View v3-only runtime contract::${SCRIPT_DIR}/check-project-view-v3-runtime.sh"
  "Project Document release contract::${SCRIPT_DIR}/test-project-document-release-contract.sh"
)

if ! run_contracts "${contracts[@]}"; then
  echo "One or more CI source contracts failed." >&2
  exit 1
fi

echo "All CI source contracts passed."
