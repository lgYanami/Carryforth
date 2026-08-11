#!/usr/bin/env bash
# Explicit legacy migration/recovery canary. This entry point never restores
# the ordinary v1/v2 CLI or Relay runtime: it invokes one exact ignored
# buzz-db operator test that constructs signed canonical v2 state in a UUID
# scratch database, performs the complete v3 cutover, and drops that database.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

# shellcheck source=/dev/null
. ./bin/activate-hermit

umask 077
export CARGO_INCREMENTAL=0

fail() {
  echo "Project View legacy migration canary: $*" >&2
  exit 1
}

for command in cargo docker jq; do
  command -v "${command}" >/dev/null || fail "missing required command: ${command}"
done

docker compose up -d postgres >/dev/null
postgres_status=""
for _ in $(seq 1 60); do
  postgres_status="$(
    docker inspect --format='{{.State.Health.Status}}' buzz-postgres 2>/dev/null || true
  )"
  [[ "${postgres_status}" == "healthy" ]] && break
  sleep 2
done
if [[ "${postgres_status}" != "healthy" ]]; then
  docker logs buzz-postgres || true
  fail "buzz-postgres did not become healthy"
fi

# The test creates and drops its own UUID-suffixed child database. Giving it a
# dedicated, validated parent database prevents a typo or ambient
# TEST_DATABASE_URL from ever targeting a developer's Carryforth database.
database_name="${PROJECT_VIEW_LEGACY_MIGRATION_DATABASE_NAME:-buzz_pv_legacy_migration_canary_$$_${RANDOM}}"
if [[ ! "${database_name}" =~ ^buzz_pv_legacy_migration_canary_[0-9_]+$ ]]; then
  fail "refusing unsafe scratch database name: ${database_name}"
fi

artifact_root="${PROJECT_VIEW_LEGACY_MIGRATION_ARTIFACT_ROOT:-${REPO_ROOT}/test-results/project-view-legacy-migration}"
run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
artifact_dir="${artifact_root}/${run_id}"
mkdir -p "${artifact_dir}"
artifact_dir="$(cd "${artifact_dir}" && pwd)"

database_created=0
cleanup() {
  if [[ "${database_created}" == "1" && "${PROJECT_VIEW_LEGACY_MIGRATION_KEEP_DB:-0}" != "1" ]]; then
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
  fi
}
trap cleanup EXIT

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
  -c "CREATE DATABASE ${database_name}" >/dev/null
database_created=1

test_name="project_view::tests::legacy_v2_to_v3_operator_cutover_preserves_full_continuity_history"
test_database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"

echo "Running the isolated operator migration canary: ${test_name}"
set +e
TEST_DATABASE_URL="${test_database_url}" \
  cargo test -p buzz-db "${test_name}" -- \
    --ignored --exact --nocapture --test-threads=1 \
  2>&1 | tee "${artifact_dir}/cargo-test.log"
test_status=${PIPESTATUS[0]}
set -e
if [[ "${test_status}" != "0" ]]; then
  fail "operator migration canary failed (see ${artifact_dir}/cargo-test.log)"
fi

jq -n \
  --arg accepted_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --arg test_name "${test_name}" \
  --arg scratch_parent "${database_name}" '{
    accepted_at: $accepted_at,
    execution: "ignored_scratch_db_operator_test",
    test: $test_name,
    scratch_parent_database: $scratch_parent,
    ordinary_legacy_runtime_used: false,
    assertions: {
      canonical_v2_fixture: "signed and structurally ready",
      cutover: "maintenance begin/readiness/freeze/cutover/verify/resume",
      target_schema_version: 3,
      continuity_history: "identity-preserved and reprojected as strict v3",
      advertised_runtime: "strict v3 ready"
    }
  }' >"${artifact_dir}/acceptance-summary.json"

echo "Project View legacy v2-to-v3 operator migration canary passed."
echo "Evidence: ${artifact_dir}"
