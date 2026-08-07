#!/usr/bin/env bash
# Current Project View Stage 5 acceptance is the schema-v3 greenfield Relay +
# real CLI E2E. Legacy v2-to-v3 evidence lives only in the explicitly named
# test-project-view-legacy-v2-to-v3-migration-canary.sh recovery fixture.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

export PROJECT_VIEW_E2E_PROFILE="${PROJECT_VIEW_STAGE5_PROFILE:-${PROJECT_VIEW_E2E_PROFILE:-dev}}"
export PROJECT_VIEW_E2E_NO_BUILD="${PROJECT_VIEW_STAGE5_NO_BUILD:-${PROJECT_VIEW_E2E_NO_BUILD:-0}}"

exec "${REPO_ROOT}/scripts/test-project-view-e2e.sh"
