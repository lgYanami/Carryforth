#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PGVECTOR_IMAGE="pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0"
PROBE_CONTAINER="buzz-semantic-pgvector-probe-$$"
PROBE_USER="buzz_semantic_probe"
PROBE_PASSWORD="buzz_semantic_probe"
PROBE_DATABASE="buzz_semantic_probe"

cleanup() {
  docker rm -f "$PROBE_CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d \
  --name "$PROBE_CONTAINER" \
  -e POSTGRES_USER="$PROBE_USER" \
  -e POSTGRES_PASSWORD="$PROBE_PASSWORD" \
  -e POSTGRES_DB="$PROBE_DATABASE" \
  -p 127.0.0.1::5432 \
  "$PGVECTOR_IMAGE" >/dev/null

for _attempt in $(seq 1 30); do
  if docker exec "$PROBE_CONTAINER" pg_isready \
    -U "$PROBE_USER" -d "$PROBE_DATABASE" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$PROBE_CONTAINER" pg_isready \
  -U "$PROBE_USER" -d "$PROBE_DATABASE" >/dev/null

docker exec "$PROBE_CONTAINER" psql -v ON_ERROR_STOP=1 \
  -U "$PROBE_USER" -d "$PROBE_DATABASE" \
  -c "CREATE EXTENSION IF NOT EXISTS vector" \
  -c "CREATE TABLE semantic_vector_probe (id BIGINT PRIMARY KEY, embedding vector(2048) NOT NULL)" \
  -c "INSERT INTO semantic_vector_probe VALUES
      (1, array_fill(0.25::real, ARRAY[2048])::vector),
      (2, array_fill(0.50::real, ARRAY[2048])::vector)" \
  -c "CREATE INDEX semantic_vector_probe_halfvec_hnsw
      ON semantic_vector_probe USING hnsw
      ((embedding::halfvec(2048)) halfvec_cosine_ops)" \
  -c "SELECT vector_dims(embedding), vector_dims(embedding::halfvec)
      FROM semantic_vector_probe ORDER BY id" >/dev/null

PROBE_PORT="$(docker inspect \
  --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
  "$PROBE_CONTAINER")"

cd "$REPO_ROOT"
. ./bin/activate-hermit
DATABASE_URL="postgres://${PROBE_USER}:${PROBE_PASSWORD}@127.0.0.1:${PROBE_PORT}/${PROBE_DATABASE}" \
  cargo run -q -p buzz-admin -- semantic preflight
