#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

fail_if_present() {
  local file=$1
  shift
  local pattern
  for pattern in "$@"; do
    if rg -n --fixed-strings "$pattern" "$repo_root/$file"; then
      echo "error: legacy Project View runtime token '$pattern' is forbidden in $file" >&2
      exit 1
    fi
  done
}

fail_before_tests() {
  local file=$1
  shift
  local test_line
  test_line=$(
    rg -n '^mod tests \{' "$repo_root/$file" 2>/dev/null \
      | head -n 1 \
      | cut -d: -f1 \
      || true
  )
  local end_line
  if [[ -n "$test_line" ]]; then
    end_line=$((test_line - 2))
  else
    end_line=$(wc -l <"$repo_root/$file")
  fi
  local source
  source=$(sed -n "1,${end_line}p" "$repo_root/$file")
  local pattern
  for pattern in "$@"; do
    if rg -n --fixed-strings "$pattern" <<<"$source"; then
      echo "error: legacy Project View runtime token '$pattern' is forbidden before tests in $file" >&2
      exit 1
    fi
  done
}

require_present() {
  local file=$1
  local pattern=$2
  if ! rg -q --fixed-strings "$pattern" "$repo_root/$file"; then
    echo "error: required Project View v3 runtime token '$pattern' is missing from $file" >&2
    exit 1
  fi
}

require_fixed_count() {
  local file=$1
  local pattern=$2
  local expected=$3
  local actual
  actual=$(rg -F --count-matches "$pattern" "$repo_root/$file" || true)
  if [[ "$actual" != "$expected" ]]; then
    echo "error: required Project View v3 runtime token '$pattern' must occur $expected times in $file (found $actual)" >&2
    exit 1
  fi
}

require_test_only_item() {
  local file=$1
  local item=$2
  local match
  local found=false
  while IFS= read -r match; do
    [[ -n "$match" ]] || continue
    found=true
    local line=${match%%:*}
    local start=$((line > 5 ? line - 5 : 1))
    if ! sed -n "${start},${line}p" "$repo_root/$file" \
      | rg -q '^[[:space:]]*#\[cfg\(test\)\][[:space:]]*$'; then
      echo "error: legacy test helper '$item' has a production-compiled occurrence at $file:$line" >&2
      exit 1
    fi
  done < <(rg -n --fixed-strings "$item" "$repo_root/$file" || true)
  if [[ "$found" != true ]]; then
    echo "error: required test-only legacy helper '$item' is missing from $file" >&2
    exit 1
  fi
}

# Ordinary CLI reads and writes are schema-v3-only. The explicitly isolated
# project_view_snapshot's explicitly named legacy helpers and
# project_view_v3_approval remain available solely as read-only v2-to-v3
# operator migration input.
fail_before_tests crates/carryforth-cli/src/commands/project_view.rs \
  'ProjectViewSchema::V1' \
  'ProjectViewSchema::V2' \
  'read_verified_v2_snapshot' \
  'read_legacy_v2_migration_snapshot' \
  'ProjectViewCmd::Init {' \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'project_view_v2::'

fail_if_present crates/carryforth-cli/src/commands/roles.rs \
  'read_verified_v2_snapshot' \
  'read_legacy_v2_migration_snapshot' \
  'require_v2_identity' \
  'read_role_history_page' \
  'build_role_command(command)' \
  'ProjectViewSchema::V2'

fail_if_present crates/carryforth-cli/src/commands/project_view_v3_role_history.rs \
  'buzz-project-view-v2-entity' \
  'parse_v2_entity_projection' \
  '"scope": "role_history"'

# Desktop native and TypeScript runtime must never normalize an older major.
require_present crates/buzz-sdk/src/project_view_v3.rs \
  'pub const PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION: &str = "buzz-project-view-v3-bootstrap";'
for file in \
  desktop/src-tauri/src/commands/project_view.rs \
  desktop/src-tauri/src/commands/project_view/identity.rs \
  desktop/src-tauri/src/commands/project_view_mutation.rs \
  desktop/src-tauri/src/commands/project_view_role.rs \
  desktop/src-tauri/src/commands/project_view/role_history.rs
do
  fail_if_present "$file" \
    'ProjectViewSchema::V1' \
    'ProjectViewSchema::V2' \
    'buzz-project-view-v1' \
    'buzz-project-view-v2' \
    'parse_v2_' \
    'buzz_sdk_pkg::project_view_v2'
done
require_present desktop/src-tauri/src/commands/project_view/identity.rs \
  'if runtime_ready && bootstrap_discoverable {'
require_present desktop/src-tauri/src/commands/project_view_mutation.rs \
  'identity.require_runtime_ready("Project View mutations")?;'
require_present desktop/src-tauri/src/commands/project_view_role.rs \
  'identity.require_runtime_ready("Role continuity mutations")?;'
require_present desktop/src-tauri/src/commands/project_view/role_history.rs \
  'identity.require_runtime_ready("Project View Role history")?;'
require_present desktop/src-tauri/src/commands/project_view/v3.rs \
  '"#t": [PROJECT_VIEW_V3_META_TAG]'
require_present desktop/src-tauri/src/commands/project_view_mutation.rs \
  '"#t": [projection_tag]'
require_present desktop/src-tauri/src/commands/project_view_role.rs \
  '"#t": [PROJECT_VIEW_V3_META_TAG]'
require_present desktop/src/features/project-view/liveSync.ts \
  '"#t": [...PROJECT_VIEW_V3_PROJECTION_TAGS]'
require_present desktop/src/shared/constants/projectView.ts \
  'PROJECT_VIEW_V3_ENTITY_TAG = "buzz-project-view-v3-entity"'

fail_if_present desktop/src/shared/api/tauriProjectView.ts \
  'raw.schema_version === 1' \
  'raw.schema_version === 2' \
  'export type RawProjectView =' \
  'export function normalizeProjectView(' \
  'export function normalizeProjectViewObject('
fail_if_present desktop/src/shared/api/tauriProjectViewMutation.ts \
  'operation: "initialize"'
fail_before_tests desktop/src-tauri/src/commands/project_view_mutation.rs \
  'ProjectViewMutationInput::Initialize' \
  'Initialize {'
fail_if_present desktop/src/shared/api/tauriProjectViewRole.ts \
  'schemaVersion === 2'
fail_if_present desktop/src/features/community-members/ui/CommunityMembersSettingsCard.tsx \
  'schemaVersion === 1' \
  'schemaVersion === 2'
fail_if_present desktop/tests/e2e/project-view.spec.ts \
  'schema_version: 1' \
  'schema_version: 2' \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'operation: "initialize"'

# The primary real-process acceptance path is greenfield schema v3. Historical
# majors are exercised only by the explicitly named migration/recovery canary,
# never by the ordinary CI E2E or Stage 5 entry point.
fail_if_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'cutover-v2' \
  'project-view init ' \
  'buzz-project-view-active' \
  'v2_current_entities' \
  'v2_migration_current_entities'
require_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION'
require_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'PROJECT_VIEW_V3_OBJECT_TAG'
require_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'PROJECT_VIEW_V3_ENTITY_TAG'
require_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'PROJECT_VIEW_V3_META_TAG'
require_present crates/buzz-test-client/tests/e2e_project_view.rs \
  'PROJECT_VIEW_E2E_SCRATCH_DATABASE'

fail_if_present scripts/test-project-view-e2e.sh \
  'project-view enable --community' \
  'cutover-v2' \
  'project-view init '
require_present scripts/test-project-view-e2e.sh \
  'PROJECT_VIEW_E2E_SCRATCH_DATABASE=1'
require_present scripts/test-project-view-e2e.sh \
  'project_view_schema_version)'

# CI no longer qualifies a fixed old Project-View-aware Relay as a supported
# post-mutation runtime. The pre-feature database smoke remains useful only
# before initialization; legacy state is covered by the explicit migration
# canary below.
fail_if_present .github/workflows/ci.yml \
  'PROJECT_VIEW_COMPATIBLE_REF' \
  'Post-mutation compatible rollback smoke' \
  'target/compatible/buzz-relay' \
  'test-project-view-compatible-rollback-smoke.sh'
require_present .github/workflows/ci.yml \
  'Current additive schema with pre-feature Relay smoke'
fail_if_present Justfile \
  'test-project-view-compatible-rollback-smoke.sh'
require_present scripts/test-project-view-rollback-smoke.sh \
  'version = 48 AND success'
require_present scripts/test-project-view-rollback-smoke.sh \
  'NOT EXISTS (SELECT 1 FROM project_view_state)'
if [[ -e "$repo_root/scripts/test-project-view-compatible-rollback-smoke.sh" ]]; then
  echo "error: retired old-runtime compatible rollback smoke must stay removed" >&2
  exit 1
fi

# The real-provider Meeting acceptance is an ordinary runtime consumer. Seed
# its disposable Project with the greenfield v3 prepare/init/enable lifecycle,
# then establish the moderator Assignment through v3 Role governance.
fail_if_present scripts/meeting-v2-actions-live-acceptance.sh \
  'project-view init ' \
  'cutover-v2' \
  'project-view-enable-v1' \
  'project-view-enable-v2' \
  'project-view-v2.json' \
  'Project View did not reach schema v2'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  'admin_project_view prepare-v3'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  'cf_as_supervisor project-view init-v3'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  'admin_project_view enable --community "${relay_host}"'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  'cf_as_supervisor roles offer'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  'cf_as_moderator roles proposal accept'
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  ".accepted_project_revision // empty"
require_present scripts/meeting-v2-actions-live-acceptance.sh \
  '[[ "${project_schema}" == 3 ]]'

# Project Document is an independent schema-3-era protocol. Its real-process
# canary may share a Project coordinate but must not quietly restore a v1/v2
# CLI path to obtain one.
fail_if_present scripts/test-project-document-e2e.sh \
  'project-view init ' \
  'cutover-v2' \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'project_view_schema_version == 1' \
  'project_view_schema_version == 2'
require_present scripts/test-project-document-e2e.sh \
  'PROJECT_DOCUMENT_E2E_SCRATCH_DATABASE=1'
require_present scripts/test-project-document-e2e.sh \
  '[[ "${owner_pubkey}" != "${relay_signer_pubkey}" ]]'
require_present scripts/test-project-document-e2e.sh \
  'VALUES ('"'"'00000000-0000-4000-8000-00000000d0c0'"'"', '"'"'${test_host}'"'"', 3);'
require_present scripts/test-project-document-e2e.sh \
  'buzz_project_view_admin prepare-v3'
require_present scripts/test-project-document-e2e.sh \
  '  --operator-pubkey "${owner_pubkey}"'
require_present scripts/test-project-document-e2e.sh \
  'cf_owner_cli --format compact project-view init-v3'
require_present scripts/test-project-document-e2e.sh \
  'buzz_project_view_admin enable --community "${test_host}"'
require_present scripts/test-project-document-e2e.sh \
  'buzz-project-view-v3-bootstrap'
require_present scripts/test-project-document-e2e.sh \
  'or . == "buzz-project-view-v3"'
fail_if_present docs/lora/stage/document/stage2-canary.md \
  '范围：隔离的 Project View v2 Community' \
  'project-view init --profile' \
  'cutover-v2'
require_present docs/lora/stage/document/stage2-canary.md \
  'prepare-v3 → owner-signed init-v3 → checked enable'
fail_if_present docs/lora/stage/document/implementation-design.md \
  '只选 Project View v2 Community；'
require_present docs/lora/stage/document/implementation-design.md \
  'prepare-v3 → direct Human owner签名init-v3 → checked enable'

fail_if_present scripts/test-project-view-stage5-canary.sh \
  'cutover-v2' \
  'project-view init ' \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'exec "${REPO_ROOT}/scripts/test-project-view-legacy'
require_present scripts/test-project-view-stage5-canary.sh \
  'test-project-view-e2e.sh'
fail_if_present scripts/test-project-view-stage6-canary.sh \
  'test-project-view-legacy-v2-to-v3-migration-canary.sh' \
  'PROJECT_VIEW_STAGE5' \
  'stage5_summary' \
  'stage5_run_dir' \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'project-view init ' \
  'cutover-v2' \
  'BUZZ_MANAGED_AGENT' \
  'retired Runtime fence mutated Context' \
  'stale-runtime.stderr'
require_present scripts/test-project-view-stage6-canary.sh \
  'buzz_pv_stage6_canary_$$_${RANDOM}'
require_present scripts/test-project-view-stage6-canary.sh \
  "VALUES ('\${community_id}'::uuid, '\${test_host}', 3);"
require_present scripts/test-project-view-stage6-canary.sh \
  'pv_admin prepare-v3'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-view init-v3'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-document bootstrap'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-view create resource'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-view create role'
require_present scripts/test-project-view-stage6-canary.sh \
  'roles offer'
require_present scripts/test-project-view-stage6-canary.sh \
  'roles proposal accept'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-runtime bind'
require_present scripts/test-project-view-stage6-canary.sh \
  'BUZZ_MANAGED_RUNTIME=1'
require_present scripts/test-project-view-stage6-canary.sh \
  'project-runtime status'
require_present scripts/test-project-view-stage6-canary.sh \
  'operational_supervision_not_context_acl'
require_present scripts/test-project-view-stage6-canary.sh \
  'first_runtime_retired: true'
require_present scripts/test-project-view-stage6-canary.sh \
  'fixture_origin: "greenfield_v3"'
require_present docs/lora/stage/document/stage6-context-canary.md \
  'prepare-v3'
require_present docs/lora/stage/document/stage6-context-canary.md \
  'PROJECT_VIEW_STAGE6_NO_BUILD=1'
require_present docs/lora/stage/document/stage6-context-canary.md \
  'Runtime supervision只治理进程lease、恢复与观测，不作为Context业务ACL'
require_present scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh \
  'Explicit legacy migration/recovery canary'
require_present scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh \
  'project_view::tests::legacy_v2_to_v3_operator_cutover_preserves_full_continuity_history'
require_present scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh \
  'TEST_DATABASE_URL="${test_database_url}"'
fail_if_present scripts/test-project-view-legacy-v2-to-v3-migration-canary.sh \
  'project-view init ' \
  'project-view create ' \
  'roles offer ' \
  'cf_as' \
  'buzz-relay' \
  'buzz-acp'
require_present crates/buzz-db/src/project_view.rs \
  'async fn legacy_v2_to_v3_operator_cutover_preserves_full_continuity_history()'
require_present crates/buzz-db/src/project_view.rs \
  'async fn enable_canonical_legacy_v2_fixture_if_ready('
require_present crates/buzz-db/src/project_view.rs \
  '.cutover_project_view_v3('
require_present crates/buzz-db/src/project_view_v3_migration.rs \
  'schema-v2 migration Document catalog must remain capability-disabled'
require_present crates/buzz-db/src/project_view_v3_migration.rs \
  'Project Document canonical/current/history projection parity failed'
require_present crates/buzz-db/src/project_view_v3_migration.rs \
  'crate::project_document::document_projection_parity('
require_present crates/buzz-db/src/project_view.rs \
  'begin unbootstrapped Document preflight'
require_present crates/buzz-db/src/project_view.rs \
  'begin wrong Document signer preflight'
require_present crates/buzz-db/src/project_view.rs \
  'begin missing Document projection preflight'
require_present crates/buzz-db/src/project_view.rs \
  'begin inconsistent Document projection preflight'

# An enabled old schema is a deployment incident, not a capability-discovery
# fallback. Readiness and fleet metrics must expose it before a Desktop can
# degrade into a generic unsupported message.
require_present crates/buzz-db/src/project_view.rs \
  'pub async fn project_view_migration_required_count('
require_present crates/buzz-db/src/project_view.rs \
  'if self.project_view_migration_required_count().await? != 0 {'
require_present crates/buzz-db/src/project_document.rs \
  'pub async fn project_document_migration_required_count('
require_present crates/buzz-db/src/project_document.rs \
  'if self.project_document_migration_required_count().await? != 0 {'
require_present crates/buzz-relay/src/main.rs \
  'buzz_project_view_migration_required_communities'
require_present crates/buzz-relay/src/main.rs \
  'buzz_project_document_migration_required_communities'
require_present crates/buzz-relay/src/main.rs \
  'readiness will remain unavailable'

# The current runbook must describe the same prepare -> initialize -> checked
# enable boundary and may mention old majors only through the explicitly named
# migration fixture.
fail_if_present docs/project-view-operations.md \
  'buzz-project-view-v1' \
  'buzz-project-view-v2' \
  'project-view cutover-v2' \
  'project-view init --profile'
require_present docs/project-view-operations.md \
  'buzz-project-view-v3-bootstrap'
require_present docs/project-view-operations.md \
  'cf --format compact project-view init-v3'
require_present docs/project-view-operations.md \
  'buzz-admin project-view enable'
require_present docs/project-view-operations.md \
  'test-project-view-legacy-v2-to-v3-migration-canary.sh'
require_present docs/project-view-operations.md \
  'Project Documents used as migration input remain capability-disabled'
fail_if_present crates/carryforth-cli/TESTING.md \
  'Project View v2/v3 Community' \
  'verified Project View v2 identity'
require_present crates/carryforth-cli/TESTING.md \
  'strict-ready 的 Project View v3 Community'
require_present crates/carryforth-cli/TESTING.md \
  '普通 Document CRUD 使用'

# Project Document and Context are ordinary v3-governed runtime surfaces even
# though their own extension majors remain v1. Legacy Document/bootstrap data
# stays available only behind operator migration code.
fail_if_present desktop/src-tauri/src/commands/project_document_tests.rs \
  'buzz-project-view-v1' \
  'buzz-project-view-v2'

# ACP Role Brief has no old capability, projection, or cache fallback.
fail_if_present crates/buzz-acp/src/role_brief.rs \
  'PROJECT_VIEW_V2_EXTENSION' \
  'VerifiedMeta::V2' \
  'resolve_verified_v2' \
  'parse_v2_entity_projection' \
  'parse_v2_meta_projection' \
  'parse_v2_project_object_projection'
require_present crates/buzz-acp/src/runtime_supervisor.rs \
  'let legacy_cutover_maintenance = status.project_view_schema_version == 2'
require_present crates/buzz-acp/src/runtime_supervisor.rs \
  'status.project_view_schema_version != 3 && !legacy_cutover_maintenance'
require_present crates/buzz-acp/src/runtime_supervisor.rs \
  'It must never make a normal/enabled v2 Community look runnable again.'

# Relay ordinary command ingress and NIP-11 discovery are v3-only. Legacy DB
# reducers remain reachable only from buzz-admin cutover/recovery code.
require_present crates/buzz-relay/src/handlers/project_view.rs \
  'require_project_view_v3_runtime(schema_version)?'
require_present crates/buzz-relay/src/handlers/project_view.rs \
  'filter_allows_v3_projections'
require_present crates/buzz-relay/src/handlers/project_view.rs \
  'event_is_v3_projection'
require_present crates/buzz-relay/src/handlers/project_view.rs \
  'expected_relay_pubkey: &PublicKey'
require_present crates/buzz-relay/src/handlers/project_view.rs \
  'configured_projection_signer'
fail_if_present crates/buzz-relay/src/handlers/project_view.rs \
  'parse_meta_projection(event, &event.pubkey)' \
  'parse_project_object_projection(event, &event.pubkey' \
  'parse_entity_projection(event, &event.pubkey'
require_present crates/buzz-relay/src/handlers/req.rs \
  'unsupported:project_view:v3_projection_filter_required'
require_present crates/buzz-relay/src/handlers/req.rs \
  'projection_event_visible_for_filter('
require_present crates/buzz-relay/src/handlers/req.rs \
  'project_view_projection_signer.as_ref()'
require_present crates/buzz-relay/src/handlers/count.rs \
  'unsupported:project_view:v3_projection_filter_required'
require_present crates/buzz-relay/src/handlers/count.rs \
  'projection_event_visible_for_filter('
require_present crates/buzz-relay/src/handlers/count.rs \
  'project_view_projection_signer.as_ref()'
require_present crates/buzz-relay/src/api/bridge.rs \
  'unsupported:project_view:v3_projection_filter_required'
require_present crates/buzz-relay/src/api/bridge.rs \
  'projection_event_visible_for_filter('
require_present crates/buzz-relay/src/api/bridge.rs \
  'project_view_projection_signer.as_ref()'
require_present crates/buzz-relay/src/subscription.rs \
  'filter_allows_v3_projections(filter)'
require_present crates/buzz-relay/src/handlers/event.rs \
  'project_view_projection_passes_final_fanout_gate('
require_present crates/buzz-relay/src/handlers/event.rs \
  'configured_projection_signer.as_ref()'
require_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  '"#t": [PROJECT_VIEW_V3_META_TAG]'
require_present crates/buzz-acp/src/role_brief.rs \
  '.custom_tags(t_tag, [PROJECT_VIEW_V3_META_TAG])'
fail_if_present crates/buzz-relay/src/handlers/project_view.rs \
  'handle_v2_mutation' \
  'handle_v1_mutation' \
  'schema_version == 2' \
  'schema_version == 1'

# Runtime-supervisor terminal system actions are canonical v3 writes. The old
# v2 transaction remains explicitly test-only so production cannot silently
# rewrite a schema-v3 Community with v2 projection wire formats or state.
require_present crates/buzz-relay/src/runtime_supervision.rs \
  '.end_unrecoverable_assignment_v3(&claim, &state.relay_keypair)'
fail_if_present crates/buzz-relay/src/runtime_supervision.rs \
  '.end_unrecoverable_assignment(&claim, &state.relay_keypair)'
require_present crates/buzz-db/src/project_view_v3.rs \
  'pub async fn end_unrecoverable_assignment_v3('
require_present crates/buzz-db/src/project_view_v3.rs \
  'load_continuity_state(&mut tx, claim.community_id, 3)'
require_present crates/buzz-db/src/project_view_v3.rs \
  'V3ProjectionSource::System {'
require_present crates/buzz-db/src/project_view_v3.rs \
  'V3_UNRECOVERABLE_ASSIGNMENT_STATE_UPDATE_SQL'
require_present crates/buzz-db/src/project_view_v3.rs \
  'AND schema_version = 3'
require_present crates/buzz-db/src/project_view_v3.rs \
  'V3_RUNTIME_SYSTEM_WRITE_AVAILABLE_SQL'
require_present crates/buzz-db/src/project_view_v3.rs \
  "maintenance.state = 'normal'"
require_present crates/buzz-db/src/project_view_v3.rs \
  'assert_counts_in_tx(&mut tx, claim.community_id, counts).await?;'
fail_before_tests crates/buzz-db/src/project_view_v2.rs \
  'pub async fn end_unrecoverable_assignment('
require_present crates/buzz-db/src/project_view_v2.rs \
  'pub async fn end_unrecoverable_assignment_legacy_v2_for_test('

fail_if_present crates/buzz-relay/src/nip11.rs \
  'PROJECT_VIEW_V1_EXTENSION' \
  'PROJECT_VIEW_V2_EXTENSION' \
  'ProjectViewCapability::V1' \
  'ProjectViewCapability::V2' \
  'project_view_v2_capability_ready' \
  'project_view_capability_ready('
require_present crates/buzz-relay/src/nip11.rs \
  'project_view_v3_advertised_write_ready'
require_present crates/buzz-relay/src/nip11.rs \
  'PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION'
require_present crates/buzz-relay/src/nip11.rs \
  'project_view_v3_bootstrap_discoverable(tenant.community())'
fail_if_present crates/buzz-relay/src/nip11.rs \
  'project_view_status_by_host'
require_present crates/buzz-db/src/project_view_v3.rs \
  'pub async fn project_view_v3_bootstrap_discoverable('

require_present crates/buzz-relay/src/handlers/project_document.rs \
  'status.project_view_schema_version != 3'
fail_before_tests crates/buzz-relay/src/handlers/project_document.rs \
  'matches!(status.project_view_schema_version, 2 | 3)'
require_present crates/buzz-relay/src/handlers/community_private.rs \
  'status.project_view_schema_version != 3'
fail_before_tests crates/buzz-relay/src/handlers/community_private.rs \
  'matches!(status.project_view_schema_version, 2 | 3)'
require_present crates/buzz-db/src/project_document.rs \
  'c.project_view_schema_version = 3'
fail_if_present crates/buzz-db/src/project_view.rs \
  'pub async fn set_project_view_enabled(' \
  'pub async fn set_all_project_views_enabled(' \
  'pub async fn project_view_capability_ready(' \
  'pub async fn project_view_snapshot_page(' \
  'pub async fn begin_project_view_reproject(' \
  'pub async fn begin_project_view_write('
require_present crates/buzz-db/src/project_view.rs \
  'Project View enable requires completed schema-v3 migration'
require_present crates/buzz-db/src/project_view.rs \
  'begin_legacy_v1_project_view_reproject'

# The old ordinary v2 transaction exists only to exercise historical reducer
# fixtures. Production builds retain operator cutover/readback APIs, never a
# callable v2 mutation transaction.
for item in \
  'pub struct PreparedV2RoleChange' \
  'pub enum ProjectViewV2PrepareOutcome' \
  'pub enum ProjectViewV2ProjectObjectPrepareOutcome' \
  'pub struct ProjectViewV2WriteTx' \
  'pub async fn project_view_v2_capability_ready(' \
  'pub async fn begin_project_view_v2_write(' \
  'impl ProjectViewV2WriteTx'; do
  require_test_only_item crates/buzz-db/src/project_view_v2.rs "$item"
done
for item in \
  'struct ProjectViewEntryStorageMetadata' \
  'async fn write_project_view_entry(' \
  'fn object_body('; do
  require_test_only_item crates/buzz-db/src/project_view.rs "$item"
done

# Document schema 2 is a disabled operator cutover input, not a runtime major;
# schema 1 must not receive new catalogs and schema-2 admin use is explicit.
fail_before_tests crates/buzz-db/src/project_document.rs \
  'matches!(schema_version, 1..=3)'
require_present crates/buzz-db/src/project_document.rs \
  'matches!(schema_version, 2 | 3)'
require_present crates/buzz-admin/src/project_document.rs \
  'for_v3_cutover: bool'
require_present crates/buzz-admin/src/project_document.rs \
  'Project View schema-2 Document input is migration-only; pass --for-v3-cutover'

# Greenfield provisioning defaults to schema v3 without rewriting any
# existing Community. Both production creation paths also spell out the v3
# value instead of depending only on the SQL default.
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  'ALTER COLUMN project_view_schema_version SET DEFAULT 3'
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  'CREATE OR REPLACE FUNCTION project_role_continuity_validate_community'
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  'CREATE FUNCTION project_view_v3_bootstrap_lifecycle_valid'
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  'CREATE OR REPLACE FUNCTION project_view_v3_validate_row'
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  "maintenance.state = 'normal'"
require_present migrations/0048_project_view_v3_greenfield_default.sql \
  'project_view_context_operations context_operation'
fail_if_present migrations/0048_project_view_v3_greenfield_default.sql \
  'UPDATE communities' \
  'DELETE FROM communities'
require_fixed_count crates/buzz-db/src/lib.rs \
  'INSERT INTO communities (host, project_view_schema_version, project_view_enabled)' 2
require_fixed_count crates/buzz-db/src/lib.rs \
  'VALUES ($1, 3, FALSE)' 2
require_fixed_count crates/buzz-db/src/relay_members.rs \
  'Project View schema migration 0026 is required' 2
fail_before_tests crates/buzz-db/src/relay_members.rs \
  'return Ok(1);'
require_present crates/buzz-db/src/project_view_v3.rs \
  'project_view_v3_bootstrap_lifecycle_valid(community.id)'
require_present crates/buzz-db/src/project_view_maintenance.rs \
  'project_view_v3_bootstrap_lifecycle_valid(c.id)'
require_present crates/buzz-db/src/relay_members.rs \
  'project_view_v3_bootstrap_lifecycle_valid(community.id)'

# Project Document is an independent asset capability. Desktop resolves its
# NIP-11 signer directly and must not require initialized Project View state.
require_present desktop/src-tauri/src/commands/project_document.rs \
  'read_project_document_identity_at(state, &api_base_url)'
fail_before_tests desktop/src-tauri/src/commands/project_document.rs \
  'read_identity_at(' \
  'require_runtime_ready("Project Document")'
require_present desktop/src-tauri/src/commands/project_view/identity.rs \
  'extension == PROJECT_DOCUMENT_CAPABILITY'

# Startup may seed only the first direct Human owner of a completely empty v3
# Community. All later governed membership transitions remain coordinator-
# controlled and use a version-neutral fail-closed reason.
require_present crates/buzz-db/src/relay_members.rs \
  'greenfield_v3_owner_bootstrap_allowed_in_tx'
require_present crates/buzz-db/src/relay_members.rs \
  'unavailable:project_view:membership_coordinator'
fail_if_present crates/buzz-db/src/relay_members.rs \
  'project_view_v2:membership_coordinator'
require_present crates/buzz-db/src/project_view.rs \
  'pub strict_ready: Option<bool>'
require_present crates/buzz-db/src/project_view.rs \
  'list_project_view_statuses_with_strict_readiness'
require_present crates/buzz-db/src/project_view.rs \
  'project_view_status_by_host_with_strict_readiness'
require_present crates/buzz-admin/src/project_view.rs \
  'LegacyV1Reproject'
fail_if_present crates/buzz-admin/src/project_view.rs \
  'ProjectViewCommand::Reproject'

# Capability readiness must cryptographically verify every canonical pointer;
# SQL JSON predicates alone cannot prove signature or exact v3 tags.
require_present crates/buzz-db/src/project_view_v3.rs \
  'strict_v3_projection_wires_ready_in_tx'
require_present crates/buzz-db/src/project_view_v3.rs \
  'buzz_sdk::project_view_v3::parse_project_object_projection'
require_present crates/buzz-db/src/project_view_v3.rs \
  'buzz_sdk::project_view_v3::parse_entity_projection'
require_present crates/buzz-db/src/project_view_v3.rs \
  'buzz_sdk::project_view_v3::parse_meta_projection'

# The retained v2 pages are closed cutover source readers, not runtime
# fallbacks. Ordinary-object migration must never fall back to query_all,
# because the ordinary bridge path intentionally rejects every v2 projection.
require_present crates/buzz-relay/src/api/bridge.rs \
  'ProjectViewPageRequest::V2MigrationCurrentEntities'
require_present crates/buzz-relay/src/api/bridge.rs \
  'ProjectViewPageRequest::V2MigrationObjects'
require_present crates/buzz-relay/src/api/bridge.rs \
  '"v2_migration_objects"'
require_present crates/buzz-relay/src/api/bridge.rs \
  '"v2_migration_current_entities"'
fail_if_present crates/buzz-relay/src/api/bridge.rs \
  '"v2_current_entities"' \
  'project_view_v2_current_entities_page'
require_present crates/buzz-db/src/project_view.rs \
  'project_view_v2_migration_objects_page'
require_present crates/buzz-db/src/project_view.rs \
  'project_view_v2_migration_current_entities_page'
require_present crates/buzz-db/src/project_view.rs \
  'buzz_sdk::project_view_v2::parse_project_object_projection'
require_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  'read_legacy_v2_migration_objects'
require_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  'read_legacy_v2_migration_current_entities'
require_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  '"scope": "v2_migration_objects"'
require_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  '"scope": "v2_migration_current_entities"'
fail_if_present crates/carryforth-cli/src/commands/project_view_snapshot.rs \
  '"scope": "v2_current_entities"' \
  'V2_ENTITY_PAGE_SIZE'

legacy_snapshot_reader=$(sed -n \
  '/pub(crate) async fn read_legacy_v2_migration_snapshot/,/^async fn read_legacy_v2_migration_current_entities/p' \
  "$repo_root/crates/carryforth-cli/src/commands/project_view_snapshot.rs")
if rg -n --fixed-strings 'query_all' <<<"$legacy_snapshot_reader"; then
  echo "error: the closed v2 migration snapshot must not use ordinary query_all" >&2
  exit 1
fi
if ! rg -q --fixed-strings 'read_legacy_v2_migration_objects' <<<"$legacy_snapshot_reader"; then
  echo "error: the v2 migration snapshot is missing its closed object-page reader" >&2
  exit 1
fi
fail_before_tests crates/buzz-relay/src/api/bridge.rs \
  'ProjectViewPageRequest::ActiveObjects' \
  'project_view_snapshot_page(' \
  'unwrap_or("active_objects")' \
  '"active_objects"'

relay_bridge="$repo_root/crates/buzz-relay/src/api/bridge.rs"
v3_current_dispatch=$(sed -n \
  '/ProjectViewPageRequest::V3CurrentEntities {/,/ProjectViewPageRequest::V3RoleHistory {/p' \
  "$relay_bridge")
if rg -n --fixed-strings 'project_view_v2_migration_current_entities_page' <<<"$v3_current_dispatch"; then
  echo "error: Relay schema-v3 dispatch must not reuse the v2 migration reader" >&2
  exit 1
fi
if ! rg -q --fixed-strings 'project_view_v3_current_entities_page' <<<"$v3_current_dispatch"; then
  echo "error: Relay schema-v3 current dispatch is missing its strict v3 DB reader" >&2
  exit 1
fi
require_present crates/buzz-relay/src/api/bridge.rs \
  'project_view_v3_role_history_page'

# Retained schema-v2 source pages are Human-review migration input, not a
# managed-Agent compatibility API. The Relay must apply the exact shared
# direct-Human eligibility predicate before dispatching any such page.
require_present crates/buzz-relay/src/api/bridge.rs \
  'if page.is_v2_migration()'
require_present crates/buzz-relay/src/api/bridge.rs \
  '.project_view_v3_migration_reader_authorized_pubkey('
require_present crates/buzz-relay/src/api/bridge.rs \
  'restricted:project_view:v2_migration_direct_human_required'
require_present crates/buzz-db/src/project_view_v3_migration.rs \
  'crate::relay_members::eligible_direct_human_role_in_tx('
require_present crates/buzz-db/src/lib.rs \
  'relay_members::eligible_direct_human_role(&self.pool, community, pubkey, false)'
require_present crates/buzz-db/src/relay_members.rs \
  'AND actor.agent_owner_pubkey IS NULL'
require_present crates/buzz-db/src/relay_members.rs \
  'OR restriction.muted_until > clock_timestamp()'
require_present crates/buzz-relay/src/api/bridge.rs \
  'fn project_view_page_request_marks_only_explicit_v2_migration_scopes()'

echo "Project View ordinary runtime is schema-v3-only"
