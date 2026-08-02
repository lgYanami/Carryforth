#!/usr/bin/env bash
# Single-machine Project Document Stage 7 capacity acceptance.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

export CARGO_INCREMENTAL=0

total_revisions="${STAGE7_REVISION_COUNT:-100000}"
if [[ ! "${total_revisions}" =~ ^[0-9]+$ ]] || (( total_revisions < 100000 )); then
  echo "Stage 7 capacity requires STAGE7_REVISION_COUNT >= 100000" >&2
  exit 1
fi
hot_revisions="$((total_revisions / 2))"
wide_documents="$((total_revisions - hot_revisions))"
pilot_hot=500
pilot_wide=500
run_id="$(date -u +%Y%m%dT%H%M%SZ)_$$_${RANDOM}"
pilot_database="buzz_pd_stage7_pilot_$$_${RANDOM}"
capacity_database="buzz_pd_stage7_capacity_$$_${RANDOM}"
for database_name in "${pilot_database}" "${capacity_database}"; do
  if [[ ! "${database_name}" =~ ^buzz_pd_stage7_(pilot|capacity)_[0-9_]+$ ]]; then
    echo "Refusing unsafe scratch database name: ${database_name}" >&2
    exit 1
  fi
done

report_dir="${STAGE7_REPORT_DIR:-${REPO_ROOT}/test-results/stage7-capacity/${run_id}}"
mkdir -p "${report_dir}"
probe_json="${report_dir}/history-probe.json"
report_json="${report_dir}/capacity-report.json"
report_md="${report_dir}/capacity-report.md"
lock_log="$(mktemp)"
temporary_files=("${lock_log}")

cleanup() {
  for database_name in "${pilot_database}" "${capacity_database}"; do
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
  done
  rm -f "${temporary_files[@]}"
  find "${REPO_ROOT}/target" "${REPO_ROOT}/desktop/src-tauri/target" \
    -type d -name incremental -prune -exec rm -rf -- {} + 2>/dev/null || true
}
trap cleanup EXIT

docker compose up -d postgres >/dev/null
postgres_status=""
for _ in $(seq 1 60); do
  postgres_status="$(docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true)"
  [[ "${postgres_status}" == "healthy" ]] && break
  sleep 1
done
if [[ "${postgres_status}" != "healthy" ]]; then
  echo "Stage 7 capacity: PostgreSQL did not become healthy" >&2
  exit 1
fi

relay_pubkey="79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
reader_pubkey="c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5"
fixture_host="stage7-capacity.local"

create_database() {
  local database_name="$1"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "CREATE DATABASE ${database_name}" >/dev/null
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
    ./bin/pgschema apply --file schema/schema.sql --auto-approve >/dev/null
  docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
    <scripts/attach-schema-partitions.sql
}

seed_fixture() {
  local database_name="$1"
  local hot_count="$2"
  local wide_count="$3"
  docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
      -v fixture_host="${fixture_host}" \
      -v relay_pubkey="${relay_pubkey}" \
      -v reader_pubkey="${reader_pubkey}" \
      -v hot_revisions="${hot_count}" \
      -v wide_documents="${wide_count}" \
      <scripts/project-document-capacity-fixture.sql >/dev/null
}

database_bytes() {
  local database_name="$1"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc \
    "SELECT pg_database_size(current_database())"
}

# Pilot the exact fixture shape before allocating the mandatory data set.
create_database "${pilot_database}"
pilot_base_bytes="$(database_bytes "${pilot_database}")"
seed_fixture "${pilot_database}" "${pilot_hot}" "${pilot_wide}"
pilot_final_bytes="$(database_bytes "${pilot_database}")"
pilot_delta_bytes="$((pilot_final_bytes - pilot_base_bytes))"
if (( pilot_delta_bytes <= 0 )); then
  echo "Stage 7 capacity pilot did not produce measurable growth" >&2
  exit 1
fi
projected_fixture_bytes="$((pilot_delta_bytes * total_revisions / (pilot_hot + pilot_wide)))"
projected_with_margin_bytes="$((projected_fixture_bytes * 3 / 2))"
free_bytes="$(docker exec buzz-postgres sh -c \
  "df -PB1 /var/lib/postgresql/data | awk 'NR==2 {print \$4}'")"
minimum_free_after_bytes="${STAGE7_MIN_FREE_AFTER_BYTES:-2147483648}"
if (( projected_with_margin_bytes + minimum_free_after_bytes > free_bytes )); then
  echo "Stage 7 capacity disk fuse opened: projected=${projected_with_margin_bytes}, free=${free_bytes}, reserve=${minimum_free_after_bytes}" >&2
  exit 1
fi
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "DROP DATABASE ${pilot_database} WITH (FORCE)" >/dev/null

create_database "${capacity_database}"
capacity_base_bytes="$(database_bytes "${capacity_database}")"
seed_started_ns="$(date +%s%N)"
seed_fixture "${capacity_database}" "${hot_revisions}" "${wide_documents}"
seed_elapsed_ms="$((( $(date +%s%N) - seed_started_ns ) / 1000000))"
capacity_final_bytes="$(database_bytes "${capacity_database}")"
capacity_delta_bytes="$((capacity_final_bytes - capacity_base_bytes))"

if [[ "${STAGE7_CAPACITY_NO_BUILD:-0}" != "1" ]]; then
  cargo build -p buzz-admin
fi
if [[ ! -x target/debug/buzz-admin ]]; then
  echo "Stage 7 capacity: target/debug/buzz-admin is missing" >&2
  exit 1
fi

env \
  DATABASE_URL="postgres://buzz:buzz_dev@localhost:5432/${capacity_database}" \
  target/debug/buzz-admin project-document capacity-probe \
    --community "${fixture_host}" \
    --expected-pubkey "${relay_pubkey}" \
    --reader-pubkey "${reader_pubkey}" \
    --document-id "00000000-0000-4000-8000-00000000c001" \
    --max-revision "${hot_revisions}" \
    --pages "$(((hot_revisions + 49) / 50))" \
    --timeout-ms 2000 >"${probe_json}"

# Deliberately hold the real Community advisory writer lock for 250 ms and
# measure a shared-lock waiter. This records the current coarse-lock cost; it
# is not a production contention simulation.
lock_key="buzz_project_view:00000000-0000-4000-8000-00000000c000"
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${capacity_database}" -v ON_ERROR_STOP=1 -Atc \
  "BEGIN;
   SELECT pg_advisory_xact_lock(hashtextextended('${lock_key}', 0));
   SELECT pg_sleep(0.25);
   COMMIT;" >"${lock_log}" 2>&1 &
lock_holder_pid=$!
for _ in $(seq 1 100); do
  lock_seen="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${capacity_database}" -Atc \
    "SELECT count(*) FROM pg_locks
     WHERE locktype = 'advisory' AND granted" 2>/dev/null || true)"
  [[ "${lock_seen}" =~ ^[1-9][0-9]*$ ]] && break
  sleep 0.01
done
lock_wait_started_ns="$(date +%s%N)"
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${capacity_database}" -v ON_ERROR_STOP=1 -Atc \
  "BEGIN;
   SELECT pg_advisory_xact_lock_shared(hashtextextended('${lock_key}', 0));
   COMMIT;" >/dev/null
lock_wait_ms="$((( $(date +%s%N) - lock_wait_started_ns ) / 1000000))"
wait "${lock_holder_pid}"

storage_json="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${capacity_database}" -Atc \
  "SELECT json_build_object(
      'database_bytes', pg_database_size(current_database()),
      'project_document_revisions_table_bytes', pg_relation_size('project_document_revisions'),
      'project_document_revisions_index_bytes', pg_indexes_size('project_document_revisions'),
      'project_documents_table_bytes', pg_relation_size('project_documents'),
      'project_documents_index_bytes', pg_indexes_size('project_documents'),
      'project_document_changes_table_bytes', pg_relation_size('project_document_changes'),
      'project_document_changes_index_bytes', pg_indexes_size('project_document_changes'),
      'events_total_bytes', (SELECT sum(pg_total_relation_size(relid))
        FROM pg_partition_tree('events') WHERE relid <> 'events'::regclass),
      'revision_rows', (SELECT count(*) FROM project_document_revisions),
      'document_rows', (SELECT count(*) FROM project_documents),
      'body_bytes', (SELECT sum(octet_length(content_markdown)) FROM project_document_revisions),
      'body_min_bytes', (SELECT min(octet_length(content_markdown)) FROM project_document_revisions),
      'body_max_bytes', (SELECT max(octet_length(content_markdown)) FROM project_document_revisions)
    )")"

jq -n \
  --arg run_id "${run_id}" \
  --argjson requested_revisions "${total_revisions}" \
  --argjson hot_revisions "${hot_revisions}" \
  --argjson wide_documents "${wide_documents}" \
  --argjson pilot_rows "$((pilot_hot + pilot_wide))" \
  --argjson pilot_delta_bytes "${pilot_delta_bytes}" \
  --argjson projected_fixture_bytes "${projected_fixture_bytes}" \
  --argjson projected_with_margin_bytes "${projected_with_margin_bytes}" \
  --argjson free_bytes_at_preflight "${free_bytes}" \
  --argjson minimum_free_after_bytes "${minimum_free_after_bytes}" \
  --argjson seed_elapsed_ms "${seed_elapsed_ms}" \
  --argjson capacity_base_bytes "${capacity_base_bytes}" \
  --argjson capacity_final_bytes "${capacity_final_bytes}" \
  --argjson capacity_delta_bytes "${capacity_delta_bytes}" \
  --argjson lock_wait_ms "${lock_wait_ms}" \
  --argjson storage "${storage_json}" \
  --slurpfile probe "${probe_json}" \
  '{
    schema_version: 1,
    run_id: $run_id,
    mode: "single_machine_prerelease",
    requested_revisions: $requested_revisions,
    hot_revisions: $hot_revisions,
    wide_documents: $wide_documents,
    body_profile_bytes: "256..1024",
    cryptographic_fixture: false,
    cryptographic_fixture_reason: "set-based capacity only; real signer rotation/parity is a separate Stage 7 recovery gate",
    disk_preflight: {
      pilot_rows: $pilot_rows,
      pilot_delta_bytes: $pilot_delta_bytes,
      projected_fixture_bytes: $projected_fixture_bytes,
      projected_with_50_percent_margin_bytes: $projected_with_margin_bytes,
      free_bytes: $free_bytes_at_preflight,
      reserved_free_bytes: $minimum_free_after_bytes,
      passed: true
    },
    seed: {
      elapsed_ms: $seed_elapsed_ms,
      base_database_bytes: $capacity_base_bytes,
      final_database_bytes: $capacity_final_bytes,
      delta_bytes: $capacity_delta_bytes
    },
    storage: $storage,
    history_probe: $probe[0],
    lock_measurement: {
      forced_exclusive_hold_ms: 250,
      measured_shared_wait_ms: $lock_wait_ms,
      per_document_lock_implemented: false,
      decision: "keep Community shared lock; bounded page latency passes and no organic deployment contention exists"
    },
    million_revision_extended_soak: ($requested_revisions >= 1000000),
    passed: ($storage.revision_rows >= 100000
      and $probe[0].timeout_passed
      and $probe[0].bounded_memory
      and $probe[0].uses_expected_index
      and ($probe[0].revision_seq_scan | not))
  }' >"${report_json}"

jq -e '.passed == true' "${report_json}" >/dev/null
{
  echo "# Project Document Stage 7 capacity report"
  echo
  echo "- Run: \`${run_id}\`"
  echo "- Revisions: ${total_revisions} (${hot_revisions} hot + ${wide_documents} wide)"
  echo "- Database growth: ${capacity_delta_bytes} bytes"
  echo "- Seed time: ${seed_elapsed_ms} ms"
  echo "- Max history page: $(jq -r '.history_probe.max_page_ms' "${report_json}") ms (limit 50)"
  echo "- RSS peak growth: $(jq -r '.history_probe.rss_peak_growth_kib' "${report_json}") KiB"
  echo "- Expected index used: $(jq -r '.history_probe.uses_expected_index' "${report_json}")"
  echo "- Forced shared-lock wait: ${lock_wait_ms} ms"
  echo "- Result: PASS"
} >"${report_md}"

echo "${report_dir}"
