#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILDER="${SCRIPT_DIR}/build-ci-relay-artifacts.sh"
RELAY_E2E_RUNNER="${SCRIPT_DIR}/run-ci-relay-e2e.sh"
WORKFLOW="${REPO_ROOT}/.github/workflows/ci.yml"

# shellcheck source=build-ci-relay-artifacts.sh
source "${BUILDER}"

fail() {
  echo "Relay artifact contract: $*" >&2
  exit 1
}

require_literal() {
  local literal="$1"
  local path="$2"
  rg -Fq -- "${literal}" "${path}" ||
    fail "'${literal}' is missing from ${path#"${REPO_ROOT}/"}"
}

assert_exact_array() {
  local name="$1"
  shift
  local -n actual="${name}"
  local -a expected=("$@")
  [[ "${actual[*]}" == "${expected[*]}" ]] ||
    fail "${name} does not match the frozen manifest"
}

assert_unique_array() {
  local name="$1"
  local -n values="${name}"
  local unique_count
  unique_count="$(printf '%s\n' "${values[@]}" | sort -u | wc -l)"
  [[ "${unique_count}" -eq "${#values[@]}" ]] || fail "${name} contains duplicates"
}

[[ -x "${BUILDER}" ]] || fail "build-ci-relay-artifacts.sh must be executable"
[[ -x "${RELAY_E2E_RUNNER}" ]] || fail "run-ci-relay-e2e.sh must be executable"

assert_exact_array BINARY_PACKAGES \
  buzz-relay carryforth-cli buzz-admin git-credential-nostr
assert_exact_array ARCHIVE_PACKAGES buzz-db buzz-relay buzz-test-client
[[ "${ARCHIVE_INCLUDE_LIBS}" -eq 1 ]] || fail "archive must include package library tests"
assert_exact_array ARCHIVE_TESTS \
  e2e_event_reminder \
  e2e_project_document_disabled \
  e2e_project_document_enabled \
  e2e_project_context_stage1 \
  e2e_project_context_stage3 \
  e2e_meeting \
  e2e_meeting_floor \
  e2e_meeting_baton \
  e2e_meeting_v2_stage1 \
  e2e_meeting_rollout \
  e2e_project_view \
  e2e_persona \
  e2e_nostr_interop \
  e2e_relay
assert_exact_array ARTIFACT_FILENAMES \
  buzz-relay cf buzz-admin git-credential-nostr backend-integration-tests.tar.zst

assert_unique_array BINARY_PACKAGES
assert_unique_array ARCHIVE_PACKAGES
assert_unique_array ARCHIVE_TESTS
assert_unique_array ARTIFACT_FILENAMES

require_literal "bash scripts/build-ci-relay-artifacts.sh" "${WORKFLOW}"
require_literal "bash scripts/build-ci-relay-artifacts.sh --verify-only" "${WORKFLOW}"
require_literal "scripts/run-ci-relay-e2e.sh" "${WORKFLOW}"
require_literal "test-ci-relay-artifact-contract.sh" "${REPO_ROOT}/scripts/test-ci-source-contracts.sh"
require_literal "binary(=e2e_persona) | binary(=e2e_nostr_interop)" "${RELAY_E2E_RUNNER}"
require_literal "binary(=e2e_relay) and test(/invite/)" "${RELAY_E2E_RUNNER}"
require_literal "binary(=e2e_relay) and test(=nip43_membership_snapshots_are_rejected)" "${RELAY_E2E_RUNNER}"
require_literal "cargo nextest list" "${RELAY_E2E_RUNNER}"
require_literal "--run-ignored ignored-only" "${RELAY_E2E_RUNNER}"

tmp="$(mktemp -d)"
trap 'rm -rf "${tmp}"' EXIT

for artifact in "${ARTIFACT_FILENAMES[@]}"; do
  printf 'artifact\n' >"${tmp}/${artifact}"
done
"${BUILDER}" --verify-only --artifact-dir "${tmp}"

for artifact in "${ARTIFACT_FILENAMES[@]}"; do
  rm "${tmp}/${artifact}"
  if "${BUILDER}" --verify-only --artifact-dir "${tmp}" >/dev/null 2>&1; then
    fail "verifier accepted missing artifact ${artifact}"
  fi
  printf 'artifact\n' >"${tmp}/${artifact}"

  : >"${tmp}/${artifact}"
  if "${BUILDER}" --verify-only --artifact-dir "${tmp}" >/dev/null 2>&1; then
    fail "verifier accepted empty artifact ${artifact}"
  fi
  printf 'artifact\n' >"${tmp}/${artifact}"
done

if "${BUILDER}" --unknown-option >/dev/null 2>&1; then
  fail "builder accepted an unknown option"
fi
if "${BUILDER}" --artifact-dir "${tmp}" >/dev/null 2>&1; then
  fail "builder accepted --artifact-dir outside --verify-only"
fi

echo "Relay artifact contract passed."
