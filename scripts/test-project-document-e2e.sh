#!/usr/bin/env bash
# Start an isolated Stage 1 Relay and prove Project Document stays flag-off:
# the real CLI can operate as an ordinary member, while Document submission,
# advertisement, historical reads, COUNT, and behind-Relay rows fail closed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

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
    echo "Project Document E2E: ${container} did not become healthy" >&2
    exit 1
  fi
done

database_name="buzz_pd_e2e_$$_${RANDOM}"
if [[ ! "${database_name}" =~ ^buzz_pd_e2e_[0-9_]+$ ]]; then
  echo "Refusing unsafe scratch database name: ${database_name}" >&2
  exit 1
fi

profile="${PROJECT_DOCUMENT_E2E_PROFILE:-dev}"
if [[ "${profile}" == "dev" ]]; then
  bin_dir="${REPO_ROOT}/target/debug"
else
  bin_dir="${REPO_ROOT}/target/${profile}"
fi

port="${PROJECT_DOCUMENT_E2E_PORT:-$((23000 + ($$ % 9000)))}"
health_port="$((port + 1))"
metrics_port="$((port + 2))"
test_host="project-document-${database_name}.localhost:${port}"
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

export PGHOST=localhost
export PGPORT=5432
export PGUSER=buzz
export PGPASSWORD=buzz_dev
export PGDATABASE="${database_name}"
export PGSCHEMA_PLAN_HOST=localhost
export PGSCHEMA_PLAN_PORT=5432
export PGSCHEMA_PLAN_DB=postgres
export PGSCHEMA_PLAN_USER=buzz
export PGSCHEMA_PLAN_PASSWORD=buzz_dev

./bin/pgschema apply --file schema/schema.sql --auto-approve >/dev/null
docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  < scripts/attach-schema-partitions.sql

relay_private_key=0000000000000000000000000000000000000000000000000000000000000001
relay_owner_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
member_private_key=0000000000000000000000000000000000000000000000000000000000000002
member_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host)
      VALUES ('00000000-0000-4000-8000-00000000d0c0', '${test_host}');
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES ('00000000-0000-4000-8000-00000000d0c0', '${member_pubkey}', 'member');" >/dev/null

if [[ "${PROJECT_DOCUMENT_E2E_NO_BUILD:-0}" != "1" ]]; then
  if [[ "${profile}" == "dev" ]]; then
    cargo build -p buzz-relay -p buzz-cli -p buzz-admin
  else
    cargo build --profile "${profile}" -p buzz-relay -p buzz-cli -p buzz-admin
  fi
fi
for binary in buzz-relay buzz buzz-admin; do
  if [[ ! -x "${bin_dir}/${binary}" ]]; then
    echo "Project Document E2E: missing executable ${bin_dir}/${binary}" >&2
    exit 1
  fi
done

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
status_json="$(
  env DATABASE_URL="${database_url}" \
    "${bin_dir}/buzz-admin" project-document status --community "${test_host}"
)"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].catalog_revision == null
  and .[0].revision_count == 0
' <<<"${status_json}" >/dev/null

relay_url="ws://${test_host}"
env \
  DATABASE_URL="${database_url}" \
  REDIS_URL=redis://localhost:6379 \
  RELAY_URL="${relay_url}" \
  BUZZ_BIND_ADDR="0.0.0.0:${port}" \
  BUZZ_HEALTH_PORT="${health_port}" \
  BUZZ_METRICS_PORT="${metrics_port}" \
  BUZZ_AUTO_MIGRATE=false \
  BUZZ_REQUIRE_AUTH_TOKEN=false \
  BUZZ_REQUIRE_RELAY_MEMBERSHIP=true \
  BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
  RELAY_OWNER_PUBKEY="${relay_owner_pubkey}" \
  "${bin_dir}/buzz-relay" >"${relay_log}" 2>&1 &
relay_pid=$!

for _ in $(seq 1 60); do
  if ! kill -0 "${relay_pid}" 2>/dev/null; then
    cat "${relay_log}" >&2
    echo "Project Document E2E: Relay exited before readiness" >&2
    exit 1
  fi
  status_code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:${port}/_readiness" || true)"
  [[ "${status_code}" == "200" ]] && break
  sleep 1
done
if [[ "${status_code:-}" != "200" ]]; then
  cat "${relay_log}" >&2
  echo "Project Document E2E: Relay did not become ready" >&2
  exit 1
fi

# A packaged ordinary CLI must still work against the same host. There is no
# Document CLI in Stage 1; adding one here would prematurely expose Stage 2.
env \
  BUZZ_RELAY_URL="http://${test_host}" \
  BUZZ_PRIVATE_KEY="${member_private_key}" \
  "${bin_dir}/buzz" channels list >/dev/null

export DATABASE_URL="${database_url}"
export PROJECT_DOCUMENT_E2E_RELAY_URL="${relay_url}"
export PROJECT_DOCUMENT_E2E_MEMBER_PRIVATE_KEY="${member_private_key}"
export PROJECT_DOCUMENT_E2E_RELAY_PRIVATE_KEY="${relay_private_key}"
export REDIS_URL=redis://localhost:6379

if [[ -n "${PROJECT_DOCUMENT_TEST_ARCHIVE:-}" ]]; then
  cargo nextest run \
    --archive-file "${PROJECT_DOCUMENT_TEST_ARCHIVE}" \
    -E 'binary(e2e_project_document_disabled)' \
    --run-ignored ignored-only \
    --test-threads 1
elif command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run \
    -p buzz-test-client \
    --test e2e_project_document_disabled \
    --run-ignored ignored-only \
    --test-threads 1
else
  cargo test -p buzz-test-client --test e2e_project_document_disabled -- \
    --ignored \
    --nocapture \
    --test-threads=1
fi
