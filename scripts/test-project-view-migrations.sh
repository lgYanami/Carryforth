#!/usr/bin/env bash
# Exercise migrations against a disposable database, then ask pgschema whether
# migration-built Project View objects differ from schema/schema.sql.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

docker compose up -d postgres >/dev/null
for _ in $(seq 1 60); do
  status="$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)"
  [[ "${status}" == "healthy" ]] && break
  sleep 2
done
if [[ "${status:-}" != "healthy" ]]; then
  docker logs buzz-postgres || true
  echo "Project View migration tests: Postgres did not become healthy" >&2
  exit 1
fi

database_name="buzz_pv_migrations_$$_${RANDOM}"
if [[ ! "${database_name}" =~ ^buzz_pv_migrations_[0-9_]+$ ]]; then
  echo "Refusing unsafe scratch database name: ${database_name}" >&2
  exit 1
fi
plan_file="$(mktemp)"

cleanup() {
  rm -f "${plan_file}"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null
}
trap cleanup EXIT

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
export BUZZ_TEST_DATABASE_URL="${database_url}"
export DATABASE_URL="${database_url}"

if [[ -n "${PROJECT_VIEW_TEST_ARCHIVE:-}" ]]; then
  cargo nextest run \
    --archive-file "${PROJECT_VIEW_TEST_ARCHIVE}" \
    -E 'package(buzz-db) and test(migration::tests)' \
    --run-ignored ignored-only \
    --test-threads 1
elif command -v cargo-nextest >/dev/null 2>&1; then
  cargo nextest run \
    -p buzz-db \
    --lib \
    -E 'test(migration::tests)' \
    --run-ignored ignored-only \
    --test-threads 1
else
  cargo test -p buzz-db --lib migration::tests -- \
    --ignored \
    --nocapture \
    --test-threads=1
fi

# Every ignored migration test leaves the disposable base database at the
# latest migration. Verify the ledger and an old, pre-Project-View query before
# comparing only the Project View portion of desired state. Other legacy
# trigger differences are tracked independently and must not weaken this gate.
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 -qtA \
  -c "SELECT CASE WHEN count(*) = 1 THEN 'ok' ELSE 'bad' END FROM _sqlx_migrations WHERE version = 25 AND success" \
  | grep -qx ok
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "SELECT id, host FROM communities LIMIT 0" >/dev/null

PGHOST=localhost \
PGPORT=5432 \
PGUSER=buzz \
PGPASSWORD=buzz_dev \
PGDATABASE="${database_name}" \
PGSCHEMA_PLAN_HOST=localhost \
PGSCHEMA_PLAN_PORT=5432 \
PGSCHEMA_PLAN_DB=postgres \
PGSCHEMA_PLAN_USER=buzz \
PGSCHEMA_PLAN_PASSWORD=buzz_dev \
  ./bin/pgschema plan \
    --file schema/schema.sql \
    --output-json "${plan_file}" \
    --no-color >/dev/null

project_view_drift="$(
  jq '[
    .groups[].steps[]?
    | select(
        ((.path // "") | test("project_view"; "i"))
        or ((.sql // "") | test("project_view"; "i"))
      )
  ]' "${plan_file}"
)"
if [[ "$(jq 'length' <<<"${project_view_drift}")" != "0" ]]; then
  echo "Project View migration/schema drift detected:" >&2
  jq . <<<"${project_view_drift}" >&2
  exit 1
fi

echo "Project View migration and schema-drift gates passed."
