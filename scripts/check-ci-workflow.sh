#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

if ! command -v actionlint >/dev/null 2>&1; then
  echo "actionlint is required; activate the repository Hermit environment" >&2
  exit 127
fi

actionlint "${REPO_ROOT}/.github/workflows/ci.yml"
