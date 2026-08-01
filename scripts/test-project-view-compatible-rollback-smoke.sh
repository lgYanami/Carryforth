#!/usr/bin/env bash
# Write Project View state with the current Relay, then restart the exact same
# database with a fixed older Project-View-aware Relay. This proves the only
# permitted rollback boundary after the first mutation: the rollback binary
# must retain migration 25, kind classifiers, the reader gate, and projections.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

compatible_relay="${PROJECT_VIEW_COMPATIBLE_RELAY_BIN:?set PROJECT_VIEW_COMPATIBLE_RELAY_BIN}"
current_relay="${PROJECT_VIEW_CURRENT_RELAY_BIN:?set PROJECT_VIEW_CURRENT_RELAY_BIN}"
current_admin="${PROJECT_VIEW_CURRENT_ADMIN_BIN:?set PROJECT_VIEW_CURRENT_ADMIN_BIN}"
current_buzz="${PROJECT_VIEW_CURRENT_BUZZ_BIN:?set PROJECT_VIEW_CURRENT_BUZZ_BIN}"
for binary in "${compatible_relay}" "${current_relay}" "${current_admin}" "${current_buzz}"; do
  if [[ ! -x "${binary}" ]]; then
    echo "Project View compatible rollback smoke: missing executable ${binary}" >&2
    exit 1
  fi
done

docker compose up -d postgres redis minio minio-init >/dev/null
for container in buzz-postgres buzz-redis buzz-minio; do
  status=""
  for _ in $(seq 1 60); do
    status="$(docker inspect --format='{{.State.Health.Status}}' "${container}" 2>/dev/null || true)"
    [[ "${status}" == "healthy" ]] && break
    sleep 2
  done
  if [[ "${status}" != "healthy" ]]; then
    docker logs "${container}" || true
    echo "Project View compatible rollback smoke: ${container} did not become healthy" >&2
    exit 1
  fi
done

database_name="buzz_pv_compatible_$$_${RANDOM}"
if [[ ! "${database_name}" =~ ^buzz_pv_compatible_[0-9_]+$ ]]; then
  echo "Refusing unsafe scratch database name: ${database_name}" >&2
  exit 1
fi
port="${PROJECT_VIEW_COMPATIBLE_ROLLBACK_PORT:-$((42000 + ($$ % 10000)))}"
health_port="$((port + 1))"
metrics_port="$((port + 2))"
host_name="project-view-compatible-${database_name}.localhost"
community_host="${host_name}:${port}"
database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
relay_owner_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
writer_private_key=0000000000000000000000000000000000000000000000000000000000000002
writer_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
outsider_private_key=0000000000000000000000000000000000000000000000000000000000000003
relay_pid=""
relay_log="$(mktemp)"
profile_file="$(mktemp)"
goal_file="$(mktemp)"
outsider_log="$(mktemp)"

stop_relay() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  relay_pid=""
}

cleanup() {
  stop_relay
  rm -f "${relay_log}" "${profile_file}" "${goal_file}" "${outsider_log}"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null
}
trap cleanup EXIT

start_relay() {
  local binary="$1"
  local label="$2"
  local status_code=""
  : >"${relay_log}"
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    RELAY_URL="ws://${community_host}" \
    BUZZ_BIND_ADDR="0.0.0.0:${port}" \
    BUZZ_HEALTH_PORT="${health_port}" \
    BUZZ_METRICS_PORT="${metrics_port}" \
    BUZZ_AUTO_MIGRATE=false \
    BUZZ_REQUIRE_AUTH_TOKEN=false \
    BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    RELAY_OWNER_PUBKEY="${relay_owner_pubkey}" \
    "${binary}" >"${relay_log}" 2>&1 &
  relay_pid=$!

  for _ in $(seq 1 60); do
    if ! kill -0 "${relay_pid}" 2>/dev/null; then
      cat "${relay_log}" >&2
      echo "Project View compatible rollback smoke: ${label} Relay exited" >&2
      exit 1
    fi
    status_code="$(
      curl -s -o /dev/null -w '%{http_code}' \
        --resolve "${host_name}:${port}:127.0.0.1" \
        "http://${community_host}/_readiness" || true
    )"
    [[ "${status_code}" == "200" ]] && return
    sleep 1
  done

  cat "${relay_log}" >&2
  echo "Project View compatible rollback smoke: ${label} Relay did not become ready" >&2
  exit 1
}

run_buzz() {
  local private_key="$1"
  shift
  env \
    BUZZ_RELAY_URL="http://${community_host}" \
    BUZZ_PRIVATE_KEY="${private_key}" \
    "${current_buzz}" "$@"
}

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null

env DATABASE_URL="${database_url}" "${current_admin}" migrate
docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  < scripts/attach-schema-partitions.sql
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host)
      VALUES ('00000000-0000-4000-8000-00000000c0de', '${community_host}');
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES (
        '00000000-0000-4000-8000-00000000c0de',
        '${writer_pubkey}',
        'member'
      );" >/dev/null

env \
  DATABASE_URL="${database_url}" \
  BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  "${current_admin}" project-view enable --community "${community_host}"

jq -n '{
  name: "Compatible rollback",
  positioning: "One canonical Project View survives an application rollback",
  purpose: "Exercise the post-mutation rollback boundary",
  problem: "A pre-feature binary cannot safely read stored projections",
  scope: "Project View backend v0"
}' >"${profile_file}"
jq -n '{
  id: "00000000-0000-4000-8000-000000000005",
  title: "Retain the strict Project View contract",
  desired_outcome: "The older compatible Relay reads the same revision",
  directions: ["Keep migration 25, classifiers, and reader gates"]
}' >"${goal_file}"

start_relay "${current_relay}" "current"
write_result="$(
  run_buzz "${writer_private_key}" \
    --format compact \
    project-view init \
    --profile "${profile_file}" \
    --goal "${goal_file}"
)"
if ! jq -e '.accepted == true' <<<"${write_result}" >/dev/null; then
  echo "Project View compatible rollback smoke: current write was not accepted" >&2
  jq . <<<"${write_result}" >&2
  exit 1
fi
database_state="$(
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 -qtA \
  -c "SELECT CASE WHEN
        (SELECT project_revision FROM project_view_state
          WHERE community_id = '00000000-0000-4000-8000-00000000c0de') = 1
        AND
        (SELECT count(*) FROM project_view_objects
          WHERE community_id = '00000000-0000-4000-8000-00000000c0de'
            AND deleted_at IS NULL) = 2
      THEN 'ok' ELSE 'bad' END"
)"
if [[ "${database_state}" != "ok" ]]; then
  echo "Project View compatible rollback smoke: current state was not revision 1 with two objects" >&2
  exit 1
fi
stop_relay

start_relay "${compatible_relay}" "compatible rollback"
info="$(
  curl -fsS \
    --resolve "${host_name}:${port}:127.0.0.1" \
    "http://${community_host}/info"
)"
if ! jq -e '
  .self == "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
  and any(.supported_extensions[]?; . == "buzz-project-view-v1")
' <<<"${info}" >/dev/null; then
  echo "Project View compatible rollback smoke: compatible Relay capability is invalid" >&2
  jq . <<<"${info}" >&2
  exit 1
fi

snapshot="$(run_buzz "${writer_private_key}" --format compact project-view get)"
if ! jq -e '
  .initialized == true
  and .project_revision == 1
  and .project.data.data.name == "Compatible rollback"
  and (.goals | length) == 1
' <<<"${snapshot}" >/dev/null; then
  echo "Project View compatible rollback smoke: compatible Relay returned the wrong snapshot" >&2
  jq . <<<"${snapshot}" >&2
  exit 1
fi

if run_buzz "${outsider_private_key}" --format compact project-view get \
  >"${outsider_log}" 2>&1; then
  echo "Project View compatible rollback smoke: non-member read unexpectedly succeeded" >&2
  exit 1
fi
if ! rg -q "restricted" "${outsider_log}"; then
  cat "${outsider_log}" >&2
  echo "Project View compatible rollback smoke: non-member rejection was not restricted" >&2
  exit 1
fi
kill -0 "${relay_pid}"

echo "Project View post-mutation compatible rollback smoke passed."
