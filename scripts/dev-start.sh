#!/usr/bin/env bash
# Start the complete local Carryforth stack in the background.
#
# Runtime state and logs are stored under target/dev-lifecycle/ so this script
# can safely distinguish its own process group from unrelated local services.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
STATE_DIR="${BUZZ_DEV_STATE_DIR:-${REPO_ROOT}/target/dev-lifecycle}"
PID_FILE="${STATE_DIR}/carryforth-dev.pid"
LOG_FILE="${STATE_DIR}/carryforth-dev.log"
LEGACY_PID_FILE="${STATE_DIR}/buzz-dev.pid"
LEGACY_LOG_FILE="${STATE_DIR}/buzz-dev.log"
START_TIMEOUT_SECONDS="${BUZZ_DEV_START_TIMEOUT_SECONDS:-900}"

log() { printf '[dev-start] %s\n' "$*"; }
fail() {
  printf '[dev-start] ERROR: %s\n' "$*" >&2
  exit 1
}

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

process_belongs_to_this_checkout() {
  local pid="$1"
  local args cwd

  [[ "${pid}" =~ ^[0-9]+$ ]] || return 1
  kill -0 "${pid}" 2>/dev/null || return 1
  args="$(ps -o args= -p "${pid}" 2>/dev/null || true)"
  [[ "${args}" =~ (^|/)just[[:space:]]+dev([[:space:]]|$) ]] || return 1
  cwd="$(process_cwd "${pid}")"
  [[ "${cwd}" == "${REPO_ROOT}" || "${cwd}" == "${REPO_ROOT}/"* ]] || return 1

  return 0
}

container_health() {
  docker inspect --format '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' \
    "$1" 2>/dev/null || printf 'missing'
}

wait_for_docker_services() {
  local deadline=$((SECONDS + 180))
  local postgres redis minio keycloak prometheus

  log "等待 Postgres、Redis、MinIO、Keycloak 和 Prometheus 就绪..."
  while ((SECONDS < deadline)); do
    postgres="$(container_health buzz-postgres)"
    redis="$(container_health buzz-redis)"
    minio="$(container_health buzz-minio)"
    keycloak="$(container_health buzz-keycloak)"
    prometheus="$(container_health buzz-prometheus)"

    if [[ "${postgres}" == "healthy" &&
          "${redis}" == "healthy" &&
          "${minio}" == "healthy" &&
          "${keycloak}" == "healthy" &&
          "${prometheus}" == "running" ]]; then
      return 0
    fi
    sleep 2
  done

  docker compose ps -a >&2 || true
  printf '[dev-start] Keycloak 最近日志：\n' >&2
  docker compose logs --tail=40 keycloak >&2 || true
  return 1
}

cd "${REPO_ROOT}"

if [[ ! "${START_TIMEOUT_SECONDS}" =~ ^[1-9][0-9]*$ ]]; then
  fail "BUZZ_DEV_START_TIMEOUT_SECONDS 必须是正整数"
fi

# Use the repository-pinned Rust/Node toolchain. The system CMake avoids an
# unnecessary Hermit lazy download on Linux machines that already provide it.
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

command -v docker >/dev/null 2>&1 || fail "未找到 Docker"
docker info >/dev/null 2>&1 || fail "Docker daemon 未运行"

mkdir -p "${STATE_DIR}"

existing_pid=""
existing_log_file="${LOG_FILE}"
for candidate_pid_file in "${PID_FILE}" "${LEGACY_PID_FILE}"; do
  [[ -f "${candidate_pid_file}" ]] || continue
  candidate_pid="$(tr -d '[:space:]' <"${candidate_pid_file}")"
  if ! process_belongs_to_this_checkout "${candidate_pid}"; then
    # Only the current Carryforth coordinate is owned by this script. Legacy
    # state remains untouched so an older checkout can still account for it.
    [[ "${candidate_pid_file}" == "${PID_FILE}" ]] && rm -f "${PID_FILE}"
    continue
  fi
  if [[ -n "${existing_pid}" && "${existing_pid}" != "${candidate_pid}" ]]; then
    fail "检测到当前 checkout 有多个受管理的开发进程（PID ${existing_pid}、${candidate_pid}）"
  fi
  if [[ -z "${existing_pid}" ]]; then
    # The current Carryforth coordinate is checked first and wins when both
    # coordinates describe the same live process.
    existing_pid="${candidate_pid}"
    if [[ "${candidate_pid_file}" == "${LEGACY_PID_FILE}" ]]; then
      existing_log_file="${LEGACY_LOG_FILE}"
    else
      existing_log_file="${LOG_FILE}"
    fi
  fi
done

log "启动或恢复 Docker 容器（不会删除 volume）..."
docker compose up -d
wait_for_docker_services || fail "Docker 服务未能在 180 秒内全部就绪"

if [[ -n "${existing_pid}" ]]; then
  log "Carryforth 已在运行（PID ${existing_pid}）"
  log "日志：${existing_log_file}"
  exit 0
fi

health_port="${BUZZ_HEALTH_PORT:-8080}"
if curl --silent --fail --max-time 1 "http://127.0.0.1:${health_port}/_readiness" >/dev/null 2>&1; then
  fail "端口 ${health_port} 上已有未受脚本管理的 Carryforth Relay；请先运行 ./scripts/dev-stop.sh"
fi
bind_addr="${BUZZ_BIND_ADDR:-0.0.0.0:3000}"
relay_port="${bind_addr##*:}"

: >"${LOG_FILE}"
log "后台启动 relay 与桌面端..."
rm -f "${PID_FILE}"
# A background job can itself be a process-group leader under interactive job
# control. The small launcher forks only in that case, creates a new session,
# records the actual session leader, and then replaces itself with `just dev`.
nohup python3 -c '
import os
import sys

pid_file, executable, *arguments = sys.argv[1:]
if os.getpid() == os.getpgrp():
    if os.fork() != 0:
        os._exit(0)
os.setsid()
with open(pid_file, "w", encoding="utf-8") as handle:
    handle.write(f"{os.getpid()}\n")
os.execv(executable, [executable, *arguments])
' "${PID_FILE}" "${REPO_ROOT}/bin/just" dev >>"${LOG_FILE}" 2>&1 </dev/null &
launcher_pid=$!

dev_pid=""
for _ in $(seq 1 50); do
  if [[ -s "${PID_FILE}" ]]; then
    dev_pid="$(tr -d '[:space:]' <"${PID_FILE}")"
    if process_belongs_to_this_checkout "${dev_pid}"; then
      break
    fi
  fi
  if ! kill -0 "${launcher_pid}" 2>/dev/null && [[ -z "${dev_pid}" ]]; then
    break
  fi
  sleep 0.1
done
if [[ -z "${dev_pid}" ]] || ! process_belongs_to_this_checkout "${dev_pid}"; then
  rm -f "${PID_FILE}"
  tail -n 80 "${LOG_FILE}" >&2 || true
  fail "无法创建受管理的 Carryforth 后台进程"
fi

deadline=$((SECONDS + START_TIMEOUT_SECONDS))
next_progress=$((SECONDS + 15))
while ((SECONDS < deadline)); do
  if ! process_belongs_to_this_checkout "${dev_pid}"; then
    rm -f "${PID_FILE}"
    printf '[dev-start] 启动日志末尾：\n' >&2
    tail -n 80 "${LOG_FILE}" >&2 || true
    fail "Carryforth 在启动过程中退出"
  fi

  desktop_running=false
  if ps -eo pgid=,comm= 2>/dev/null |
    awk -v group="${dev_pid}" '$1 == group && $2 == "buzz-desktop" { found=1 } END { exit !found }'; then
    desktop_running=true
  fi

  if [[ "${desktop_running}" == "true" ]] &&
    curl --silent --fail --max-time 1 "http://127.0.0.1:${health_port}/_readiness" >/dev/null 2>&1; then
    log "Carryforth 已启动（PID ${dev_pid}）"
    log "Relay：ws://localhost:${relay_port:-3000}"
    log "Prometheus：http://localhost:${BUZZ_PROMETHEUS_PORT:-9091}"
    log "日志：${LOG_FILE}"
    exit 0
  fi

  if ((SECONDS >= next_progress)); then
    log "仍在编译或等待桌面端启动..."
    next_progress=$((SECONDS + 15))
  fi
  sleep 1
done

"${SCRIPT_DIR}/dev-stop.sh" --app-only >/dev/null 2>&1 || true
printf '[dev-start] 启动日志末尾：\n' >&2
tail -n 80 "${LOG_FILE}" >&2 || true
fail "Carryforth 未能在 ${START_TIMEOUT_SECONDS} 秒内启动"
