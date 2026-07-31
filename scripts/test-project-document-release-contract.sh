#!/usr/bin/env bash
# Static Stage 2 packaging/CI contract. Runtime behavior is covered by the
# dedicated unit, DB/migration, and disabled/enabled real Relay E2E gates.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

require_literal() {
  local literal="$1"
  local path="$2"
  if ! rg -Fq -- "${literal}" "${path}"; then
    echo "Project Document release contract: '${literal}' is missing from ${path}" >&2
    exit 1
  fi
}

for script in \
  scripts/test-project-document-db.sh \
  scripts/test-project-document-e2e.sh \
  scripts/test-project-document-release-contract.sh \
  scripts/test-project-view-migrations.sh; do
  if [[ ! -x "${script}" ]]; then
    echo "Project Document release contract: ${script} must be executable" >&2
    exit 1
  fi
done

# A workspace crate is not automatically covered by Buzz's explicit unit gate.
require_literal "just project-document-test-unit" Justfile
require_literal "project-document-test-db:" Justfile
require_literal "project-document-test-e2e:" Justfile
require_literal "project-document-test:" Justfile
require_literal "cargo test -p buzz-project-document" scripts/run-tests.sh
require_literal "cargo test -p buzz-acp --lib project_document" scripts/run-tests.sh
require_literal "cargo test -p buzz-admin project_document" scripts/run-tests.sh

# CI must package the black-box target and route every affected backend surface
# to the dedicated integration job. ACP and admin are intentional entries: later
# stages consume Document coordinates through those processes.
require_literal "project-document: \${{ steps.filter.outputs.project-document }}" .github/workflows/ci.yml
require_literal "crates/buzz-project-document/**" .github/workflows/ci.yml
require_literal "crates/buzz-acp/**" .github/workflows/ci.yml
require_literal "crates/buzz-admin/**" .github/workflows/ci.yml
require_literal "scripts/test-project-document-*.sh" .github/workflows/ci.yml
require_literal "--test e2e_project_document_disabled" .github/workflows/ci.yml
require_literal "--test e2e_project_document_enabled" .github/workflows/ci.yml
require_literal "project-document-integration:" .github/workflows/ci.yml
require_literal "just project-document-test-db" .github/workflows/ci.yml
require_literal "just project-document-test-e2e" .github/workflows/ci.yml

# The database default remains fail-closed while Stage 2 exposes only the
# controlled operator lifecycle and the closed Agent-first CLI surface.
require_literal "project_document_enabled BOOLEAN NOT NULL DEFAULT FALSE" schema/schema.sql
require_literal "DEFAULT FALSE" migrations/0032_project_document.sql
require_literal "unavailable:project_document:disabled" crates/buzz-relay/src/handlers/project_document.rs
require_literal "e2e_project_document_enabled" scripts/test-project-document-e2e.sh
require_literal "ProjectDocumentCommand::Bootstrap" crates/buzz-admin/src/project_document.rs
require_literal "ProjectDocumentCommand::Enable" crates/buzz-admin/src/project_document.rs
require_literal "ProjectDocumentCommand::Disable" crates/buzz-admin/src/project_document.rs
require_literal "enum DocumentsCmd" crates/buzz-cli/src/lib.rs
require_literal "Cmd::Documents" crates/buzz-cli/src/lib.rs
require_literal "Project Documents are not a Secret Store" crates/buzz-cli/src/commands/documents.rs
require_literal "buzz-project-document-v1" crates/buzz-relay/src/nip11.rs
require_literal "synthetic Secret incident drill" scripts/test-project-document-e2e.sh

echo "Project Document Stage 2 release contract passed."
