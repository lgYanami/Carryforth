#!/usr/bin/env bash
# Static packaging/deployment contract for the server-first Project View
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

for script in \
  scripts/test-project-view-db.sh \
  scripts/test-project-view-migrations.sh \
  scripts/test-project-view-e2e.sh \
  scripts/test-project-view-rollback-smoke.sh \
  scripts/test-project-view-compatible-rollback-smoke.sh; do
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

# Managed agents receive the real `buzz` multicall entry and PRs exercise it.
require_literal "COMMANDS=(buzz-acp buzz-agent buzz-dev-mcp buzz)" scripts/build-sprig.sh
require_literal "- \"crates/buzz-cli/**\"" .github/workflows/sprig.yml
require_literal "target/ci/buzz" .github/workflows/ci.yml
require_literal "target/ci/buzz-admin" .github/workflows/ci.yml
require_literal "--test e2e_project_view" .github/workflows/ci.yml
require_literal "just test-migrations" .github/workflows/ci.yml
require_literal "just project-view-test-e2e" .github/workflows/ci.yml
require_literal "PROJECT_VIEW_PRE_FEATURE_REF: ab3af828714ab699dfc87644d234014987a4fe6b" .github/workflows/ci.yml
require_literal "PROJECT_VIEW_COMPATIBLE_REF: 8ef125c12a9b488a2c047361bf1c1072b735b738" .github/workflows/ci.yml
require_literal "BUZZ_AUTO_MIGRATE=false" scripts/test-project-view-rollback-smoke.sh
require_literal "BUZZ_AUTO_MIGRATE=false" scripts/test-project-view-compatible-rollback-smoke.sh
require_literal "Post-mutation compatible rollback smoke" .github/workflows/ci.yml

# Observability names are an operator API; keep the full documented set wired.
require_literal "buzz_project_view_mutations_total" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_mutation_duration_seconds" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_conflicts_total" crates/buzz-relay/src/handlers/project_view.rs
require_literal "buzz_project_view_snapshot_duration_seconds" crates/buzz-relay/src/api/bridge.rs
require_literal "buzz_project_view_snapshot_retries_total" crates/buzz-relay/src/api/bridge.rs
require_literal "buzz_project_view_objects" crates/buzz-relay/src/main.rs
require_literal "buzz_project_view_projection_dispatch_errors_total" crates/buzz-relay/src/handlers/event.rs
require_literal "buzz_project_view_schema_ready" crates/buzz-relay/src/main.rs

# Kubernetes and Compose must use the centralized database flag. A Pod-local
# Project View env flag would make mixed-version rollouts unsafe.
require_literal "BUZZ_RELAY_PRIVATE_KEY" deploy/charts/buzz/templates/deployment.yaml
require_literal "secretKeyRef" deploy/charts/buzz/templates/deployment.yaml
require_literal "BUZZ_AUTO_MIGRATE" deploy/charts/buzz/templates/deployment.yaml
if rg -n "BUZZ_PROJECT_VIEW_ENABLED" deploy/charts/buzz deploy/compose; then
  echo "Project View release contract: do not add a Pod-local Project View flag" >&2
  exit 1
fi

# The runbook must retain both sides of the operational safety boundary.
require_literal "Server-first rollout" docs/project-view-operations.md
require_literal "buzz-admin project-view enable" docs/project-view-operations.md
require_literal "buzz-admin project-view disable" docs/project-view-operations.md
require_literal "After any Project View mutation has been accepted" docs/project-view-operations.md
require_literal "BUZZ_AUTO_MIGRATE=false" docs/project-view-operations.md

echo "Project View release contract passed."
