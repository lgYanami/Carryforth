#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

if ! command -v actionlint >/dev/null 2>&1; then
  echo "actionlint is required; activate the repository Hermit environment" >&2
  exit 127
fi

# Keep workflow schema/expression validation deterministic across developer and
# GitHub environments. Inline-shell linting can be re-enabled once ShellCheck
# is pinned in the repository toolchain.
actionlint -shellcheck= "${REPO_ROOT}/.github/workflows/ci.yml"
