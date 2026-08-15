#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PGVECTOR_IMAGE="pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0"
OWNER_LABEL="io.carryforth.coordinate-search-qualification.owner"
OWNER_TOKEN="$$-${RANDOM}-${SECONDS}"
CONTAINER=""
DATABASE="buzz_coordinate_search_qualification"
DATABASE_USER="carryforth_coordinate_search"
DATABASE_PASSWORD="carryforth_coordinate_search"
DEFAULT_OUTPUT_ROOT="${REPO_ROOT}/test-results/coordinate-search-exact-qualification"
OUTPUT_DIR="${1:-${DEFAULT_OUTPUT_ROOT}/$(date -u +%Y%m%dT%H%M%SZ)-$$}"

fail() {
  printf 'Coordinate-search qualification failed: %s\n' "$*" >&2
  exit 1
}

effective_docker_endpoint() {
  if [[ -n "${DOCKER_CONTEXT:-}" ]]; then
    docker context inspect "$DOCKER_CONTEXT" \
      --format '{{(index .Endpoints "docker").Host}}'
    return
  fi
  if [[ -n "${DOCKER_HOST:-}" ]]; then
    printf '%s\n' "$DOCKER_HOST"
    return
  fi
  local context
  context="$(docker context show)" || fail "cannot resolve active Docker context"
  docker context inspect "$context" --format '{{(index .Endpoints "docker").Host}}'
}

cleanup() {
  local container="${CONTAINER:-}"
  if [[ -z "$container" ]]; then
    return 0
  fi
  local owner
  owner="$(
    docker inspect \
      --format '{{index .Config.Labels "io.carryforth.coordinate-search-qualification.owner"}}' \
      "$container" 2>/dev/null || true
  )"
  if [[ "$owner" != "$OWNER_TOKEN" ]]; then
    printf 'Coordinate-search qualification: refusing to remove unowned container %s\n' \
      "$container" >&2
    return 1
  fi
  docker rm -f -v "$container" >/dev/null
  CONTAINER=""
}

cleanup_on_exit() {
  local status=$?
  trap - EXIT
  if ! cleanup && ((status == 0)); then
    status=1
  fi
  exit "$status"
}
trap cleanup_on_exit EXIT

DOCKER_ENDPOINT="$(effective_docker_endpoint)"
if [[ "$DOCKER_ENDPOINT" != unix://* ]]; then
  fail "refusing non-local Docker endpoint: ${DOCKER_ENDPOINT}"
fi
unset DOCKER_CONTEXT
export DOCKER_HOST="$DOCKER_ENDPOINT"

if [[ -e "$OUTPUT_DIR" ]] && \
  [[ -n "$(find "$OUTPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]]; then
  fail "output directory already exists and is not empty: ${OUTPUT_DIR}"
fi
mkdir -p "$OUTPUT_DIR"

CONTAINER="$(
  docker run -d --rm \
    --label "${OWNER_LABEL}=${OWNER_TOKEN}" \
    -e POSTGRES_USER="$DATABASE_USER" \
    -e POSTGRES_PASSWORD="$DATABASE_PASSWORD" \
    -e POSTGRES_DB="$DATABASE" \
    -p 127.0.0.1::5432 \
    "$PGVECTOR_IMAGE"
)"

owner="$(
  docker inspect \
    --format '{{index .Config.Labels "io.carryforth.coordinate-search-qualification.owner"}}' \
    "$CONTAINER"
)"
container_image="$(docker inspect --format '{{.Image}}' "$CONTAINER")"
expected_image="$(docker image inspect --format '{{.Id}}' "$PGVECTOR_IMAGE")"
if [[ "$owner" != "$OWNER_TOKEN" || "$container_image" != "$expected_image" ]]; then
  fail "qualification container ownership or image contract failed"
fi

for _attempt in $(seq 1 30); do
  if docker exec "$CONTAINER" pg_isready \
    -U "$DATABASE_USER" -d "$DATABASE" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$CONTAINER" pg_isready \
  -U "$DATABASE_USER" -d "$DATABASE" >/dev/null
docker exec "$CONTAINER" psql -X -v ON_ERROR_STOP=1 \
  -U "$DATABASE_USER" -d postgres \
  -c "ALTER DATABASE ${DATABASE} SET buzz.disposable_test = 'coordinate-search-qualification-v1'" \
  >/dev/null
docker exec "$CONTAINER" psql -X -v ON_ERROR_STOP=1 \
  -U "$DATABASE_USER" -d "$DATABASE" -c "CREATE EXTENSION vector" >/dev/null

PORT="$(
  docker inspect --format '{{(index (index .NetworkSettings.Ports "5432/tcp") 0).HostPort}}' \
    "$CONTAINER"
)"
DATABASE_URL="postgres://${DATABASE_USER}:${DATABASE_PASSWORD}@127.0.0.1:${PORT}/${DATABASE}"
RAW_OUTPUT="${OUTPUT_DIR}/cargo-test.log"

cd "$REPO_ROOT"
. ./bin/activate-hermit
set +e
BUZZ_TEST_SEMANTIC_DISPOSABLE="coordinate-search-qualification-v1" \
BUZZ_TEST_SEMANTIC_DATABASE_URL="$DATABASE_URL" \
  cargo test -p buzz-db --lib \
    coordinate_search_target_scale_exact_sql_qualification \
    -- --ignored --nocapture 2>&1 | tee "$RAW_OUTPUT"
test_status="${PIPESTATUS[0]}"
set -e
if [[ "$test_status" != "0" ]]; then
  fail "target-scale Rust qualification test failed"
fi

summary_line="$(rg '^coordinate_search_qualification=' "$RAW_OUTPUT" | tail -n 1)"
if [[ -z "$summary_line" ]]; then
  fail "qualification summary was not emitted"
fi
summary_json="${summary_line#coordinate_search_qualification=}"
if ! jq -e '
  .status == "measurement_complete_slo_not_frozen"
  and (.postgres_version_num | startswith("17"))
' <<<"$summary_json" >/dev/null; then
  fail "qualification summary failed its structural gate"
fi
jq -S . <<<"$summary_json" >"${OUTPUT_DIR}/qualification.json"
sha256sum "${OUTPUT_DIR}/qualification.json" >"${OUTPUT_DIR}/qualification.sha256"
printf 'Coordinate-search exact qualification complete: %s\n' "$OUTPUT_DIR"
