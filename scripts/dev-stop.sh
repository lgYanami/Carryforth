#!/usr/bin/env bash
# Stop the local Carryforth app and its Docker containers without deleting anything.
#
# Usage:
#   ./scripts/dev-stop.sh             # app + docker compose stop
#   ./scripts/dev-stop.sh --app-only  # app only (used by rebuild script)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
STATE_DIR="${BUZZ_DEV_STATE_DIR:-${REPO_ROOT}/target/dev-lifecycle}"
PID_FILE="${STATE_DIR}/carryforth-dev.pid"
LEGACY_PID_FILE="${STATE_DIR}/buzz-dev.pid"
MODE="all"

log() { printf '[dev-stop] %s\n' "$*"; }
warn() { printf '[dev-stop] WARNING: %s\n' "$*" >&2; }

case "${1:-}" in
  "")
    ;;
  --app-only)
    MODE="app-only"
    ;;
  *)
    printf 'Usage: %s [--app-only]\n' "$0" >&2
    exit 2
    ;;
esac

cd "${REPO_ROOT}"

process_cwd() {
  local pid="$1"
  local cwd=""

  if [[ -L "/proc/${pid}/cwd" ]]; then
    readlink "/proc/${pid}/cwd" 2>/dev/null || true
    return
  fi

  if command -v lsof >/dev/null 2>&1; then
    cwd="$(lsof -a -p "${pid}" -d cwd -Fn 2>/dev/null | sed -n 's/^n//p' | head -n 1)"
  fi
  printf '%s' "${cwd}"
}

pid_belongs_to_checkout() {
  local pid="$1"
  local cwd

  [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
  kill -0 "${pid}" 2>/dev/null || return 1
  cwd="$(process_cwd "${pid}")"
  [[ "${cwd}" == "${REPO_ROOT}" || "${cwd}" == "${REPO_ROOT}/"* ]]
}

wait_for_pid_exit() {
  local pid="$1"
  local deadline=$((SECONDS + 20))

  while kill -0 "${pid}" 2>/dev/null && ((SECONDS < deadline)); do
    sleep 0.5
  done
  ! kill -0 "${pid}" 2>/dev/null
}

group_has_live_members() {
  local pgid="$1"
  ps -eo pgid=,stat= 2>/dev/null |
    awk -v group="${pgid}" '$1 == group && $2 !~ /^Z/ { found=1 } END { exit !found }'
}

wait_for_group_exit() {
  local pgid="$1"
  local deadline=$((SECONDS + 20))

  while group_has_live_members "${pgid}" && ((SECONDS < deadline)); do
    sleep 0.5
  done
  ! group_has_live_members "${pgid}"
}

terminate_dev_leader() {
  local pid="$1"
  local pgid sid

  pid_belongs_to_checkout "${pid}" || return 1
  pgid="$(ps -o pgid= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"
  sid="$(ps -o sid= -p "${pid}" 2>/dev/null | tr -d '[:space:]')"

  log "停止 Carryforth 进程组（PID ${pid}）..."
  if [[ "${pgid}" == "${pid}" || "${sid}" == "${pid}" ]]; then
    kill -TERM -- "-${pgid}" 2>/dev/null || true
    if ! wait_for_group_exit "${pgid}"; then
      warn "进程未在 20 秒内退出，发送 KILL"
      kill -KILL -- "-${pgid}" 2>/dev/null || true
      wait_for_group_exit "${pgid}" || true
    fi
  else
    # A manually backgrounded `just dev` may share its terminal's process
    # group. Never signal that whole group; terminate only the recipe leader.
    kill -TERM "${pid}" 2>/dev/null || true
    if ! wait_for_pid_exit "${pid}"; then
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  fi
  return 0
}

stop_tracked_process() {
  local pid="" args="" candidate_pid_file stopped=false stopped_pid=""

  for candidate_pid_file in "${PID_FILE}" "${LEGACY_PID_FILE}"; do
    [[ -f "${candidate_pid_file}" ]] || continue
    pid="$(tr -d '[:space:]' <"${candidate_pid_file}")"
    args="$(ps -o args= -p "${pid}" 2>/dev/null || true)"
    if [[ "${pid}" == "${stopped_pid}" ]]; then
      continue
    fi
    if [[ "${args}" =~ (^|/)just[[:space:]]+dev([[:space:]]|$) ]] &&
      terminate_dev_leader "${pid}"; then
      # Never mutate the legacy coordinate. A later invocation will ignore its
      # stale PID after the exact checkout/process validation fails.
      if [[ "${candidate_pid_file}" == "${PID_FILE}" ]]; then
        rm -f "${PID_FILE}"
      fi
      stopped=true
      stopped_pid="${pid}"
      continue
    fi
    if [[ "${candidate_pid_file}" == "${PID_FILE}" ]]; then
      warn "忽略无效或已过期的 Carryforth PID 文件"
      rm -f "${PID_FILE}"
    fi
  done
  [[ "${stopped}" == "true" ]]
}

stop_untracked_process() {
  local pid args

  while read -r pid; do
    [[ -n "${pid}" ]] || continue
    args="$(ps -o args= -p "${pid}" 2>/dev/null || true)"
    if [[ "${args}" =~ (^|/)just[[:space:]]+dev([[:space:]]|$) ]] &&
      pid_belongs_to_checkout "${pid}"; then
      terminate_dev_leader "${pid}" || true
      return 0
    fi
  done < <(ps -eo pid= 2>/dev/null)
  return 1
}

stop_leftover_binaries() {
  local pid exe cwd found=false
  local -a pids=()

  while read -r pid; do
    [[ -n "${pid}" ]] || continue
    exe=""
    cwd=""
    if [[ -L "/proc/${pid}/exe" ]]; then
      exe="$(readlink "/proc/${pid}/exe" 2>/dev/null || true)"
    fi
    exe="${exe% (deleted)}"

    if [[ "${exe}" != "${REPO_ROOT}/target/debug/buzz-relay" &&
          "${exe}" != "${REPO_ROOT}/desktop/src-tauri/target/debug/buzz-desktop" ]]; then
      continue
    fi

    cwd="$(process_cwd "${pid}")"
    if [[ "${cwd}" == "${REPO_ROOT}" || "${cwd}" == "${REPO_ROOT}/"* ]]; then
      kill -TERM "${pid}" 2>/dev/null || true
      pids+=("${pid}")
      found=true
    fi
  done < <(ps -eo pid= 2>/dev/null)

  for pid in "${pids[@]}"; do
    if ! wait_for_pid_exit "${pid}"; then
      kill -KILL "${pid}" 2>/dev/null || true
    fi
  done

  [[ "${found}" == "true" ]]
}

app_stopped=false
if stop_tracked_process; then
  app_stopped=true
elif stop_untracked_process; then
  app_stopped=true
fi
if stop_leftover_binaries; then
  log "已停止遗留的 Carryforth 可执行进程"
  app_stopped=true
fi

if [[ "${app_stopped}" != "true" ]]; then
  log "未发现正在运行的 Carryforth 应用进程"
fi

if [[ "${MODE}" == "app-only" ]]; then
  exit 0
fi

if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
  warn "Docker daemon 未运行；应用进程已停止"
  exit 0
fi

log "停止 Docker 容器（保留容器、网络和全部 volume）..."
docker compose stop
log "已关闭。本地数据未删除。"
