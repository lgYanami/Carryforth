#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PGVECTOR_IMAGE="pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0"
TEST_CONTAINER="buzz-semantic-migrations-$$"
TEST_USER="buzz_semantic_test"
TEST_PASSWORD="buzz_semantic_test"
TEST_DATABASE="buzz_semantic_disposable"
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
  -U "$TEST_USER" -d postgres \
  -c "ALTER DATABASE ${TEST_DATABASE} SET buzz.disposable_test = 'fleet-policy-v1'" >/dev/null
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
BUZZ_TEST_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db semantic_query_upgrade_is_additive_and_index_disable_is_atomic \
    -- --ignored --nocapture
BUZZ_TEST_SEMANTIC_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db semantic_pipeline_activates_only_a_complete_fenced_set -- --nocapture
BUZZ_TEST_SEMANTIC_DISPOSABLE="fleet-policy-v1" \
BUZZ_TEST_SEMANTIC_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db coordinate_search_real_pgvector_is_coordinate_only_deduplicated_and_stable \
    -- --nocapture
BUZZ_TEST_SEMANTIC_DISPOSABLE="fleet-policy-v1" \
BUZZ_TEST_SEMANTIC_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db one_hop_scoped_search_real_pgvector_is_direct_complete_and_hydrated \
    -- --nocapture
BUZZ_TEST_SEMANTIC_DISPOSABLE="fleet-policy-v1" \
BUZZ_TEST_SEMANTIC_DATABASE_URL="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${TEST_DATABASE}" \
  cargo test -p buzz-db semantic_fleet::tests:: -- --ignored --nocapture

# Upgraded databases intentionally retain the zero-vector constraint as NOT
# VALID until historical rows have been repaired. Verify the exact live
# expression directly before excluding this one validation-state-only path
# from desired-schema drift below.
docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$TEST_DATABASE" -qtA \
  -c "SELECT CASE WHEN NOT convalidated
          AND position('vector_norm(embedding)' IN pg_get_constraintdef(oid)) > 0
          AND position('l2_norm' IN pg_get_constraintdef(oid)) = 0
        THEN 'ok' ELSE 'bad' END
      FROM pg_constraint
      WHERE conrelid='semantic_embeddings'::regclass
        AND conname='semantic_embeddings_nonzero_cosine'" | grep -qx ok

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
        ((.path // "") | test("semantic|coordinate[_ -]?search"; "i"))
        or ((.sql // "") | test("semantic|coordinate[_ -]?search"; "i"))
      )
    | select(
        (.path // "") !=
          "public.semantic_embeddings.semantic_embeddings_nonzero_cosine"
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

# Exercise the supported source-start semantic lifecycle on a genuinely empty
# Community. No Provider request occurs because the fresh canonical catalogs
# contain no eligible sources. Repeating both phases must reuse one generation.
docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$FRESH_DATABASE" \
  -c "INSERT INTO communities(host) VALUES ('localhost:3000')" >/dev/null
fresh_database_url="postgres://${TEST_USER}:${TEST_PASSWORD}@127.0.0.1:${TEST_PORT}/${FRESH_DATABASE}"
local_bootstrap_environment=(
  DATABASE_URL="${fresh_database_url}"
  RELAY_URL=ws://localhost:3000
  BUZZ_BIND_ADDR=127.0.0.1:3000
  BUZZ_SEMANTIC_WORKER_ENABLED=true
  BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true
  CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=true
  CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=true
  BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay
  BUZZ_SEMANTIC_API_KEY=synthetic-local-bootstrap-key
  BUZZ_SEMANTIC_BASE_URL=https://provider.invalid/v1/
  BUZZ_SEMANTIC_REQUEST_MODEL=synthetic-local-bootstrap-model
  BUZZ_RELAY_PRIVATE_KEY=0000000000000000000000000000000000000000000000000000000000000001
)
for _attempt in 1 2; do
  env "${local_bootstrap_environment[@]}" \
    cargo run -p buzz-admin -- semantic local-bootstrap \
      --phase prepare --acknowledge-local-provider-egress >/dev/null
  env "${local_bootstrap_environment[@]}" \
    cargo run -p buzz-admin -- semantic local-bootstrap \
      --phase finalize --acknowledge-local-provider-egress --wait-seconds 5 >/dev/null
done

docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$FRESH_DATABASE" -qtA \
  -c "SELECT CASE WHEN
        (SELECT count(*) FROM semantic_index_generations) = 1
        AND (SELECT count(*) FROM semantic_index_generations
             WHERE lifecycle='active' AND rebuild_completed_at IS NOT NULL) = 1
        AND (SELECT count(*) FROM communities
             WHERE host='localhost:3000'
               AND semantic_index_enabled
               AND semantic_graph_query_enabled
               AND NOT project_view_enabled
               AND NOT project_context_edge_enabled
               AND signing_key IS NULL
               AND semantic_active_generation_id IS NOT NULL) = 1
      THEN 'ok' ELSE 'bad' END" | grep -qx ok

docker exec "$TEST_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$TEST_USER" -d "$FRESH_DATABASE" -qtA \
  -c "SELECT CASE WHEN
        to_regclass('semantic_index_generations') IS NOT NULL
        AND to_regclass('semantic_rebuild_operations') IS NOT NULL
        AND to_regclass('semantic_provider_rate_gates') IS NOT NULL
        AND to_regclass('semantic_query_provider_admission') IS NOT NULL
        AND to_regclass('semantic_graph_http_fleet_attestations') IS NOT NULL
        AND EXISTS (
          SELECT 1 FROM pg_attribute
          WHERE attrelid = 'communities'::regclass
            AND attname = 'semantic_graph_query_enabled'
            AND NOT attisdropped
        )
        AND EXISTS (
          SELECT 1 FROM pg_constraint
          WHERE conrelid = 'events'::regclass
            AND conname = 'events_kind_not_semantic_graph_query_result'
            AND convalidated
        )
        AND EXISTS (
          SELECT 1 FROM pg_constraint
          WHERE conrelid = 'events'::regclass
            AND conname = 'events_kind_not_project_context_coordinate_search_result'
            AND convalidated
        )
        AND EXISTS (
          SELECT 1 FROM pg_constraint
          WHERE conrelid = 'events'::regclass
            AND conname = 'events_kind_not_project_context_one_hop_semantic_search_result'
            AND convalidated
        )
        AND EXISTS (
          SELECT 1 FROM pg_constraint
          WHERE conrelid = 'semantic_embeddings'::regclass
            AND conname = 'semantic_embeddings_nonzero_cosine'
            AND convalidated
        )
        AND to_regtype('vector') IS NOT NULL
      THEN 'ok' ELSE 'bad' END" | grep -qx ok

echo "Semantic migration, desired-schema, and ledger-less fresh-schema gates passed."
