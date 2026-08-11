#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PGVECTOR_IMAGE="pgvector/pgvector@sha256:d2ef61f42ef767baa5a1475393303cc235bcd92febd9d7014eddb48b41f3bad0"
QUALIFICATION_CONTAINER=""
QUALIFICATION_OWNER_LABEL="io.carryforth.semantic-exact-qualification.owner"
QUALIFICATION_OWNER_TOKEN="$$-${RANDOM}-${SECONDS}"
QUALIFICATION_USER="carryforth_semantic_exact"
QUALIFICATION_PASSWORD="carryforth_semantic_exact"
QUALIFICATION_DATABASE="carryforth_semantic_exact"

# The budget dimensions are frozen by buzz-semantic-query. Source counts are
# an explicit repeatable local qualification scale, not a production capacity
# claim. Operators may raise the source counts for their target Community.
MEDIUM_SOURCES="${SEMANTIC_QUALIFICATION_MEDIUM_SOURCES:-2000}"
TARGET_SOURCES="${SEMANTIC_QUALIFICATION_TARGET_SOURCES:-10000}"
DISTRACTOR_SOURCES="${SEMANTIC_QUALIFICATION_DISTRACTOR_SOURCES:-5000}"
REPRESENTATIVE_CHANNELS=4
HARD_CAP_CHANNELS=9
DEFAULT_RECALL=64
HARD_CAP_RECALL=256
MEDIUM_ITERATIONS="${SEMANTIC_QUALIFICATION_MEDIUM_ITERATIONS:-20}"
TARGET_ITERATIONS="${SEMANTIC_QUALIFICATION_TARGET_ITERATIONS:-15}"
HARD_CAP_ITERATIONS="${SEMANTIC_QUALIFICATION_HARD_CAP_ITERATIONS:-10}"
SOAK_SECONDS="${SEMANTIC_QUALIFICATION_SOAK_SECONDS:-8}"
SOAK_CLIENTS="${SEMANTIC_QUALIFICATION_SOAK_CLIENTS:-4}"
SOAK_JOBS="${SEMANTIC_QUALIFICATION_SOAK_JOBS:-4}"

DEFAULT_OUTPUT_ROOT="${REPO_ROOT}/test-results/semantic-exact-query-qualification"
OUTPUT_DIR="${1:-${DEFAULT_OUTPUT_ROOT}/$(date -u +%Y%m%dT%H%M%SZ)-$$}"

fail() {
  printf 'semantic exact-query qualification failed: %s\n' "$*" >&2
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
  context="$(docker context show)" || fail "cannot resolve the active Docker context"
  docker context inspect "$context" \
    --format '{{(index .Endpoints "docker").Host}}'
}

require_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    fail "${name} must be a positive integer"
  fi
}

for pair in \
  "MEDIUM_SOURCES:${MEDIUM_SOURCES}" \
  "TARGET_SOURCES:${TARGET_SOURCES}" \
  "DISTRACTOR_SOURCES:${DISTRACTOR_SOURCES}" \
  "MEDIUM_ITERATIONS:${MEDIUM_ITERATIONS}" \
  "TARGET_ITERATIONS:${TARGET_ITERATIONS}" \
  "HARD_CAP_ITERATIONS:${HARD_CAP_ITERATIONS}" \
  "SOAK_SECONDS:${SOAK_SECONDS}" \
  "SOAK_CLIENTS:${SOAK_CLIENTS}" \
  "SOAK_JOBS:${SOAK_JOBS}"; do
  require_positive_integer "${pair%%:*}" "${pair#*:}"
done

if ((MEDIUM_SOURCES > TARGET_SOURCES)); then
  fail "MEDIUM_SOURCES cannot exceed TARGET_SOURCES"
fi
if ((MEDIUM_SOURCES < 5)) || ((DISTRACTOR_SOURCES < 5)); then
  fail "MEDIUM_SOURCES and DISTRACTOR_SOURCES must each be at least 5"
fi
if ((SOAK_JOBS > SOAK_CLIENTS)); then
  fail "SOAK_JOBS cannot exceed SOAK_CLIENTS"
fi

DOCKER_ENDPOINT="$(effective_docker_endpoint)"
if [[ "$DOCKER_ENDPOINT" != unix://* ]]; then
  fail "refusing non-local Docker endpoint: ${DOCKER_ENDPOINT}"
fi
# Pin every subsequent Docker operation, including EXIT cleanup, to the exact
# local endpoint that passed the check. This prevents an active-context change
# in another shell from redirecting part of the run to another daemon.
unset DOCKER_CONTEXT
export DOCKER_HOST="$DOCKER_ENDPOINT"

# Refuse silent drift from the frozen query budget used to name the profiles.
rg -Fqx 'pub const MAX_CONTEXT_COORDINATES: usize = 8;' \
  "${REPO_ROOT}/crates/buzz-semantic-query/src/contract.rs" ||
  fail "MAX_CONTEXT_COORDINATES drifted from the hard-cap profile"
rg -Fqx 'pub const MAX_QUERY_CHANNELS: usize = 1 + MAX_CONTEXT_COORDINATES;' \
  "${REPO_ROOT}/crates/buzz-semantic-query/src/contract.rs" ||
  fail "MAX_QUERY_CHANNELS drifted from the hard-cap profile"
rg -Fqx 'pub const MAX_RECALL_PER_CHANNEL: u16 = 256;' \
  "${REPO_ROOT}/crates/buzz-semantic-query/src/contract.rs" ||
  fail "MAX_RECALL_PER_CHANNEL drifted from the hard-cap profile"
rg -Fqx 'pub const MAX_WALL_TIME_MS: u32 = 30_000;' \
  "${REPO_ROOT}/crates/buzz-semantic-query/src/contract.rs" ||
  fail "MAX_WALL_TIME_MS drifted from the hard-cap profile"
rg -Fqx '    max_recall_per_channel: 64,' \
  "${REPO_ROOT}/crates/buzz-semantic-query/src/contract.rs" ||
  fail "default recall budget drifted from the qualification profile"
rg -Fqx "SET LOCAL statement_timeout = '30s';" \
  "${REPO_ROOT}/scripts/semantic-exact-query-qualification-pgbench.sql" ||
  fail "pgbench statement timeout drifted from MAX_WALL_TIME_MS"

if [[ -e "$OUTPUT_DIR" ]] && [[ -n "$(find "$OUTPUT_DIR" -mindepth 1 -maxdepth 1 -print -quit 2>/dev/null)" ]]; then
  fail "output directory already exists and is not empty: ${OUTPUT_DIR}"
fi
mkdir -p "$OUTPUT_DIR"

cleanup() {
  local container="${QUALIFICATION_CONTAINER:-}"
  if [[ -z "$container" ]]; then
    return 0
  fi
  local owner
  owner="$(
    docker inspect \
      --format '{{index .Config.Labels "io.carryforth.semantic-exact-qualification.owner"}}' \
      "$container" 2>/dev/null || true
  )"
  if [[ "$owner" != "$QUALIFICATION_OWNER_TOKEN" ]]; then
    printf 'semantic exact-query qualification: refusing to remove unowned container %s\n' \
      "$container" >&2
    return 1
  fi
  if ! docker rm -f -v "$container" >/dev/null 2>&1; then
    printf 'semantic exact-query qualification: failed to remove owned container %s\n' \
      "$container" >&2
    return 1
  fi
  QUALIFICATION_CONTAINER=""
}

cleanup_on_exit() {
  local original_status=$?
  trap - EXIT
  if ! cleanup && ((original_status == 0)); then
    original_status=1
  fi
  exit "$original_status"
}
trap cleanup_on_exit EXIT

QUALIFICATION_CONTAINER="$(
  docker run -d \
    --rm \
    --network none \
    --label "${QUALIFICATION_OWNER_LABEL}=${QUALIFICATION_OWNER_TOKEN}" \
    -e POSTGRES_USER="$QUALIFICATION_USER" \
    -e POSTGRES_PASSWORD="$QUALIFICATION_PASSWORD" \
    -e POSTGRES_DB="$QUALIFICATION_DATABASE" \
    "$PGVECTOR_IMAGE"
)"

container_owner="$(
  docker inspect \
    --format '{{index .Config.Labels "io.carryforth.semantic-exact-qualification.owner"}}' \
    "$QUALIFICATION_CONTAINER"
)"
container_network="$(
  docker inspect --format '{{.HostConfig.NetworkMode}}' "$QUALIFICATION_CONTAINER"
)"
container_binds="$(
  docker inspect --format '{{json .HostConfig.Binds}}' "$QUALIFICATION_CONTAINER"
)"
container_image="$(docker inspect --format '{{.Image}}' "$QUALIFICATION_CONTAINER")"
expected_image="$(docker image inspect --format '{{.Id}}' "$PGVECTOR_IMAGE")"
if [[ "$container_owner" != "$QUALIFICATION_OWNER_TOKEN" \
  || "$container_network" != "none" \
  || "$container_binds" != "null" \
  || "$container_image" != "$expected_image" ]]; then
  fail "qualification container ownership or isolation contract failed"
fi

for _attempt in $(seq 1 30); do
  if docker exec "$QUALIFICATION_CONTAINER" pg_isready \
    -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
docker exec "$QUALIFICATION_CONTAINER" pg_isready \
  -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" >/dev/null

docker cp \
  "${REPO_ROOT}/scripts/semantic-exact-query-qualification-setup.sql" \
  "${QUALIFICATION_CONTAINER}:/tmp/semantic-exact-query-qualification-setup.sql"
docker cp \
  "${REPO_ROOT}/scripts/semantic-exact-query-qualification-explain.sql" \
  "${QUALIFICATION_CONTAINER}:/tmp/semantic-exact-query-qualification-explain.sql"
docker cp \
  "${REPO_ROOT}/scripts/semantic-exact-query-qualification-pgbench.sql" \
  "${QUALIFICATION_CONTAINER}:/tmp/semantic-exact-query-qualification-pgbench.sql"

psql_in_container() {
  docker exec "$QUALIFICATION_CONTAINER" psql \
    -X -v ON_ERROR_STOP=1 -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" "$@"
}

setup_result="$({
  psql_in_container -qAt \
    -v expected_database="$QUALIFICATION_DATABASE" \
    -v expected_user="$QUALIFICATION_USER" \
    -v target_sources="$TARGET_SOURCES" \
    -v distractor_sources="$DISTRACTOR_SOURCES" \
    -f /tmp/semantic-exact-query-qualification-setup.sql
} | tail -n 1)"
if ! jq -e \
  --argjson target "$TARGET_SOURCES" \
  --argjson distractors "$DISTRACTOR_SOURCES" \
  '.eligible_sources == $target and
   .distractor_sources == $distractors and
   .vector_dimensions == 2048' <<<"$setup_result" >/dev/null; then
  fail "synthetic source fixture did not satisfy its closed counts"
fi

environment_result="$(psql_in_container -qAt -c "
  SELECT json_build_object(
    'postgres_version', current_setting('server_version'),
    'postgres_version_num', current_setting('server_version_num')::integer,
    'pgvector_version', (SELECT extversion FROM pg_extension WHERE extname='vector'),
    'work_mem', current_setting('work_mem'),
    'shared_buffers', current_setting('shared_buffers'),
    'effective_cache_size', current_setting('effective_cache_size'),
    'max_connections', current_setting('max_connections')::integer
  )
")"

summarize_explain() {
  local profile="$1"
  local requested_scale="$2"
  local requested_channels="$3"
  local recall_per_channel="$4"
  local work_mem="$5"
  local explain_path="${OUTPUT_DIR}/explain-${profile}.json"
  local summary_path="${OUTPUT_DIR}/explain-${profile}-summary.json"
  local expected_distance_rows=$((requested_scale * requested_channels))
  local window_result

  window_result="$(psql_in_container -qAt -c "
    WITH scale_window AS (
      SELECT *
      FROM semantic_exact_qualification.sources
      WHERE scale_ordinal <= ${requested_scale}
    )
    SELECT json_build_object(
      'pre_gate_rows', count(*),
      'eligible_rows', count(*) FILTER (WHERE
        community_ordinal=1 AND generation_ordinal=1
        AND current_head AND authorized AND eligible
      ),
      'rejected_rows', count(*) FILTER (WHERE NOT (
        community_ordinal=1 AND generation_ordinal=1
        AND current_head AND authorized AND eligible
      )),
      'community_rejected', count(*) FILTER (WHERE community_ordinal<>1),
      'generation_rejected', count(*) FILTER (WHERE generation_ordinal<>1),
      'current_head_rejected', count(*) FILTER (WHERE NOT current_head),
      'authorization_rejected', count(*) FILTER (WHERE NOT authorized),
      'eligibility_rejected', count(*) FILTER (WHERE NOT eligible),
      'exclusive_rejection_partition', bool_and(
        ((community_ordinal<>1)::int + (generation_ordinal<>1)::int
         + (NOT current_head)::int + (NOT authorized)::int
         + (NOT eligible)::int) = 1
      ) FILTER (WHERE NOT (
        community_ordinal=1 AND generation_ordinal=1
        AND current_head AND authorized AND eligible
      ))
    )
    FROM scale_window
  ")"
  docker exec \
    -e PGOPTIONS="-c work_mem=${work_mem}" \
    "$QUALIFICATION_CONTAINER" psql \
      -X -qAt -v ON_ERROR_STOP=1 \
      -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" \
      -v requested_scale="$requested_scale" \
      -v requested_channels="$requested_channels" \
      -v recall_per_channel="$recall_per_channel" \
      -f /tmp/semantic-exact-query-qualification-explain.sql >"$explain_path"

  jq \
    --arg profile "$profile" \
    --arg work_mem "$work_mem" \
    --argjson requested_scale "$requested_scale" \
    --argjson requested_channels "$requested_channels" \
    --argjson recall_per_channel "$recall_per_channel" \
    --argjson expected_distance_rows "$expected_distance_rows" \
    --argjson gate_window "$window_result" '
      .[0] as $explain |
      ([
        $explain.Plan | .. | objects |
        select(."Subplan Name"? == "CTE pre_gate") |
        ."Actual Rows"
      ] | max // 0) as $pre_gate_rows |
      ([
        $explain.Plan | .. | objects |
        select(."Subplan Name"? == "CTE eligible") |
        ."Actual Rows"
      ] | max // 0) as $eligible_rows |
      ([
        $explain.Plan | .. | objects |
        select(."Subplan Name"? == "CTE rejected_by_gate") |
        ."Actual Rows"
      ] | max // 0) as $rejected_rows |
      ([
        $explain.Plan | .. | objects |
        select(."Subplan Name"? == "CTE finite_distances") |
        ."Actual Rows"
      ] | max // 0) as $distance_rows |
      {
        profile: $profile,
        requested_sources: $requested_scale,
        requested_channels: $requested_channels,
        recall_per_channel: $recall_per_channel,
        expected_distance_rows: $expected_distance_rows,
        pre_gate_actual_rows: $pre_gate_rows,
        eligible_actual_rows: $eligible_rows,
        rejected_by_gate_actual_rows: $rejected_rows,
        distance_actual_rows: $distance_rows,
        planning_ms: $explain."Planning Time",
        execution_ms: $explain."Execution Time",
        shared_hit_blocks: ($explain.Plan."Shared Hit Blocks" // 0),
        shared_read_blocks: ($explain.Plan."Shared Read Blocks" // 0),
        temp_read_blocks: ($explain.Plan."Temp Read Blocks" // 0),
        temp_written_blocks: ($explain.Plan."Temp Written Blocks" // 0),
        wal_records: ($explain.Plan."WAL Records" // 0),
        settings: $explain.Settings,
        work_mem: $work_mem,
        cosine_operator_visible: (($explain.Plan | tostring) | contains("<=>")),
        gate_window: $gate_window,
        predicate_gate_before_distance:
          ($gate_window.exclusive_rejection_partition and
           $gate_window.eligible_rows == $requested_scale and
           $gate_window.rejected_rows > 0 and
           ($gate_window.community_rejected > 0) and
           ($gate_window.generation_rejected > 0) and
           ($gate_window.current_head_rejected > 0) and
           ($gate_window.authorization_rejected > 0) and
           ($gate_window.eligibility_rejected > 0) and
           $pre_gate_rows == $gate_window.pre_gate_rows and
           $eligible_rows == $gate_window.eligible_rows and
           $rejected_rows == $gate_window.rejected_rows and
           $distance_rows == $expected_distance_rows)
      }
    ' "$explain_path" >"$summary_path"

  jq -e \
    '.cosine_operator_visible and .predicate_gate_before_distance' \
    "$summary_path" >/dev/null ||
    fail "${profile} EXPLAIN did not prove the eligible-before-distance invariant"
}

summarize_explain \
  medium-default "$MEDIUM_SOURCES" "$REPRESENTATIVE_CHANNELS" "$DEFAULT_RECALL" 4MB
summarize_explain \
  target-default "$TARGET_SOURCES" "$REPRESENTATIVE_CHANNELS" "$DEFAULT_RECALL" 4MB
summarize_explain \
  target-hard-cap "$TARGET_SOURCES" "$HARD_CAP_CHANNELS" "$HARD_CAP_RECALL" 4MB
summarize_explain \
  target-hard-cap-forced-spill "$TARGET_SOURCES" "$HARD_CAP_CHANNELS" "$HARD_CAP_RECALL" 64kB

if ! jq -e \
  '(.temp_read_blocks > 0) and (.temp_written_blocks > 0)' \
  "${OUTPUT_DIR}/explain-target-hard-cap-forced-spill-summary.json" >/dev/null; then
  fail "forced low-work_mem EXPLAIN did not exercise temp spill accounting"
fi

latency_summary_from_prefix() {
  local prefix="$1"
  docker exec "$QUALIFICATION_CONTAINER" sh -c \
    "awk '!/^#/ {print \$3 / 1000.0}' /tmp/${prefix}.*" |
    jq -Rsc '
      split("\n") |
      map(select(length > 0) | tonumber) |
      sort as $values |
      ($values | length) as $count |
      def nearest_rank($fraction):
        $values[(((($count * $fraction) | ceil) - 1) | if . < 0 then 0 else . end)];
      if $count == 0 then
        error("pgbench latency log contained no samples")
      else
        {
          samples: $count,
          min_ms: $values[0],
          p50_ms: nearest_rank(0.50),
          p95_ms: nearest_rank(0.95),
          p99_ms: nearest_rank(0.99),
          max_ms: $values[-1],
          mean_ms: (($values | add) / $count)
        }
      end
    '
}

run_latency_profile() {
  local profile="$1"
  local requested_scale="$2"
  local requested_channels="$3"
  local recall_per_channel="$4"
  local iterations="$5"
  local prefix="semantic-${profile}"

  docker exec "$QUALIFICATION_CONTAINER" sh -c \
    "rm -f /tmp/${prefix}.*"

  docker exec \
    -e PGAPPNAME="semantic-exact-${profile}-warmup" \
    "$QUALIFICATION_CONTAINER" pgbench \
      -n -M extended -c 1 -j 1 -t 2 \
      -D requested_scale="$requested_scale" \
      -D requested_channels="$requested_channels" \
      -D recall_per_channel="$recall_per_channel" \
      -f /tmp/semantic-exact-query-qualification-pgbench.sql \
      -U "$QUALIFICATION_USER" "$QUALIFICATION_DATABASE" >/dev/null

  docker exec \
    -e PGAPPNAME="semantic-exact-${profile}" \
    "$QUALIFICATION_CONTAINER" pgbench \
      -n -M extended -c 1 -j 1 -t "$iterations" \
      --exit-on-abort --max-tries=1 --failures-detailed \
      -l --log-prefix="/tmp/${prefix}" \
      -D requested_scale="$requested_scale" \
      -D requested_channels="$requested_channels" \
      -D recall_per_channel="$recall_per_channel" \
      -f /tmp/semantic-exact-query-qualification-pgbench.sql \
      -U "$QUALIFICATION_USER" "$QUALIFICATION_DATABASE" \
      >"${OUTPUT_DIR}/pgbench-${profile}.txt"

  latency_summary_from_prefix "$prefix" |
    jq \
      --arg profile "$profile" \
      --argjson requested_scale "$requested_scale" \
      --argjson requested_channels "$requested_channels" \
      --argjson recall_per_channel "$recall_per_channel" \
      '. + {
        profile: $profile,
        requested_sources: $requested_scale,
        requested_channels: $requested_channels,
        recall_per_channel: $recall_per_channel
      }' >"${OUTPUT_DIR}/latency-${profile}.json"
}

run_latency_profile \
  medium-default "$MEDIUM_SOURCES" "$REPRESENTATIVE_CHANNELS" "$DEFAULT_RECALL" "$MEDIUM_ITERATIONS"
run_latency_profile \
  target-default "$TARGET_SOURCES" "$REPRESENTATIVE_CHANNELS" "$DEFAULT_RECALL" "$TARGET_ITERATIONS"
run_latency_profile \
  target-hard-cap "$TARGET_SOURCES" "$HARD_CAP_CHANNELS" "$HARD_CAP_RECALL" "$HARD_CAP_ITERATIONS"

cancel_output="${OUTPUT_DIR}/statement-cancellation.txt"
set +e
docker exec "$QUALIFICATION_CONTAINER" psql \
  -X -qAt -v ON_ERROR_STOP=1 \
  -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" \
  -c "SET statement_timeout='1ms'; SELECT semantic_exact_qualification.exact_count(${TARGET_SOURCES}, ${HARD_CAP_CHANNELS}, ${HARD_CAP_RECALL});" \
  >"$cancel_output" 2>&1
cancel_status=$?
set -e
if ((cancel_status == 0)) || ! rg -q 'canceling statement due to statement timeout' "$cancel_output"; then
  fail "hard-cap exact query did not produce the expected statement_timeout cancellation"
fi
lingering_after_cancel="$(psql_in_container -qAt -c "
  SELECT count(*)
  FROM pg_stat_activity
  WHERE datname = current_database()
    AND pid <> pg_backend_pid()
    AND (state = 'idle in transaction' OR query LIKE '%exact_count%')
")"
if [[ "$lingering_after_cancel" != "0" ]]; then
  fail "statement cancellation left a semantic exact query or idle transaction"
fi

run_soak() {
  local profile="$1"
  local with_vacuum="$2"
  local prefix="semantic-${profile}"
  local pgbench_output="${OUTPUT_DIR}/pgbench-${profile}.txt"
  local resource_samples="${OUTPUT_DIR}/resources-${profile}.txt"
  local max_transaction_age_ms=0
  local vacuum_duration_ms=0
  local vacuum_status="not_run"

  docker exec "$QUALIFICATION_CONTAINER" sh -c \
    "rm -f /tmp/${prefix}.*"

  docker exec \
    -e PGAPPNAME="semantic-exact-${profile}" \
    "$QUALIFICATION_CONTAINER" pgbench \
      -n -M extended \
      -c "$SOAK_CLIENTS" -j "$SOAK_JOBS" -T "$SOAK_SECONDS" \
      --exit-on-abort --max-tries=1 --failures-detailed \
      -l --log-prefix="/tmp/${prefix}" \
      -D requested_scale="$TARGET_SOURCES" \
      -D requested_channels="$REPRESENTATIVE_CHANNELS" \
      -D recall_per_channel="$DEFAULT_RECALL" \
      -f /tmp/semantic-exact-query-qualification-pgbench.sql \
      -U "$QUALIFICATION_USER" "$QUALIFICATION_DATABASE" \
      >"$pgbench_output" 2>&1 &
  local soak_pid=$!

  sleep 1
  local vacuum_pid=""
  local vacuum_started_ns=0
  local vacuum_finished_path="${OUTPUT_DIR}/vacuum-${profile}-finished-ns.txt"
  if [[ "$with_vacuum" == "true" ]]; then
    vacuum_status="running"
    vacuum_started_ns="$(date +%s%N)"
    (
      docker exec "$QUALIFICATION_CONTAINER" psql \
        -X -qAt -v ON_ERROR_STOP=1 \
        -U "$QUALIFICATION_USER" -d "$QUALIFICATION_DATABASE" \
        -c 'VACUUM (ANALYZE) semantic_exact_qualification.sources' \
        >"${OUTPUT_DIR}/vacuum-${profile}.txt" 2>&1
      date +%s%N >"$vacuum_finished_path"
    ) &
    vacuum_pid=$!
  fi

  while kill -0 "$soak_pid" >/dev/null 2>&1; do
    local observed_age_ms
    observed_age_ms="$(psql_in_container -qAt -c "
      SELECT coalesce(max(
        extract(epoch FROM (clock_timestamp() - xact_start)) * 1000
      ), 0)::bigint
      FROM pg_stat_activity
      WHERE application_name = 'semantic-exact-${profile}'
        AND xact_start IS NOT NULL
    ")"
    if ((observed_age_ms > max_transaction_age_ms)); then
      max_transaction_age_ms="$observed_age_ms"
    fi
    docker stats --no-stream \
      --format '{{.CPUPerc}} {{.MemUsage}}' "$QUALIFICATION_CONTAINER" \
      >>"$resource_samples"
    sleep 0.2
  done
  wait "$soak_pid"

  if [[ -n "$vacuum_pid" ]]; then
    wait "$vacuum_pid"
    vacuum_status="completed"
    local vacuum_finished_ns
    vacuum_finished_ns="$(<"$vacuum_finished_path")"
    vacuum_duration_ms=$(((vacuum_finished_ns - vacuum_started_ns) / 1000000))
  fi

  local latency_json
  latency_json="$(latency_summary_from_prefix "$prefix")"
  local failed_transactions
  local failed_line_count
  failed_line_count="$(
    awk '/^number of failed transactions:/ {count++} END {print count + 0}' \
      "$pgbench_output"
  )"
  if [[ "$failed_line_count" != "1" ]]; then
    fail "${profile} pgbench output must contain exactly one failed-transaction line"
  fi
  failed_transactions="$(
    awk '/^number of failed transactions:/ {print $5}' "$pgbench_output"
  )"
  if [[ ! "$failed_transactions" =~ ^[0-9]+$ ]]; then
    fail "${profile} pgbench failed-transaction count is not an integer"
  fi
  local processed_transactions
  local processed_line_count
  processed_line_count="$(
    awk '/^number of transactions actually processed:/ {count++} END {print count + 0}' \
      "$pgbench_output"
  )"
  if [[ "$processed_line_count" != "1" ]]; then
    fail "${profile} pgbench output must contain exactly one processed-transaction line"
  fi
  processed_transactions="$(
    awk '/^number of transactions actually processed:/ {print $6}' "$pgbench_output"
  )"
  if [[ ! "$processed_transactions" =~ ^[0-9]+$ ]]; then
    fail "${profile} pgbench processed-transaction count is not an integer"
  fi
  local latency_samples
  latency_samples="$(jq -r '.samples' <<<"$latency_json")"
  if [[ ! "$latency_samples" =~ ^[1-9][0-9]*$ \
    || "$latency_samples" != "$processed_transactions" ]]; then
    fail "${profile} latency samples do not match processed transactions"
  fi
  local cpu_peak_percent
  cpu_peak_percent="$(
    awk '{gsub(/%/, "", $1); if ($1 + 0 > max) max=$1 + 0} END {print max + 0}' \
      "$resource_samples"
  )"

  jq -n \
    --arg profile "$profile" \
    --argjson duration_seconds "$SOAK_SECONDS" \
    --argjson clients "$SOAK_CLIENTS" \
    --argjson jobs "$SOAK_JOBS" \
    --argjson failed_transactions "$failed_transactions" \
    --argjson max_transaction_age_ms "$max_transaction_age_ms" \
    --arg vacuum_status "$vacuum_status" \
    --argjson vacuum_duration_ms "$vacuum_duration_ms" \
    --argjson cpu_peak_percent "$cpu_peak_percent" \
    --argjson latency "$latency_json" '
      {
        profile: $profile,
        duration_seconds: $duration_seconds,
        clients: $clients,
        jobs: $jobs,
        failed_transactions: $failed_transactions,
        max_observed_transaction_age_ms: $max_transaction_age_ms,
        vacuum_status: $vacuum_status,
        vacuum_duration_ms: $vacuum_duration_ms,
        sampled_container_cpu_peak_percent: $cpu_peak_percent,
        latency: $latency,
        measured_tps: ($latency.samples / $duration_seconds)
      }
    ' >"${OUTPUT_DIR}/soak-${profile}.json"

  if [[ "$failed_transactions" != "0" ]]; then
    fail "${profile} reported failed pgbench transactions"
  fi
}

run_soak target-default-baseline false
run_soak target-default-with-vacuum true

post_soak_activity="$(psql_in_container -qAt -c "
  SELECT json_build_object(
    'idle_in_transaction', count(*) FILTER (WHERE state='idle in transaction'),
    'semantic_queries', count(*) FILTER (WHERE query LIKE '%exact_count%')
  )
  FROM pg_stat_activity
  WHERE datname=current_database() AND pid <> pg_backend_pid()
")"
if ! jq -e \
  '.idle_in_transaction == 0 and .semantic_queries == 0' \
  <<<"$post_soak_activity" >/dev/null; then
  fail "short soak left an idle transaction or semantic exact query"
fi

table_observation="$(psql_in_container -qAt -c "
  SELECT json_build_object(
    'relation_bytes', pg_total_relation_size('semantic_exact_qualification.sources'),
    'live_tuples', n_live_tup,
    'dead_tuples', n_dead_tup,
    'vacuum_count', vacuum_count,
    'analyze_count', analyze_count
  )
  FROM pg_stat_user_tables
  WHERE schemaname='semantic_exact_qualification' AND relname='sources'
")"

jq -s '.' "${OUTPUT_DIR}"/explain-*-summary.json >"${OUTPUT_DIR}/explain-summaries.json"
jq -s '.' "${OUTPUT_DIR}"/latency-*.json >"${OUTPUT_DIR}/latency-summaries.json"
jq -s '.' "${OUTPUT_DIR}"/soak-*.json >"${OUTPUT_DIR}/soak-summaries.json"

jq -n \
  --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg image "$PGVECTOR_IMAGE" \
  --argjson environment "$environment_result" \
  --argjson setup "$setup_result" \
  --argjson medium_sources "$MEDIUM_SOURCES" \
  --argjson target_sources "$TARGET_SOURCES" \
  --argjson distractor_sources "$DISTRACTOR_SOURCES" \
  --argjson representative_channels "$REPRESENTATIVE_CHANNELS" \
  --argjson hard_cap_channels "$HARD_CAP_CHANNELS" \
  --argjson default_recall "$DEFAULT_RECALL" \
  --argjson hard_cap_recall "$HARD_CAP_RECALL" \
  --argjson cancellation_status "$cancel_status" \
  --argjson lingering_after_cancel "$lingering_after_cancel" \
  --argjson post_soak_activity "$post_soak_activity" \
  --argjson table_observation "$table_observation" \
  --slurpfile explains "${OUTPUT_DIR}/explain-summaries.json" \
  --slurpfile latencies "${OUTPUT_DIR}/latency-summaries.json" \
  --slurpfile soaks "${OUTPUT_DIR}/soak-summaries.json" '
    {
      status: "measurement_complete_slo_not_frozen",
      generated_at: $generated_at,
      content_free: true,
      isolated_ephemeral_database: true,
      image: $image,
      environment: $environment,
      scale: {
        medium_sources: $medium_sources,
        target_sources: $target_sources,
        distractor_sources: $distractor_sources,
        representative_channels: $representative_channels,
        hard_cap_channels: $hard_cap_channels,
        default_recall_per_channel: $default_recall,
        hard_cap_recall_per_channel: $hard_cap_recall
      },
      setup: $setup,
      explains: $explains[0],
      latencies: $latencies[0],
      soaks: $soaks[0],
      statement_cancellation: {
        expected_nonzero_exit_status: $cancellation_status,
        lingering_sessions: $lingering_after_cancel,
        passed: ($cancellation_status != 0 and $lingering_after_cancel == 0)
      },
      post_soak_activity: $post_soak_activity,
      table_observation: $table_observation,
      qualification_boundary: {
        records_one_local_synthetic_kernel_measurement: true,
        closes_target_community_slo: false,
        closes_full_canonical_sql_plan: false,
        closes_multi_pod_or_provider_soak: false,
        reason: "No numeric SLO is frozen and the fixture does not reproduce target canonical graph joins or deployment topology."
      }
    }
  ' >"${OUTPUT_DIR}/qualification.json"

jq -e '
  .content_free and
  .isolated_ephemeral_database and
  .statement_cancellation.passed and
  ([.explains[] | .predicate_gate_before_distance] | all) and
  ([.soaks[] | .failed_transactions == 0] | all)
' "${OUTPUT_DIR}/qualification.json" >/dev/null ||
  fail "final qualification structural checks did not pass"

if ! cleanup; then
  trap - EXIT
  fail "qualification measurements passed but owned-container cleanup failed"
fi
trap - EXIT
printf '%s\n' "${OUTPUT_DIR}/qualification.json"
