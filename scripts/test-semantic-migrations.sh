#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PGVECTOR_IMAGE="pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0"
TEST_CONTAINER="buzz-semantic-migrations-$$"
TEST_USER="buzz_semantic_test"
TEST_PASSWORD="buzz_semantic_test"
TEST_DATABASE="buzz_semantic_test"
FRESH_DATABASE="buzz_semantic_fresh"
PLAN_FILE="$(mktemp)"

cleanup() {
  rm -f "$PLAN_FILE"
  docker rm -f "$TEST_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d \
  --name "$TEST_CONTAINER" \
  -e POSTGRES_USER="$TEST_USER" \
  -e POSTGRES_PASSWORD="$TEST_PASSWORD" \
  -e POSTGRES_DB="$TEST_DATABASE" \
  -p 127.0.0.1::5432 \
  "$PGVECTOR_IMAGE" >/dev/null

for _attempt in $(seq 1 30); do
  if docker exec "$TEST_CONTAINER" pg_isready \
    -U "$TEST_USER" -d "$TEST_DATABASE" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$TEST_CONTAINER" pg_isready \
  -U "$TEST_USER" -d "$TEST_DATABASE" >/dev/null
docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$TEST_DATABASE" \
  -c "CREATE EXTENSION vector" >/dev/null
docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d postgres \
  -c "CREATE EXTENSION vector" >/dev/null

TEST_PORT="$(docker inspect \
  --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
  "$TEST_CONTAINER")"

cd "$REPO_ROOT"
. ./bin/activate-hermit
BUZZ_TEST_SEMANTIC_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db semantic_pipeline_activates_only_a_complete_fenced_set -- --nocapture

PGHOST=127.0.0.1 \
PGPORT="$TEST_PORT" \
PGUSER="$TEST_USER" \
PGPASSWORD="$TEST_PASSWORD" \
PGDATABASE="$TEST_DATABASE" \
PGSCHEMA_PLAN_HOST=127.0.0.1 \
PGSCHEMA_PLAN_PORT="$TEST_PORT" \
PGSCHEMA_PLAN_DB=postgres \
PGSCHEMA_PLAN_USER="$TEST_USER" \
PGSCHEMA_PLAN_PASSWORD="$TEST_PASSWORD" \
  ./bin/pgschema plan \
    --file schema/schema.sql \
    --output-json "$PLAN_FILE" \
    --no-color >/dev/null

semantic_drift="$(
  jq '[
    .groups[].steps[]?
    | select(
        ((.path // "") | test("semantic"; "i"))
        or ((.sql // "") | test("semantic"; "i"))
      )
  ]' "$PLAN_FILE"
)"
if [[ "$(jq 'length' <<<"$semantic_drift")" != "0" ]]; then
  echo "Semantic migration/desired-schema drift detected:" >&2
  jq . <<<"$semantic_drift" >&2
  exit 1
fi

docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d postgres \
  -c "CREATE DATABASE ${FRESH_DATABASE}" >/dev/null
docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$FRESH_DATABASE" \
  -c "CREATE EXTENSION vector" >/dev/null

PGHOST=127.0.0.1 \
PGPORT="$TEST_PORT" \
PGUSER="$TEST_USER" \
PGPASSWORD="$TEST_PASSWORD" \
PGDATABASE="$FRESH_DATABASE" \
PGSCHEMA_PLAN_HOST=127.0.0.1 \
PGSCHEMA_PLAN_PORT="$TEST_PORT" \
PGSCHEMA_PLAN_DB=postgres \
PGSCHEMA_PLAN_USER="$TEST_USER" \
PGSCHEMA_PLAN_PASSWORD="$TEST_PASSWORD" \
  ./bin/pgschema apply --file schema/schema.sql --auto-approve >/dev/null

docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$FRESH_DATABASE" -qtA \
  -c "SELECT CASE WHEN
        to_regclass('semantic_index_generations') IS NOT NULL
        AND to_regclass('semantic_rebuild_operations') IS NOT NULL
        AND to_regclass('semantic_provider_rate_gates') IS NOT NULL
        AND to_regtype('vector') IS NOT NULL
      THEN 'ok' ELSE 'bad' END" | grep -qx ok

echo "Semantic migration, desired-schema, and ledger-less fresh-schema gates passed."
