#!/usr/bin/env bash
# Build and verify the Relay-side binaries and nextest archive shared by CI.
# The arrays in this file are the canonical artifact manifest.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

readonly DEFAULT_ARTIFACT_DIR="target/ci"
readonly ARCHIVE_FILENAME="backend-integration-tests.tar.zst"
readonly ARCHIVE_INCLUDE_LIBS=1

readonly -a BINARY_PACKAGES=(
  buzz-relay
  carryforth-cli
  buzz-admin
  git-credential-nostr
)

readonly -a ARCHIVE_PACKAGES=(
  buzz-db
  buzz-relay
  buzz-test-client
)

readonly -a ARCHIVE_TESTS=(
  e2e_event_reminder
  e2e_project_document_disabled
  e2e_project_document_enabled
  e2e_project_context_stage1
  e2e_project_context_stage3
  e2e_meeting
  e2e_meeting_floor
  e2e_meeting_baton
  e2e_meeting_v2_stage1
  e2e_meeting_rollout
  e2e_project_view
  e2e_persona
  e2e_nostr_interop
  e2e_relay
)

readonly -a ARTIFACT_FILENAMES=(
  buzz-relay
  cf
  buzz-admin
  git-credential-nostr
  "${ARCHIVE_FILENAME}"
)

usage() {
  echo "Usage: $0 [--verify-only [--artifact-dir <directory>]]" >&2
}

verify_artifacts() {
  local artifact_dir="$1"
  local filename path
  for filename in "${ARTIFACT_FILENAMES[@]}"; do
    path="${artifact_dir}/${filename}"
    if [[ ! -s "${path}" ]]; then
      printf 'Expected CI artifact is missing or empty: %s\n' "${path}" >&2
      return 1
    fi
  done
}

build_artifacts() {
  local -a build_args=(--locked --profile ci)
  local package test
  for package in "${BINARY_PACKAGES[@]}"; do
    build_args+=(-p "${package}")
  done
  cargo build "${build_args[@]}"

  local -a archive_args=(--locked --cargo-profile ci)
  for package in "${ARCHIVE_PACKAGES[@]}"; do
    archive_args+=(-p "${package}")
  done
  if [[ "${ARCHIVE_INCLUDE_LIBS}" -eq 1 ]]; then
    archive_args+=(--lib)
  fi
  for test in "${ARCHIVE_TESTS[@]}"; do
    archive_args+=(--test "${test}")
  done
  archive_args+=(--archive-file "${DEFAULT_ARTIFACT_DIR}/${ARCHIVE_FILENAME}")
  cargo nextest archive "${archive_args[@]}"
}

main() {
  local mode="build"
  local artifact_dir="${DEFAULT_ARTIFACT_DIR}"

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --verify-only)
        mode="verify"
        shift
        ;;
      --artifact-dir)
        if [[ "${mode}" != "verify" || $# -lt 2 || -z "$2" ]]; then
          usage
          return 2
        fi
        artifact_dir="$2"
        shift 2
        ;;
      *)
        usage
        return 2
        ;;
    esac
  done

  cd "${REPO_ROOT}"
  if [[ "${mode}" == "verify" ]]; then
    verify_artifacts "${artifact_dir}"
    return
  fi

  build_artifacts
  verify_artifacts "${DEFAULT_ARTIFACT_DIR}"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
fi
