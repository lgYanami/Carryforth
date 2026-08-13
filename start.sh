#!/usr/bin/env bash
# Public one-command entry for building and starting Carryforth from source.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${REPO_ROOT}/scripts/dev-start.sh" "$@"

