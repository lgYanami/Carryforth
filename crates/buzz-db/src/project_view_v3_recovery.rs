//! Frozen, monotonic Project View v3 repair and reprojection operations.
//!
//! These are deliberately narrower than ordinary Project View writes. Repair
//! plans can only reconstruct values from immutable cutover/source evidence or
//! from the current closed business body. Reprojection changes only the Relay
//! projection generation. Both paths retain the exact maintenance fence until
//! a later explicit verify/resume.

use std::collections::{BTreeMap, BTreeSet};

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_core::{CommunityId, EventId, PublicKey, StoredEvent};
use buzz_project_view::v2::{ChangeSource, RoleLevel};
use buzz_project_view::v3::{
    maintenance_repair_plan_digest, CanonicalMaintenanceRepairPlanV1,
    CanonicalResourceCutoverEnvelopeV1, ProjectContextReference, ProjectResourceV3,
    ProjectViewEntryV3, ProjectViewObjectDataV3, RepairActionV1,
};
use buzz_project_view::{ProjectViewObjectType, MAX_SAFE_REVISION};
use buzz_sdk::project_view_v3::{
    V3ChangedHead, V3EntityChange, V3EntityCounts, V3ProjectionContext, V3ProjectionSource,
};
use chrono::{DateTime, Utc};
use nostr::{Event, Keys};
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::project_view_maintenance::{
    idempotency_hash, replay_operation, require_human_operator_in_tx, ProjectViewMaintenanceError,
    ProjectViewMaintenanceReceipt, ProjectViewMaintenanceResult,
};
use crate::project_view_v3_migration::{
    load_all_continuity_entities_in_tx, load_object_origin_in_tx, StoredOrigin,
};
use crate::Db;

const REPAIR_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-repair-request-v1\0";
const REPROJECT_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-reproject-request-v1\0";
const MEMBERSHIP_SNAPSHOT_RESTORE_REQUEST_DOMAIN: &[u8] =
    b"carryforth-pv3-membership-snapshot-restore-request-v1\0";
const BUSINESS_BODY_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-business-body-v1\0";
const SOURCE_EVIDENCE_DIGEST_DOMAIN: &[u8] = b"buzz-pv3-source-evidence-v1\0";

/// Successful frozen repair/reprojection plus Relay events to publish only
/// after the transaction commits.
#[derive(Debug, Clone)]
pub struct ProjectViewV3RecoveryOutcome {
    /// Durable exact-operation receipt.
    pub receipt: ProjectViewMaintenanceReceipt,
    /// Newly committed Relay events; empty for exact replay.
    pub events: Vec<Event>,
}

/// Durable receipt for the bounded local membership-snapshot restoration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectViewV3MembershipSnapshotRecoveryReceipt {
    /// Community whose canonical snapshot coordinate was restored.
    #[serde(serialize_with = "serialize_recovery_community_id")]
    pub community_id: CommunityId,
    /// Stable recovery operation spelling.
    pub operation: String,
    /// Whether this response is an exact durable idempotency replay.
    pub replayed: bool,
    /// Complete stored result body.
    pub result: Value,
}

fn serialize_recovery_community_id<S>(value: &CommunityId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[derive(Debug)]
struct FrozenV3Coordinate {
    maintenance_epoch: i64,
    cutover_change_id: [u8; 32],
    project_revision: u64,
    projection_generation: u64,
    meta_projection_event_id: [u8; 32],
    membership_snapshot_event_id: EventId,
}

#[derive(Debug, Clone)]
struct RecoveryObject {
    entry: ProjectViewEntryV3,
    role_level: Option<RoleLevel>,
    responsible_role_id: Option<Uuid>,
    old_projection_event_id: [u8; 32],
    raw_body: Value,
    origin: StoredOrigin,
}

#[derive(Debug)]
struct CommittedResource {
    resource: ProjectResourceV3,
    reviewer: PublicKey,
}

#[derive(Debug, Serialize)]
struct SourceEvidenceDigestV1 {
    provenance_id: Uuid,
    object_id: Uuid,
    object_type: String,
    source_type: String,
    source_change_id: Vec<u8>,
    source_event_id: Option<Vec<u8>>,
    source_project_revision: i64,
    source_actor_pubkey: Option<Vec<u8>>,
    legacy_mutation_event_id: Option<Vec<u8>>,
    project_view_change_id: Option<Vec<u8>>,
}

impl Db {
    /// Validate an exact repair plan against the current frozen coordinate
    /// without changing canonical state. The returned digest is the value that
    /// an execution receipt will bind.
    pub async fn validate_project_view_v3_repair_plan(
        &self,
        community_id: CommunityId,
        requested_by: PublicKey,
        plan: &CanonicalMaintenanceRepairPlanV1,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewMaintenanceResult<Value> {
        plan.validate()
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let digest = maintenance_repair_plan_digest(plan)
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;
        let coordinate = load_frozen_v3_coordinate_in_tx(
            &mut tx,
            community_id,
            plan.maintenance_epoch,
            relay_pubkey,
        )
        .await?;
        validate_plan_coordinate(community_id, plan, &coordinate)?;
        let objects = load_recovery_objects_in_tx(&mut tx, community_id).await?;
        validate_repair_actions_in_tx(&mut tx, community_id, plan, &coordinate, &objects).await?;
        tx.rollback().await?;
        Ok(json!({
            "operation": "repair",
            "community_id": community_id.to_string(),
            "maintenance_epoch": plan.maintenance_epoch,
            "plan_digest": hex::encode(digest),
            "action_count": plan.actions.len(),
            "expected_project_revision": plan.expected_project_revision,
            "expected_projection_generation": plan.expected_projection_generation,
            "dry_run": true,
        }))
    }

    /// Apply one bounded typed repair as one new Project revision while the
    /// exact post-cutover maintenance epoch remains frozen.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn repair_project_view_v3(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        plan: &CanonicalMaintenanceRepairPlanV1,
        relay_keys: &Keys,
    ) -> ProjectViewMaintenanceResult<ProjectViewV3RecoveryOutcome> {
        plan.validate()
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        if plan.maintenance_epoch != maintenance_epoch {
            return Err(ProjectViewMaintenanceError::Invalid(
                "repair plan maintenance_epoch differs from the command".to_owned(),
            ));
        }
        if Uuid::from_bytes(plan.community_id) != *community_id.as_uuid() {
            return Err(ProjectViewMaintenanceError::Invalid(
                "repair plan belongs to another Community".to_owned(),
            ));
        }
        let plan_digest = maintenance_repair_plan_digest(plan)
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = recovery_request_hash(
            REPAIR_REQUEST_DOMAIN,
            community_id,
            maintenance_epoch,
            &plan_digest,
            &relay_keys.public_key(),
        );
        let epoch = maintenance_epoch_i64(maintenance_epoch)?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;
        if let Some(receipt) = replay_operation(
            &mut tx,
            community_id,
            epoch,
            "repair",
            &idempotency_key_hash,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(ProjectViewV3RecoveryOutcome {
                receipt,
                events: Vec::new(),
            });
        }

        let coordinate = load_frozen_v3_coordinate_in_tx(
            &mut tx,
            community_id,
            maintenance_epoch,
            &relay_keys.public_key(),
        )
        .await?;
        validate_plan_coordinate(community_id, plan, &coordinate)?;
        let mut objects = load_recovery_objects_in_tx(&mut tx, community_id).await?;
        let committed =
            validate_repair_actions_in_tx(&mut tx, community_id, plan, &coordinate, &objects)
                .await?;

        let next_revision = checked_next(coordinate.project_revision, "project_revision")?;
        let canonical_time = recovery_canonical_time_in_tx(&mut tx, community_id).await?;
        apply_repair_actions(
            plan,
            &committed,
            next_revision,
            canonical_time,
            &mut objects,
        )?;
        let affected_ids = plan
            .actions
            .iter()
            .map(repair_action_object_id)
            .collect::<BTreeSet<_>>();

        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewMaintenance,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(maintenance_epoch.to_string()),
                detail: json!({
                    "operation": "repair",
                    "maintenance_epoch": maintenance_epoch,
                    "plan_digest": hex::encode(plan_digest),
                    "action_count": plan.actions.len(),
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let source = ChangeSource::operator(audit.seq, idempotency_key_hash)
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let change_id = source.change_id();
        let audit_seq = u64::try_from(audit.seq).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("audit sequence must be positive".to_owned())
        })?;
        let projection_source = V3ProjectionSource::Operator {
            change_id: event_id(change_id, "repair change_id")?,
            audit_seq,
        };
        let projection_context = V3ProjectionContext {
            project_id: community_id,
            projection_generation: coordinate.projection_generation,
            project_revision: next_revision,
            source: projection_source,
            updated_at: canonical_time,
        };

        let mut signed = Vec::with_capacity(affected_ids.len());
        let mut changed_heads = Vec::with_capacity(affected_ids.len());
        for object_id in &affected_ids {
            let object = objects.get(object_id).ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict(format!(
                    "repair object {object_id} disappeared"
                ))
            })?;
            let event = sign_repair_object(&projection_context, object, relay_keys)?;
            changed_heads.push(changed_head_for_recovery(
                &projection_context,
                object,
                &event,
            )?);
            signed.push((*object_id, event));
        }
        changed_heads.sort_by(|left, right| left.coordinate().cmp(right.coordinate()));
        let counts = load_v3_counts_in_tx(&mut tx, community_id).await?;
        let meta_event = buzz_sdk::project_view_v3::build_meta_projection(
            &projection_context,
            counts,
            coordinate.membership_snapshot_event_id,
            false,
            &changed_heads,
        )
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewMaintenanceError::Invalid(format!("sign repair metadata: {error}"))
        })?;
        verify_repair_meta(
            &meta_event,
            &projection_context,
            counts,
            coordinate.membership_snapshot_event_id,
            &changed_heads,
            &relay_keys.public_key(),
        )?;

        let operation_id = Uuid::new_v4();
        let result = json!({
            "operation": "repair",
            "community_id": community_id.to_string(),
            "maintenance_epoch": maintenance_epoch,
            "change_id": hex::encode(change_id),
            "plan_digest": hex::encode(plan_digest),
            "action_count": plan.actions.len(),
            "affected_object_count": affected_ids.len(),
            "project_revision": next_revision,
            "projection_generation": coordinate.projection_generation,
            "meta_projection_event_id": meta_event.id.to_hex(),
            "state": "frozen",
        });
        insert_repair_change_in_tx(
            &mut tx,
            community_id,
            &change_id,
            audit.seq,
            &idempotency_key_hash,
            plan,
            next_revision,
            &result,
            canonical_time,
        )
        .await?;
        for (object_id, event) in &signed {
            let object = objects.get(object_id).ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict(format!(
                    "repair object {object_id} disappeared"
                ))
            })?;
            persist_repaired_object_in_tx(&mut tx, community_id, &change_id, object, event).await?;
        }
        replace_meta_in_tx(
            &mut tx,
            community_id,
            &coordinate,
            next_revision,
            coordinate.projection_generation,
            &change_id,
            Some(requested_by),
            canonical_time,
            &meta_event,
            &relay_keys.public_key(),
        )
        .await?;
        insert_maintenance_operation_in_tx(
            &mut tx,
            community_id,
            epoch,
            operation_id,
            "repair",
            &idempotency_key_hash,
            &request_hash,
            &requester,
            audit.seq,
            &result,
            canonical_time,
        )
        .await?;
        validate_and_resolve_in_tx(
            &mut tx,
            community_id,
            epoch,
            operation_id,
            &meta_event,
            next_revision,
            coordinate.projection_generation,
        )
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        let mut events = signed
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>();
        events.push(meta_event);
        Ok(ProjectViewV3RecoveryOutcome {
            receipt: ProjectViewMaintenanceReceipt {
                community_id,
                maintenance_epoch,
                operation: "repair".to_owned(),
                state: "frozen".to_owned(),
                replayed: false,
                result,
            },
            events,
        })
    }

    /// Restore the exact canonical NIP-43 snapshot already referenced by an
    /// initialized schema-v3 Project View after a semantically equal generic
    /// publisher incorrectly replaced it.
    ///
    /// This recovery is intentionally narrower than reprojection: it does not
    /// change Project/object revisions, projection generation, projection
    /// pointers, or business rows. The current replacement is accepted only
    /// as evidence that membership did not change; the previously referenced
    /// snapshot must still pass the full canonical v3 wire verifier before it
    /// can be made current again.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn restore_project_view_v3_membership_snapshot(
        &self,
        community_id: CommunityId,
        requested_by: PublicKey,
        idempotency_key: &str,
        expected_project_revision: u64,
        expected_projection_generation: u64,
        expected_old_membership_event_id: EventId,
        candidate_current_membership_event_id: EventId,
        relay_keys: &Keys,
    ) -> ProjectViewMaintenanceResult<ProjectViewV3MembershipSnapshotRecoveryReceipt> {
        if expected_old_membership_event_id == candidate_current_membership_event_id {
            return Err(ProjectViewMaintenanceError::Invalid(
                "old and candidate membership snapshot IDs must differ".to_owned(),
            ));
        }
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = membership_snapshot_restore_request_hash(
            community_id,
            requested_by,
            expected_project_revision,
            expected_projection_generation,
            expected_old_membership_event_id,
            candidate_current_membership_event_id,
            &relay_keys.public_key(),
        );
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;

        if let Some(receipt) = replay_membership_snapshot_restore_in_tx(
            &mut tx,
            community_id,
            &idempotency_key_hash,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(receipt);
        }

        let row = sqlx::query(
            "SELECT community.project_view_schema_version, \
                    community.project_view_enabled, community.archived_at, \
                    state.schema_version, state.project_revision, \
                    state.projection_generation, state.projection_pubkey, \
                    state.meta_projection_event_id, \
                    state.membership_snapshot_event_id, \
                    maintenance.state AS maintenance_state, \
                    maintenance.current_epoch \
             FROM communities community \
             JOIN project_view_state state ON state.community_id = community.id \
             JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = community.id \
             WHERE community.id = $1 FOR UPDATE OF community, state, maintenance",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Conflict(
                "initialized Project View v3 coordinate is missing".to_owned(),
            )
        })?;
        let stored_signer = public_key(
            &row.try_get::<Vec<u8>, _>("projection_pubkey")?,
            "projection_pubkey",
        )?;
        let project_revision = revision_u64(row.try_get("project_revision")?, "project_revision")?;
        let projection_generation = revision_u64(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?;
        let state_membership_event_id = event_id(
            bytes32(
                row.try_get::<Vec<u8>, _>("membership_snapshot_event_id")?,
                "membership_snapshot_event_id",
            )?,
            "membership_snapshot_event_id",
        )?;
        if row.try_get::<i16, _>("project_view_schema_version")? != 3
            || row.try_get::<i16, _>("schema_version")? != 3
            || !row.try_get::<bool, _>("project_view_enabled")?
            || row
                .try_get::<Option<DateTime<Utc>>, _>("archived_at")?
                .is_some()
            || row.try_get::<String, _>("maintenance_state")? != "normal"
            || row.try_get::<Option<i64>, _>("current_epoch")?.is_some()
            || stored_signer != relay_keys.public_key()
            || project_revision != expected_project_revision
            || projection_generation != expected_projection_generation
            || state_membership_event_id != expected_old_membership_event_id
        {
            return Err(ProjectViewMaintenanceError::Conflict(
                "Project View coordinate differs from the exact membership recovery request"
                    .to_owned(),
            ));
        }

        let meta_event_id = bytes32(
            row.try_get::<Vec<u8>, _>("meta_projection_event_id")?,
            "meta_projection_event_id",
        )?;
        let (meta_event, meta_deleted) = load_membership_recovery_event_in_tx(
            &mut tx,
            community_id,
            &meta_event_id,
            "Project View metadata",
        )
        .await?;
        if meta_deleted {
            return Err(ProjectViewMaintenanceError::Conflict(
                "current Project View metadata projection is retired".to_owned(),
            ));
        }
        let parsed_meta = buzz_sdk::project_view_v3::parse_meta_projection(
            &meta_event.event,
            &relay_keys.public_key(),
        )
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        if parsed_meta.project_id != community_id
            || parsed_meta.project_revision != project_revision
            || parsed_meta.projection_generation != projection_generation
            || parsed_meta.membership_snapshot_event_id != expected_old_membership_event_id
        {
            return Err(ProjectViewMaintenanceError::Conflict(
                "current Project View metadata does not reference the expected old snapshot"
                    .to_owned(),
            ));
        }

        let members = crate::project_view_v2::load_membership(&mut tx, community_id)
            .await
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let old_id = expected_old_membership_event_id.to_bytes();
        let candidate_id = candidate_current_membership_event_id.to_bytes();
        let (old_event, old_deleted) = load_membership_recovery_event_in_tx(
            &mut tx,
            community_id,
            &old_id,
            "referenced membership snapshot",
        )
        .await?;
        let (candidate_event, candidate_deleted) = load_membership_recovery_event_in_tx(
            &mut tx,
            community_id,
            &candidate_id,
            "candidate membership snapshot",
        )
        .await?;
        if !old_deleted || candidate_deleted {
            return Err(ProjectViewMaintenanceError::Conflict(
                "recovery requires one retired referenced snapshot and one current candidate"
                    .to_owned(),
            ));
        }
        verify_canonical_membership_snapshot(&old_event, &relay_keys.public_key(), &members)?;
        verify_semantically_equal_membership_snapshot(
            &candidate_event,
            &relay_keys.public_key(),
            &members,
        )?;

        let live_snapshot_ids: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT id FROM events WHERE community_id = $1 AND kind = $2 \
               AND pubkey = $3 AND channel_id IS NULL AND deleted_at IS NULL \
             ORDER BY id",
        )
        .bind(community_id.as_uuid())
        .bind(
            i32::try_from(buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
                ProjectViewMaintenanceError::Invalid(
                    "membership event kind exceeds database integer".to_owned(),
                )
            })?,
        )
        .bind(relay_keys.public_key().as_bytes())
        .fetch_all(&mut *tx)
        .await?;
        if live_snapshot_ids.len() != 1
            || live_snapshot_ids[0].as_slice() != candidate_id.as_slice()
        {
            return Err(ProjectViewMaintenanceError::Conflict(
                "candidate is not the unique current Relay membership snapshot".to_owned(),
            ));
        }

        let requested_by_bytes = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewMaintenance,
                actor_pubkey: Some(requested_by_bytes.to_vec()),
                object_id: Some(expected_old_membership_event_id.to_hex()),
                detail: json!({
                    "operation": "restore_membership_snapshot",
                    "project_revision": project_revision,
                    "projection_generation": projection_generation,
                    "restored_membership_event_id": expected_old_membership_event_id.to_hex(),
                    "retired_membership_event_id": candidate_current_membership_event_id.to_hex(),
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;

        let retired = sqlx::query(
            "UPDATE events SET deleted_at = clock_timestamp() \
             WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(candidate_id.as_slice())
        .execute(&mut *tx)
        .await?;
        let restored = sqlx::query(
            "UPDATE events SET deleted_at = NULL \
             WHERE community_id = $1 AND id = $2 AND deleted_at IS NOT NULL",
        )
        .bind(community_id.as_uuid())
        .bind(old_id.as_slice())
        .execute(&mut *tx)
        .await?;
        if retired.rows_affected() != 1 || restored.rows_affected() != 1 {
            return Err(ProjectViewMaintenanceError::Conflict(
                "membership snapshot current-head state changed during recovery".to_owned(),
            ));
        }

        sqlx::query("SELECT project_view_v3_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        if !crate::project_view_v3::strict_v3_projection_wires_ready_in_tx(
            &mut tx,
            community_id,
            &relay_keys.public_key(),
        )
        .await?
        {
            return Err(ProjectViewMaintenanceError::Invalid(
                "restored Project View v3 projection wires are not strictly ready".to_owned(),
            ));
        }

        let result = json!({
            "operation": "restore_membership_snapshot",
            "community_id": community_id.to_string(),
            "project_revision": project_revision,
            "projection_generation": projection_generation,
            "restored_membership_event_id": expected_old_membership_event_id.to_hex(),
            "retired_membership_event_id": candidate_current_membership_event_id.to_hex(),
            "state": "normal",
            "strict_ready": true,
        });
        sqlx::query(
            "INSERT INTO project_view_v3_membership_snapshot_recoveries \
                (community_id, recovery_id, idempotency_key_hash, \
                 canonical_request_hash, requested_by, audit_seq, \
                 expected_project_revision, expected_projection_generation, \
                 restored_membership_event_id, retired_membership_event_id, \
                 result_receipt, accepted_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,clock_timestamp())",
        )
        .bind(community_id.as_uuid())
        .bind(Uuid::new_v4())
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(requested_by_bytes.as_slice())
        .bind(audit.seq)
        .bind(revision_i64(project_revision, "project_revision")?)
        .bind(revision_i64(
            projection_generation,
            "projection_generation",
        )?)
        .bind(old_id.as_slice())
        .bind(candidate_id.as_slice())
        .bind(&result)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        Ok(ProjectViewV3MembershipSnapshotRecoveryReceipt {
            community_id,
            operation: "restore_membership_snapshot".to_owned(),
            replayed: false,
            result,
        })
    }

    /// Re-sign every canonical v3 current head at the next projection
    /// generation. No Project or object revision is changed.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub async fn reproject_project_view_v3(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        relay_keys: &Keys,
    ) -> ProjectViewMaintenanceResult<ProjectViewV3RecoveryOutcome> {
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = recovery_request_hash(
            REPROJECT_REQUEST_DOMAIN,
            community_id,
            maintenance_epoch,
            &[],
            &relay_keys.public_key(),
        );
        let epoch = maintenance_epoch_i64(maintenance_epoch)?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;
        if let Some(receipt) = replay_operation(
            &mut tx,
            community_id,
            epoch,
            "reproject",
            &idempotency_key_hash,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(ProjectViewV3RecoveryOutcome {
                receipt,
                events: Vec::new(),
            });
        }
        let coordinate = load_frozen_v3_coordinate_in_tx(
            &mut tx,
            community_id,
            maintenance_epoch,
            &relay_keys.public_key(),
        )
        .await?;
        sqlx::query("SELECT project_view_v3_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        let objects = load_recovery_objects_in_tx(&mut tx, community_id).await?;
        let entities = load_all_continuity_entities_in_tx(&mut tx, community_id, 3)
            .await
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let next_generation =
            checked_next(coordinate.projection_generation, "projection_generation")?;
        let canonical_time = recovery_canonical_time_in_tx(&mut tx, community_id).await?;
        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewMaintenance,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(maintenance_epoch.to_string()),
                detail: json!({
                    "operation": "reproject",
                    "maintenance_epoch": maintenance_epoch,
                    "from_projection_generation": coordinate.projection_generation,
                    "to_projection_generation": next_generation,
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let meta_change = ChangeSource::operator(audit.seq, idempotency_key_hash)
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let meta_source = V3ProjectionSource::Operator {
            change_id: event_id(meta_change.change_id(), "reproject change_id")?,
            audit_seq: u64::try_from(audit.seq).map_err(|_| {
                ProjectViewMaintenanceError::Invalid("audit sequence must be positive".to_owned())
            })?,
        };

        let mut object_events = Vec::with_capacity(objects.len());
        for (object_id, object) in &objects {
            let context = V3ProjectionContext {
                project_id: community_id,
                projection_generation: next_generation,
                project_revision: object.entry.project_revision(),
                source: object
                    .origin
                    .projection_source()
                    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?,
                updated_at: entry_updated_at(&object.entry),
            };
            let event = sign_repair_object(&context, object, relay_keys)?;
            object_events.push((*object_id, event));
        }
        let mut entity_events = Vec::with_capacity(entities.len());
        for entity in &entities {
            let context = V3ProjectionContext {
                project_id: community_id,
                projection_generation: next_generation,
                project_revision: entity_project_revision(&entity.entity),
                source: entity
                    .origin
                    .projection_source()
                    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?,
                updated_at: entity.updated_at,
            };
            let event =
                buzz_sdk::project_view_v3::build_entity_projection(&context, &entity.entity)
                    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
                    .sign_with_keys(relay_keys)
                    .map_err(|error| {
                        ProjectViewMaintenanceError::Invalid(format!(
                            "sign v3 continuity reprojection: {error}"
                        ))
                    })?;
            let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
                &event,
                &relay_keys.public_key(),
                community_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            if parsed.entity != entity.entity
                || parsed.projection_generation != next_generation
                || parsed.project_revision != context.project_revision
                || parsed.source != context.source
                || parsed.updated_at != context.updated_at
            {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "signed continuity reprojection differs from canonical state".to_owned(),
                ));
            }
            entity_events.push((
                entity.entity.entity_type(),
                entity.entity.entity_id(),
                entity.old_projection_event_id,
                event,
            ));
        }
        let counts = load_v3_counts_in_tx(&mut tx, community_id).await?;
        let meta_context = V3ProjectionContext {
            project_id: community_id,
            projection_generation: next_generation,
            project_revision: coordinate.project_revision,
            source: meta_source,
            updated_at: canonical_time,
        };
        let meta_event = buzz_sdk::project_view_v3::build_meta_projection(
            &meta_context,
            counts,
            coordinate.membership_snapshot_event_id,
            true,
            &[],
        )
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewMaintenanceError::Invalid(format!("sign v3 reset metadata: {error}"))
        })?;
        verify_repair_meta(
            &meta_event,
            &meta_context,
            counts,
            coordinate.membership_snapshot_event_id,
            &[],
            &relay_keys.public_key(),
        )?;

        for (object_id, event) in &object_events {
            let object = objects.get(object_id).ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict(format!(
                    "reproject object {object_id} disappeared"
                ))
            })?;
            retire_if_live_in_tx(
                &mut tx,
                community_id,
                &object.old_projection_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?;
            insert_new_event_in_tx(&mut tx, community_id, event, "object reprojection").await?;
            let updated = sqlx::query(
                "UPDATE project_view_objects SET projection_event_id = $3 \
                 WHERE community_id = $1 AND object_id = $2 AND projection_event_id = $4",
            )
            .bind(community_id.as_uuid())
            .bind(object_id)
            .bind(event.id.as_bytes().as_slice())
            .bind(object.old_projection_event_id.as_slice())
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(ProjectViewMaintenanceError::Conflict(format!(
                    "object {object_id} projection pointer changed during reproject"
                )));
            }
        }
        for (entity_type, entity_id, old_event_id, event) in &entity_events {
            retire_if_live_in_tx(
                &mut tx,
                community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?;
            insert_new_event_in_tx(&mut tx, community_id, event, "continuity reprojection").await?;
            crate::project_view_v2::update_projection_pointer(
                &mut tx,
                community_id,
                *entity_type,
                *entity_id,
                event.id.as_bytes(),
            )
            .await
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        }
        retire_if_live_in_tx(
            &mut tx,
            community_id,
            &coordinate.meta_projection_event_id,
            KIND_PROJECT_VIEW_META,
        )
        .await?;
        insert_new_event_in_tx(&mut tx, community_id, &meta_event, "metadata reprojection").await?;
        let relay = relay_keys.public_key().to_bytes();
        let state_updated = sqlx::query(
            "UPDATE project_view_state SET meta_projection_event_id = $2, \
                 projection_pubkey = $3, projection_generation = $4 \
             WHERE community_id = $1 AND schema_version = 3 \
               AND project_revision = $5 AND projection_generation = $6 \
               AND meta_projection_event_id = $7",
        )
        .bind(community_id.as_uuid())
        .bind(meta_event.id.as_bytes().as_slice())
        .bind(relay.as_slice())
        .bind(revision_i64(next_generation, "projection_generation")?)
        .bind(revision_i64(
            coordinate.project_revision,
            "project_revision",
        )?)
        .bind(revision_i64(
            coordinate.projection_generation,
            "projection_generation",
        )?)
        .bind(coordinate.meta_projection_event_id.as_slice())
        .execute(&mut *tx)
        .await?;
        if state_updated.rows_affected() != 1 {
            return Err(ProjectViewMaintenanceError::Conflict(
                "Project View coordinate changed during reproject".to_owned(),
            ));
        }

        let operation_id = Uuid::new_v4();
        let result = json!({
            "operation": "reproject",
            "community_id": community_id.to_string(),
            "maintenance_epoch": maintenance_epoch,
            "project_revision": coordinate.project_revision,
            "projection_generation": next_generation,
            "meta_projection_event_id": meta_event.id.to_hex(),
            "object_head_count": object_events.len(),
            "continuity_head_count": entity_events.len(),
            "state": "frozen",
        });
        insert_maintenance_operation_in_tx(
            &mut tx,
            community_id,
            epoch,
            operation_id,
            "reproject",
            &idempotency_key_hash,
            &request_hash,
            &requester,
            audit.seq,
            &result,
            canonical_time,
        )
        .await?;
        validate_and_resolve_in_tx(
            &mut tx,
            community_id,
            epoch,
            operation_id,
            &meta_event,
            coordinate.project_revision,
            next_generation,
        )
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        let mut events = object_events
            .into_iter()
            .map(|(_, event)| event)
            .collect::<Vec<_>>();
        events.extend(entity_events.into_iter().map(|(_, _, _, event)| event));
        events.push(meta_event);
        Ok(ProjectViewV3RecoveryOutcome {
            receipt: ProjectViewMaintenanceReceipt {
                community_id,
                maintenance_epoch,
                operation: "reproject".to_owned(),
                state: "frozen".to_owned(),
                replayed: false,
                result,
            },
            events,
        })
    }
}

async fn load_frozen_v3_coordinate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    maintenance_epoch: u64,
    relay_pubkey: &PublicKey,
) -> ProjectViewMaintenanceResult<FrozenV3Coordinate> {
    let epoch = maintenance_epoch_i64(maintenance_epoch)?;
    let row = sqlx::query(
        "SELECT maintenance.current_epoch, maintenance.state, epoch.outcome, \
                community.project_view_schema_version, community.project_view_enabled, \
                state.schema_version, state.project_revision, state.projection_generation, \
                state.projection_pubkey, state.meta_projection_event_id, \
                state.membership_snapshot_event_id, cutover.cutover_change_id \
         FROM communities community \
         JOIN project_view_state state ON state.community_id = community.id \
         JOIN project_view_maintenance maintenance ON maintenance.community_id = community.id \
         JOIN project_view_maintenance_epochs epoch \
           ON epoch.community_id = community.id AND epoch.maintenance_epoch = $2 \
         JOIN project_view_v3_cutovers cutover \
           ON cutover.community_id = community.id \
          AND cutover.maintenance_epoch = epoch.maintenance_epoch \
         WHERE community.id = $1 FOR UPDATE OF community, state, maintenance, epoch",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewMaintenanceError::Conflict(
            "exact committed v3 maintenance epoch is missing".to_owned(),
        )
    })?;
    let stored_signer = public_key(
        &row.try_get::<Vec<u8>, _>("projection_pubkey")?,
        "projection_pubkey",
    )?;
    if row.try_get::<Option<i64>, _>("current_epoch")? != Some(epoch)
        || row.try_get::<String, _>("state")? != "frozen"
        || row.try_get::<String, _>("outcome")? != "cutover_committed"
        || row.try_get::<i16, _>("project_view_schema_version")? != 3
        || row.try_get::<i16, _>("schema_version")? != 3
        || row.try_get::<bool, _>("project_view_enabled")?
        || stored_signer != *relay_pubkey
    {
        return Err(ProjectViewMaintenanceError::Conflict(
            "repair/reproject requires the exact disabled, committed, frozen schema-v3 epoch and stable signer"
                .to_owned(),
        ));
    }
    let membership = row
        .try_get::<Option<Vec<u8>>, _>("membership_snapshot_event_id")?
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Conflict(
                "v3 membership snapshot pointer is missing".to_owned(),
            )
        })?;
    Ok(FrozenV3Coordinate {
        maintenance_epoch: epoch,
        cutover_change_id: bytes32(row.try_get("cutover_change_id")?, "cutover_change_id")?,
        project_revision: revision_u64(row.try_get("project_revision")?, "project_revision")?,
        projection_generation: revision_u64(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?,
        meta_projection_event_id: bytes32(
            row.try_get("meta_projection_event_id")?,
            "meta_projection_event_id",
        )?,
        membership_snapshot_event_id: EventId::from_slice(&membership).map_err(|error| {
            ProjectViewMaintenanceError::Invalid(format!(
                "invalid membership snapshot event ID: {error}"
            ))
        })?,
    })
}

fn validate_plan_coordinate(
    community_id: CommunityId,
    plan: &CanonicalMaintenanceRepairPlanV1,
    coordinate: &FrozenV3Coordinate,
) -> ProjectViewMaintenanceResult<()> {
    if Uuid::from_bytes(plan.community_id) != *community_id.as_uuid()
        || plan.maintenance_epoch != u64::try_from(coordinate.maintenance_epoch).unwrap_or_default()
        || plan.cutover_change_id != coordinate.cutover_change_id
        || plan.expected_project_revision != coordinate.project_revision
        || plan.expected_projection_generation != coordinate.projection_generation
    {
        return Err(ProjectViewMaintenanceError::Conflict(
            "repair plan does not match the exact frozen v3 coordinate".to_owned(),
        ));
    }
    Ok(())
}

async fn load_recovery_objects_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewMaintenanceResult<BTreeMap<Uuid, RecoveryObject>> {
    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, updated_by, \
                deleted_at, role_level, responsible_role_id, source_type, \
                source_change_id, source_event_id, source_provenance_id, \
                projection_event_id \
         FROM project_view_objects WHERE community_id = $1 AND schema_version = 3 \
         ORDER BY object_id FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut objects = BTreeMap::new();
    for row in rows {
        let object_id: Uuid = row.try_get("object_id")?;
        let raw_body = row
            .try_get::<Option<Value>, _>("body")?
            .unwrap_or(Value::Null);
        let source_type: String = row.try_get("source_type")?;
        let source_change_id = bytes32(row.try_get("source_change_id")?, "source_change_id")?;
        let source_event_id = row
            .try_get::<Option<Vec<u8>>, _>("source_event_id")?
            .map(|value| bytes32(value, "source_event_id"))
            .transpose()?;
        let role_level = row
            .try_get::<Option<String>, _>("role_level")?
            .map(|value| parse_role_level(&value))
            .transpose()?;
        let responsible_role_id: Option<Uuid> = row.try_get("responsible_role_id")?;
        let old_projection_event_id =
            bytes32(row.try_get("projection_event_id")?, "projection_event_id")?;
        let entry = crate::project_view_v3::v3_entry_from_row(row).map_err(|error| {
            ProjectViewMaintenanceError::Invalid(format!(
                "load canonical v3 object {object_id}: {error}"
            ))
        })?;
        let updated_by = entry_updated_by(&entry);
        let origin = load_object_origin_in_tx(
            tx,
            community_id,
            &source_type,
            source_change_id,
            source_event_id,
            object_id,
            entry.object_type(),
            entry.project_revision(),
            updated_by,
        )
        .await
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        if entry.object_type() == ProjectViewObjectType::Role && role_level.is_none() {
            return Err(ProjectViewMaintenanceError::Invalid(format!(
                "v3 Role {object_id} has no governance level"
            )));
        }
        if entry.object_type() != ProjectViewObjectType::Role && role_level.is_some() {
            return Err(ProjectViewMaintenanceError::Invalid(format!(
                "non-Role v3 object {object_id} carries a governance level"
            )));
        }
        if objects
            .insert(
                object_id,
                RecoveryObject {
                    entry,
                    role_level,
                    responsible_role_id,
                    old_projection_event_id,
                    raw_body,
                    origin,
                },
            )
            .is_some()
        {
            return Err(ProjectViewMaintenanceError::Invalid(format!(
                "duplicate canonical object {object_id}"
            )));
        }
    }
    Ok(objects)
}

async fn validate_repair_actions_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    plan: &CanonicalMaintenanceRepairPlanV1,
    coordinate: &FrozenV3Coordinate,
    objects: &BTreeMap<Uuid, RecoveryObject>,
) -> ProjectViewMaintenanceResult<BTreeMap<Uuid, CommittedResource>> {
    let mut resources = BTreeMap::new();
    for action in &plan.actions {
        let object_id = repair_action_object_id(action);
        let object = objects.get(&object_id).ok_or_else(|| {
            ProjectViewMaintenanceError::Conflict(format!(
                "repair target object {object_id} does not exist"
            ))
        })?;
        match action {
            RepairActionV1::ReapplyCommittedResource {
                mapping_entry_digest,
                ..
            } => {
                if object.entry.object_type() != ProjectViewObjectType::Resource
                    || !matches!(object.entry, ProjectViewEntryV3::Active(_))
                {
                    return Err(ProjectViewMaintenanceError::Conflict(format!(
                        "committed mapping target {object_id} is not an active Resource"
                    )));
                }
                let committed = load_committed_resource_in_tx(
                    tx,
                    community_id,
                    &coordinate.cutover_change_id,
                    object_id,
                    mapping_entry_digest,
                )
                .await?;
                resources.insert(object_id, committed);
            }
            RepairActionV1::RebuildObjectProvenance {
                expected_business_body_digest,
                expected_source_digest,
                ..
            } => {
                require_body_digest(object_id, &object.raw_body, expected_business_body_digest)?;
                let actual = source_evidence_digest_in_tx(
                    tx,
                    community_id,
                    object_id,
                    object.entry.project_revision(),
                )
                .await?;
                if &actual != expected_source_digest {
                    return Err(ProjectViewMaintenanceError::Conflict(format!(
                        "object {object_id} immutable source digest changed"
                    )));
                }
            }
            RepairActionV1::RebuildNormalizedContext {
                expected_business_body_digest,
                ..
            } => {
                if matches!(object.entry, ProjectViewEntryV3::Tombstone(_)) {
                    return Err(ProjectViewMaintenanceError::Invalid(format!(
                        "tombstone {object_id} has no normalized Context to rebuild"
                    )));
                }
                require_body_digest(object_id, &object.raw_body, expected_business_body_digest)?;
            }
        }
    }
    Ok(resources)
}

async fn load_committed_resource_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    cutover_change_id: &[u8; 32],
    resource_id: Uuid,
    mapping_entry_digest: &[u8; 32],
) -> ProjectViewMaintenanceResult<CommittedResource> {
    let row = sqlx::query(
        "SELECT reviewed_v3_payload, reviewed_by_pubkey \
         FROM project_view_v3_committed_resource_entries \
         WHERE community_id = $1 AND cutover_change_id = $2 \
           AND resource_id = $3 AND mapping_entry_digest = $4 FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(cutover_change_id.as_slice())
    .bind(resource_id)
    .bind(mapping_entry_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewMaintenanceError::Conflict(format!(
            "committed Resource evidence for {resource_id} does not match the plan"
        ))
    })?;
    let envelope: CanonicalResourceCutoverEnvelopeV1 =
        serde_json::from_value(row.try_get("reviewed_v3_payload")?).map_err(|error| {
            ProjectViewMaintenanceError::Invalid(format!(
                "invalid committed Resource payload for {resource_id}: {error}"
            ))
        })?;
    if !envelope.context_references.is_empty() {
        return Err(ProjectViewMaintenanceError::Invalid(format!(
            "committed Resource {resource_id} has non-empty cutover Context"
        )));
    }
    let resource = ProjectResourceV3 {
        name: envelope.resource_data.name,
        resource_kind: envelope.resource_data.resource_kind,
        summary: envelope.resource_data.summary,
        guide_document_id: envelope.resource_data.guide_document_id,
    };
    resource
        .validate()
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
    Ok(CommittedResource {
        resource,
        reviewer: public_key(
            &row.try_get::<Vec<u8>, _>("reviewed_by_pubkey")?,
            "reviewed_by_pubkey",
        )?,
    })
}

async fn source_evidence_digest_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    object_id: Uuid,
    project_revision: u64,
) -> ProjectViewMaintenanceResult<[u8; 32]> {
    let rows = sqlx::query(
        "SELECT provenance_id, object_id, object_type, source_type, source_change_id, \
                source_event_id, source_project_revision, source_actor_pubkey, \
                legacy_mutation_event_id, project_view_change_id \
         FROM project_view_object_provenance \
         WHERE community_id = $1 AND object_id = $2 AND source_project_revision = $3 \
         ORDER BY provenance_id FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .bind(revision_i64(project_revision, "source_project_revision")?)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != 1 {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "object {object_id} must have exactly one immutable source at its current business revision"
        )));
    }
    let row = rows.into_iter().next().ok_or_else(|| {
        ProjectViewMaintenanceError::Conflict("source evidence disappeared".to_owned())
    })?;
    let evidence = SourceEvidenceDigestV1 {
        provenance_id: row.try_get("provenance_id")?,
        object_id: row.try_get("object_id")?,
        object_type: row.try_get("object_type")?,
        source_type: row.try_get("source_type")?,
        source_change_id: row.try_get("source_change_id")?,
        source_event_id: row.try_get("source_event_id")?,
        source_project_revision: row.try_get("source_project_revision")?,
        source_actor_pubkey: row.try_get("source_actor_pubkey")?,
        legacy_mutation_event_id: row.try_get("legacy_mutation_event_id")?,
        project_view_change_id: row.try_get("project_view_change_id")?,
    };
    canonical_json_digest(SOURCE_EVIDENCE_DIGEST_DOMAIN, &evidence)
}

fn apply_repair_actions(
    plan: &CanonicalMaintenanceRepairPlanV1,
    committed: &BTreeMap<Uuid, CommittedResource>,
    next_project_revision: u64,
    canonical_time: DateTime<Utc>,
    objects: &mut BTreeMap<Uuid, RecoveryObject>,
) -> ProjectViewMaintenanceResult<()> {
    let affected = plan
        .actions
        .iter()
        .map(repair_action_object_id)
        .collect::<BTreeSet<_>>();
    for action in &plan.actions {
        if let RepairActionV1::ReapplyCommittedResource { .. } = action {
            let object_id = repair_action_object_id(action);
            let evidence = committed.get(&object_id).ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict(format!(
                    "committed Resource evidence for {object_id} disappeared"
                ))
            })?;
            let object = objects.get_mut(&object_id).ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict(format!(
                    "repair object {object_id} disappeared"
                ))
            })?;
            let ProjectViewEntryV3::Active(entry) = &mut object.entry else {
                return Err(ProjectViewMaintenanceError::Conflict(format!(
                    "committed Resource {object_id} became a tombstone"
                )));
            };
            entry.data = ProjectViewObjectDataV3::Resource(evidence.resource.clone());
            entry.context_references.clear();
            entry.updated_by = evidence.reviewer;
        }
    }
    for object_id in affected {
        let object = objects.get_mut(&object_id).ok_or_else(|| {
            ProjectViewMaintenanceError::Conflict(format!("repair object {object_id} disappeared"))
        })?;
        match &mut object.entry {
            ProjectViewEntryV3::Active(entry) => {
                entry.object_revision = checked_next(entry.object_revision, "object_revision")?;
                entry.project_revision = next_project_revision;
                entry.updated_at = canonical_time;
            }
            ProjectViewEntryV3::Tombstone(tombstone) => {
                tombstone.object_revision =
                    checked_next(tombstone.object_revision, "object_revision")?;
                tombstone.project_revision = next_project_revision;
                tombstone.deleted_at = canonical_time;
            }
        }
        object.raw_body = match &object.entry {
            ProjectViewEntryV3::Active(entry) => {
                crate::project_view_v3::v3_object_body(&entry.data, &entry.context_references)
                    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
            }
            ProjectViewEntryV3::Tombstone(_) => Value::Null,
        };
        if let (ProjectViewEntryV3::Active(entry), Some(level)) = (&object.entry, object.role_level)
        {
            if entry.object_type == ProjectViewObjectType::Role {
                object
                    .raw_body
                    .as_object_mut()
                    .ok_or_else(|| {
                        ProjectViewMaintenanceError::Invalid(
                            "serialized Role repair body is not an object".to_owned(),
                        )
                    })?
                    .insert("level".to_owned(), Value::String(level.as_str().to_owned()));
            }
        }
    }
    Ok(())
}

fn sign_repair_object(
    context: &V3ProjectionContext,
    object: &RecoveryObject,
    relay_keys: &Keys,
) -> ProjectViewMaintenanceResult<Event> {
    let event = match &object.entry {
        ProjectViewEntryV3::Active(entry) if entry.object_type == ProjectViewObjectType::Role => {
            let level = object.role_level.ok_or_else(|| {
                ProjectViewMaintenanceError::Invalid(format!(
                    "Role {} has no governance level",
                    entry.id
                ))
            })?;
            let role = entry
                .role_definition(level)
                .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            let entity = V3EntityChange::Role(role.clone());
            let event = buzz_sdk::project_view_v3::build_entity_projection(context, &entity)
                .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewMaintenanceError::Invalid(format!(
                        "sign v3 Role recovery projection: {error}"
                    ))
                })?;
            let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
                &event,
                &relay_keys.public_key(),
                context.project_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            if parsed.entity != entity
                || parsed.project_revision != context.project_revision
                || parsed.projection_generation != context.projection_generation
                || parsed.source != context.source
                || parsed.updated_at != context.updated_at
            {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "signed Role recovery head differs from canonical state".to_owned(),
                ));
            }
            event
        }
        ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => {
            let event = buzz_sdk::project_view_v3::build_project_object_projection(
                context,
                &object.entry,
                object.responsible_role_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
            .sign_with_keys(relay_keys)
            .map_err(|error| {
                ProjectViewMaintenanceError::Invalid(format!(
                    "sign v3 object recovery projection: {error}"
                ))
            })?;
            let parsed = buzz_sdk::project_view_v3::parse_project_object_projection(
                &event,
                &relay_keys.public_key(),
                context.project_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            if parsed.object.id() != object.entry.id()
                || parsed.object.object_revision() != object.entry.object_revision()
                || parsed.project_revision != context.project_revision
                || parsed.projection_generation != context.projection_generation
                || parsed.source != context.source
                || parsed.responsible_role_id != object.responsible_role_id
                || parsed.updated_at != context.updated_at
            {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "signed object recovery head differs from canonical state".to_owned(),
                ));
            }
            event
        }
    };
    verify_projected_object(&event, object, context, &relay_keys.public_key())?;
    Ok(event)
}

fn changed_head_for_recovery(
    context: &V3ProjectionContext,
    object: &RecoveryObject,
    event: &Event,
) -> ProjectViewMaintenanceResult<V3ChangedHead> {
    match &object.entry {
        ProjectViewEntryV3::Active(entry) if entry.object_type == ProjectViewObjectType::Role => {
            let role = entry
                .role_definition(object.role_level.ok_or_else(|| {
                    ProjectViewMaintenanceError::Invalid(format!(
                        "Role {} has no governance level",
                        entry.id
                    ))
                })?)
                .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            buzz_sdk::project_view_v3::changed_head_for_entity(
                context,
                &V3EntityChange::Role(role),
                event,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))
        }
        ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => {
            buzz_sdk::project_view_v3::changed_head_for_project_object(
                context,
                &object.entry,
                event,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))
        }
    }
}

async fn persist_repaired_object_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8; 32],
    object: &RecoveryObject,
    event: &Event,
) -> ProjectViewMaintenanceResult<()> {
    let object_id = object.entry.id();
    retire_if_live_in_tx(
        tx,
        community_id,
        &object.old_projection_event_id,
        KIND_PROJECT_VIEW_OBJECT,
    )
    .await?;
    insert_new_event_in_tx(tx, community_id, event, "repair object projection").await?;
    let provenance_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO project_view_object_provenance \
            (community_id, provenance_id, object_id, object_type, source_type, \
             source_change_id, source_event_id, source_project_revision, \
             source_actor_pubkey, legacy_mutation_event_id, project_view_change_id) \
         VALUES ($1,$2,$3,$4,'operator',$5,NULL,$6,NULL,NULL,$5)",
    )
    .bind(community_id.as_uuid())
    .bind(provenance_id)
    .bind(object_id)
    .bind(object.entry.object_type().as_str())
    .bind(change_id.as_slice())
    .bind(revision_i64(
        object.entry.project_revision(),
        "source_project_revision",
    )?)
    .execute(&mut **tx)
    .await?;

    let (body, updated_at, updated_by, deleted_at, guide_document_id, context) = match &object.entry
    {
        ProjectViewEntryV3::Active(entry) => (
            Some(object.raw_body.clone()),
            entry.updated_at,
            entry.updated_by,
            None,
            match &entry.data {
                ProjectViewObjectDataV3::Resource(resource) => Some(resource.guide_document_id),
                _ => None,
            },
            entry.context_references.as_slice(),
        ),
        ProjectViewEntryV3::Tombstone(tombstone) => (
            None,
            tombstone.deleted_at,
            tombstone.deleted_by,
            Some(tombstone.deleted_at),
            None,
            &[][..],
        ),
    };
    let updated_by = updated_by.to_bytes();
    let updated = sqlx::query(
        "UPDATE project_view_objects SET object_revision = $3, project_revision = $4, \
             body = $5, updated_at = $6, updated_by = $7, source_event_id = NULL, \
             projection_event_id = $8, deleted_at = $9, guide_document_id = $10, \
             source_type = 'operator', source_change_id = $11, \
             source_provenance_id = $12 \
         WHERE community_id = $1 AND object_id = $2 AND schema_version = 3 \
           AND object_revision + 1 = $3 AND projection_event_id = $13",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .bind(revision_i64(
        object.entry.object_revision(),
        "object_revision",
    )?)
    .bind(revision_i64(
        object.entry.project_revision(),
        "project_revision",
    )?)
    .bind(body)
    .bind(updated_at)
    .bind(updated_by.as_slice())
    .bind(event.id.as_bytes().as_slice())
    .bind(deleted_at)
    .bind(guide_document_id)
    .bind(change_id.as_slice())
    .bind(provenance_id)
    .bind(object.old_projection_event_id.as_slice())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "object {object_id} changed while applying repair"
        )));
    }
    rebuild_normalized_context_in_tx(tx, community_id, object_id, context).await
}

async fn rebuild_normalized_context_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    object_id: Uuid,
    references: &[ProjectContextReference],
) -> ProjectViewMaintenanceResult<()> {
    sqlx::query(
        "DELETE FROM project_view_resource_context_references \
         WHERE community_id = $1 AND source_object_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "DELETE FROM project_view_document_context_references \
         WHERE community_id = $1 AND source_object_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .execute(&mut **tx)
    .await?;
    for reference in references {
        match reference {
            ProjectContextReference::Resource { resource_id } => {
                sqlx::query(
                    "INSERT INTO project_view_resource_context_references \
                        (community_id, source_object_id, target_resource_id) \
                     VALUES ($1,$2,$3)",
                )
                .bind(community_id.as_uuid())
                .bind(object_id)
                .bind(resource_id)
                .execute(&mut **tx)
                .await?;
            }
            ProjectContextReference::Document {
                document_id,
                mode,
                document_revision,
            } => {
                sqlx::query(
                    "INSERT INTO project_view_document_context_references \
                        (community_id, source_object_id, target_document_id, \
                         reference_mode, target_document_revision) \
                     VALUES ($1,$2,$3,$4,$5)",
                )
                .bind(community_id.as_uuid())
                .bind(object_id)
                .bind(document_id)
                .bind(match mode {
                    buzz_project_view::v3::DocumentReferenceMode::Live => "live",
                    buzz_project_view::v3::DocumentReferenceMode::Pinned => "pinned",
                })
                .bind(
                    document_revision
                        .map(|revision| revision_i64(revision, "document_revision"))
                        .transpose()?,
                )
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_repair_change_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8; 32],
    audit_seq: i64,
    idempotency_key_hash: &[u8; 32],
    plan: &CanonicalMaintenanceRepairPlanV1,
    project_revision: u64,
    result: &Value,
    accepted_at: DateTime<Utc>,
) -> ProjectViewMaintenanceResult<()> {
    let subject = json!({
        "repair_schema_version": plan.schema_version,
        "maintenance_epoch": plan.maintenance_epoch,
        "cutover_change_id": hex::encode(plan.cutover_change_id),
        "expected_project_revision": plan.expected_project_revision,
        "expected_projection_generation": plan.expected_projection_generation,
        "action_count": plan.actions.len(),
        "plan_digest": hex::encode(
            maintenance_repair_plan_digest(plan)
                .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?
        ),
    });
    sqlx::query(
        "INSERT INTO project_view_changes \
            (community_id, change_id, source_type, source_event_id, \
             source_request_hash, source_audit_seq, idempotency_key_hash, \
             actor_pubkey, acting_assignment_id, operation, subject, \
             project_revision, result, accepted_at) \
         VALUES ($1,$2,'operator',NULL,NULL,$3,$4,NULL,NULL, \
                 'maintenance_repair',$5,$6,$7,$8)",
    )
    .bind(community_id.as_uuid())
    .bind(change_id.as_slice())
    .bind(audit_seq)
    .bind(idempotency_key_hash.as_slice())
    .bind(subject)
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(result)
    .bind(accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn replace_meta_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    coordinate: &FrozenV3Coordinate,
    project_revision: u64,
    projection_generation: u64,
    change_id: &[u8; 32],
    actor: Option<PublicKey>,
    canonical_time: DateTime<Utc>,
    meta_event: &Event,
    relay_pubkey: &PublicKey,
) -> ProjectViewMaintenanceResult<()> {
    retire_if_live_in_tx(
        tx,
        community_id,
        &coordinate.meta_projection_event_id,
        KIND_PROJECT_VIEW_META,
    )
    .await?;
    insert_new_event_in_tx(tx, community_id, meta_event, "repair metadata projection").await?;
    let actor = actor.map(PublicKey::to_bytes);
    let relay = relay_pubkey.to_bytes();
    let updated = sqlx::query(
        "UPDATE project_view_state SET project_revision = $2, updated_at = $3, \
             last_event_id = $4, last_actor_pubkey = $5, \
             meta_projection_event_id = $6, projection_pubkey = $7, \
             projection_generation = $8, last_change_id = $4, \
             last_source_event_id = NULL \
         WHERE community_id = $1 AND schema_version = 3 \
           AND project_revision = $9 AND projection_generation = $10 \
           AND meta_projection_event_id = $11",
    )
    .bind(community_id.as_uuid())
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(canonical_time)
    .bind(change_id.as_slice())
    .bind(actor.as_ref().map(<[u8; 32]>::as_slice))
    .bind(meta_event.id.as_bytes().as_slice())
    .bind(relay.as_slice())
    .bind(revision_i64(
        projection_generation,
        "projection_generation",
    )?)
    .bind(revision_i64(
        coordinate.project_revision,
        "expected_project_revision",
    )?)
    .bind(revision_i64(
        coordinate.projection_generation,
        "expected_projection_generation",
    )?)
    .bind(coordinate.meta_projection_event_id.as_slice())
    .execute(&mut **tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(ProjectViewMaintenanceError::Conflict(
            "Project View state changed while replacing repair metadata".to_owned(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_maintenance_operation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    operation_id: Uuid,
    operation: &str,
    idempotency_key_hash: &[u8; 32],
    request_hash: &[u8; 32],
    requested_by: &[u8; 32],
    audit_seq: i64,
    result: &Value,
    accepted_at: DateTime<Utc>,
) -> ProjectViewMaintenanceResult<()> {
    sqlx::query(
        "INSERT INTO project_view_maintenance_operations \
            (community_id, maintenance_epoch, operation_id, operation, \
             idempotency_key_hash, canonical_request_hash, requested_by, \
             audit_seq, result_receipt, accepted_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(operation_id)
    .bind(operation)
    .bind(idempotency_key_hash.as_slice())
    .bind(request_hash.as_slice())
    .bind(requested_by.as_slice())
    .bind(audit_seq)
    .bind(result)
    .bind(accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn validate_and_resolve_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    operation_id: Uuid,
    meta_event: &Event,
    project_revision: u64,
    projection_generation: u64,
) -> ProjectViewMaintenanceResult<()> {
    sqlx::query("SELECT project_view_v3_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "UPDATE project_view_maintenance_invalidations SET \
             resolved_by_operation_id = $3, resolved_meta_event_id = $4, \
             resolved_project_revision = $5, resolved_projection_generation = $6 \
         WHERE community_id = $1 AND maintenance_epoch = $2 \
           AND phase = 'post_cutover' AND resolved_by_operation_id IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(operation_id)
    .bind(meta_event.id.as_bytes().as_slice())
    .bind(revision_i64(project_revision, "resolved_project_revision")?)
    .bind(revision_i64(
        projection_generation,
        "resolved_projection_generation",
    )?)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn verify_repair_meta(
    event: &Event,
    context: &V3ProjectionContext,
    counts: V3EntityCounts,
    membership_snapshot_event_id: EventId,
    changed_heads: &[V3ChangedHead],
    relay_pubkey: &PublicKey,
) -> ProjectViewMaintenanceResult<()> {
    let parsed = buzz_sdk::project_view_v3::parse_meta_projection(event, relay_pubkey)
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
    if parsed.project_id != context.project_id
        || parsed.project_revision != context.project_revision
        || parsed.projection_generation != context.projection_generation
        || parsed.entity_counts != counts
        || parsed.membership_snapshot_event_id != membership_snapshot_event_id
        || parsed.reset != changed_heads.is_empty()
        || parsed.changed_heads != changed_heads
        || parsed.source != context.source
        || parsed.updated_at != context.updated_at
    {
        return Err(ProjectViewMaintenanceError::Invalid(
            "signed v3 recovery metadata differs from the prepared coordinate".to_owned(),
        ));
    }
    Ok(())
}

async fn load_v3_counts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewMaintenanceResult<V3EntityCounts> {
    let counts = crate::project_view_v2::load_counts(tx, community_id)
        .await
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
    Ok(V3EntityCounts {
        active_objects: counts.active_objects,
        open_proposals: counts.open_proposals,
        active_assignments: counts.active_assignments,
        active_commitments: counts.active_commitments,
        checkpoints: counts.checkpoints,
        handoffs: counts.handoffs,
    })
}

async fn recovery_canonical_time_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewMaintenanceResult<DateTime<Utc>> {
    let time =
        sqlx::query_scalar(
            "SELECT GREATEST( \
             clock_timestamp(), \
             COALESCE((SELECT max(updated_at) + interval '1 microsecond' \
                       FROM project_view_objects WHERE community_id = $1), \
                      '-infinity'::timestamptz), \
             COALESCE((SELECT max(created_at) + interval '1 second' FROM events \
                       WHERE community_id = $1 AND kind IN ($2,$3) \
                         AND deleted_at IS NULL), '-infinity'::timestamptz) \
         )",
        )
        .bind(community_id.as_uuid())
        .bind(i32::try_from(KIND_PROJECT_VIEW_META).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("metadata kind exceeds INT".to_owned())
        })?)
        .bind(i32::try_from(KIND_PROJECT_VIEW_OBJECT).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("object kind exceeds INT".to_owned())
        })?)
        .fetch_one(&mut **tx)
        .await?;
    Ok(time)
}

async fn retire_if_live_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8; 32],
    kind: u32,
) -> ProjectViewMaintenanceResult<()> {
    crate::event::retire_projection_head_in_tx(tx, community_id, event_id, kind).await?;
    Ok(())
}

async fn insert_new_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    label: &str,
) -> ProjectViewMaintenanceResult<()> {
    let (_, inserted) = crate::event::insert_event_in_tx(tx, community_id, event, None).await?;
    if !inserted {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "{label} {} already exists",
            event.id
        )));
    }
    Ok(())
}

fn repair_action_object_id(action: &RepairActionV1) -> Uuid {
    match action {
        RepairActionV1::ReapplyCommittedResource { resource_id, .. } => {
            Uuid::from_bytes(*resource_id)
        }
        RepairActionV1::RebuildObjectProvenance { object_id, .. }
        | RepairActionV1::RebuildNormalizedContext { object_id, .. } => {
            Uuid::from_bytes(*object_id)
        }
    }
}

fn require_body_digest(
    object_id: Uuid,
    body: &Value,
    expected: &[u8; 32],
) -> ProjectViewMaintenanceResult<()> {
    let actual = canonical_json_digest(BUSINESS_BODY_DIGEST_DOMAIN, body)?;
    if &actual != expected {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "object {object_id} business-body digest changed"
        )));
    }
    Ok(())
}

fn canonical_json_digest<T: Serialize>(
    domain: &[u8],
    value: &T,
) -> ProjectViewMaintenanceResult<[u8; 32]> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
    Ok(hash_parts(domain, &[&encoded]))
}

fn recovery_request_hash(
    domain: &[u8],
    community_id: CommunityId,
    maintenance_epoch: u64,
    payload_digest: &[u8],
    relay_pubkey: &PublicKey,
) -> [u8; 32] {
    hash_parts(
        domain,
        &[
            community_id.as_uuid().as_bytes(),
            &maintenance_epoch.to_be_bytes(),
            payload_digest,
            relay_pubkey.as_bytes(),
        ],
    )
}

#[allow(clippy::too_many_arguments)]
fn membership_snapshot_restore_request_hash(
    community_id: CommunityId,
    requested_by: PublicKey,
    expected_project_revision: u64,
    expected_projection_generation: u64,
    old_membership_event_id: EventId,
    candidate_membership_event_id: EventId,
    relay_pubkey: &PublicKey,
) -> [u8; 32] {
    hash_parts(
        MEMBERSHIP_SNAPSHOT_RESTORE_REQUEST_DOMAIN,
        &[
            community_id.as_uuid().as_bytes(),
            requested_by.as_bytes(),
            &expected_project_revision.to_be_bytes(),
            &expected_projection_generation.to_be_bytes(),
            old_membership_event_id.as_bytes(),
            candidate_membership_event_id.as_bytes(),
            relay_pubkey.as_bytes(),
        ],
    )
}

async fn replay_membership_snapshot_restore_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    idempotency_key_hash: &[u8; 32],
    request_hash: &[u8; 32],
) -> ProjectViewMaintenanceResult<Option<ProjectViewV3MembershipSnapshotRecoveryReceipt>> {
    let row = sqlx::query(
        "SELECT canonical_request_hash, result_receipt \
         FROM project_view_v3_membership_snapshot_recoveries \
         WHERE community_id = $1 AND idempotency_key_hash = $2",
    )
    .bind(community_id.as_uuid())
    .bind(idempotency_key_hash.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if bytes32(
        row.try_get("canonical_request_hash")?,
        "canonical_request_hash",
    )? != *request_hash
    {
        return Err(ProjectViewMaintenanceError::Conflict(
            "membership recovery idempotency key was reused for another request".to_owned(),
        ));
    }
    Ok(Some(ProjectViewV3MembershipSnapshotRecoveryReceipt {
        community_id,
        operation: "restore_membership_snapshot".to_owned(),
        replayed: true,
        result: row.try_get("result_receipt")?,
    }))
}

async fn load_membership_recovery_event_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8; 32],
    label: &str,
) -> ProjectViewMaintenanceResult<(StoredEvent, bool)> {
    let row = sqlx::query(
        "SELECT id, pubkey, created_at, kind, tags, content, sig, received_at, \
                channel_id, deleted_at \
         FROM events WHERE community_id = $1 AND id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event_id.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| ProjectViewMaintenanceError::Conflict(format!("{label} is missing")))?;
    let deleted = row
        .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
        .is_some();
    let event = crate::event::row_to_stored_event(row)?
        .ok_or_else(|| ProjectViewMaintenanceError::Invalid(format!("{label} is malformed")))?;
    if event.channel_id.is_some() {
        return Err(ProjectViewMaintenanceError::Invalid(format!(
            "{label} is unexpectedly channel-scoped"
        )));
    }
    Ok((event, deleted))
}

fn verify_canonical_membership_snapshot(
    event: &StoredEvent,
    relay_pubkey: &PublicKey,
    members: &[crate::project_view_v2::V2MembershipEntry],
) -> ProjectViewMaintenanceResult<()> {
    let timestamp = i64::try_from(event.event.created_at.as_secs())
        .ok()
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Invalid(
                "referenced membership snapshot timestamp is invalid".to_owned(),
            )
        })?;
    crate::project_view_v2::verify_membership_projection(
        &event.event,
        *relay_pubkey,
        members,
        timestamp,
    )
    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))
}

fn verify_semantically_equal_membership_snapshot(
    event: &StoredEvent,
    relay_pubkey: &PublicKey,
    members: &[crate::project_view_v2::V2MembershipEntry],
) -> ProjectViewMaintenanceResult<()> {
    event.event.verify().map_err(|error| {
        ProjectViewMaintenanceError::Invalid(format!(
            "candidate membership snapshot signature is invalid: {error}"
        ))
    })?;
    if event.event.pubkey != *relay_pubkey
        || event.event.kind.as_u16() as u32 != buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST
        || !event.event.content.is_empty()
    {
        return Err(ProjectViewMaintenanceError::Invalid(
            "candidate membership signer, kind, or content is invalid".to_owned(),
        ));
    }
    let tags = event
        .event
        .tags
        .iter()
        .map(|tag| tag.as_slice())
        .collect::<Vec<_>>();
    if !tags
        .first()
        .is_some_and(|tag| tag.len() == 1 && tag[0] == "-")
    {
        return Err(ProjectViewMaintenanceError::Invalid(
            "candidate membership snapshot lacks the leading protected tag".to_owned(),
        ));
    }
    let mut candidate = Vec::with_capacity(tags.len().saturating_sub(1));
    for tag in tags.iter().skip(1) {
        if tag.len() != 3 || tag.first().map(String::as_str) != Some("member") {
            return Err(ProjectViewMaintenanceError::Invalid(
                "candidate membership snapshot contains a noncanonical tag".to_owned(),
            ));
        }
        candidate.push((tag[1].to_ascii_lowercase(), tag[2].clone()));
    }
    candidate.sort_unstable();
    let expected = members
        .iter()
        .map(|member| (member.pubkey.to_ascii_lowercase(), member.role.clone()))
        .collect::<Vec<_>>();
    if candidate != expected {
        return Err(ProjectViewMaintenanceError::Conflict(
            "candidate membership snapshot differs from canonical Relay Members".to_owned(),
        ));
    }
    Ok(())
}

fn hash_parts(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

fn entry_updated_by(entry: &ProjectViewEntryV3) -> PublicKey {
    match entry {
        ProjectViewEntryV3::Active(object) => object.updated_by,
        ProjectViewEntryV3::Tombstone(tombstone) => tombstone.deleted_by,
    }
}

fn entry_updated_at(entry: &ProjectViewEntryV3) -> DateTime<Utc> {
    match entry {
        ProjectViewEntryV3::Active(object) => object.updated_at,
        ProjectViewEntryV3::Tombstone(tombstone) => tombstone.deleted_at,
    }
}

const fn entity_project_revision(entity: &V3EntityChange) -> u64 {
    match entity {
        V3EntityChange::Role(value) => value.project_revision,
        V3EntityChange::Proposal(value) => value.project_revision,
        V3EntityChange::Assignment(value) => value.project_revision,
        V3EntityChange::Commitment(value) => value.project_revision,
        V3EntityChange::Checkpoint(value) => value.project_revision,
        V3EntityChange::Handoff(value) => value.project_revision,
    }
}

fn verify_projected_object(
    event: &Event,
    object: &RecoveryObject,
    context: &V3ProjectionContext,
    relay_pubkey: &PublicKey,
) -> ProjectViewMaintenanceResult<()> {
    match &object.entry {
        ProjectViewEntryV3::Active(entry) if entry.object_type == ProjectViewObjectType::Role => {
            let expected = V3EntityChange::Role(
                entry
                    .role_definition(object.role_level.ok_or_else(|| {
                        ProjectViewMaintenanceError::Invalid(format!(
                            "Role {} has no governance level",
                            entry.id
                        ))
                    })?)
                    .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?,
            );
            let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
                event,
                relay_pubkey,
                context.project_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            if parsed.entity != expected
                || parsed.project_revision != context.project_revision
                || parsed.projection_generation != context.projection_generation
                || parsed.source != context.source
                || parsed.updated_at != context.updated_at
            {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "Role recovery projection failed strict roundtrip".to_owned(),
                ));
            }
        }
        ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => {
            let parsed = buzz_sdk::project_view_v3::parse_project_object_projection(
                event,
                relay_pubkey,
                context.project_id,
            )
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
            if !projected_object_matches_entry(&parsed.object, &object.entry)
                || parsed.project_revision != context.project_revision
                || parsed.projection_generation != context.projection_generation
                || parsed.source != context.source
                || parsed.responsible_role_id != object.responsible_role_id
                || parsed.updated_at != context.updated_at
            {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "object recovery projection failed strict roundtrip".to_owned(),
                ));
            }
        }
    }
    Ok(())
}

fn projected_object_matches_entry(
    projected: &buzz_sdk::project_view_v3::V3ProjectedObject,
    entry: &ProjectViewEntryV3,
) -> bool {
    match (projected, entry) {
        (
            buzz_sdk::project_view_v3::V3ProjectedObject::Active(projected),
            ProjectViewEntryV3::Active(entry),
        ) => projected.as_ref() == entry.as_ref(),
        (
            buzz_sdk::project_view_v3::V3ProjectedObject::Tombstone(projected),
            ProjectViewEntryV3::Tombstone(entry),
        ) => projected == entry,
        _ => false,
    }
}

fn parse_role_level(value: &str) -> ProjectViewMaintenanceResult<RoleLevel> {
    match value {
        "admin" => Ok(RoleLevel::Admin),
        "member" => Ok(RoleLevel::Member),
        _ => Err(ProjectViewMaintenanceError::Invalid(format!(
            "unknown Role level {value}"
        ))),
    }
}

fn maintenance_epoch_i64(value: u64) -> ProjectViewMaintenanceResult<i64> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(ProjectViewMaintenanceError::Invalid(
            "maintenance_epoch must be JavaScript-safe and positive".to_owned(),
        ));
    }
    i64::try_from(value).map_err(|_| {
        ProjectViewMaintenanceError::Invalid("maintenance_epoch exceeds BIGINT".to_owned())
    })
}

fn checked_next(value: u64, field: &str) -> ProjectViewMaintenanceResult<u64> {
    value
        .checked_add(1)
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| ProjectViewMaintenanceError::Conflict(format!("{field} overflow")))
}

fn revision_i64(value: u64, field: &str) -> ProjectViewMaintenanceResult<i64> {
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(ProjectViewMaintenanceError::Invalid(format!(
            "{field} must be JavaScript-safe and positive"
        )));
    }
    i64::try_from(value)
        .map_err(|_| ProjectViewMaintenanceError::Invalid(format!("{field} exceeds BIGINT")))
}

fn revision_u64(value: i64, field: &str) -> ProjectViewMaintenanceResult<u64> {
    let value = u64::try_from(value).map_err(|_| {
        ProjectViewMaintenanceError::Invalid(format!("stored {field} must be non-negative"))
    })?;
    if !(1..=MAX_SAFE_REVISION).contains(&value) {
        return Err(ProjectViewMaintenanceError::Invalid(format!(
            "stored {field} must be JavaScript-safe and positive"
        )));
    }
    Ok(value)
}

fn bytes32(value: Vec<u8>, field: &str) -> ProjectViewMaintenanceResult<[u8; 32]> {
    value.try_into().map_err(|value: Vec<u8>| {
        ProjectViewMaintenanceError::Invalid(format!(
            "stored {field} must contain 32 bytes, got {}",
            value.len()
        ))
    })
}

fn public_key(value: &[u8], field: &str) -> ProjectViewMaintenanceResult<PublicKey> {
    PublicKey::from_slice(value).map_err(|error| {
        ProjectViewMaintenanceError::Invalid(format!("invalid stored {field}: {error}"))
    })
}

fn event_id(value: [u8; 32], field: &str) -> ProjectViewMaintenanceResult<EventId> {
    EventId::from_slice(&value).map_err(|error| {
        ProjectViewMaintenanceError::Invalid(format!("invalid stored {field}: {error}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Kind, Tag};

    #[test]
    fn recovery_request_hash_binds_epoch_payload_and_signer() {
        let community = CommunityId::from_uuid(
            Uuid::parse_str("0f85e5f0-c7d5-4c30-a0f2-c18478d21001").expect("UUID"),
        );
        let signer =
            PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
                .expect("pubkey");
        let first = recovery_request_hash(REPAIR_REQUEST_DOMAIN, community, 7, &[1; 32], &signer);
        assert_eq!(
            first,
            recovery_request_hash(REPAIR_REQUEST_DOMAIN, community, 7, &[1; 32], &signer,)
        );
        assert_ne!(
            first,
            recovery_request_hash(REPAIR_REQUEST_DOMAIN, community, 8, &[1; 32], &signer,)
        );
        assert_ne!(
            first,
            recovery_request_hash(REPAIR_REQUEST_DOMAIN, community, 7, &[2; 32], &signer,)
        );
    }

    #[test]
    fn membership_restore_hash_binds_every_exact_coordinate() {
        let community = CommunityId::from_uuid(
            Uuid::parse_str("0f85e5f0-c7d5-4c30-a0f2-c18478d21001").expect("UUID"),
        );
        let requester = Keys::generate().public_key();
        let relay = Keys::generate().public_key();
        let old = EventId::from_byte_array([1; 32]);
        let candidate = EventId::from_byte_array([2; 32]);
        let first = membership_snapshot_restore_request_hash(
            community, requester, 69, 7, old, candidate, &relay,
        );
        assert_eq!(
            first,
            membership_snapshot_restore_request_hash(
                community, requester, 69, 7, old, candidate, &relay,
            )
        );
        assert_ne!(
            first,
            membership_snapshot_restore_request_hash(
                community, requester, 70, 7, old, candidate, &relay,
            )
        );
        assert_ne!(
            first,
            membership_snapshot_restore_request_hash(
                community, requester, 69, 7, candidate, old, &relay,
            )
        );
    }

    #[test]
    fn candidate_membership_may_be_semantically_equal_but_not_canonical_order() {
        let relay = Keys::generate();
        let members = vec![
            crate::project_view_v2::V2MembershipEntry {
                pubkey: "11".repeat(32),
                role: "member".to_owned(),
            },
            crate::project_view_v2::V2MembershipEntry {
                pubkey: "22".repeat(32),
                role: "owner".to_owned(),
            },
        ];
        let event = EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST as u16),
            "",
        )
        .tags([
            Tag::parse(["-"]).expect("protected tag"),
            Tag::parse(["member", members[1].pubkey.as_str(), "owner"]).expect("owner tag"),
            Tag::parse(["member", members[0].pubkey.as_str(), "member"]).expect("member tag"),
        ])
        .sign_with_keys(&relay)
        .expect("sign membership");
        let stored = StoredEvent::new(event, None);

        verify_semantically_equal_membership_snapshot(&stored, &relay.public_key(), &members)
            .expect("unordered replacement is semantically equal evidence");
        assert!(
            verify_canonical_membership_snapshot(&stored, &relay.public_key(), &members).is_err()
        );
    }
}
