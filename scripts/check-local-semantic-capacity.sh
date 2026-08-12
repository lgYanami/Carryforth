#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
compose_file="$repo_root/docker-compose.yml"
minimum_memory_bytes=$((2 * 1024 * 1024 * 1024))
expected_pgvector_version="0.8.5"

fail() {
  printf 'semantic local capacity check failed: %s\n' "$*" >&2
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
  context=$(docker context show) || fail "cannot resolve the active Docker context"
  docker context inspect "$context" \
    --format '{{(index .Endpoints "docker").Host}}'
}

read_numeric_env() {
  local name=$1
  local fallback=$2
  local value=""
  if [[ -f "$repo_root/.env" ]]; then
    value=$(sed -n -E "s/^${name}=([0-9]+)[[:space:]]*$/\\1/p" "$repo_root/.env" | tail -n 1)
  fi
  if [[ -z "$value" ]]; then
    value=$fallback
  fi
  [[ "$value" =~ ^[1-9][0-9]*$ ]] || fail "$name must be a positive integer"
  printf '%s\n' "$value"
}

read_boolean_env() {
  local name=$1
  local fallback=$2
  local value=""
  if [[ -f "$repo_root/.env" ]]; then
    value=$(sed -n -E "s/^${name}=(true|false)[[:space:]]*$/\\1/p" "$repo_root/.env" | tail -n 1)
  fi
  printf '%s\n' "${value:-$fallback}"
}

docker_endpoint=$(effective_docker_endpoint)
[[ "$docker_endpoint" == unix://* ]] || fail "refusing non-local Docker endpoint"
unset DOCKER_CONTEXT
export DOCKER_HOST="$docker_endpoint"

command -v jq >/dev/null 2>&1 || fail "jq is required"

compose_json=$(docker compose -f "$compose_file" config --format json)
compose_image=$(jq -er '.services.postgres.image' <<<"$compose_json")
compose_memory=$(jq -er '.services.postgres.deploy.resources.limits.memory | tonumber' <<<"$compose_json")
[[ "$compose_image" == pgvector/pgvector:0.8.5-pg17-bookworm@sha256:* ]] \
  || fail "root Compose must pin the approved PostgreSQL 17 / pgvector 0.8.5 image"
((compose_memory >= minimum_memory_bytes)) \
  || fail "root Compose PostgreSQL memory limit is below 2 GiB"

for required_setting in \
  max_connections=40 \
  shared_buffers=256MB \
  work_mem=4MB \
  maintenance_work_mem=128MB \
  effective_cache_size=1536MB \
  max_parallel_workers_per_gather=1; do
  jq -e --arg setting "$required_setting" \
    '.services.postgres.command | index($setting) != null' \
    <<<"$compose_json" >/dev/null \
    || fail "root Compose is missing PostgreSQL setting $required_setting"
done

container_id=$(docker compose -f "$compose_file" ps -q postgres)
[[ -n "$container_id" ]] || fail "the repository PostgreSQL container is not running"
[[ "$(wc -w <<<"$container_id")" == 1 ]] \
  || fail "expected exactly one repository PostgreSQL container"

inspect_json=$(docker inspect "$container_id")
working_dir=$(jq -r '.[0].Config.Labels["com.docker.compose.project.working_dir"] // ""' \
  <<<"$inspect_json")
service_label=$(jq -r '.[0].Config.Labels["com.buzz.service"] // ""' <<<"$inspect_json")
container_memory=$(jq -er '.[0].HostConfig.Memory' <<<"$inspect_json")
container_swap=$(jq -er '.[0].HostConfig.MemorySwap' <<<"$inspect_json")
[[ "$working_dir" == "$repo_root" ]] \
  || fail "PostgreSQL container is not owned by this checkout"
[[ "$service_label" == "postgres" ]] || fail "PostgreSQL ownership label is missing"
((container_memory >= minimum_memory_bytes)) \
  || fail "running PostgreSQL container has not applied the 2 GiB profile"
if ((container_swap > 0 && container_swap < container_memory)); then
  fail "running PostgreSQL swap ceiling is smaller than its memory ceiling"
fi

postgres_value() {
  local sql=$1
  docker exec "$container_id" sh -ceu \
    'exec psql -XAtq -v ON_ERROR_STOP=1 -U "$POSTGRES_USER" -d "$POSTGRES_DB" -c "$1"' \
    sh "$sql"
}

server_version_num=$(postgres_value "SELECT current_setting('server_version_num')")
((server_version_num >= 170000 && server_version_num < 180000)) \
  || fail "running database is not PostgreSQL 17"
pgvector_version=$(postgres_value "SELECT extversion FROM pg_extension WHERE extname = 'vector'")
[[ "$pgvector_version" == "$expected_pgvector_version" ]] \
  || fail "running vector extension is not 0.8.5"

declare -A expected_settings=(
  [max_connections]="40"
  [shared_buffers]="256MB"
  [work_mem]="4MB"
  [maintenance_work_mem]="128MB"
  [effective_cache_size]="1536MB"
  [max_parallel_workers_per_gather]="1"
)
for setting in "${!expected_settings[@]}"; do
  actual=$(postgres_value "SELECT current_setting('${setting}')")
  [[ "$actual" == "${expected_settings[$setting]}" ]] \
    || fail "running PostgreSQL setting $setting does not match the supported profile"
done

migration_count=$(postgres_value \
  "SELECT count(*) FROM _sqlx_migrations WHERE version IN (57, 58) AND success")
[[ "$migration_count" == "2" ]] || fail "semantic migrations 0057/0058 are not both successful"
schema_count=$(postgres_value \
  "SELECT count(*) FROM pg_class WHERE relnamespace = 'public'::regnamespace AND relname IN ('semantic_index_generations','semantic_graph_http_fleet_attestations')")
[[ "$schema_count" == "2" ]] || fail "semantic schema contract is incomplete"

main_max=$(read_numeric_env BUZZ_DB_MAIN_MAX_CONNECTIONS 12)
control_max=$(read_numeric_env BUZZ_DB_CONTROL_MAX_CONNECTIONS 2)
audit_max=$(read_numeric_env BUZZ_DB_AUDIT_MAX_CONNECTIONS 2)
search_max=$(read_numeric_env BUZZ_DB_SEARCH_MAX_CONNECTIONS 2)
read_max=$(read_numeric_env BUZZ_DB_READ_MAX_CONNECTIONS 8)
reserve=$(read_numeric_env BUZZ_DB_SERVER_CONNECTION_RESERVE 4)
ordinary_reserve=$(read_numeric_env BUZZ_DB_ORDINARY_MAIN_RESERVE 4)
traversal_max=$(read_numeric_env BUZZ_SEMANTIC_GRAPH_TRAVERSAL_MAX_IN_FLIGHT 2)
audit_enabled=$(read_boolean_env BUZZ_AUDIT_ENABLED true)

read_configured=false
if [[ -f "$repo_root/.env" ]] \
  && sed -n -E 's/^READ_DATABASE_URL=(.+)$/\1/p' "$repo_root/.env" \
    | rg -q '[^[:space:]]'; then
  read_configured=true
fi
writer_budget=$((main_max + control_max + search_max + reserve))
if [[ "$audit_enabled" == true ]]; then
  writer_budget=$((writer_budget + audit_max))
fi
# This read-only preflight uses the same conservative rule as Relay startup:
# an unverified read endpoint is charged to the writer instead of undercounted.
if [[ "$read_configured" == true ]]; then
  writer_budget=$((writer_budget + read_max))
fi
server_max=$(postgres_value "SELECT current_setting('max_connections')::integer")
((writer_budget <= server_max)) \
  || fail "configured Relay pool budget exceeds PostgreSQL max_connections"
((ordinary_reserve < main_max)) \
  || fail "ordinary main-pool reserve must be smaller than the main pool"
((traversal_max <= main_max - ordinary_reserve)) \
  || fail "semantic traversal admission exceeds its writer-pool share"

memory_events=$(docker exec "$container_id" sh -ceu \
  'test -r /sys/fs/cgroup/memory.events && tr "\n" " " </sys/fs/cgroup/memory.events')

jq -n \
  --arg status "ok" \
  --arg postgres_major "17" \
  --arg pgvector "$pgvector_version" \
  --arg memory_events "$memory_events" \
  --argjson memory_limit_bytes "$container_memory" \
  --argjson server_max_connections "$server_max" \
  --argjson relay_pool_budget "$writer_budget" \
  --argjson traversal_limit "$traversal_max" \
  '{status:$status,postgres_major:$postgres_major,pgvector:$pgvector,memory_limit_bytes:$memory_limit_bytes,server_max_connections:$server_max_connections,relay_pool_budget:$relay_pool_budget,semantic_traversal_limit:$traversal_limit,memory_events:$memory_events}'
