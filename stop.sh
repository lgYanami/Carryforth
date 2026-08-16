#!/usr/bin/env bash
# Public one-command entry for stopping Carryforth without deleting local data.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${REPO_ROOT}/scripts/dev-stop.sh" "$@"
