#!/usr/bin/env bash
# Static packaging/deployment contract for the source-first Project View
# release. Runtime behavior is covered by the dedicated DB/migration/E2E gates.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

require_literal() {
  local literal="$1"
  local path="$2"
  if ! rg -Fq -- "${literal}" "${path}"; then
    echo "Project View release contract: '${literal}' is missing from ${path}" >&2
    exit 1
  fi
}

reject_literal() {
  local literal="$1"
  local path="$2"
  if rg -Fq -- "${literal}" "${path}"; then
    echo "Project View release contract: retired token '${literal}' remains in ${path}" >&2
    exit 1
  fi
}

for script in \
  scripts/test-project-view-db.sh \
  scripts/test-project-view-migrations.sh \
  scripts/test-project-view-e2e.sh \
  scripts/test-project-view-stage5-canary.sh \
  scripts/test-project-view-stage6-canary.sh \
  scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh \
  scripts/check-project-view-v3-runtime.sh \
  scripts/test-project-view-rollback-smoke.sh; do
  if [[ ! -x "${script}" ]]; then
    echo "Project View release contract: ${script} must be executable" >&2
    exit 1
  fi
done

# The Relay image is also the operator migration/control image.
require_literal "-p buzz-admin --bin buzz-admin" Dockerfile
require_literal "/target/release/buzz-admin /usr/local/bin/buzz-admin" Dockerfile
require_literal "- \"migrations/**\"" .github/workflows/docker.yml
require_literal "- \"schema/**\"" .github/workflows/docker.yml

# Managed agents receive the real `cf` multicall entry and PRs exercise it.
require_literal "COMMANDS=(buzz-acp buzz-agent buzz-dev-mcp cf)" scripts/build-sprig.sh
require_literal "- \"crates/carryforth-cli/**\"" .github/workflows/sprig.yml
require_literal "target/ci/cf" .github/workflows/ci.yml
require_literal "target/ci/buzz-admin" .github/workflows/ci.yml
require_literal "e2e_project_view" scripts/build-ci-relay-artifacts.sh
require_literal "just test-migrations" .github/workflows/ci.yml
require_literal "just project-view-test-e2e" .github/workflows/ci.yml
require_literal "- 'scripts/check-project-view-v3-runtime.sh'" .github/workflows/ci.yml
require_literal "scripts/test-ci-source-contracts.sh" .github/workflows/ci.yml
require_literal "check-project-view-v3-runtime.sh" scripts/test-ci-source-contracts.sh
require_literal "- 'scripts/meeting-v2-actions-live-acceptance.sh'" .github/workflows/ci.yml
require_literal "PROJECT_VIEW_PRE_FEATURE_REF: aeced53115b2892c557fe54d094070f1071dbb60" .github/workflows/ci.yml
require_literal "BUZZ_AUTO_MIGRATE=false" scripts/test-project-view-rollback-smoke.sh
require_literal "Current additive schema with pre-feature Relay smoke" .github/workflows/ci.yml
require_literal "version = 48 AND success" scripts/test-project-view-rollback-smoke.sh
require_literal "version = 50 AND success" scripts/test-project-view-rollback-smoke.sh
reject_literal "PROJECT_VIEW_COMPATIBLE_REF" .github/workflows/ci.yml
reject_literal "test-project-view-compatible-rollback-smoke.sh" .github/workflows/ci.yml
reject_literal "test-project-view-compatible-rollback-smoke.sh" Justfile
require_literal "scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh" Justfile
require_literal "scripts/check-project-view-v3-runtime.sh" Justfile
require_literal "scripts/meeting-v2-actions-live-acceptance.sh" Justfile
require_literal "docs/stage/meeting/" Justfile
if [[ -e scripts/test-project-view-compatible-rollback-smoke.sh ]]; then
  echo "Project View release contract: retired old-runtime rollback smoke must stay removed" >&2
  exit 1
fi
require_literal "ALTER COLUMN project_view_schema_version SET DEFAULT 3" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "CREATE OR REPLACE FUNCTION project_role_continuity_validate_community" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "ADD COLUMN project_context_edge_enabled BOOLEAN NOT NULL DEFAULT FALSE" migrations/0049_project_context_edge.sql
require_literal "RETURN sha256(payload)" migrations/0050_project_context_edge_builtin_sha256.sql
require_literal "CREATE FUNCTION project_view_v3_bootstrap_lifecycle_valid" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "CREATE OR REPLACE FUNCTION project_view_v3_validate_row" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "maintenance.state = 'normal'" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "project_view_context_operations context_operation" migrations/0048_project_view_v3_greenfield_default.sql
reject_literal "UPDATE communities" migrations/0048_project_view_v3_greenfield_default.sql
reject_literal "DELETE FROM communities" migrations/0048_project_view_v3_greenfield_default.sql
require_literal "read_project_document_identity_at(state, &api_base_url)" desktop/src-tauri/src/commands/project_document.rs
reject_literal "require_runtime_ready(\"Project Document\")" desktop/src-tauri/src/commands/project_document.rs
require_literal "PROJECT_VIEW_E2E_SCRATCH_DATABASE=1" scripts/test-project-view-e2e.sh
require_literal "fixture_origin: \"greenfield_v3\"" scripts/test-project-view-stage6-canary.sh
require_literal "project-document-stage6-context" Justfile

# Observability names are an operator API; keep the full documented set wired.
require_literal "buzz_project_view_mutations_total" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_mutation_duration_seconds" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_conflicts_total" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_snapshot_duration_seconds" crates/buzz-relay/src/api/bridge.rs
require_literal "buzz_project_view_snapshot_retries_total" crates/buzz-relay/src/api/bridge.rs
require_literal "buzz_project_view_objects" crates/buzz-relay/src/main.rs
require_literal "buzz_project_view_projection_dispatch_errors_total" crates/buzz-relay/src/handlers/event.rs
require_literal "buzz_project_view_schema_ready" crates/buzz-relay/src/main.rs
require_literal "buzz_project_view_migration_required_communities" crates/buzz-relay/src/main.rs
require_literal "buzz_project_document_migration_required_communities" crates/buzz-relay/src/main.rs
require_literal "project_view_migration_required_count" crates/buzz-db/src/project_view.rs
require_literal "project_document_migration_required_count" crates/buzz-db/src/project_document.rs

# The only active local deployment surface must use one stable Relay identity
# and the centralized database migration gate. A process-local Project View
# switch would make restarts and mixed developer binaries unsafe.
require_literal "CARRYFORTH_RELAY_PRIVATE_KEY" deploy/local/compose.yml
require_literal "BUZZ_RELAY_PRIVATE_KEY" deploy/local/compose.yml
require_literal "BUZZ_AUTO_MIGRATE" deploy/local/compose.yml
if rg -n "BUZZ_PROJECT_VIEW_ENABLED" deploy/local deploy/compose; then
  echo "Project View release contract: do not add a process-local Project View flag" >&2
  exit 1
fi

echo "Project View release contract passed."
