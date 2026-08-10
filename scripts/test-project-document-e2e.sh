#!/usr/bin/env bash
# Exercise the Stage 2 Project Document contract against an isolated database:
# prove the disabled boundary, controlled bootstrap/enable, private WS/HTTP/
# Redis behavior, verified CLI CRUD/history, incident disable/re-enable, and
# final disable with canonical-state preservation.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

export CARGO_INCREMENTAL=0

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
restore_database_name=""

cleanup() {
  if [[ -n "${relay_pid}" ]] && kill -0 "${relay_pid}" 2>/dev/null; then
    kill "${relay_pid}" 2>/dev/null || true
    wait "${relay_pid}" 2>/dev/null || true
  fi
  rm -f "${temporary_files[@]}"
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "DROP DATABASE IF EXISTS ${database_name} WITH (FORCE)" >/dev/null || true
  if [[ -n "${restore_database_name}" ]]; then
    docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
      psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
      -c "DROP DATABASE IF EXISTS ${restore_database_name} WITH (FORCE)" >/dev/null || true
  fi
  find "${REPO_ROOT}/target" "${REPO_ROOT}/desktop/src-tauri/target" \
    -type d -name incremental -prune -exec rm -rf -- {} + 2>/dev/null || true
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
relay_signer_pubkey=79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798
member_private_key=0000000000000000000000000000000000000000000000000000000000000002
member_pubkey=c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5
writer_private_key=0000000000000000000000000000000000000000000000000000000000000003
writer_pubkey=f9308a019258c31049344f85f89d5229b531c845836f99b08601f113bce036f9
outsider_private_key=0000000000000000000000000000000000000000000000000000000000000004
owner_private_key=0000000000000000000000000000000000000000000000000000000000000005
owner_pubkey=2f8bde4d1a07209355b4a7250a5c5128e88b84bddc619ab7cba8d569b240efe4
[[ "${owner_pubkey}" != "${relay_signer_pubkey}" ]]

docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "INSERT INTO communities (id, host, project_view_schema_version)
      VALUES ('00000000-0000-4000-8000-00000000d0c0', '${test_host}', 3);
      INSERT INTO relay_members (community_id, pubkey, role)
      VALUES
        ('00000000-0000-4000-8000-00000000d0c0', '${owner_pubkey}', 'owner'),
        ('00000000-0000-4000-8000-00000000d0c0', '${member_pubkey}', 'member'),
        ('00000000-0000-4000-8000-00000000d0c0', '${writer_pubkey}', 'member');" >/dev/null

if [[ "${PROJECT_DOCUMENT_E2E_NO_BUILD:-0}" != "1" ]]; then
  if [[ "${profile}" == "dev" ]]; then
    cargo build -p buzz-relay -p carryforth-cli -p buzz-admin
  else
    cargo build --profile "${profile}" -p buzz-relay -p carryforth-cli -p buzz-admin
  fi
fi
for binary in buzz-relay cf buzz-admin; do
  if [[ ! -x "${bin_dir}/${binary}" ]]; then
    echo "Project Document E2E: missing executable ${bin_dir}/${binary}" >&2
    exit 1
  fi
done

database_url="postgres://buzz:buzz_dev@localhost:5432/${database_name}"
relay_url="ws://${test_host}"

start_relay() {
  # Document authorization always consults current relay_members / managed
  # owner state. The legacy startup allowlist backfill is intentionally off;
  # the greenfield schema-v3 fixture declares its owner explicitly.
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
    RELAY_OWNER_PUBKEY="${owner_pubkey}" \
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

cf_cli() {
  env \
    CARRYFORTH_RELAY_URL="http://${test_host}" \
    CARRYFORTH_PRIVATE_KEY="${member_private_key}" \
    "${bin_dir}/cf" "$@"
}

cf_owner_cli() {
  env \
    CARRYFORTH_RELAY_URL="http://${test_host}" \
    CARRYFORTH_PRIVATE_KEY="${owner_private_key}" \
    "${bin_dir}/cf" "$@"
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
export PROJECT_DOCUMENT_E2E_SCRATCH_DATABASE=1
export REDIS_URL=redis://localhost:6379

status_json="$(buzz_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == false
  and .[0].project_view_schema_version == 3
  and .[0].catalog_revision == null
  and .[0].revision_count == 0
' <<<"${status_json}" >/dev/null

# Disabled remains an independently tested security state.
start_relay
cf_cli channels list >/dev/null
run_e2e_binary e2e_project_context_stage1
if [[ "${PROJECT_CONTEXT_STAGE1_ONLY:-0}" == "1" ]]; then
  stop_relay
  echo "Project Context Stage 1 E2E passed."
  exit 0
fi
run_e2e_binary e2e_project_document_disabled
stop_relay

# The disabled test intentionally inserts a malformed, unreferenced projection
# behind the Relay. Remove only that isolated fixture before bootstrap.
docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -v ON_ERROR_STOP=1 \
  -c "DELETE FROM events
      WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'
        AND kind IN (44301, 40905, 40906, 40907);" >/dev/null

# Establish the current greenfield Project View lifecycle without routing this
# ordinary canary through a legacy runtime: operator prepare, owner-signed
# initialize while still disabled, then strict checked enable.
prepare_json="$(buzz_project_view_admin prepare-v3 \
  --community "${test_host}" \
  --idempotency-key "project-document-stage2-v3-${database_name}" \
  --operator-pubkey "${owner_pubkey}")"
preparation_operation_id="$(jq -er '.operation_id' <<<"${prepare_json}")"

initialize_v3_file="$(mktemp)"
temporary_files+=("${initialize_v3_file}")
jq -n \
  --arg preparation_operation_id "${preparation_operation_id}" \
  --arg owner_pubkey "${owner_pubkey}" \
  '{
    schema_version: 3,
    expected_project_revision: 0,
    request: {
      type: "initialize",
      preparation_operation_id: $preparation_operation_id,
      profile: {
        name: "Project Document canary",
        positioning: "An isolated schema-v3 Project View fixture",
        purpose: "Exercise Project Document Stage 2",
        problem: "Document delivery needs a canonical Project coordinate",
        scope: "Local canary only"
      },
      goals: [{
        id: "00000000-0000-4000-8000-00000000d002",
        title: "Deliver Project Document Stage 2",
        desired_outcome: "Verified private Document CRUD and history",
        directions: ["Keep capability fail closed"]
      }],
      initial_roles: [{
        role_id: "00000000-0000-4000-8000-00000000d003",
        name: "Project Document canary owner",
        purpose: "Own the isolated canary governance boundary",
        responsibilities: ["Administer the scratch Project View"],
        boundaries: ["Scratch canary only"],
        level: "admin",
        active: true,
        context_references: []
      }],
      initial_governance_assignments: [{
        member_pubkey: $owner_pubkey,
        role_id: "00000000-0000-4000-8000-00000000d003",
        proposal_id: "00000000-0000-4000-8000-00000000d004",
        assignment_id: "00000000-0000-4000-8000-00000000d005"
      }]
    }
  }' >"${initialize_v3_file}"

start_relay
pre_initialize_info="$(curl -fsS "http://${test_host}/info")"
jq -e '
  (.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v3-bootstrap")) != null
    and ($extensions | all(
      (startswith("buzz-project-view-") | not)
      or . == "buzz-project-view-v3-bootstrap"
    ))
' <<<"${pre_initialize_info}" >/dev/null
initialize_json="$(cf_owner_cli --format compact project-view init-v3 \
  --command "${initialize_v3_file}")"
jq -e '.accepted == true' <<<"${initialize_json}" >/dev/null
initialized_disabled_info="$(curl -fsS "http://${test_host}/info")"
jq -e '
  (.supported_extensions // [])
  | all(startswith("buzz-project-view-") | not)
' <<<"${initialized_disabled_info}" >/dev/null
stop_relay

project_view_schema="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT project_view_schema_version FROM communities
   WHERE id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${project_view_schema}" == "3" ]]
project_view_basis="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT project_revision::text || ':' || projection_generation::text
   FROM project_view_state
   WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${project_view_basis}" == "1:1" ]]
project_view_signer="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT encode(projection_pubkey, 'hex')
   FROM project_view_state
   WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${project_view_signer}" == "${relay_signer_pubkey}" ]]
project_view_enabled="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
  psql -U buzz -d "${database_name}" -Atc \
  "SELECT project_view_enabled FROM communities
   WHERE id = '00000000-0000-4000-8000-00000000d0c0'")"
[[ "${project_view_enabled}" == "f" ]]

buzz_project_view_admin enable --community "${test_host}" >/dev/null

buzz_admin bootstrap \
  --community "${test_host}" \
  --expected-pubkey "${relay_signer_pubkey}" >/dev/null
buzz_admin verify \
  --community "${test_host}" \
  --expected-pubkey "${relay_signer_pubkey}" >/dev/null
buzz_admin enable \
  --community "${test_host}" \
  --expected-pubkey "${relay_signer_pubkey}" >/dev/null

status_json="$(buzz_admin status --community "${test_host}")"
jq -e '
  length == 1
  and .[0].enabled == true
  and .[0].ready == true
  and .[0].project_view_schema_version == 3
  and .[0].catalog_revision == 0
  and .[0].active_document_count == 0
' <<<"${status_json}" >/dev/null

start_relay
enabled_info="$(curl -fsS "http://${test_host}/info")"
jq -e '
  (.supported_extensions // []) as $extensions
  | ($extensions | index("buzz-project-view-v3")) != null
    and ($extensions | index("buzz-project-view-v3-bootstrap")) == null
    and ($extensions | all(
      (startswith("buzz-project-view-") | not)
      or . == "buzz-project-view-v3"
    ))
' <<<"${enabled_info}" >/dev/null
run_e2e_binary e2e_project_document_enabled

# Real CLI CRUD, metadata-only list/history, pinned reads, exact patch, and
# conflict exit-code behavior.
create_json="$(cf_cli documents create \
  --title "Stage 2 CLI canary" \
  --summary "isolated metadata" \
  --content "# Stage 2 CLI canary")"
document_id="$(jq -er '.document_id' <<<"${create_json}")"
jq -e '.accepted == true and .document_revision == 1 and .confirmation == "receipt_and_readback"' \
  <<<"${create_json}" >/dev/null

list_json="$(cf_cli documents list)"
jq -e --arg id "${document_id}" '
  any(.[]; .document_id == $id)
  and all(.[]; has("content_markdown") | not)
' <<<"${list_json}" >/dev/null
[[ "$(cf_cli documents get "${document_id}" --content-only)" == "# Stage 2 CLI canary" ]]

cf_cli documents update "${document_id}" \
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
cf_cli documents patch "${document_id}" \
  --expected-revision 2 \
  --patch-file "${patch_file}" >/dev/null

history_json="$(cf_cli documents history "${document_id}")"
jq -e '
  length == 3
  and .[0].document_revision == 3
  and .[2].document_revision == 1
  and all(.[]; has("content_markdown") | not)
' <<<"${history_json}" >/dev/null
jq -e '.document_revision == 1 and .content_markdown == "# Stage 2 CLI canary"' \
  <<<"$(cf_cli documents get "${document_id}" --revision 1)" >/dev/null

conflict_log="$(mktemp)"
temporary_files+=("${conflict_log}")
set +e
cf_cli documents update "${document_id}" \
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

cf_cli documents delete "${document_id}" --expected-revision 3 >/dev/null
jq -e 'length == 4 and .[0].state == "deleted"' \
  <<<"$(cf_cli documents history "${document_id}")" >/dev/null
jq -e '.document_revision == 1 and .content_markdown == "# Stage 2 CLI canary"' \
  <<<"$(cf_cli documents get "${document_id}" --revision 1)" >/dev/null

# Synthetic Secret incident drill: no credential value is created. Keep only
# event/Document coordinates, disable, verify public fail-closed behavior,
# simulate external rotation/assessment, then perform reviewed re-enable.
incident_json="$(cf_cli documents create \
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
if cf_cli documents list >/dev/null 2>&1; then
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
  --expected-pubkey "${relay_signer_pubkey}" >/dev/null
buzz_admin enable \
  --community "${test_host}" \
  --expected-pubkey "${relay_signer_pubkey}" >/dev/null
jq -e '.document_revision == 1 and .state == "active"' \
  <<<"$(cf_cli documents get "${incident_document_id}" --revision 1)" >/dev/null
cf_cli documents delete "${incident_document_id}" --expected-revision 1 >/dev/null

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

if [[ "${PROJECT_DOCUMENT_STAGE7_RECOVERY:-0}" == "1" ]]; then
  # Real local signer rotation: keep the capability disabled, build every
  # historical projection in an invisible generation, then activate once.
  stop_relay
  generated_key="$(${bin_dir}/buzz-admin generate-key)"
  rotated_relay_pubkey="$(awk '/Public key:/ {print $3}' <<<"${generated_key}")"
  rotated_relay_private_key="$(awk '/Secret key:/ {print $3}' <<<"${generated_key}")"
  [[ "${rotated_relay_pubkey}" =~ ^[0-9a-f]{64}$ ]]
  [[ "${rotated_relay_private_key}" =~ ^[0-9a-f]{64}$ ]]
  rotated_key_file="$(mktemp)"
  backup_file="$(mktemp)"
  temporary_files+=("${rotated_key_file}" "${backup_file}")
  printf '%s\n' "${rotated_relay_private_key}" >"${rotated_key_file}"
  chmod 600 "${rotated_key_file}"

  buzz_admin reproject \
    --community "${test_host}" \
    --all-revisions \
    --relay-key-file "${rotated_key_file}" \
    --expected-pubkey "${rotated_relay_pubkey}" >/dev/null
  jq -e '.replayed == true and .projection_parity == true' \
    <<<"$(buzz_admin reproject \
      --community "${test_host}" \
      --all-revisions \
      --relay-key-file "${rotated_key_file}" \
      --expected-pubkey "${rotated_relay_pubkey}")" >/dev/null
  buzz_admin verify \
    --community "${test_host}" \
    --expected-pubkey "${rotated_relay_pubkey}" >/dev/null
  status_json="$(buzz_admin status --community "${test_host}")"
  jq -e --arg signer "${rotated_relay_pubkey}" '
    length == 1
    and .[0].enabled == false
    and .[0].projection_generation == 2
    and .[0].projection_pubkey == $signer
    and .[0].meta_parity == true
    and .[0].orphan_projection_count == 0
    and .[0].pointer_mismatch_count == 0
    and .[0].reproject.state == "activated"
  ' <<<"${status_json}" >/dev/null

  # Back up the rotated, disabled generation and restore it into a second
  # exact scratch database. Verify canonical business rows and every active
  # projection with the new signer before starting a Relay from that identity.
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    pg_dump -U buzz -d "${database_name}" -Fc >"${backup_file}"
  [[ -s "${backup_file}" ]]
  restore_database_name="${database_name}_restore"
  if [[ ! "${restore_database_name}" =~ ^buzz_pd_e2e_[0-9_]+_restore$ ]]; then
    echo "Refusing unsafe restore database name: ${restore_database_name}" >&2
    exit 1
  fi
  docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d postgres -v ON_ERROR_STOP=1 \
    -c "CREATE DATABASE ${restore_database_name}" >/dev/null
  docker exec -i -e PGPASSWORD=buzz_dev buzz-postgres \
    pg_restore -U buzz -d "${restore_database_name}" --no-owner --no-privileges \
    <"${backup_file}"
  restored_database_url="postgres://buzz:buzz_dev@localhost:5432/${restore_database_name}"
  env \
    DATABASE_URL="${restored_database_url}" \
    BUZZ_RELAY_PRIVATE_KEY="${rotated_relay_private_key}" \
    "${bin_dir}/buzz-admin" project-document verify \
      --community "${test_host}" \
      --expected-pubkey "${rotated_relay_pubkey}" >/dev/null
  canonical_digest_sql="SELECT md5(string_agg(
      document_id::text || ':' || document_revision::text || ':' || catalog_revision::text || ':' ||
      state || ':' || encode(actor_pubkey, 'hex') || ':' || canonical_at::text || ':' ||
      coalesce(title, '') || ':' || coalesce(summary, '') || ':' || coalesce(content_markdown, ''),
      E'\\n' ORDER BY document_id, document_revision))
    FROM project_document_revisions
    WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'"
  source_digest="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc "${canonical_digest_sql}")"
  restored_digest="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${restore_database_name}" -Atc "${canonical_digest_sql}")"
  [[ -n "${source_digest}" && "${source_digest}" == "${restored_digest}" ]]

  relay_private_key="${rotated_relay_private_key}"
  export BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN=3
  export BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN=3
  export BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC=100
  docker exec buzz-redis redis-cli DEL \
    "buzz:00000000-0000-4000-8000-00000000d0c0:ratelimit:${member_pubkey}:msg" \
    "buzz:00000000-0000-4000-8000-00000000d0c0:ratelimit:${member_pubkey}:ws" \
    "buzz:00000000-0000-4000-8000-00000000d0c0:ratelimit:${member_pubkey}:api" \
    >/dev/null
  start_relay
  buzz_admin enable \
    --community "${test_host}" \
    --relay-key-file "${rotated_key_file}" \
    --expected-pubkey "${rotated_relay_pubkey}" >/dev/null
  jq -e 'length == 4 and .[0].state == "deleted"' \
    <<<"$(cf_cli documents history "${document_id}")" >/dev/null

  # Bounded abuse burst: the normal shared HTTP admission limiter must reject
  # the fourth/following private Document history query. This exercises the
  # real Document query gate without creating ambiguous write/readback results.
  docker exec buzz-redis redis-cli DEL \
    "buzz:00000000-0000-4000-8000-00000000d0c0:ratelimit:${member_pubkey}:api" \
    >/dev/null
  current_catalog_revision="$(docker exec -e PGPASSWORD=buzz_dev buzz-postgres \
    psql -U buzz -d "${database_name}" -Atc \
    "SELECT catalog_revision FROM project_document_state
     WHERE community_id = '00000000-0000-4000-8000-00000000d0c0'")"
  burst_query="$(jq -cn \
    --arg signer "${rotated_relay_pubkey}" \
    --argjson catalog_revision "${current_catalog_revision}" \
    '[{
      kinds: [40905],
      authors: [$signer],
      "#t": ["buzz-project-document-head"],
      limit: 1,
      buzz_project_document: {
        scope: "active_heads",
        projection_generation: 2,
        catalog_revision: $catalog_revision
      }
    }]')"
  burst_accepted=0
  burst_rejected=0
  for burst_index in $(seq 1 6); do
    burst_log="$(mktemp)"
    temporary_files+=("${burst_log}")
    burst_status="$(curl -sS -o "${burst_log}" -w '%{http_code}' \
      -X POST "http://${test_host}/query" \
      -H 'Content-Type: application/json' \
      -H "X-Pubkey: ${member_pubkey}" \
      --data-binary "${burst_query}")"
    if [[ "${burst_status}" == "200" ]]; then
      burst_accepted=$((burst_accepted + 1))
    elif [[ "${burst_status}" == "429" ]] \
      && rg -qi "rate-limited|quota exceeded" "${burst_log}"; then
      burst_rejected=$((burst_rejected + 1))
    else
      cat "${burst_log}" >&2
      echo "Project Document Stage 7: unexpected bounded-burst failure" >&2
      exit 1
    fi
  done
  if (( burst_accepted != 3 || burst_rejected != 3 )); then
    echo "Project Document Stage 7: bounded burst accepted=${burst_accepted}, rejected=${burst_rejected}, expected 3/3" >&2
    exit 1
  fi
  admission_metric="$(curl -fsS "http://127.0.0.1:${metrics_port}/metrics" \
    | rg 'buzz_admission_rejections_total.*reason="quota"' || true)"
  [[ "${admission_metric}" == *'transport="http"'* ]]
  buzz_admin disable --community "${test_host}" >/dev/null
  stop_relay
  unset BUZZ_RATE_LIMIT_HUMAN_MESSAGES_PER_MIN
  unset BUZZ_RATE_LIMIT_HUMAN_API_CALLS_PER_MIN
  unset BUZZ_RATE_LIMIT_HUMAN_WS_EVENTS_PER_SEC

  recovery_run_id="$(date -u +%Y%m%dT%H%M%SZ)-$$"
  recovery_report_dir="${STAGE7_RECOVERY_REPORT_DIR:-${REPO_ROOT}/test-results/stage7-recovery/${recovery_run_id}}"
  mkdir -p "${recovery_report_dir}"
  backup_bytes="$(wc -c <"${backup_file}")"
  jq -n \
    --arg run_id "${recovery_run_id}" \
    --arg community_id "00000000-0000-4000-8000-00000000d0c0" \
    --arg target_pubkey "${rotated_relay_pubkey}" \
    --argjson revision_count "${revision_rows_after_disable}" \
    --argjson backup_bytes "${backup_bytes}" \
    --argjson burst_accepted "${burst_accepted}" \
    --argjson burst_rejected "${burst_rejected}" \
    '{
      schema_version: 1,
      run_id: $run_id,
      mode: "single_machine_prerelease",
      community_id: $community_id,
      source_generation: 1,
      target_generation: 2,
      target_pubkey: $target_pubkey,
      revision_count_before_rotation: $revision_count,
      inactive_generation_staged: true,
      activated: true,
      replay_after_commit_verified: true,
      orphan_projection_count: 0,
      pointer_mismatch_count: 0,
      backup_bytes: $backup_bytes,
      restored_to_independent_database: true,
      canonical_digest_matched: true,
      restored_projection_parity: true,
      new_signer_relay_history_read: true,
      secret_incident_drill: true,
      bounded_abuse: {
        http_budget: 3,
        accepted: $burst_accepted,
        rejected_429: $burst_rejected
      },
      final_enabled: false,
      passed: true
    }' >"${recovery_report_dir}/recovery-report.json"
  {
    echo "# Project Document Stage 7 recovery report"
    echo
    echo "- Run: \`${recovery_run_id}\`"
    echo "- Signer generation: 1 → 2"
    echo "- Revisions reprojected: ${revision_rows_after_disable}"
    echo "- Backup bytes: ${backup_bytes}"
    echo "- Independent restore parity: PASS"
    echo "- Bounded HTTP burst: ${burst_accepted} accepted / ${burst_rejected} rejected"
    echo "- Final capability: disabled"
    echo "- Result: PASS"
  } >"${recovery_report_dir}/recovery-report.md"
  echo "Stage 7 recovery evidence: ${recovery_report_dir}"
fi

echo "Project Document Stage 2 E2E, Secret incident, and requested recovery drills passed."
