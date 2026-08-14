#!/usr/bin/env bash
# Run the exact Relay E2E selection from the shared nextest archive.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
ARCHIVE="${REPO_ROOT}/target/ci/backend-integration-tests.tar.zst"

readonly PERSONA_INTEROP_FILTER='binary(=e2e_persona) | binary(=e2e_nostr_interop)'
readonly RELAY_INVITE_FILTER='binary(=e2e_relay) and test(/invite/)'
readonly RELAY_NIP43_FILTER='binary(=e2e_relay) and test(=nip43_membership_snapshots_are_rejected)'

usage() {
  echo "Usage: $0 [--archive-file <path>] [--verify-selection-only]" >&2
}

list_tests() {
  local filter="$1"
  cargo nextest list \
    --archive-file "${ARCHIVE}" \
    --run-ignored ignored-only \
    --message-format oneline \
    -E "${filter}"
}

verify_selection() {
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN

  list_tests "${PERSONA_INTEROP_FILTER}" >"${tmp}/persona-interop"
  list_tests 'binary(=e2e_persona)' >"${tmp}/persona"
  list_tests 'binary(=e2e_nostr_interop)' >"${tmp}/interop"
  cat "${tmp}/persona" "${tmp}/interop" | sort -u >"${tmp}/persona-interop-expected"
  sort -u "${tmp}/persona-interop" -o "${tmp}/persona-interop"
  diff -u "${tmp}/persona-interop-expected" "${tmp}/persona-interop"

  list_tests "${RELAY_INVITE_FILTER} | ${RELAY_NIP43_FILTER}" >"${tmp}/relay-selected"
  list_tests 'binary(=e2e_relay)' >"${tmp}/relay-all"
  awk '/invite|nip43_membership_snapshots_are_rejected/' "${tmp}/relay-all" | sort -u >"${tmp}/relay-expected"
  sort -u "${tmp}/relay-selected" -o "${tmp}/relay-selected"
  diff -u "${tmp}/relay-expected" "${tmp}/relay-selected"

  [[ -s "${tmp}/persona" ]] || { echo "Relay E2E selection has no Persona tests" >&2; return 1; }
  [[ -s "${tmp}/interop" ]] || { echo "Relay E2E selection has no Nostr Interop tests" >&2; return 1; }
  [[ -s "${tmp}/relay-expected" ]] || { echo "Relay E2E selection has no Relay tests" >&2; return 1; }
}

verify_only=0
while [[ $# -gt 0 ]]; do
  case "$1" in
    --archive-file)
      [[ $# -ge 2 && -n "$2" ]] || { usage; exit 2; }
      ARCHIVE="$2"
      shift 2
      ;;
    --verify-selection-only)
      verify_only=1
      shift
      ;;
    *)
      usage
      exit 2
      ;;
  esac
done

[[ -s "${ARCHIVE}" ]] || { echo "Relay E2E archive is missing or empty: ${ARCHIVE}" >&2; exit 1; }
verify_selection
if [[ "${verify_only}" -eq 1 ]]; then
  echo "Relay E2E archive selection contract passed."
  exit 0
fi

cargo nextest run \
  --archive-file "${ARCHIVE}" \
  --run-ignored ignored-only \
  --no-capture \
  --test-threads 1 \
  -E "${PERSONA_INTEROP_FILTER}"
cargo nextest run \
  --archive-file "${ARCHIVE}" \
  --run-ignored ignored-only \
  --no-capture \
  --test-threads 1 \
  -E "${RELAY_INVITE_FILTER}"
cargo nextest run \
  --archive-file "${ARCHIVE}" \
  --run-ignored ignored-only \
  --no-capture \
  --test-threads 1 \
  -E "${RELAY_NIP43_FILTER}"
