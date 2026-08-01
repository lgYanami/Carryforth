#!/usr/bin/env bash
# Stop the local app, force-rebuild Buzz executable packages, and start again.
#
# Dependency build caches are retained; only Buzz-owned executable artifacts
# are cleaned. Docker containers and all data volumes remain untouched.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

log() { printf '[dev-rebuild] %s\n' "$*"; }

cd "${REPO_ROOT}"

log "停止现有 Buzz 应用进程（Docker 保持运行）..."
"${SCRIPT_DIR}/dev-stop.sh" --app-only

# shellcheck disable=SC1091
source "${REPO_ROOT}/bin/activate-hermit"
if [[ -z "${CMAKE:-}" && -x /usr/bin/cmake ]]; then
  export CMAKE=/usr/bin/cmake
fi

if [[ -f "${REPO_ROOT}/.env" ]]; then
  set -o allexport
  # shellcheck disable=SC1091
  source "${REPO_ROOT}/.env"
  set +o allexport
fi

log "清理 Buzz 可执行程序的构建产物（保留依赖缓存）..."
cargo clean \
  -p buzz-relay \
  -p buzz-admin \
  -p buzz-acp \
  -p buzz-agent \
  -p buzz-dev-mcp \
  -p buzz-cli \
  -p git-credential-nostr
cargo clean \
  --manifest-path "${REPO_ROOT}/desktop/src-tauri/Cargo.toml" \
  -p buzz-desktop

log "重新编译并启动..."
exec "${SCRIPT_DIR}/dev-start.sh"
