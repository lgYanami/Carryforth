//! Durable Project View v3 maintenance and greenfield provisioning.
//!
//! Every state-changing path takes the shared Community advisory-lock
//! namespace before reading the mutable maintenance pointer. Historical
//! epochs, operations, baselines, acknowledgements, and provisioning receipts
//! remain durable after the current pointer returns to `normal`.

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::v3::{
    MaintenanceAckCommand, MaintenanceAckRequest, MaintenanceRuntimeAckStatus,
};
use chrono::{DateTime, Utc};
use serde::{Serialize, Serializer};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

const IDEMPOTENCY_DOMAIN: &[u8] = b"buzz-pv3-maintenance-idempotency-v1\0";
const BEGIN_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-begin-request-v1\0";
const OPERATION_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-operation-request-v1\0";
const ACK_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-maintenance-ack-request-v1\0";
const PREPARE_REQUEST_DOMAIN: &[u8] = b"buzz-pv3-prepare-request-v1\0";
const CONTEXT_REQUEST_DOMAIN: &[u8] = b"buzz-project-context-control-request-v1\0";
const PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION: u64 = 1;

/// Stable failures from maintenance, acknowledgement, or provisioning.
#[derive(Debug, thiserror::Error)]
pub enum ProjectViewMaintenanceError {
    /// Database abstraction failure.
    #[error(transparent)]
    Database(#[from] DbError),
    /// Direct SQL failure.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Tamper-evident audit append failure.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// A request violated its closed local or canonical shape.
    #[error("invalid Project View maintenance request: {0}")]
    Invalid(String),
    /// Exact epoch, idempotency, baseline, or state compare-and-set failed.
    #[error("Project View maintenance conflict: {0}")]
    Conflict(String),
    /// The Community cannot perform the requested operation in its current state.
    #[error("Project View maintenance unavailable: {0}")]
    Unavailable(String),
    /// The authenticated actor is not an eligible Human operator/supervisor.
    #[error("Project View maintenance authorization failed: {0}")]
    Forbidden(String),
}

/// Convenient maintenance result.
pub type ProjectViewMaintenanceResult<T> = Result<T, ProjectViewMaintenanceError>;

/// Append an audit-backed invalidation for a security-sensitive Community
/// mutation while the caller holds the exclusive Community lock.
///
/// Schema-v2/v3 changes are always audited. When a maintenance epoch is
/// current, the same audit sequence is also linked from an immutable
/// pre/post-cutover invalidation so freeze/resume cannot overlook a concurrent
/// membership, moderation, or archive decision.
pub(crate) async fn record_security_invalidation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor_pubkey: Option<&[u8]>,
    operation: &str,
    target: Option<String>,
) -> crate::Result<()> {
    if operation.is_empty() || operation.len() > 128 || operation.contains('\0') {
        return Err(DbError::InvalidData(
            "security operation must contain 1..=128 non-NUL bytes".to_owned(),
        ));
    }
    if actor_pubkey.is_some_and(|actor| actor.len() != 32) {
        return Err(DbError::InvalidData(
            "security actor pubkey must contain 32 bytes".to_owned(),
        ));
    }
    let row = sqlx::query(
        "SELECT community.project_view_schema_version, maintenance.current_epoch \
         FROM communities community \
         LEFT JOIN project_view_maintenance maintenance \
           ON maintenance.community_id = community.id \
         WHERE community.id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("community {community_id}")))?;
    let schema_version: i16 = row.try_get("project_view_schema_version")?;
    if !matches!(schema_version, 2 | 3) {
        return Ok(());
    }
    let maintenance_epoch: Option<i64> = row.try_get("current_epoch")?;
    let audit = buzz_audit::append_in_transaction(
        tx,
        NewAuditEntry {
            community_id,
            action: AuditAction::ProjectViewSecurityInvalidation,
            actor_pubkey: actor_pubkey.map(ToOwned::to_owned),
            object_id: target,
            detail: json!({
                "operation": operation,
                "schema_version": schema_version,
                "maintenance_epoch": maintenance_epoch,
            }),
        },
    )
    .await
    .map_err(|error| match error {
        buzz_audit::AuditError::Database(error) => DbError::Sqlx(error),
        other => DbError::InvalidData(format!("append security audit: {other}")),
    })?;
    if let Some(epoch) = maintenance_epoch {
        sqlx::query(
            "INSERT INTO project_view_maintenance_invalidations \
                (community_id, maintenance_epoch, invalidation_id, phase, \
                 source_type, source_audit_seq, invalidated_at) \
             VALUES ($1,$2,$3,$4,'community_audit',$5,clock_timestamp())",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .bind(Uuid::new_v4())
        .bind(if schema_version == 2 {
            "pre_cutover"
        } else {
            "post_cutover"
        })
        .bind(audit.seq)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Durable receipt returned by begin and exact-epoch operator operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectViewMaintenanceReceipt {
    /// Community identity.
    #[serde(serialize_with = "serialize_community_id")]
    pub community_id: CommunityId,
    /// Exact immutable maintenance epoch.
    pub maintenance_epoch: u64,
    /// Stable operation spelling.
    pub operation: String,
    /// Current pointer state after the operation.
    pub state: String,
    /// Whether this response came from an exact durable receipt replay.
    pub replayed: bool,
    /// Complete stored result body.
    pub result: Value,
}

/// Durable receipt returned by one supervisor acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectViewMaintenanceAckReceipt {
    /// Community identity.
    #[serde(serialize_with = "serialize_community_id")]
    pub community_id: CommunityId,
    /// Exact maintenance epoch.
    pub maintenance_epoch: u64,
    /// Assignment or runtime acknowledgement.
    pub ack_type: String,
    /// Stable acknowledgement request identity.
    pub ack_request_id: Uuid,
    /// Whether the exact idempotent request was replayed.
    pub replayed: bool,
    /// Complete stored result body.
    pub result: Value,
}

/// Receipt for disabled empty-state v3 preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectViewV3PreparationReceipt {
    /// Community identity.
    #[serde(serialize_with = "serialize_community_id")]
    pub community_id: CommunityId,
    /// Stable provisioning operation identity consumed by initialization.
    pub operation_id: Uuid,
    /// Target schema version, always three.
    pub target_schema_version: u16,
    /// Whether the exact request was replayed.
    pub replayed: bool,
    /// Complete stored result body.
    pub result: Value,
}

/// Durable receipt for one Project Context capability transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectContextCapabilityReceipt {
    /// Community identity.
    #[serde(serialize_with = "serialize_community_id")]
    pub community_id: CommunityId,
    /// Stable control operation identity.
    pub operation_id: Uuid,
    /// Desired capability state recorded by the request.
    pub enabled: bool,
    /// Deployed Role Brief closure protocol checked by enable.
    pub closure_protocol_version: u64,
    /// Whether the exact durable receipt was replayed.
    pub replayed: bool,
    /// Complete stored result body.
    pub result: Value,
}

impl Db {
    /// Start one immutable schema-2 drain epoch and atomically disable the
    /// ordinary Project View capability.
    #[allow(clippy::too_many_lines)]
    pub async fn begin_project_view_v3_maintenance(
        &self,
        community_id: CommunityId,
        requested_by: PublicKey,
        required_client_protocol_version: u64,
        idempotency_key: &str,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        require_safe_positive(
            required_client_protocol_version,
            "required_client_protocol_version",
        )?;
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash =
            begin_request_hash(community_id, required_client_protocol_version, relay_pubkey);
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;

        if let Some(receipt) =
            replay_begin(&mut tx, community_id, &idempotency_key_hash, &request_hash).await?
        {
            tx.rollback().await?;
            return Ok(receipt);
        }

        let current = sqlx::query(
            "SELECT c.project_view_schema_version, c.project_view_enabled, \
                    c.archived_at, maintenance.state, maintenance.current_epoch, \
                    state.project_revision, state.projection_generation, \
                    state.meta_projection_event_id \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             JOIN project_view_state state ON state.community_id = c.id \
             WHERE c.id = $1 FOR UPDATE OF c, maintenance, state",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Unavailable("Community or state missing".into())
        })?;
        if current.try_get::<i16, _>("project_view_schema_version")? != 2
            || !current.try_get::<bool, _>("project_view_enabled")?
            || current
                .try_get::<Option<DateTime<Utc>>, _>("archived_at")?
                .is_some()
        {
            return Err(ProjectViewMaintenanceError::Unavailable(
                "begin requires one active, enabled schema-2 Community".to_owned(),
            ));
        }
        if current.try_get::<String, _>("state")? != "normal"
            || current
                .try_get::<Option<i64>, _>("current_epoch")?
                .is_some()
        {
            return Err(ProjectViewMaintenanceError::Conflict(
                "another maintenance epoch is already active".to_owned(),
            ));
        }
        if !crate::project_view_v2::project_view_v2_enable_ready_in_tx(
            &mut tx,
            community_id,
            relay_pubkey,
        )
        .await
        .map_err(|error| ProjectViewMaintenanceError::Unavailable(error.to_string()))?
        {
            return Err(ProjectViewMaintenanceError::Unavailable(
                "schema-2 structural/signer readiness failed".to_owned(),
            ));
        }

        let managed_assignment_rows = sqlx::query(
            "SELECT assignment.assignment_id, assignment.member_pubkey \
             FROM project_role_assignments assignment \
             JOIN users actor \
               ON actor.community_id = assignment.community_id \
              AND actor.pubkey = decode(assignment.member_pubkey, 'hex') \
              AND actor.agent_owner_pubkey IS NOT NULL \
             WHERE assignment.community_id = $1 AND assignment.ended_at IS NULL \
             ORDER BY assignment.assignment_id FOR UPDATE OF assignment",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        let mut managed_assignments = Vec::with_capacity(managed_assignment_rows.len());
        for row in managed_assignment_rows {
            let assignment_id: Uuid = row.try_get("assignment_id")?;
            let member_pubkey: String = row.try_get("member_pubkey")?;
            let binding_rows = sqlx::query(
                "SELECT binding_id, supervisor_pubkey \
                 FROM project_runtime_supervisor_bindings \
                 WHERE community_id = $1 AND assignment_id = $2 \
                   AND revoked_at IS NULL AND system_change_id IS NULL \
                 ORDER BY binding_id FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(assignment_id)
            .fetch_all(&mut *tx)
            .await?;
            let [binding] = binding_rows.as_slice() else {
                return Err(ProjectViewMaintenanceError::Conflict(format!(
                    "managed Assignment {assignment_id} does not have exactly one active supervisor binding"
                )));
            };
            managed_assignments.push((
                assignment_id,
                member_pubkey,
                binding.try_get::<Uuid, _>("binding_id")?,
                binding.try_get::<Vec<u8>, _>("supervisor_pubkey")?,
            ));
        }

        let project_revision = db_u64(current.try_get("project_revision")?, "project_revision")?;
        let projection_generation = db_u64(
            current.try_get("projection_generation")?,
            "projection_generation",
        )?;
        let base_meta_event_id = bytes32(
            current.try_get("meta_projection_event_id")?,
            "meta_projection_event_id",
        )?;
        let next_epoch: i64 = sqlx::query_scalar(
            "SELECT COALESCE(max(maintenance_epoch), 0) + 1 \
             FROM project_view_maintenance_epochs WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        let maintenance_epoch = db_u64(next_epoch, "maintenance_epoch")?;
        require_safe_positive(maintenance_epoch, "maintenance_epoch")?;
        let requested_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewMaintenance,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(maintenance_epoch.to_string()),
                detail: json!({
                    "operation": "begin",
                    "maintenance_epoch": maintenance_epoch,
                    "base_project_revision": project_revision,
                    "base_projection_generation": projection_generation,
                    "required_client_protocol_version": required_client_protocol_version,
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let result = json!({
            "community_id": community_id.to_string(),
            "maintenance_epoch": maintenance_epoch,
            "state": "draining",
            "base_meta_event_id": hex::encode(base_meta_event_id),
            "base_project_revision": project_revision,
            "base_projection_generation": projection_generation,
            "required_client_protocol_version": required_client_protocol_version,
            "assignment_baseline_count": managed_assignments.len(),
        });
        sqlx::query(
            "INSERT INTO project_view_maintenance_epochs \
                (community_id, maintenance_epoch, base_meta_event_id, \
                 base_project_revision, base_projection_generation, \
                 required_client_protocol_version, requested_by, requested_at, \
                 begin_audit_seq, begin_idempotency_key_hash, begin_request_hash, \
                 begin_receipt, outcome, completed_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,'active',NULL,$8)",
        )
        .bind(community_id.as_uuid())
        .bind(next_epoch)
        .bind(base_meta_event_id.as_slice())
        .bind(i64::try_from(project_revision).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("project_revision exceeds BIGINT".into())
        })?)
        .bind(i64::try_from(projection_generation).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("projection_generation exceeds BIGINT".into())
        })?)
        .bind(
            i64::try_from(required_client_protocol_version).map_err(|_| {
                ProjectViewMaintenanceError::Invalid("protocol version exceeds BIGINT".into())
            })?,
        )
        .bind(requester.as_slice())
        .bind(requested_at)
        .bind(audit.seq)
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(&result)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE project_view_maintenance SET state = 'draining', \
                    current_epoch = $2, updated_at = $3 WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(next_epoch)
        .bind(requested_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("UPDATE communities SET project_view_enabled = FALSE WHERE id = $1")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;

        for (assignment_id, member_pubkey, binding_id, supervisor_pubkey) in managed_assignments {
            let runtime_count: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM project_runtime_leases \
                 WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL",
            )
            .bind(community_id.as_uuid())
            .bind(binding_id)
            .fetch_one(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO project_view_maintenance_assignment_baselines \
                    (community_id, maintenance_epoch, assignment_id, member_pubkey, \
                     binding_id, supervisor_pubkey, state_at_begin) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7)",
            )
            .bind(community_id.as_uuid())
            .bind(next_epoch)
            .bind(assignment_id)
            .bind(member_pubkey)
            .bind(binding_id)
            .bind(&supervisor_pubkey)
            .bind(if runtime_count == 0 {
                "idle"
            } else {
                "has_runtime"
            })
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO project_view_maintenance_runtime_baselines \
                    (community_id, maintenance_epoch, binding_id, assignment_id, \
                     runtime_id, runtime_epoch, supervisor_pubkey, availability_at_begin) \
                 SELECT community_id, $2, binding_id, assignment_id, runtime_id, \
                        runtime_epoch, $3, availability \
                 FROM project_runtime_leases \
                 WHERE community_id = $1 AND binding_id = $4 AND ended_at IS NULL \
                 ORDER BY runtime_id, runtime_epoch",
            )
            .bind(community_id.as_uuid())
            .bind(next_epoch)
            .bind(&supervisor_pubkey)
            .bind(binding_id)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectViewMaintenanceReceipt {
            community_id,
            maintenance_epoch,
            operation: "begin".to_owned(),
            state: "draining".to_owned(),
            replayed: false,
            result,
        })
    }

    /// Return current or historical maintenance state without deleting old
    /// epochs, receipts, baselines, or acknowledgements.
    pub async fn project_view_maintenance_status(
        &self,
        community_id: CommunityId,
        epoch: Option<u64>,
    ) -> ProjectViewMaintenanceResult<Value> {
        let epoch = epoch
            .map(|value| {
                require_safe_positive(value, "maintenance_epoch")?;
                i64::try_from(value).map_err(|_| {
                    ProjectViewMaintenanceError::Invalid(
                        "maintenance_epoch exceeds BIGINT".to_owned(),
                    )
                })
            })
            .transpose()?;
        maintenance_status_with_executor(&self.pool, community_id, epoch).await
    }

    /// Return bounded fleet protocol diagnostics and the exact durable ACK /
    /// retirement predicate consumed by cutover automation. This is read-only:
    /// polling diagnostics are written only by authenticated supervisors.
    #[allow(clippy::too_many_lines)]
    pub async fn project_view_maintenance_readiness(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        max_poll_age_seconds: u64,
    ) -> ProjectViewMaintenanceResult<Value> {
        require_safe_positive(maintenance_epoch, "maintenance_epoch")?;
        require_safe_positive(max_poll_age_seconds, "max_poll_age_seconds")?;
        if max_poll_age_seconds > 86_400 {
            return Err(ProjectViewMaintenanceError::Invalid(
                "max_poll_age_seconds must be at most 86400".to_owned(),
            ));
        }
        let epoch = i64::try_from(maintenance_epoch).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("maintenance_epoch exceeds BIGINT".to_owned())
        })?;
        let poll_age = i64::try_from(max_poll_age_seconds).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("max_poll_age_seconds exceeds BIGINT".to_owned())
        })?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let coordinate = sqlx::query(
            "SELECT maintenance.state, maintenance.current_epoch, epoch.outcome, \
                    epoch.required_client_protocol_version, clock_timestamp() AS observed_at \
             FROM project_view_maintenance maintenance \
             JOIN project_view_maintenance_epochs epoch \
               ON epoch.community_id = maintenance.community_id \
              AND epoch.maintenance_epoch = $2 \
             WHERE maintenance.community_id = $1 FOR SHARE OF maintenance, epoch",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ProjectViewMaintenanceError::Conflict("maintenance epoch missing".into()))?;
        let state: String = coordinate.try_get("state")?;
        let current_epoch: Option<i64> = coordinate.try_get("current_epoch")?;
        let outcome: String = coordinate.try_get("outcome")?;
        let required_protocol: i64 = coordinate.try_get("required_client_protocol_version")?;
        let observed_at: DateTime<Utc> = coordinate.try_get("observed_at")?;
        let poll_cutoff = observed_at - chrono::Duration::seconds(poll_age);

        let assignment_rows = sqlx::query(
            "SELECT baseline.assignment_id, baseline.binding_id, baseline.member_pubkey, \
                    baseline.state_at_begin, baseline.last_polled_at, \
                    baseline.client_protocol_version, baseline.client_build, \
                    ack.status AS ack_status, ack.acked_at \
             FROM project_view_maintenance_assignment_baselines baseline \
             LEFT JOIN project_view_maintenance_assignment_acks ack \
               ON ack.community_id = baseline.community_id \
              AND ack.maintenance_epoch = baseline.maintenance_epoch \
              AND ack.assignment_id = baseline.assignment_id \
             WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
             ORDER BY baseline.assignment_id",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_all(&mut *tx)
        .await?;
        let mut assignments = Vec::with_capacity(assignment_rows.len());
        let mut protocol_pending = Vec::new();
        let mut poll_pending = Vec::new();
        let mut assignment_ack_pending = Vec::new();
        for row in assignment_rows {
            let assignment_id: Uuid = row.try_get("assignment_id")?;
            let protocol: Option<i64> = row.try_get("client_protocol_version")?;
            let last_polled_at: Option<DateTime<Utc>> = row.try_get("last_polled_at")?;
            let ack_status: Option<String> = row.try_get("ack_status")?;
            let protocol_ready = protocol.is_some_and(|version| version >= required_protocol);
            let poll_recent = ack_status.as_deref() == Some("quiesced")
                || last_polled_at.is_some_and(|polled| polled >= poll_cutoff);
            if !protocol_ready {
                protocol_pending.push(assignment_id);
            }
            if !poll_recent {
                poll_pending.push(assignment_id);
            }
            if ack_status.as_deref() != Some("quiesced") {
                assignment_ack_pending.push(assignment_id);
            }
            assignments.push(json!({
                "assignment_id": assignment_id,
                "binding_id": row.try_get::<Uuid, _>("binding_id")?,
                "member_pubkey": row.try_get::<String, _>("member_pubkey")?,
                "state_at_begin": row.try_get::<String, _>("state_at_begin")?,
                "last_polled_at": last_polled_at,
                "client_protocol_version": protocol
                    .map(|value| db_u64(value, "client_protocol_version"))
                    .transpose()?,
                "client_build": row.try_get::<Option<String>, _>("client_build")?,
                "protocol_ready": protocol_ready,
                "poll_recent": poll_recent,
                "ack_status": ack_status,
                "acked_at": row.try_get::<Option<DateTime<Utc>>, _>("acked_at")?,
            }));
        }

        let runtime_rows = sqlx::query(
            "SELECT baseline.binding_id, baseline.assignment_id, baseline.runtime_id, \
                    baseline.runtime_epoch, baseline.availability_at_begin, \
                    ack.status AS ack_status, lease.availability, lease.ended_at \
             FROM project_view_maintenance_runtime_baselines baseline \
             LEFT JOIN project_view_maintenance_acks ack \
               ON ack.community_id = baseline.community_id \
              AND ack.maintenance_epoch = baseline.maintenance_epoch \
              AND ack.binding_id = baseline.binding_id \
              AND ack.assignment_id = baseline.assignment_id \
              AND ack.runtime_id = baseline.runtime_id \
              AND ack.runtime_epoch = baseline.runtime_epoch \
             LEFT JOIN project_runtime_leases lease \
               ON lease.community_id = baseline.community_id \
              AND lease.binding_id = baseline.binding_id \
              AND lease.assignment_id = baseline.assignment_id \
              AND lease.runtime_id = baseline.runtime_id \
              AND lease.runtime_epoch = baseline.runtime_epoch \
             WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
             ORDER BY baseline.assignment_id, baseline.runtime_id, baseline.runtime_epoch",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_all(&mut *tx)
        .await?;
        let mut runtimes = Vec::with_capacity(runtime_rows.len());
        let mut runtime_ack_pending = Vec::new();
        let mut runtime_live = Vec::new();
        for row in runtime_rows {
            let runtime_id: Uuid = row.try_get("runtime_id")?;
            let runtime_epoch = db_u64(row.try_get("runtime_epoch")?, "runtime_epoch")?;
            let ack_status: Option<String> = row.try_get("ack_status")?;
            let ended_at: Option<DateTime<Utc>> = row.try_get("ended_at")?;
            if ack_status.is_none() {
                runtime_ack_pending.push(json!({
                    "runtime_id": runtime_id,
                    "runtime_epoch": runtime_epoch,
                }));
            }
            if ended_at.is_none() {
                runtime_live.push(json!({
                    "runtime_id": runtime_id,
                    "runtime_epoch": runtime_epoch,
                }));
            }
            runtimes.push(json!({
                "binding_id": row.try_get::<Uuid, _>("binding_id")?,
                "assignment_id": row.try_get::<Uuid, _>("assignment_id")?,
                "runtime_id": runtime_id,
                "runtime_epoch": runtime_epoch,
                "availability_at_begin": row.try_get::<String, _>("availability_at_begin")?,
                "availability": row.try_get::<Option<String>, _>("availability")?,
                "ended_at": ended_at,
                "ack_status": ack_status,
            }));
        }

        let blockers = sqlx::query(
            "SELECT \
                 (SELECT count(*) FROM project_view_maintenance_invalidations \
                  WHERE community_id = $1 AND maintenance_epoch = $2 \
                    AND phase = 'pre_cutover') AS invalidations, \
                 (SELECT count(*) FROM project_runtime_supervisor_bindings binding \
                  JOIN project_view_maintenance_assignment_baselines baseline \
                    ON baseline.community_id = binding.community_id \
                   AND baseline.binding_id = binding.binding_id \
                   AND baseline.maintenance_epoch = $2 \
                  WHERE binding.community_id = $1 \
                    AND binding.scheduler_claim_token IS NOT NULL \
                    AND binding.scheduler_claimed_until > clock_timestamp()) AS claims, \
                 (SELECT count(*) FROM project_runtime_leases runtime \
                  JOIN project_view_maintenance_assignment_baselines assignment \
                    ON assignment.community_id = runtime.community_id \
                   AND assignment.binding_id = runtime.binding_id \
                   AND assignment.maintenance_epoch = $2 \
                  LEFT JOIN project_view_maintenance_runtime_baselines baseline \
                    ON baseline.community_id = runtime.community_id \
                   AND baseline.maintenance_epoch = assignment.maintenance_epoch \
                   AND baseline.binding_id = runtime.binding_id \
                   AND baseline.assignment_id = runtime.assignment_id \
                   AND baseline.runtime_id = runtime.runtime_id \
                   AND baseline.runtime_epoch = runtime.runtime_epoch \
                  WHERE runtime.community_id = $1 AND baseline.runtime_id IS NULL) AS new_runtimes",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_one(&mut *tx)
        .await?;
        let invalidations: i64 = blockers.try_get("invalidations")?;
        let claims: i64 = blockers.try_get("claims")?;
        let new_runtimes: i64 = blockers.try_get("new_runtimes")?;
        let exact_current_epoch = current_epoch == Some(epoch);
        let fleet_protocol_ready = protocol_pending.is_empty();
        let fleet_poll_ready = poll_pending.is_empty();
        let durable_acks_complete =
            assignment_ack_pending.is_empty() && runtime_ack_pending.is_empty();
        let runtime_retirement_complete =
            runtime_live.is_empty() && claims == 0 && new_runtimes == 0;
        let ready_to_freeze = state == "draining"
            && outcome == "active"
            && exact_current_epoch
            && fleet_protocol_ready
            && fleet_poll_ready
            && durable_acks_complete
            && runtime_retirement_complete
            && invalidations == 0;
        tx.commit().await?;
        Ok(json!({
            "community_id": community_id.to_string(),
            "maintenance_epoch": maintenance_epoch,
            "state": state,
            "outcome": outcome,
            "exact_current_epoch": exact_current_epoch,
            "required_client_protocol_version": db_u64(
                required_protocol,
                "required_client_protocol_version",
            )?,
            "observed_at": observed_at,
            "max_poll_age_seconds": max_poll_age_seconds,
            "fleet_protocol_ready": fleet_protocol_ready,
            "fleet_poll_ready": fleet_poll_ready,
            "durable_acks_complete": durable_acks_complete,
            "runtime_retirement_complete": runtime_retirement_complete,
            "ready_to_freeze": ready_to_freeze,
            "protocol_pending_assignment_ids": protocol_pending,
            "poll_pending_assignment_ids": poll_pending,
            "assignment_ack_pending_ids": assignment_ack_pending,
            "runtime_ack_pending": runtime_ack_pending,
            "live_runtimes": runtime_live,
            "scheduler_claim_count": claims,
            "post_begin_runtime_count": new_runtimes,
            "pre_cutover_invalidation_count": invalidations,
            "assignments": assignments,
            "runtimes": runtimes,
        }))
    }

    /// Return only the exact maintenance baselines owned by an authenticated
    /// runtime supervisor. Optional poll diagnostics are updated monotonically
    /// in the same shared-lock transaction; they are never treated as an ACK.
    #[allow(clippy::too_many_lines)]
    pub async fn project_view_maintenance_supervisor_status(
        &self,
        community_id: CommunityId,
        supervisor_pubkey: PublicKey,
        requested_epoch: Option<u64>,
        client_protocol_version: Option<u64>,
        client_build: Option<&str>,
    ) -> ProjectViewMaintenanceResult<Value> {
        if requested_epoch.is_some() {
            require_safe_positive(requested_epoch.unwrap_or_default(), "maintenance_epoch")?;
        }
        match (client_protocol_version, client_build) {
            (Some(version), Some(build)) => {
                require_safe_positive(version, "client_protocol_version")?;
                if build.is_empty() || build.len() > 256 || build.contains('\0') {
                    return Err(ProjectViewMaintenanceError::Invalid(
                        "client_build must contain 1..=256 non-NUL UTF-8 bytes".to_owned(),
                    ));
                }
            }
            (None, None) => {}
            _ => {
                return Err(ProjectViewMaintenanceError::Invalid(
                    "client_protocol_version and client_build must be supplied together".to_owned(),
                ));
            }
        }

        let requested_epoch = requested_epoch
            .map(|value| {
                i64::try_from(value).map_err(|_| {
                    ProjectViewMaintenanceError::Invalid(
                        "maintenance_epoch exceeds BIGINT".to_owned(),
                    )
                })
            })
            .transpose()?;
        let supervisor = supervisor_pubkey.to_bytes();
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let pointer = sqlx::query(
            "SELECT maintenance.state, maintenance.current_epoch, community.host, \
                    community.project_view_schema_version, community.project_view_enabled, \
                    community.archived_at IS NOT NULL AS archived, \
                    (SELECT max(epoch.maintenance_epoch) \
                     FROM project_view_maintenance_epochs epoch \
                     WHERE epoch.community_id = maintenance.community_id) AS latest_epoch \
             FROM project_view_maintenance maintenance \
             JOIN communities community ON community.id = maintenance.community_id \
             WHERE maintenance.community_id = $1 FOR SHARE OF maintenance, community",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ProjectViewMaintenanceError::Unavailable("Community not found".into()))?;
        let current_epoch: Option<i64> = pointer.try_get("current_epoch")?;
        let latest_epoch: Option<i64> = pointer.try_get("latest_epoch")?;
        let epoch = requested_epoch.or(current_epoch).or(latest_epoch);

        if let (Some(epoch), Some(version), Some(build)) =
            (epoch, client_protocol_version, client_build)
        {
            let version = i64::try_from(version).map_err(|_| {
                ProjectViewMaintenanceError::Invalid(
                    "client_protocol_version exceeds BIGINT".to_owned(),
                )
            })?;
            sqlx::query(
                "UPDATE project_view_maintenance_assignment_baselines SET \
                     client_build = CASE \
                         WHEN client_protocol_version IS NULL \
                           OR $4 >= client_protocol_version THEN $5 \
                         ELSE client_build END, \
                     client_protocol_version = GREATEST( \
                         COALESCE(client_protocol_version, 0), $4), \
                     last_polled_at = GREATEST( \
                         COALESCE(last_polled_at, clock_timestamp()), clock_timestamp()) \
                 WHERE community_id = $1 AND maintenance_epoch = $2 \
                   AND supervisor_pubkey = $3",
            )
            .bind(community_id.as_uuid())
            .bind(epoch)
            .bind(supervisor.as_slice())
            .bind(version)
            .bind(build)
            .execute(&mut *tx)
            .await?;
        }

        let epoch_body = if let Some(epoch) = epoch {
            let epoch_row = sqlx::query(
                "SELECT maintenance_epoch, required_client_protocol_version, outcome, \
                        requested_at, completed_at \
                 FROM project_view_maintenance_epochs \
                 WHERE community_id = $1 AND maintenance_epoch = $2",
            )
            .bind(community_id.as_uuid())
            .bind(epoch)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ProjectViewMaintenanceError::Conflict("maintenance epoch missing".into())
            })?;
            let assignment_rows = sqlx::query(
                "SELECT baseline.assignment_id, baseline.member_pubkey, baseline.binding_id, \
                        baseline.state_at_begin, baseline.last_polled_at, \
                        baseline.client_protocol_version, baseline.client_build, \
                        ack.status AS ack_status, ack.acked_at, \
                        request.ack_request_id, request.canonical_request_hash, \
                        request.result_receipt \
                 FROM project_view_maintenance_assignment_baselines baseline \
                 LEFT JOIN project_view_maintenance_assignment_acks ack \
                   ON ack.community_id = baseline.community_id \
                  AND ack.maintenance_epoch = baseline.maintenance_epoch \
                  AND ack.assignment_id = baseline.assignment_id \
                 LEFT JOIN project_view_maintenance_ack_requests request \
                   ON request.community_id = ack.community_id \
                  AND request.maintenance_epoch = ack.maintenance_epoch \
                  AND request.ack_request_id = ack.ack_request_id \
                 WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
                   AND baseline.supervisor_pubkey = $3 \
                 ORDER BY baseline.assignment_id",
            )
            .bind(community_id.as_uuid())
            .bind(epoch)
            .bind(supervisor.as_slice())
            .fetch_all(&mut *tx)
            .await?;
            let mut assignments = Vec::with_capacity(assignment_rows.len());
            for row in assignment_rows {
                assignments.push(json!({
                    "assignment_id": row.try_get::<Uuid, _>("assignment_id")?,
                    "member_pubkey": row.try_get::<String, _>("member_pubkey")?,
                    "binding_id": row.try_get::<Uuid, _>("binding_id")?,
                    "state_at_begin": row.try_get::<String, _>("state_at_begin")?,
                    "last_polled_at": row.try_get::<Option<DateTime<Utc>>, _>("last_polled_at")?,
                    "client_protocol_version": row.try_get::<Option<i64>, _>("client_protocol_version")?
                        .map(|value| db_u64(value, "client_protocol_version"))
                        .transpose()?,
                    "client_build": row.try_get::<Option<String>, _>("client_build")?,
                    "ack": row.try_get::<Option<String>, _>("ack_status")?.map(|status| json!({
                        "status": status,
                        "acked_at": row.try_get::<Option<DateTime<Utc>>, _>("acked_at").ok().flatten(),
                        "ack_request_id": row.try_get::<Option<Uuid>, _>("ack_request_id").ok().flatten(),
                        "canonical_request_hash": row.try_get::<Option<Vec<u8>>, _>("canonical_request_hash")
                            .ok().flatten().map(hex::encode),
                        "receipt": row.try_get::<Option<Value>, _>("result_receipt").ok().flatten(),
                    })),
                }));
            }

            let runtime_rows = sqlx::query(
                "SELECT baseline.binding_id, baseline.assignment_id, baseline.runtime_id, \
                        baseline.runtime_epoch, baseline.availability_at_begin, \
                        ack.status AS ack_status, ack.acked_at, request.ack_request_id, \
                        request.canonical_request_hash, request.result_receipt \
                 FROM project_view_maintenance_runtime_baselines baseline \
                 LEFT JOIN project_view_maintenance_acks ack \
                   ON ack.community_id = baseline.community_id \
                  AND ack.maintenance_epoch = baseline.maintenance_epoch \
                  AND ack.binding_id = baseline.binding_id \
                  AND ack.assignment_id = baseline.assignment_id \
                  AND ack.runtime_id = baseline.runtime_id \
                  AND ack.runtime_epoch = baseline.runtime_epoch \
                 LEFT JOIN project_view_maintenance_ack_requests request \
                   ON request.community_id = ack.community_id \
                  AND request.maintenance_epoch = ack.maintenance_epoch \
                  AND request.ack_request_id = ack.ack_request_id \
                 WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
                   AND baseline.supervisor_pubkey = $3 \
                 ORDER BY baseline.assignment_id, baseline.runtime_id, baseline.runtime_epoch",
            )
            .bind(community_id.as_uuid())
            .bind(epoch)
            .bind(supervisor.as_slice())
            .fetch_all(&mut *tx)
            .await?;
            let mut runtimes = Vec::with_capacity(runtime_rows.len());
            for row in runtime_rows {
                runtimes.push(json!({
                    "binding_id": row.try_get::<Uuid, _>("binding_id")?,
                    "assignment_id": row.try_get::<Uuid, _>("assignment_id")?,
                    "runtime_id": row.try_get::<Uuid, _>("runtime_id")?,
                    "runtime_epoch": db_u64(row.try_get("runtime_epoch")?, "runtime_epoch")?,
                    "availability_at_begin": row.try_get::<String, _>("availability_at_begin")?,
                    "ack": row.try_get::<Option<String>, _>("ack_status")?.map(|status| json!({
                        "status": status,
                        "acked_at": row.try_get::<Option<DateTime<Utc>>, _>("acked_at").ok().flatten(),
                        "ack_request_id": row.try_get::<Option<Uuid>, _>("ack_request_id").ok().flatten(),
                        "canonical_request_hash": row.try_get::<Option<Vec<u8>>, _>("canonical_request_hash")
                            .ok().flatten().map(hex::encode),
                        "receipt": row.try_get::<Option<Value>, _>("result_receipt").ok().flatten(),
                    })),
                }));
            }
            Some(json!({
                "maintenance_epoch": db_u64(epoch_row.try_get("maintenance_epoch")?, "maintenance_epoch")?,
                "required_client_protocol_version": db_u64(
                    epoch_row.try_get("required_client_protocol_version")?,
                    "required_client_protocol_version",
                )?,
                "outcome": epoch_row.try_get::<String, _>("outcome")?,
                "requested_at": epoch_row.try_get::<DateTime<Utc>, _>("requested_at")?,
                "completed_at": epoch_row.try_get::<Option<DateTime<Utc>>, _>("completed_at")?,
                "assignments": assignments,
                "runtimes": runtimes,
            }))
        } else {
            None
        };
        tx.commit().await?;
        Ok(json!({
            "community_id": community_id.to_string(),
            "host": pointer.try_get::<String, _>("host")?,
            "state": pointer.try_get::<String, _>("state")?,
            "current_epoch": current_epoch.map(|value| db_u64(value, "current_epoch")).transpose()?,
            "latest_epoch": latest_epoch.map(|value| db_u64(value, "latest_epoch")).transpose()?,
            "project_view_schema_version": pointer.try_get::<i16, _>("project_view_schema_version")?,
            "project_view_enabled": pointer.try_get::<bool, _>("project_view_enabled")?,
            "archived": pointer.try_get::<bool, _>("archived")?,
            "poll_after_seconds": 5,
            "epoch": epoch_body,
        }))
    }

    /// Record one exact baseline acknowledgement from its registered
    /// supervisor. Runtime retirement and the immutable receipt commit in one
    /// shared-lock transaction.
    #[allow(clippy::too_many_lines)]
    pub async fn acknowledge_project_view_maintenance(
        &self,
        community_id: CommunityId,
        supervisor_pubkey: PublicKey,
        auth_event_id: [u8; 32],
        command: &MaintenanceAckCommand,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceAckReceipt> {
        command
            .validate()
            .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
        let maintenance_epoch = command.maintenance_epoch();
        let epoch = i64::try_from(maintenance_epoch).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("maintenance_epoch exceeds BIGINT".into())
        })?;
        let idempotency_key_hash = maintenance_ack_idempotency_hash(command.idempotency_key());
        let request_hash = maintenance_ack_request_hash(community_id, command)?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let pointer = sqlx::query(
            "SELECT state, current_epoch FROM project_view_maintenance \
             WHERE community_id = $1 FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Unavailable("maintenance pointer missing".into())
        })?;
        if pointer.try_get::<String, _>("state")? != "draining"
            || pointer.try_get::<Option<i64>, _>("current_epoch")? != Some(epoch)
        {
            return Err(ProjectViewMaintenanceError::Conflict(
                "acknowledgement does not name the active draining epoch".to_owned(),
            ));
        }
        let supervisor = supervisor_pubkey.to_bytes();
        if let Some(row) = sqlx::query(
            "SELECT ack_request_id, agent_pubkey, ack_type, canonical_request_hash, \
                    result_receipt FROM project_view_maintenance_ack_requests \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            if bytes32(
                row.try_get("canonical_request_hash")?,
                "canonical_request_hash",
            )? != request_hash
                || row.try_get::<Vec<u8>, _>("agent_pubkey")?.as_slice() != supervisor.as_slice()
                || row.try_get::<String, _>("ack_type")? != command.ack_type()
            {
                return Err(ProjectViewMaintenanceError::Conflict(
                    "maintenance acknowledgement key was reused for another request".to_owned(),
                ));
            }
            let result: Value = row.try_get("result_receipt")?;
            let ack_request_id: Uuid = row.try_get("ack_request_id")?;
            tx.rollback().await?;
            return Ok(ProjectViewMaintenanceAckReceipt {
                community_id,
                maintenance_epoch,
                ack_type: command.ack_type().to_owned(),
                ack_request_id,
                replayed: true,
                result,
            });
        }

        let ack_request_id = Uuid::new_v4();
        let accepted_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let result = match &command.request {
            MaintenanceAckRequest::AssignmentQuiesced {
                binding_id,
                assignment_id,
                client_protocol_version,
                client_build,
                ..
            } => {
                acknowledge_assignment(
                    &mut tx,
                    community_id,
                    epoch,
                    ack_request_id,
                    *binding_id,
                    *assignment_id,
                    supervisor.as_slice(),
                    *client_protocol_version,
                    client_build,
                    accepted_at,
                )
                .await?
            }
            MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                binding_id,
                assignment_id,
                runtime_id,
                runtime_epoch,
                status,
                ..
            } => {
                acknowledge_runtime(
                    &mut tx,
                    community_id,
                    epoch,
                    ack_request_id,
                    *binding_id,
                    *assignment_id,
                    *runtime_id,
                    *runtime_epoch,
                    *status,
                    supervisor.as_slice(),
                    accepted_at,
                )
                .await?
            }
        };
        sqlx::query(
            "INSERT INTO project_view_maintenance_ack_requests \
                (community_id, maintenance_epoch, ack_request_id, agent_pubkey, \
                 ack_type, idempotency_key_hash, canonical_request_hash, \
                 auth_event_id, result_receipt, accepted_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .bind(ack_request_id)
        .bind(supervisor.as_slice())
        .bind(command.ack_type())
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(auth_event_id.as_slice())
        .bind(&result)
        .bind(accepted_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectViewMaintenanceAckReceipt {
            community_id,
            maintenance_epoch,
            ack_type: command.ack_type().to_owned(),
            ack_request_id,
            replayed: false,
            result,
        })
    }

    /// Freeze an exact drained epoch after every immutable Assignment and
    /// Runtime baseline has a compatible durable acknowledgement.
    pub async fn freeze_project_view_v3_maintenance(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        self.project_view_maintenance_operation(
            community_id,
            maintenance_epoch,
            requested_by,
            idempotency_key,
            "freeze",
            None,
        )
        .await
    }

    /// Abort one pre-cutover drain/freeze and fence every baseline runtime.
    pub async fn abort_project_view_v3_maintenance(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        self.project_view_maintenance_operation(
            community_id,
            maintenance_epoch,
            requested_by,
            idempotency_key,
            "abort",
            Some(relay_pubkey),
        )
        .await
    }

    /// Record a structural v3 verification and resolve all prior post-cutover
    /// invalidations to the exact verified metadata coordinate.
    pub async fn verify_project_view_v3_maintenance(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        self.project_view_maintenance_operation(
            community_id,
            maintenance_epoch,
            requested_by,
            idempotency_key,
            "verify",
            Some(relay_pubkey),
        )
        .await
    }

    /// Resume only a committed and reverified v3 epoch. Archived Communities
    /// return to `normal` while remaining capability-disabled.
    pub async fn resume_project_view_v3_maintenance(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        relay_pubkey: &PublicKey,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        self.project_view_maintenance_operation(
            community_id,
            maintenance_epoch,
            requested_by,
            idempotency_key,
            "resume",
            Some(relay_pubkey),
        )
        .await
    }

    /// Prepare one completely empty, disabled Community for owner-signed v3
    /// initialization without creating Project View state implicitly.
    pub async fn prepare_project_view_v3(
        &self,
        community_id: CommunityId,
        requested_by: PublicKey,
        idempotency_key: &str,
    ) -> ProjectViewMaintenanceResult<ProjectViewV3PreparationReceipt> {
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = prepare_request_hash(community_id);
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;
        if let Some(row) = sqlx::query(
            "SELECT operation_id, canonical_request_hash, result_receipt \
             FROM project_view_provisioning_operations \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            if bytes32(
                row.try_get("canonical_request_hash")?,
                "canonical_request_hash",
            )? != request_hash
            {
                return Err(ProjectViewMaintenanceError::Conflict(
                    "prepare-v3 key was reused for another request".to_owned(),
                ));
            }
            let operation_id: Uuid = row.try_get("operation_id")?;
            let result: Value = row.try_get("result_receipt")?;
            tx.rollback().await?;
            return Ok(ProjectViewV3PreparationReceipt {
                community_id,
                operation_id,
                target_schema_version: 3,
                replayed: true,
                result,
            });
        }
        let eligible: Option<bool> = sqlx::query_scalar(
            "SELECT c.archived_at IS NULL AND NOT c.project_view_enabled \
                    AND NOT c.project_context_enabled \
                    AND c.project_view_preparation_operation_id IS NULL \
                    AND maintenance.state = 'normal' \
                    AND NOT EXISTS (SELECT 1 FROM project_view_state WHERE community_id = c.id) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_objects WHERE community_id = c.id) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_mutations WHERE community_id = c.id) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_changes WHERE community_id = c.id) \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             WHERE c.id = $1 FOR UPDATE OF c, maintenance",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if eligible != Some(true) {
            return Err(ProjectViewMaintenanceError::Unavailable(
                "prepare-v3 requires a non-archived, disabled, completely uninitialized Community"
                    .to_owned(),
            ));
        }
        let operation_id = Uuid::new_v4();
        let accepted_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewProvisioning,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(operation_id.to_string()),
                detail: json!({
                    "operation": "prepare_v3",
                    "target_schema_version": 3,
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let result = json!({
            "community_id": community_id.to_string(),
            "operation_id": operation_id,
            "operation": "prepare_v3",
            "target_schema_version": 3,
            "prepared": true,
        });
        sqlx::query(
            "INSERT INTO project_view_provisioning_operations \
                (community_id, operation_id, operation, target_schema_version, \
                 idempotency_key_hash, canonical_request_hash, requested_by, \
                 audit_seq, result_receipt, accepted_at) \
             VALUES ($1,$2,'prepare_v3',3,$3,$4,$5,$6,$7,$8)",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(requester.as_slice())
        .bind(audit.seq)
        .bind(&result)
        .bind(accepted_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE communities SET project_view_schema_version = 3, \
                    project_view_preparation_operation_id = $2, \
                    project_view_enabled = FALSE, project_context_enabled = FALSE \
             WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectViewV3PreparationReceipt {
            community_id,
            operation_id,
            target_schema_version: 3,
            replayed: false,
            result,
        })
    }

    /// Atomically enable or disable the staged Project Context capability.
    ///
    /// Exact idempotency receipts are replayed before evaluating mutable
    /// readiness. Enable holds the exclusive Community lock while validating
    /// Project View v3 structure, normalized Context parity, Project Document
    /// signer/projection parity, and the deployed closure protocol. Disable is
    /// fail-closed and preserves all canonical references.
    #[allow(clippy::too_many_lines)]
    pub async fn set_project_context_enabled_checked(
        &self,
        community_id: CommunityId,
        enabled: bool,
        requested_by: PublicKey,
        idempotency_key: &str,
    ) -> ProjectViewMaintenanceResult<ProjectContextCapabilityReceipt> {
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = context_request_hash(community_id, enabled);
        let operation = if enabled { "enable" } else { "disable" };
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;

        if let Some(row) = sqlx::query(
            "SELECT operation_id, canonical_request_hash, result_receipt \
             FROM project_view_context_operations \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            if bytes32(
                row.try_get("canonical_request_hash")?,
                "canonical_request_hash",
            )? != request_hash
            {
                return Err(ProjectViewMaintenanceError::Conflict(
                    "Context capability key was reused for another request".to_owned(),
                ));
            }
            let operation_id: Uuid = row.try_get("operation_id")?;
            let result: Value = row.try_get("result_receipt")?;
            tx.rollback().await?;
            return Ok(ProjectContextCapabilityReceipt {
                community_id,
                operation_id,
                enabled,
                closure_protocol_version: PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION,
                replayed: true,
                result,
            });
        }

        let pointer = sqlx::query(
            "SELECT community.archived_at IS NULL AS active, \
                    community.project_view_schema_version, \
                    community.project_view_enabled, community.project_context_enabled, \
                    community.project_document_enabled, maintenance.state \
             FROM communities community \
             JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = community.id \
             WHERE community.id = $1 FOR UPDATE OF community, maintenance",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ProjectViewMaintenanceError::Unavailable("Community does not exist".to_owned())
        })?;
        let current_enabled: bool = pointer.try_get("project_context_enabled")?;

        if enabled {
            let ready = pointer.try_get::<bool, _>("active")?
                && pointer.try_get::<i16, _>("project_view_schema_version")? == 3
                && pointer.try_get::<bool, _>("project_view_enabled")?
                && pointer.try_get::<bool, _>("project_document_enabled")?
                && pointer.try_get::<String, _>("state")? == "normal";
            if !ready {
                return Err(ProjectViewMaintenanceError::Unavailable(
                    "Context enable requires an active, normal, Project View v3 Community with Project View and Document capabilities enabled"
                        .to_owned(),
                ));
            }
            let project_view_pubkey: Vec<u8> = sqlx::query_scalar(
                "SELECT projection_pubkey FROM project_view_state \
                 WHERE community_id = $1 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ProjectViewMaintenanceError::Unavailable(
                    "Project View v3 state is missing".to_owned(),
                )
            })?;
            let relay_pubkey = PublicKey::from_slice(&project_view_pubkey).map_err(|error| {
                ProjectViewMaintenanceError::Invalid(format!(
                    "stored Project View projection_pubkey is invalid: {error}"
                ))
            })?;
            if !Self::project_view_v3_structural_ready_in_tx(&mut tx, community_id, &relay_pubkey)
                .await?
            {
                return Err(ProjectViewMaintenanceError::Unavailable(
                    "Project View v3 canonical, normalized Context, or projection parity is not ready"
                        .to_owned(),
                ));
            }

            let document = sqlx::query(
                "SELECT projection_pubkey, meta_projection_event_id, active_document_count \
                 FROM project_document_state WHERE community_id = $1 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| {
                ProjectViewMaintenanceError::Unavailable(
                    "Project Document canonical state is missing".to_owned(),
                )
            })?;
            let document_pubkey: Vec<u8> = document.try_get("projection_pubkey")?;
            if document_pubkey != project_view_pubkey {
                return Err(ProjectViewMaintenanceError::Unavailable(
                    "Project View and Document stable projection signers differ".to_owned(),
                ));
            }
            sqlx::query("SELECT project_document_validate_community($1)")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
            let meta_event_id: Vec<u8> = document.try_get("meta_projection_event_id")?;
            let active_count: i64 = document.try_get("active_document_count")?;
            if !crate::project_document::document_projection_parity(
                &mut tx,
                community_id,
                &relay_pubkey,
                Some(&meta_event_id),
                Some(active_count),
            )
            .await?
            {
                return Err(ProjectViewMaintenanceError::Unavailable(
                    "Project Document canonical/projection parity is not ready".to_owned(),
                ));
            }
        }

        let operation_id = Uuid::new_v4();
        let accepted_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let counts = sqlx::query(
            "SELECT \
                (SELECT count(*)::bigint FROM project_view_resource_context_references \
                 WHERE community_id = $1) AS resource_references, \
                (SELECT count(*)::bigint FROM project_view_document_context_references \
                 WHERE community_id = $1) AS document_references",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        let changed = current_enabled != enabled;
        sqlx::query("UPDATE communities SET project_context_enabled = $2 WHERE id = $1")
            .bind(community_id.as_uuid())
            .bind(enabled)
            .execute(&mut *tx)
            .await?;
        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectContextControl,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(operation_id.to_string()),
                detail: json!({
                    "operation": operation,
                    "enabled": enabled,
                    "changed": changed,
                    "closure_protocol_version": PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION,
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let result = json!({
            "community_id": community_id.to_string(),
            "operation_id": operation_id,
            "operation": operation,
            "enabled": enabled,
            "changed": changed,
            "preserved_resource_reference_count": counts.try_get::<i64, _>("resource_references")?,
            "preserved_document_reference_count": counts.try_get::<i64, _>("document_references")?,
            "closure_protocol_version": PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION,
        });
        sqlx::query(
            "INSERT INTO project_view_context_operations \
                (community_id, operation_id, operation, idempotency_key_hash, \
                 canonical_request_hash, requested_by, closure_protocol_version, \
                 audit_seq, result_receipt, accepted_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .bind(operation)
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(requester.as_slice())
        .bind(
            i64::try_from(PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION).map_err(|_| {
                ProjectViewMaintenanceError::Invalid(
                    "closure protocol version exceeds BIGINT".to_owned(),
                )
            })?,
        )
        .bind(audit.seq)
        .bind(&result)
        .bind(accepted_at)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectContextCapabilityReceipt {
            community_id,
            operation_id,
            enabled,
            closure_protocol_version: PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION,
            replayed: false,
            result,
        })
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn project_view_maintenance_operation(
        &self,
        community_id: CommunityId,
        maintenance_epoch: u64,
        requested_by: PublicKey,
        idempotency_key: &str,
        operation: &'static str,
        relay_pubkey: Option<&PublicKey>,
    ) -> ProjectViewMaintenanceResult<ProjectViewMaintenanceReceipt> {
        require_safe_positive(maintenance_epoch, "maintenance_epoch")?;
        if !matches!(operation, "freeze" | "abort" | "verify" | "resume") {
            return Err(ProjectViewMaintenanceError::Invalid(
                "unsupported maintenance operation".to_owned(),
            ));
        }
        let epoch = i64::try_from(maintenance_epoch).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("maintenance_epoch exceeds BIGINT".into())
        })?;
        let idempotency_key_hash = idempotency_hash(idempotency_key)?;
        let request_hash = operation_request_hash(community_id, maintenance_epoch, operation);
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_human_operator_in_tx(&mut tx, community_id, requested_by).await?;
        if let Some(receipt) = replay_operation(
            &mut tx,
            community_id,
            epoch,
            operation,
            &idempotency_key_hash,
            &request_hash,
        )
        .await?
        {
            tx.rollback().await?;
            return Ok(receipt);
        }
        let pointer = sqlx::query(
            "SELECT maintenance.state, maintenance.current_epoch, \
                    community.project_view_schema_version, community.archived_at, \
                    epoch.outcome \
             FROM project_view_maintenance maintenance \
             JOIN communities community ON community.id = maintenance.community_id \
             JOIN project_view_maintenance_epochs epoch \
               ON epoch.community_id = maintenance.community_id \
              AND epoch.maintenance_epoch = $2 \
             WHERE maintenance.community_id = $1 \
             FOR UPDATE OF maintenance, community, epoch",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ProjectViewMaintenanceError::Conflict("maintenance epoch missing".into()))?;
        let current_state: String = pointer.try_get("state")?;
        if pointer.try_get::<Option<i64>, _>("current_epoch")? != Some(epoch) {
            return Err(ProjectViewMaintenanceError::Conflict(
                "operation does not name the current maintenance epoch".to_owned(),
            ));
        }
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let operation_id = Uuid::new_v4();
        let next_state = match operation {
            "freeze" => {
                if current_state != "draining"
                    || pointer.try_get::<i16, _>("project_view_schema_version")? != 2
                {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "freeze requires the exact active schema-2 draining epoch".to_owned(),
                    ));
                }
                validate_freeze_in_tx(&mut tx, community_id, epoch).await?;
                sqlx::query(
                    "UPDATE project_view_maintenance SET state = 'frozen', updated_at = $3 \
                     WHERE community_id = $1 AND current_epoch = $2 AND state = 'draining'",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                "frozen"
            }
            "abort" => {
                if !matches!(current_state.as_str(), "draining" | "frozen")
                    || pointer.try_get::<i16, _>("project_view_schema_version")? != 2
                {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "abort requires an active pre-cutover schema-2 epoch".to_owned(),
                    ));
                }
                let cutover_exists: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM project_view_v3_cutovers \
                     WHERE community_id = $1 AND maintenance_epoch = $2)",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .fetch_one(&mut *tx)
                .await?;
                if cutover_exists {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "a committed v3 cutover cannot be aborted".to_owned(),
                    ));
                }
                fence_epoch_runtimes_in_tx(&mut tx, community_id, epoch, now).await?;
                let ready = if let Some(relay_pubkey) = relay_pubkey {
                    crate::project_view_v2::project_view_v2_enable_ready_in_tx(
                        &mut tx,
                        community_id,
                        relay_pubkey,
                    )
                    .await
                    .map_err(|error| ProjectViewMaintenanceError::Unavailable(error.to_string()))?
                } else {
                    false
                };
                sqlx::query(
                    "UPDATE communities SET project_view_enabled = \
                         (archived_at IS NULL AND $2) WHERE id = $1",
                )
                .bind(community_id.as_uuid())
                .bind(ready)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE project_view_maintenance_epochs SET outcome = 'aborted', \
                         completed_at = $3, updated_at = $3 \
                     WHERE community_id = $1 AND maintenance_epoch = $2 AND outcome = 'active'",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE project_view_maintenance SET state = 'normal', \
                         current_epoch = NULL, updated_at = $3 \
                     WHERE community_id = $1 AND current_epoch = $2",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                "normal"
            }
            "verify" => {
                if current_state != "frozen"
                    || pointer.try_get::<i16, _>("project_view_schema_version")? != 3
                {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "verify requires an exact frozen schema-3 epoch".to_owned(),
                    ));
                }
                let relay_pubkey = relay_pubkey.ok_or_else(|| {
                    ProjectViewMaintenanceError::Invalid(
                        "verify requires the stable Relay signer".to_owned(),
                    )
                })?;
                let coordinate =
                    validate_v3_structural_in_tx(&mut tx, community_id, relay_pubkey).await?;
                resolve_post_cutover_invalidations_in_tx(
                    &mut tx,
                    community_id,
                    epoch,
                    operation_id,
                    &coordinate,
                )
                .await?;
                "frozen"
            }
            "resume" => {
                if current_state != "frozen"
                    || pointer.try_get::<i16, _>("project_view_schema_version")? != 3
                    || pointer.try_get::<String, _>("outcome")? != "cutover_committed"
                {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "resume requires the exact committed frozen v3 epoch".to_owned(),
                    ));
                }
                let relay_pubkey = relay_pubkey.ok_or_else(|| {
                    ProjectViewMaintenanceError::Invalid(
                        "resume requires the stable Relay signer".to_owned(),
                    )
                })?;
                validate_v3_structural_in_tx(&mut tx, community_id, relay_pubkey).await?;
                let unresolved: bool = sqlx::query_scalar(
                    "SELECT EXISTS (SELECT 1 FROM project_view_maintenance_invalidations \
                     WHERE community_id = $1 AND maintenance_epoch = $2 \
                       AND phase = 'post_cutover' AND resolved_by_operation_id IS NULL)",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .fetch_one(&mut *tx)
                .await?;
                if unresolved {
                    return Err(ProjectViewMaintenanceError::Conflict(
                        "post-cutover security invalidations remain unresolved".to_owned(),
                    ));
                }
                sqlx::query(
                    "UPDATE communities SET project_view_enabled = (archived_at IS NULL) \
                     WHERE id = $1",
                )
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE project_view_maintenance_epochs SET outcome = 'resumed', \
                         completed_at = $3, updated_at = $3 \
                     WHERE community_id = $1 AND maintenance_epoch = $2 \
                       AND outcome = 'cutover_committed'",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                sqlx::query(
                    "UPDATE project_view_maintenance SET state = 'normal', \
                         current_epoch = NULL, updated_at = $3 \
                     WHERE community_id = $1 AND current_epoch = $2",
                )
                .bind(community_id.as_uuid())
                .bind(epoch)
                .bind(now)
                .execute(&mut *tx)
                .await?;
                "normal"
            }
            _ => unreachable!("operation checked above"),
        };
        let requester = requested_by.to_bytes();
        let audit = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::ProjectViewMaintenance,
                actor_pubkey: Some(requester.to_vec()),
                object_id: Some(maintenance_epoch.to_string()),
                detail: json!({
                    "operation": operation,
                    "maintenance_epoch": maintenance_epoch,
                    "idempotency_key_hash": hex::encode(idempotency_key_hash),
                }),
            },
        )
        .await?;
        let result = json!({
            "community_id": community_id.to_string(),
            "maintenance_epoch": maintenance_epoch,
            "operation": operation,
            "state": next_state,
        });
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
        .bind(requester.as_slice())
        .bind(audit.seq)
        .bind(&result)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(ProjectViewMaintenanceReceipt {
            community_id,
            maintenance_epoch,
            operation: operation.to_owned(),
            state: next_state.to_owned(),
            replayed: false,
            result,
        })
    }
}

async fn replay_begin(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    idempotency_key_hash: &[u8; 32],
    request_hash: &[u8; 32],
) -> ProjectViewMaintenanceResult<Option<ProjectViewMaintenanceReceipt>> {
    let row = sqlx::query(
        "SELECT maintenance_epoch, begin_request_hash, begin_receipt, outcome \
         FROM project_view_maintenance_epochs \
         WHERE community_id = $1 AND begin_idempotency_key_hash = $2",
    )
    .bind(community_id.as_uuid())
    .bind(idempotency_key_hash.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if bytes32(row.try_get("begin_request_hash")?, "begin_request_hash")? != *request_hash {
        return Err(ProjectViewMaintenanceError::Conflict(
            "maintenance begin key was reused for another request".to_owned(),
        ));
    }
    let epoch = db_u64(row.try_get("maintenance_epoch")?, "maintenance_epoch")?;
    let outcome: String = row.try_get("outcome")?;
    Ok(Some(ProjectViewMaintenanceReceipt {
        community_id,
        maintenance_epoch: epoch,
        operation: "begin".to_owned(),
        state: if outcome == "active" {
            "draining".to_owned()
        } else {
            outcome
        },
        replayed: true,
        result: row.try_get("begin_receipt")?,
    }))
}

pub(crate) async fn replay_operation(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    operation: &str,
    idempotency_key_hash: &[u8; 32],
    request_hash: &[u8; 32],
) -> ProjectViewMaintenanceResult<Option<ProjectViewMaintenanceReceipt>> {
    let row = sqlx::query(
        "SELECT maintenance_epoch, operation, canonical_request_hash, result_receipt \
         FROM project_view_maintenance_operations \
         WHERE community_id = $1 AND idempotency_key_hash = $2",
    )
    .bind(community_id.as_uuid())
    .bind(idempotency_key_hash.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.try_get::<i64, _>("maintenance_epoch")? != epoch
        || row.try_get::<String, _>("operation")? != operation
        || bytes32(
            row.try_get("canonical_request_hash")?,
            "canonical_request_hash",
        )? != *request_hash
    {
        return Err(ProjectViewMaintenanceError::Conflict(
            "maintenance operation key was reused for another request".to_owned(),
        ));
    }
    let result: Value = row.try_get("result_receipt")?;
    let state = result
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    Ok(Some(ProjectViewMaintenanceReceipt {
        community_id,
        maintenance_epoch: db_u64(epoch, "maintenance_epoch")?,
        operation: operation.to_owned(),
        state,
        replayed: true,
        result,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn acknowledge_assignment(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    ack_request_id: Uuid,
    binding_id: Uuid,
    assignment_id: Uuid,
    supervisor_pubkey: &[u8],
    client_protocol_version: u64,
    client_build: &str,
    now: DateTime<Utc>,
) -> ProjectViewMaintenanceResult<Value> {
    let baseline = sqlx::query(
        "SELECT baseline.member_pubkey, baseline.supervisor_pubkey, \
                epoch.required_client_protocol_version \
         FROM project_view_maintenance_assignment_baselines baseline \
         JOIN project_view_maintenance_epochs epoch \
           ON epoch.community_id = baseline.community_id \
          AND epoch.maintenance_epoch = baseline.maintenance_epoch \
         WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
           AND baseline.binding_id = $3 AND baseline.assignment_id = $4 \
         FOR UPDATE OF baseline",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(binding_id)
    .bind(assignment_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewMaintenanceError::Forbidden("Assignment baseline not owned".into())
    })?;
    if baseline
        .try_get::<Vec<u8>, _>("supervisor_pubkey")?
        .as_slice()
        != supervisor_pubkey
    {
        return Err(ProjectViewMaintenanceError::Forbidden(
            "authenticated supervisor does not own this Assignment baseline".to_owned(),
        ));
    }
    let required = db_u64(
        baseline.try_get("required_client_protocol_version")?,
        "required_client_protocol_version",
    )?;
    if client_protocol_version < required {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "client protocol {client_protocol_version} is below required {required}"
        )));
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_view_maintenance_assignment_acks \
         WHERE community_id = $1 AND maintenance_epoch = $2 AND assignment_id = $3)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(assignment_id)
    .fetch_one(&mut **tx)
    .await?;
    if duplicate {
        return Err(ProjectViewMaintenanceError::Conflict(
            "Assignment baseline was already acknowledged with another key".to_owned(),
        ));
    }
    let runtime_pending: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_view_maintenance_runtime_baselines baseline \
             LEFT JOIN project_view_maintenance_acks ack \
               ON ack.community_id = baseline.community_id \
              AND ack.maintenance_epoch = baseline.maintenance_epoch \
              AND ack.binding_id = baseline.binding_id \
              AND ack.assignment_id = baseline.assignment_id \
              AND ack.runtime_id = baseline.runtime_id \
              AND ack.runtime_epoch = baseline.runtime_epoch \
             WHERE baseline.community_id = $1 AND baseline.maintenance_epoch = $2 \
               AND baseline.binding_id = $3 AND baseline.assignment_id = $4 \
               AND ack.ack_request_id IS NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(binding_id)
    .bind(assignment_id)
    .fetch_one(&mut **tx)
    .await?;
    let runtime_live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_runtime_leases \
         WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL)",
    )
    .bind(community_id.as_uuid())
    .bind(binding_id)
    .fetch_one(&mut **tx)
    .await?;
    let claim_live: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_runtime_supervisor_bindings \
         WHERE community_id = $1 AND binding_id = $2 \
           AND scheduler_claim_token IS NOT NULL \
           AND scheduler_claimed_until > clock_timestamp())",
    )
    .bind(community_id.as_uuid())
    .bind(binding_id)
    .fetch_one(&mut **tx)
    .await?;
    if runtime_pending || runtime_live || claim_live {
        return Err(ProjectViewMaintenanceError::Conflict(
            "Assignment still has a pending Runtime acknowledgement, live lease, or scheduler claim"
                .to_owned(),
        ));
    }
    let member_pubkey: String = baseline.try_get("member_pubkey")?;
    let protocol = i64::try_from(client_protocol_version).map_err(|_| {
        ProjectViewMaintenanceError::Invalid("client protocol exceeds BIGINT".into())
    })?;
    sqlx::query(
        "INSERT INTO project_view_maintenance_assignment_acks \
            (community_id, maintenance_epoch, ack_request_id, binding_id, \
             assignment_id, member_pubkey, supervisor_pubkey, status, \
             client_protocol_version, client_build, acked_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,'quiesced',$8,$9,$10)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(ack_request_id)
    .bind(binding_id)
    .bind(assignment_id)
    .bind(&member_pubkey)
    .bind(supervisor_pubkey)
    .bind(protocol)
    .bind(client_build)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(json!({
        "maintenance_epoch": db_u64(epoch, "maintenance_epoch")?,
        "type": "assignment_quiesced",
        "binding_id": binding_id,
        "assignment_id": assignment_id,
        "status": "quiesced",
        "client_protocol_version": client_protocol_version,
        "client_build": client_build,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn acknowledge_runtime(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    ack_request_id: Uuid,
    binding_id: Uuid,
    assignment_id: Uuid,
    runtime_id: Uuid,
    runtime_epoch: u64,
    status: MaintenanceRuntimeAckStatus,
    supervisor_pubkey: &[u8],
    now: DateTime<Utc>,
) -> ProjectViewMaintenanceResult<Value> {
    let runtime_epoch_db = i64::try_from(runtime_epoch)
        .map_err(|_| ProjectViewMaintenanceError::Invalid("runtime_epoch exceeds BIGINT".into()))?;
    let baseline: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT supervisor_pubkey FROM project_view_maintenance_runtime_baselines \
         WHERE community_id = $1 AND maintenance_epoch = $2 AND binding_id = $3 \
           AND assignment_id = $4 AND runtime_id = $5 AND runtime_epoch = $6 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(binding_id)
    .bind(assignment_id)
    .bind(runtime_id)
    .bind(runtime_epoch_db)
    .fetch_optional(&mut **tx)
    .await?;
    if baseline.as_deref() != Some(supervisor_pubkey) {
        return Err(ProjectViewMaintenanceError::Forbidden(
            "authenticated supervisor does not own this Runtime baseline".to_owned(),
        ));
    }
    let duplicate: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_view_maintenance_acks \
         WHERE community_id = $1 AND maintenance_epoch = $2 AND binding_id = $3 \
           AND assignment_id = $4 AND runtime_id = $5 AND runtime_epoch = $6)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(binding_id)
    .bind(assignment_id)
    .bind(runtime_id)
    .bind(runtime_epoch_db)
    .fetch_one(&mut **tx)
    .await?;
    if duplicate {
        return Err(ProjectViewMaintenanceError::Conflict(
            "Runtime baseline was already acknowledged with another key".to_owned(),
        ));
    }
    let lease = sqlx::query(
        "SELECT availability, ended_at FROM project_runtime_leases \
         WHERE community_id = $1 AND binding_id = $2 AND assignment_id = $3 \
           AND runtime_id = $4 AND runtime_epoch = $5 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(binding_id)
    .bind(assignment_id)
    .bind(runtime_id)
    .bind(runtime_epoch_db)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewMaintenanceError::Conflict("baseline Runtime lease missing".into())
    })?;
    let availability: String = lease.try_get("availability")?;
    let ended_at: Option<DateTime<Utc>> = lease.try_get("ended_at")?;
    match status {
        MaintenanceRuntimeAckStatus::Suspended => {
            if ended_at.is_none() && !matches!(availability.as_str(), "available" | "recovering") {
                return Err(ProjectViewMaintenanceError::Conflict(
                    "suspended ack requires an available or recovering baseline Runtime".to_owned(),
                ));
            }
        }
        MaintenanceRuntimeAckStatus::Terminal => {
            if ended_at.is_none() && availability != "unavailable" {
                return Err(ProjectViewMaintenanceError::Conflict(
                    "terminal ack requires graceful-stop or terminal failure evidence".to_owned(),
                ));
            }
        }
    }
    sqlx::query(
        "UPDATE project_runtime_leases SET lease_expires_at = NULL, \
             recovery_attempt_in_flight = FALSE, next_recovery_at = NULL, \
             ended_at = COALESCE(ended_at, $6), updated_at = $6 \
         WHERE community_id = $1 AND binding_id = $2 AND assignment_id = $3 \
           AND runtime_id = $4 AND runtime_epoch = $5",
    )
    .bind(community_id.as_uuid())
    .bind(binding_id)
    .bind(assignment_id)
    .bind(runtime_id)
    .bind(runtime_epoch_db)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO project_view_maintenance_acks \
            (community_id, maintenance_epoch, ack_request_id, binding_id, \
             assignment_id, runtime_id, runtime_epoch, supervisor_pubkey, status, acked_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(ack_request_id)
    .bind(binding_id)
    .bind(assignment_id)
    .bind(runtime_id)
    .bind(runtime_epoch_db)
    .bind(supervisor_pubkey)
    .bind(status.as_str())
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(json!({
        "maintenance_epoch": db_u64(epoch, "maintenance_epoch")?,
        "type": "runtime_suspended_or_terminal",
        "binding_id": binding_id,
        "assignment_id": assignment_id,
        "runtime_id": runtime_id,
        "runtime_epoch": runtime_epoch,
        "status": status.as_str(),
    }))
}

pub(crate) async fn validate_freeze_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
) -> ProjectViewMaintenanceResult<()> {
    let counts = sqlx::query(
        "SELECT \
             (SELECT count(*) FROM project_view_maintenance_assignment_baselines \
              WHERE community_id = $1 AND maintenance_epoch = $2) AS assignments, \
             (SELECT count(*) FROM project_view_maintenance_assignment_acks \
              WHERE community_id = $1 AND maintenance_epoch = $2) AS assignment_acks, \
             (SELECT count(*) FROM project_view_maintenance_runtime_baselines \
              WHERE community_id = $1 AND maintenance_epoch = $2) AS runtimes, \
             (SELECT count(*) FROM project_view_maintenance_acks \
              WHERE community_id = $1 AND maintenance_epoch = $2) AS runtime_acks, \
             (SELECT count(*) FROM project_view_maintenance_invalidations \
              WHERE community_id = $1 AND maintenance_epoch = $2 \
                AND phase = 'pre_cutover') AS invalidations, \
             (SELECT count(*) FROM project_runtime_supervisor_bindings binding \
              JOIN project_view_maintenance_assignment_baselines baseline \
                ON baseline.community_id = binding.community_id \
               AND baseline.binding_id = binding.binding_id \
               AND baseline.maintenance_epoch = $2 \
              WHERE binding.community_id = $1 \
                AND binding.scheduler_claim_token IS NOT NULL \
                AND binding.scheduler_claimed_until > clock_timestamp()) AS claims, \
             (SELECT count(*) FROM project_runtime_leases runtime \
              JOIN project_view_maintenance_assignment_baselines baseline \
                ON baseline.community_id = runtime.community_id \
               AND baseline.binding_id = runtime.binding_id \
               AND baseline.maintenance_epoch = $2 \
              WHERE runtime.community_id = $1 AND runtime.ended_at IS NULL) AS live_runtimes, \
             (SELECT count(*) FROM project_runtime_leases runtime \
              JOIN project_view_maintenance_assignment_baselines assignment \
                ON assignment.community_id = runtime.community_id \
               AND assignment.binding_id = runtime.binding_id \
               AND assignment.maintenance_epoch = $2 \
              LEFT JOIN project_view_maintenance_runtime_baselines baseline \
                ON baseline.community_id = runtime.community_id \
               AND baseline.maintenance_epoch = assignment.maintenance_epoch \
               AND baseline.binding_id = runtime.binding_id \
               AND baseline.assignment_id = runtime.assignment_id \
               AND baseline.runtime_id = runtime.runtime_id \
               AND baseline.runtime_epoch = runtime.runtime_epoch \
              WHERE runtime.community_id = $1 AND baseline.runtime_id IS NULL) AS new_runtimes",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .fetch_one(&mut **tx)
    .await?;
    let assignments: i64 = counts.try_get("assignments")?;
    let assignment_acks: i64 = counts.try_get("assignment_acks")?;
    let runtimes: i64 = counts.try_get("runtimes")?;
    let runtime_acks: i64 = counts.try_get("runtime_acks")?;
    let invalidations: i64 = counts.try_get("invalidations")?;
    let claims: i64 = counts.try_get("claims")?;
    let live_runtimes: i64 = counts.try_get("live_runtimes")?;
    let new_runtimes: i64 = counts.try_get("new_runtimes")?;
    if assignments != assignment_acks
        || runtimes != runtime_acks
        || invalidations != 0
        || claims != 0
        || live_runtimes != 0
        || new_runtimes != 0
    {
        return Err(ProjectViewMaintenanceError::Conflict(format!(
            "drain incomplete: assignments={assignment_acks}/{assignments}, \
             runtimes={runtime_acks}/{runtimes}, invalidations={invalidations}, \
             claims={claims}, live_runtimes={live_runtimes}, new_runtimes={new_runtimes}"
        )));
    }
    Ok(())
}

async fn fence_epoch_runtimes_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    now: DateTime<Utc>,
) -> ProjectViewMaintenanceResult<()> {
    sqlx::query(
        "UPDATE project_runtime_leases runtime SET lease_expires_at = NULL, \
             recovery_attempt_in_flight = FALSE, next_recovery_at = NULL, \
             ended_at = COALESCE(runtime.ended_at, $3), updated_at = $3 \
         FROM project_view_maintenance_assignment_baselines baseline \
         WHERE runtime.community_id = $1 AND baseline.community_id = runtime.community_id \
           AND baseline.maintenance_epoch = $2 \
           AND baseline.binding_id = runtime.binding_id",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "UPDATE project_runtime_supervisor_bindings binding SET \
             scheduler_claim_token = NULL, scheduler_claimed_until = NULL, updated_at = $3 \
         FROM project_view_maintenance_assignment_baselines baseline \
         WHERE binding.community_id = $1 AND baseline.community_id = binding.community_id \
           AND baseline.maintenance_epoch = $2 AND baseline.binding_id = binding.binding_id",
    )
    .bind(community_id.as_uuid())
    .bind(epoch)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[derive(Debug)]
struct VerifiedV3Coordinate {
    meta_event_id: [u8; 32],
    project_revision: i64,
    projection_generation: i64,
}

async fn validate_v3_structural_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    relay_pubkey: &PublicKey,
) -> ProjectViewMaintenanceResult<VerifiedV3Coordinate> {
    sqlx::query("SELECT project_view_v3_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    let relay = relay_pubkey.to_bytes();
    let row = sqlx::query(
        "SELECT state.meta_projection_event_id, state.project_revision, \
                state.projection_generation \
         FROM communities community \
         JOIN project_view_state state ON state.community_id = community.id \
         JOIN events meta ON meta.community_id = community.id \
                         AND meta.id = state.meta_projection_event_id \
         WHERE community.id = $1 AND community.project_view_schema_version = 3 \
           AND state.schema_version = 3 AND state.projection_pubkey = $2 \
           AND meta.pubkey = $2 AND meta.kind = $3 AND meta.deleted_at IS NULL \
           AND meta.content::jsonb->>'schema_version' = '3' \
           AND (meta.content::jsonb->>'project_revision')::bigint = state.project_revision \
           AND (meta.content::jsonb->>'projection_generation')::bigint \
               = state.projection_generation \
           AND EXISTS ( \
               SELECT 1 FROM events membership \
               WHERE membership.community_id = community.id \
                 AND membership.id = state.membership_snapshot_event_id \
                 AND membership.kind = $4 AND membership.pubkey = $2 \
                 AND membership.deleted_at IS NULL \
           ) \
           AND NOT EXISTS ( \
               SELECT 1 FROM project_view_objects object \
               WHERE object.community_id = community.id \
                 AND (object.schema_version <> 3 OR object.source_provenance_id IS NULL) \
           ) \
           AND NOT EXISTS ( \
               SELECT 1 FROM ( \
                   SELECT projection_event_id FROM project_view_objects \
                   WHERE community_id = $1 \
                   UNION ALL \
                   SELECT proposal.projection_event_id \
                   FROM project_role_assignment_proposals proposal \
                   WHERE proposal.community_id = $1 \
                     AND (proposal.status = 'open' OR EXISTS ( \
                         SELECT 1 FROM project_role_assignments assignment \
                         WHERE assignment.community_id = proposal.community_id \
                           AND assignment.proposal_id = proposal.proposal_id \
                           AND assignment.ended_at IS NULL)) \
                   UNION ALL \
                   SELECT projection_event_id FROM project_role_assignments \
                   WHERE community_id = $1 AND ended_at IS NULL \
                   UNION ALL \
                   SELECT projection_event_id FROM project_work_commitments \
                   WHERE community_id = $1 AND ended_at IS NULL \
                   UNION ALL \
                   SELECT projection_event_id FROM ( \
                       SELECT checkpoint.projection_event_id, \
                              row_number() OVER (PARTITION BY checkpoint.role_id \
                                  ORDER BY checkpoint.project_revision DESC, \
                                           checkpoint.checkpoint_id DESC) AS history_rank \
                       FROM project_role_checkpoints checkpoint \
                       JOIN project_view_objects role \
                         ON role.community_id = checkpoint.community_id \
                        AND role.object_id = checkpoint.role_id \
                        AND role.object_type = 'role' AND role.deleted_at IS NULL \
                       WHERE checkpoint.community_id = $1 \
                   ) current_checkpoint WHERE history_rank = 1 \
                   UNION ALL \
                   SELECT projection_event_id FROM ( \
                       SELECT handoff.projection_event_id, \
                              row_number() OVER (PARTITION BY handoff.role_id \
                                  ORDER BY handoff.project_revision DESC, \
                                           handoff.handoff_id DESC) AS history_rank \
                       FROM project_role_handoffs handoff \
                       JOIN project_view_objects role \
                         ON role.community_id = handoff.community_id \
                        AND role.object_id = handoff.role_id \
                        AND role.object_type = 'role' AND role.deleted_at IS NULL \
                       WHERE handoff.community_id = $1 \
                   ) current_handoff WHERE history_rank <= 3 \
               ) head \
               LEFT JOIN events projection \
                 ON projection.community_id = community.id \
                AND projection.id = head.projection_event_id \
                AND projection.kind = $5 AND projection.pubkey = $2 \
                AND projection.deleted_at IS NULL \
               WHERE head.projection_event_id IS NULL OR projection.id IS NULL \
                  OR projection.content::jsonb->>'schema_version' IS DISTINCT FROM '3' \
                  OR (projection.content::jsonb->>'projection_generation')::bigint \
                     IS DISTINCT FROM state.projection_generation \
           ) \
         FOR SHARE OF community, state, meta",
    )
    .bind(community_id.as_uuid())
    .bind(relay.as_slice())
    .bind(
        i32::try_from(buzz_core::kind::KIND_PROJECT_VIEW_META).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("Project View meta kind exceeds INT".into())
        })?,
    )
    .bind(
        i32::try_from(buzz_core::kind::KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("membership kind exceeds INT".into())
        })?,
    )
    .bind(
        i32::try_from(buzz_core::kind::KIND_PROJECT_VIEW_OBJECT).map_err(|_| {
            ProjectViewMaintenanceError::Invalid("Project View object kind exceeds INT".into())
        })?,
    )
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        ProjectViewMaintenanceError::Unavailable(
            "schema-3 structural/signer verification failed".to_owned(),
        )
    })?;
    Ok(VerifiedV3Coordinate {
        meta_event_id: bytes32(row.try_get("meta_projection_event_id")?, "meta event ID")?,
        project_revision: row.try_get("project_revision")?,
        projection_generation: row.try_get("projection_generation")?,
    })
}

async fn resolve_post_cutover_invalidations_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    epoch: i64,
    operation_id: Uuid,
    coordinate: &VerifiedV3Coordinate,
) -> ProjectViewMaintenanceResult<()> {
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
    .bind(coordinate.meta_event_id.as_slice())
    .bind(coordinate.project_revision)
    .bind(coordinate.projection_generation)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(crate) async fn require_human_operator_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
) -> ProjectViewMaintenanceResult<()> {
    let actor_bytes = actor.to_bytes();
    let actor_hex = actor.to_hex();
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM relay_members member \
             LEFT JOIN users actor \
               ON actor.community_id = member.community_id \
              AND actor.pubkey = $3 \
             LEFT JOIN community_bans restriction \
               ON restriction.community_id = member.community_id \
              AND restriction.pubkey = $3 \
             WHERE member.community_id = $1 AND member.pubkey = $2 \
               AND member.role IN ('owner', 'admin') \
               AND actor.agent_owner_pubkey IS NULL \
               AND NOT COALESCE( \
                   restriction.banned \
                   AND (restriction.ban_expires_at IS NULL \
                        OR restriction.ban_expires_at > clock_timestamp()), \
                   FALSE \
               ) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(actor_hex)
    .bind(actor_bytes.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if eligible {
        Ok(())
    } else {
        Err(ProjectViewMaintenanceError::Forbidden(
            "current Human owner/admin is required".to_owned(),
        ))
    }
}

async fn maintenance_status_with_executor<'e, E>(
    executor: E,
    community_id: CommunityId,
    requested_epoch: Option<i64>,
) -> ProjectViewMaintenanceResult<Value>
where
    E: sqlx::Executor<'e, Database = Postgres> + Copy,
{
    let pointer = sqlx::query(
        "SELECT maintenance.state, maintenance.current_epoch, community.host, \
                community.project_view_schema_version, community.project_view_enabled, \
                community.archived_at IS NOT NULL AS archived \
         FROM project_view_maintenance maintenance \
         JOIN communities community ON community.id = maintenance.community_id \
         WHERE maintenance.community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(executor)
    .await?
    .ok_or_else(|| ProjectViewMaintenanceError::Unavailable("Community not found".into()))?;
    let current_epoch: Option<i64> = pointer.try_get("current_epoch")?;
    let epoch = requested_epoch.or(current_epoch);
    let epoch_value = if let Some(epoch) = epoch {
        let row = sqlx::query(
            "SELECT maintenance_epoch, base_meta_event_id, base_project_revision, \
                    base_projection_generation, required_client_protocol_version, \
                    requested_by, requested_at, outcome, completed_at \
             FROM project_view_maintenance_epochs \
             WHERE community_id = $1 AND maintenance_epoch = $2",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_optional(executor)
        .await?
        .ok_or_else(|| ProjectViewMaintenanceError::Conflict("maintenance epoch missing".into()))?;
        let counts = sqlx::query(
            "SELECT \
                 (SELECT count(*) FROM project_view_maintenance_assignment_baselines \
                  WHERE community_id = $1 AND maintenance_epoch = $2) AS assignments, \
                 (SELECT count(*) FROM project_view_maintenance_assignment_acks \
                  WHERE community_id = $1 AND maintenance_epoch = $2) AS assignment_acks, \
                 (SELECT count(*) FROM project_view_maintenance_runtime_baselines \
                  WHERE community_id = $1 AND maintenance_epoch = $2) AS runtimes, \
                 (SELECT count(*) FROM project_view_maintenance_acks \
                  WHERE community_id = $1 AND maintenance_epoch = $2) AS runtime_acks, \
                 (SELECT count(*) FROM project_view_maintenance_invalidations \
                  WHERE community_id = $1 AND maintenance_epoch = $2 \
                    AND resolved_by_operation_id IS NULL) AS unresolved_invalidations",
        )
        .bind(community_id.as_uuid())
        .bind(epoch)
        .fetch_one(executor)
        .await?;
        Some(json!({
            "maintenance_epoch": db_u64(row.try_get("maintenance_epoch")?, "maintenance_epoch")?,
            "base_meta_event_id": hex::encode(row.try_get::<Vec<u8>, _>("base_meta_event_id")?),
            "base_project_revision": db_u64(row.try_get("base_project_revision")?, "base_project_revision")?,
            "base_projection_generation": db_u64(row.try_get("base_projection_generation")?, "base_projection_generation")?,
            "required_client_protocol_version": db_u64(row.try_get("required_client_protocol_version")?, "required_client_protocol_version")?,
            "requested_by": hex::encode(row.try_get::<Vec<u8>, _>("requested_by")?),
            "requested_at": row.try_get::<DateTime<Utc>, _>("requested_at")?,
            "outcome": row.try_get::<String, _>("outcome")?,
            "completed_at": row.try_get::<Option<DateTime<Utc>>, _>("completed_at")?,
            "assignment_baseline_count": counts.try_get::<i64, _>("assignments")?,
            "assignment_ack_count": counts.try_get::<i64, _>("assignment_acks")?,
            "runtime_baseline_count": counts.try_get::<i64, _>("runtimes")?,
            "runtime_ack_count": counts.try_get::<i64, _>("runtime_acks")?,
            "unresolved_invalidation_count": counts.try_get::<i64, _>("unresolved_invalidations")?,
        }))
    } else {
        None
    };
    Ok(json!({
        "community_id": community_id.to_string(),
        "host": pointer.try_get::<String, _>("host")?,
        "state": pointer.try_get::<String, _>("state")?,
        "current_epoch": current_epoch.map(|value| db_u64(value, "current_epoch")).transpose()?,
        "project_view_schema_version": pointer.try_get::<i16, _>("project_view_schema_version")?,
        "project_view_enabled": pointer.try_get::<bool, _>("project_view_enabled")?,
        "archived": pointer.try_get::<bool, _>("archived")?,
        "epoch": epoch_value,
    }))
}

pub(crate) fn idempotency_hash(value: &str) -> ProjectViewMaintenanceResult<[u8; 32]> {
    if value.is_empty() || value.len() > 4_096 || value.contains('\0') {
        return Err(ProjectViewMaintenanceError::Invalid(
            "idempotency key must contain 1..=4096 non-NUL bytes".to_owned(),
        ));
    }
    Ok(hash_parts(IDEMPOTENCY_DOMAIN, &[value.as_bytes()]))
}

fn maintenance_ack_idempotency_hash(value: Uuid) -> [u8; 32] {
    hash_parts(IDEMPOTENCY_DOMAIN, &[value.as_bytes()])
}

fn begin_request_hash(
    community_id: CommunityId,
    required_client_protocol_version: u64,
    relay_pubkey: &PublicKey,
) -> [u8; 32] {
    hash_parts(
        BEGIN_REQUEST_DOMAIN,
        &[
            community_id.as_uuid().as_bytes(),
            &required_client_protocol_version.to_be_bytes(),
            relay_pubkey.as_bytes(),
        ],
    )
}

fn operation_request_hash(
    community_id: CommunityId,
    maintenance_epoch: u64,
    operation: &str,
) -> [u8; 32] {
    hash_parts(
        OPERATION_REQUEST_DOMAIN,
        &[
            community_id.as_uuid().as_bytes(),
            &maintenance_epoch.to_be_bytes(),
            operation.as_bytes(),
        ],
    )
}

fn maintenance_ack_request_hash(
    community_id: CommunityId,
    command: &MaintenanceAckCommand,
) -> ProjectViewMaintenanceResult<[u8; 32]> {
    let encoded = serde_json::to_vec(command)
        .map_err(|error| ProjectViewMaintenanceError::Invalid(error.to_string()))?;
    Ok(hash_parts(
        ACK_REQUEST_DOMAIN,
        &[community_id.as_uuid().as_bytes(), &encoded],
    ))
}

fn prepare_request_hash(community_id: CommunityId) -> [u8; 32] {
    hash_parts(
        PREPARE_REQUEST_DOMAIN,
        &[community_id.as_uuid().as_bytes(), &3_u16.to_be_bytes()],
    )
}

fn context_request_hash(community_id: CommunityId, enabled: bool) -> [u8; 32] {
    hash_parts(
        CONTEXT_REQUEST_DOMAIN,
        &[
            community_id.as_uuid().as_bytes(),
            &[u8::from(enabled)],
            &PROJECT_CONTEXT_CLOSURE_PROTOCOL_VERSION.to_be_bytes(),
        ],
    )
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

fn require_safe_positive(value: u64, field: &str) -> ProjectViewMaintenanceResult<()> {
    if (1..=buzz_project_view::MAX_SAFE_REVISION).contains(&value) {
        Ok(())
    } else {
        Err(ProjectViewMaintenanceError::Invalid(format!(
            "{field} must be JavaScript-safe and positive"
        )))
    }
}

fn db_u64(value: i64, field: &str) -> ProjectViewMaintenanceResult<u64> {
    u64::try_from(value).map_err(|_| {
        ProjectViewMaintenanceError::Invalid(format!("stored {field} must be non-negative"))
    })
}

fn bytes32(value: Vec<u8>, field: &str) -> ProjectViewMaintenanceResult<[u8; 32]> {
    value.try_into().map_err(|value: Vec<u8>| {
        ProjectViewMaintenanceError::Invalid(format!(
            "stored {field} must contain 32 bytes, got {}",
            value.len()
        ))
    })
}

fn serialize_community_id<S>(community_id: &CommunityId, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&community_id.to_string())
}
