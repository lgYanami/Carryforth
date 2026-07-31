#!/usr/bin/env bash
# Exercise the Stage 2 Project Document contract against an isolated database:
# prove the disabled boundary, controlled bootstrap/enable, private WS/HTTP/
# Redis behavior, verified CLI CRUD/history, incident disable/re-enable, and
# final disable with canonical-state preservation.

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
# The scratch database provides tenant isolation; use the resolver-portable
# localhost host instead of depending on wildcard .localhost DNS behavior.
test_host="localhost:${port}"
relay_pid=""
relay_log="$(mktemp)"
temporary_files=("${relay_log}")

cleanup() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  rm -f "${temporary_files[@]}"
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
writer_private_key=0000000000000000000000000000000000000000000000000000000000000003
writer_pubkey=f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
outsider_private_key=0000000000000000000000000000000000000000000000000000000000000004

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host)
      VALUES ('00000000-0000-4000-8000-00000000d0c0', '${test_host}');
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES
        ('00000000-0000-4000-8000-00000000d0c0', '${member_pubkey}', 'member'),
        ('00000000-0000-4000-8000-00000000d0c0', '${writer_pubkey}', 'member');" >/dev/null

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
relay_url="ws://${test_host}"

start_relay() {
  # Document authorization always consults current relay_members / managed
  # owner state. The legacy startup allowlist backfill is intentionally off
  # because Project View v2 forbids that compatibility writer.
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    RELAY_URL="${relay_url}" \
    BUZZ_BIND_ADDR="0.0.0.0:${port}" \
    BUZZ_HEALTH_PORT="${health_port}" \
    BUZZ_METRICS_PORT="${metrics_port}" \
    BUZZ_AUTO_MIGRATE=false \
    BUZZ_REQUIRE_AUTH_TOKEN=false \
    BUZZ_REQUIRE_RELAY_MEMBERSHIP=false \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    RELAY_OWNER_PUBKEY="${relay_owner_pubkey}" \
    "${bin_dir}/buzz-relay" >"${relay_log}" 2>&1 &
  relay_pid=$!

  local status_code=""
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
  if [[ "${status_code}" != "200" ]]; then
    cat "${relay_log}" >&2
    echo "Project Document E2E: Relay did not become ready" >&2
    exit 1
  fi
}

stop_relay() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  relay_pid=""
}

run_e2e_binary() {
  local test_binary="$1"
  if [[ -n "${PROJECT_DOCUMENT_TEST_ARCHIVE:-}" ]]; then
    cargo nextest run \
      --archive-file "${PROJECT_DOCUMENT_TEST_ARCHIVE}" \
      -E "binary(${test_binary})" \
      --run-ignored ignored-only \
      --test-threads 1
  elif command -v cargo-nextest >/dev/null 2>&1; then
    cargo nextest run \
      -p buzz-test-client \
      --test "${test_binary}" \
      --run-ignored ignored-only \
      --test-threads 1
  else
    cargo test -p buzz-test-client --test "${test_binary}" -- \
      --ignored \
      --nocapture \
      --test-threads=1
  fi
}

buzz_cli() {
  env \
    BUZZ_RELAY_URL="http://${test_host}" \
    BUZZ_PRIVATE_KEY="${member_private_key}" \
    "${bin_dir}/buzz" "$@"
}

buzz_admin() {
  env \
    DATABASE_URL="${database_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    "${bin_dir}/buzz-admin" project-document "$@"
}

buzz_project_view_admin() {
  env \
    DATABASE_URL="${database_url}" \
    REDIS_URL=redis://localhost:6379 \
    BUZZ_RELAY_PRIVATE_KEY="${relay_private_key}" \
    "${bin_dir}/buzz-admin" project-view "$@"
}

export DATABASE_URL="${database_url}"
export PROJECT_DOCUMENT_E2E_RELAY_URL="${relay_url}"
export PROJECT_DOCUMENT_E2E_MEMBER_PRIVATE_KEY="${member_private_key}"
export PROJECT_DOCUMENT_E2E_WRITER_PRIVATE_KEY="${writer_private_key}"
export PROJECT_DOCUMENT_E2E_OUTSIDER_PRIVATE_KEY="${outsider_private_key}"
export PROJECT_DOCUMENT_E2E_RELAY_PRIVATE_KEY="${relay_private_key}"
export REDIS_URL=redis://localhost:6379

status_json="$(buzz_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].project_view_schema_version == 1
  and .[0].catalog_revision == null
  and .[0].revision_count == 0
' <<<"${status_json}" >/dev/null

# Disabled remains an independently tested security state.
start_relay
buzz_cli channels list >/dev/null
run_e2e_binary e2e_project_document_disabled
stop_relay

# The disabled test intentionally inserts a malformed, unreferenced projection
# behind the Relay. Remove only that isolated fixture before bootstrap.
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "DELETE FROM events
      WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'
        AND kind IN (44301, 40905, 40906, 40907);" >/dev/null

# Establish a real initialized v1 Project View, then use the supported v2
# cutover. Directly flipping project_view_schema_version is intentionally
# rejected by the canonical continuity constraints.
buzz_project_view_admin enable --community "${test_host}" >/dev/null
profile_file="$(mktemp)"
goal_file="$(mktemp)"
temporary_files+=("${profile_file}" "${goal_file}")
jq -n '{
  name: "Project Document canary",
  positioning: "An isolated Project View v2 compatibility fixture",
  purpose: "Exercise Project Document Stage 2",
  problem: "Document delivery needs a canonical Project coordinate",
  scope: "Local canary only"
}' >"${profile_file}"
jq -n '{
  id: "00000000-0000-4000-8000-00000000d002",
  title: "Deliver Project Document Stage 2",
  desired_outcome: "Verified private Document CRUD and history",
  directions: ["Keep capability fail closed"]
}' >"${goal_file}"
start_relay
buzz_cli project-view init --profile "${profile_file}" --goal "${goal_file}" >/dev/null
stop_relay
buzz_project_view_admin disable --community "${test_host}" >/dev/null
buzz_project_view_admin cutover-v2 \
  --community "${test_host}" \
  --idempotency-key "project-document-stage2-${database_name}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null

project_view_schema="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT project_view_schema_version FROM communities
   WHERE id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${project_view_schema}" == "2" ]]

buzz_admin bootstrap \
  --community "${test_host}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null
buzz_admin verify \
  --community "${test_host}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null
buzz_admin enable \
  --community "${test_host}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null

status_json="$(buzz_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == true
  and .[0].ready == true
  and .[0].catalog_revision == 0
  and .[0].active_document_count == 0
' <<<"${status_json}" >/dev/null

start_relay
run_e2e_binary e2e_project_document_enabled

# Real CLI CRUD, metadata-only list/history, pinned reads, exact patch, and
# conflict exit-code behavior.
create_json="$(buzz_cli documents create \
  --title "Stage 2 CLI canary" \
  --summary "isolated metadata" \
  --content "# Stage 2 CLI canary")"
document_id="$(jq -er '.document_id' <<<"${create_json}")"
jq -e '.accepted == true and .document_revision == 1 and .confirmation == "receipt_and_readback"' \
  <<<"${create_json}" >/dev/null

list_json="$(buzz_cli documents list)"
jq -e --arg id "${document_id}" '
  any(.[]; .document_id == $id)
  and all(.[]; has("content_markdown") | not)
' <<<"${list_json}" >/dev/null
[[ "$(buzz_cli documents get "${document_id}" --content-only)" == "# Stage 2 CLI canary" ]]

buzz_cli documents update "${document_id}" \
  --expected-revision 1 \
  --title "Stage 2 CLI canary" \
  --clear-summary \
  --content $'line one\nline two\n' >/dev/null

patch_file="$(mktemp)"
temporary_files+=("${patch_file}")
printf '%s\n' \
  '--- a/document.md' \
  '+++ b/document.md' \
  '@@ -1,2 +1,2 @@' \
  ' line one' \
  '-line two' \
  '+line three' >"${patch_file}"
buzz_cli documents patch "${document_id}" \
  --expected-revision 2 \
  --patch-file "${patch_file}" >/dev/null

history_json="$(buzz_cli documents history "${document_id}")"
jq -e '
  length == 3
  and .[0].document_revision == 3
  and .[2].document_revision == 1
  and all(.[]; has("content_markdown") | not)
' <<<"${history_json}" >/dev/null
jq -e '.document_revision == 1 and .content_markdown == "# Stage 2 CLI canary"' \
  <<<"$(buzz_cli documents get "${document_id}" --revision 1)" >/dev/null

conflict_log="$(mktemp)"
temporary_files+=("${conflict_log}")
set +e
buzz_cli documents update "${document_id}" \
  --expected-revision 2 \
  --title "stale update" \
  --clear-summary \
  --content "must not commit" >/dev/null 2>"${conflict_log}"
conflict_status=$?
set -e
if [[ "${conflict_status}" != "5" ]]; then
  cat "${conflict_log}" >&2
  echo "Project Document E2E: stale update did not return exit 5" >&2
  exit 1
fi

buzz_cli documents delete "${document_id}" --expected-revision 3 >/dev/null
jq -e 'length == 4 and .[0].state == "deleted"' \
  <<<"$(buzz_cli documents history "${document_id}")" >/dev/null
jq -e '.document_revision == 1 and .content_markdown == "# Stage 2 CLI canary"' \
  <<<"$(buzz_cli documents get "${document_id}" --revision 1)" >/dev/null

# Synthetic Secret incident drill: no credential value is created. Keep only
# event/Document coordinates, disable, verify public fail-closed behavior,
# simulate external rotation/assessment, then perform reviewed re-enable.
incident_json="$(buzz_cli documents create \
  --title "Secret incident drill" \
  --content "Synthetic suspected-credential marker; no real secret value")"
incident_document_id="$(jq -er '.document_id' <<<"${incident_json}")"
incident_event_id="$(jq -er '.event_id' <<<"${incident_json}")"
buzz_admin disable --community "${test_host}" >/dev/null

info_json="$(curl -fsS "http://${test_host}/info")"
jq -e '
  (.supported_extensions // [])
  | all(. != "buzz-project-document-v1")
' <<<"${info_json}" >/dev/null
if buzz_cli documents list >/dev/null 2>&1; then
  echo "Project Document E2E: disabled capability remained readable" >&2
  exit 1
fi
if rg -Fq "Synthetic suspected-credential marker" "${relay_log}"; then
  echo "Project Document E2E: synthetic incident body appeared in Relay logs" >&2
  exit 1
fi

# These values model the only coordinates carried into incident handling.
[[ "${incident_document_id}" =~ ^[0-9a-f-]{36}$ ]]
[[ "${incident_event_id}" =~ ^[0-9a-f]{64}$ ]]
rotation_assessment="synthetic-external-credential-rotated-and-impact-reviewed"
[[ "${rotation_assessment}" == *"rotated-and-impact-reviewed" ]]

buzz_admin verify \
  --community "${test_host}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null
buzz_admin enable \
  --community "${test_host}" \
  --expected-pubkey "${relay_owner_pubkey}" >/dev/null
jq -e '.document_revision == 1 and .state == "active"' \
  <<<"$(buzz_cli documents get "${incident_document_id}" --revision 1)" >/dev/null
buzz_cli documents delete "${incident_document_id}" --expected-revision 1 >/dev/null

# Final kill-switch proof: canonical history remains, ordinary capability is
# unavailable, and the control sequence is audit-recorded.
revision_rows_before_disable="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM project_document_revisions
   WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'")"
buzz_admin disable --community "${test_host}" >/dev/null
revision_rows_after_disable="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM project_document_revisions
   WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${revision_rows_before_disable}" == "${revision_rows_after_disable}" ]]

control_audits="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT count(*) FROM audit_log
   WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'
     AND action = 'project_document_control'")"
if (( control_audits < 5 )); then
  echo "Project Document E2E: expected bootstrap/enable/disable audit records" >&2
  exit 1
fi

echo "Project Document Stage 2 E2E and synthetic Secret incident drill passed."
