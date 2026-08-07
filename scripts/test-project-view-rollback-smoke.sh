#!/usr/bin/env bash
# Start the last pre-Project-View Relay against the current additive schema.
# This proves only the pre-initialization database boundary: migrations have
# run, every Community remains disabled, and no Project View state exists. It
# does not qualify an old binary for post-mutation Project View traffic.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

pre_feature_relay="${PROJECT_VIEW_PRE_FEATURE_RELAY_BIN:?set PROJECT_VIEW_PRE_FEATURE_RELAY_BIN}"
current_admin="${PROJECT_VIEW_CURRENT_ADMIN_BIN:?set PROJECT_VIEW_CURRENT_ADMIN_BIN}"
for binary in "${pre_feature_relay}" "${current_admin}"; do
  if [[ ! -x "${binary}" ]]; then
    echo "Project View rollback smoke: missing executable ${binary}" >&2
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
    echo "Project View rollback smoke: ${container} did not become healthy" >&2
    exit 1
  fi
done

database_name="buzz_pv_rollback_$$_${RANDOM}"
if [[ ! "${database_name}" =~ ^buzz_pv_rollback_[0-9_]+$ ]]; then
  echo "Refusing unsafe scratch database name: ${database_name}" >&2
  exit 1
fi
port="${PROJECT_VIEW_ROLLBACK_PORT:-$((32000 + ($$ % 10000)))}"
health_port="$((port + 1))"
metrics_port="$((port + 2))"
host_name="project-view-rollback-${database_name}.localhost"
community_host="${host_name}:${port}"
relay_pid=""
relay_log="$(mktemp)"

cleanup() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  rm -f "${relay_log}"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null
}
trap cleanup EXIT

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
env DATABASE_URL="${database_url}" "${current_admin}" migrate
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host)
      VALUES ('00000000-0000-4000-8000-00000000c0de', '${community_host}')" >/dev/null

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 -qtA \
  -c "SELECT CASE WHEN
        (SELECT count(*) FROM _sqlx_migrations WHERE version = 25 AND success) = 1
        AND
        (SELECT count(*) FROM _sqlx_migrations WHERE version = 26 AND success) = 1
        AND
        (SELECT count(*) FROM _sqlx_migrations WHERE version = 27 AND success) = 1
        AND
        (SELECT count(*) FROM _sqlx_migrations WHERE version = 48 AND success) = 1
        AND
        (SELECT count(*) FROM _sqlx_migrations WHERE version = 50 AND success) = 1
        AND
        (SELECT bool_and(NOT project_view_enabled) FROM communities)
        AND
        NOT EXISTS (SELECT 1 FROM project_view_state)
      THEN 'ok' ELSE 'bad' END" |
  grep -qx ok

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
  BUZZ_RELAY_PRIVATE_KEY=0000000000000000000000000000000000000000000000000000000000000001 \
  "${pre_feature_relay}" >"${relay_log}" 2>&1 &
relay_pid=$!

for _ in $(seq 1 60); do
  if ! kill -0 "${relay_pid}" 2>/dev/null; then
    cat "${relay_log}" >&2
    echo "Project View rollback smoke: pre-feature Relay exited" >&2
    exit 1
  fi
  status_code="$(
    curl -s -o /dev/null -w '%{http_code}' \
      --resolve "${host_name}:${port}:127.0.0.1" \
      "http://${community_host}/_readiness" || true
  )"
  [[ "${status_code}" == "200" ]] && break
  sleep 1
done
if [[ "${status_code:-}" != "200" ]]; then
  cat "${relay_log}" >&2
  echo "Project View rollback smoke: pre-feature Relay did not become ready" >&2
  exit 1
fi

info="$(
  curl -fsS \
    --resolve "${host_name}:${port}:127.0.0.1" \
    "http://${community_host}/info"
)"
jq -e '(.supported_nips | type) == "array"' <<<"${info}" >/dev/null
if jq -e '.supported_extensions[]? | startswith("buzz-project-view-")' <<<"${info}" >/dev/null; then
  echo "Project View rollback smoke: baseline unexpectedly advertises Project View" >&2
  exit 1
fi
kill -0 "${relay_pid}"

echo "Project View pre-feature database smoke passed on the current additive schema."
