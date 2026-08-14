#!/usr/bin/env bash
# Build the Relay-side binaries and nextest archive shared by integration jobs.
# Keep this script as the cache-contract input instead of hashing the entire CI
# workflow, where unrelated presentation or timeout edits would evict the cache.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

cargo build \
  --profile ci \
  -p buzz-relay \
  -p carryforth-cli \
  -p buzz-admin \
  -p git-credential-nostr

cargo nextest archive \
  --cargo-profile ci \
  -p buzz-db \
  -p buzz-relay \
  -p buzz-test-client \
  --lib \
  --test e2e_event_reminder \
  --test e2e_project_document_disabled \
  --test e2e_project_document_enabled \
  --test e2e_project_context_stage1 \
  --test e2e_project_context_stage3 \
  --test e2e_meeting \
  --test e2e_meeting_floor \
  --test e2e_meeting_baton \
  --test e2e_meeting_v2_stage1 \
  --test e2e_meeting_rollout \
  --test e2e_project_view \
  --archive-file target/ci/backend-integration-tests.tar.zst

for artifact in \
  target/ci/buzz-relay \
  target/ci/cf \
  target/ci/buzz-admin \
  target/ci/git-credential-nostr \
  target/ci/backend-integration-tests.tar.zst; do
  if [[ ! -s "${artifact}" ]]; then
    printf 'Expected CI artifact is missing or empty: %s\n' "${artifact}" >&2
    exit 1
  fi
done
