//! Trusted managed-runtime supervision persistence.
//!
//! This module never treats presence, socket state, or ordinary Agent silence
//! as failure evidence. Every mutation is scoped to an operator-installed
//! `(Community, Assignment, supervisor pubkey)` binding and every runtime
//! transition is epoch-fenced.

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::v2::{
    AssignmentRuntimeStatus, RoleContinuityChange, RuntimeAvailability, RuntimeEvidence,
    RuntimeEvidenceReceipt, RuntimeEvidenceRequest, RuntimeFence, RuntimeLeaseStatus,
    RuntimeRecoveryPolicy,
};
use chrono::{DateTime, Duration, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

/// Stable runtime-supervision failures.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSupervisionError {
    /// Database operation failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// SQL operation failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Tamper-evident audit append failed.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// Request shape or transition is invalid.
    #[error("invalid runtime supervision request: {0}")]
    Invalid(String),
    /// No current binding authorizes this supervisor.
    #[error("runtime supervisor is not registered for this Assignment")]
    NotRegistered,
    /// The exact Assignment is no longer active.
    #[error("runtime Assignment is no longer active")]
    AssignmentEnded,
    /// A stale runtime epoch attempted to mutate current state.
    #[error("runtime epoch is stale")]
    StaleEpoch,
    /// A different active binding already owns this Assignment.
    #[error("Assignment already has an active runtime supervisor")]
    BindingConflict,
    /// A supervised command omitted or failed its runtime fence.
    #[error("supervised runtime command fence is missing or stale")]
    CommandFence,
}

/// Convenient runtime-supervision result.
pub type RuntimeSupervisionResult<T> = Result<T, RuntimeSupervisionError>;

/// Durable operator-installed supervisor binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSupervisorBinding {
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// Immutable binding identity.
    pub binding_id: Uuid,
    /// Exact managed-Agent Assignment.
    pub assignment_id: Uuid,
    /// NIP-98 signing identity of the trusted supervisor.
    pub supervisor_pubkey: PublicKey,
    /// Bounded recovery policy.
    pub policy: RuntimeRecoveryPolicy,
    /// Canonical registration time.
    pub registered_at: DateTime<Utc>,
}

/// One scheduler claim for an exhausted supervised Assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeUnrecoverableClaim {
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// Canonical Community host for fan-out.
    pub community_host: String,
    /// Binding that supplied the trusted evidence.
    pub binding_id: Uuid,
    /// Assignment eligible for the internal system action.
    pub assignment_id: Uuid,
    /// Opaque compare-and-clear claim token.
    pub claim_token: Uuid,
    /// Stable system-action idempotency digest.
    pub idempotency_key_hash: [u8; 32],
    /// Immutable evidence IDs supporting the decision.
    pub evidence_ids: Vec<[u8; 32]>,
}

impl Db {
    /// Install or idempotently return one Assignment-scoped supervisor.
    pub async fn register_runtime_supervisor(
        &self,
        community_id: CommunityId,
        assignment_id: Uuid,
        supervisor_pubkey: PublicKey,
        registered_by: PublicKey,
        policy: RuntimeRecoveryPolicy,
    ) -> RuntimeSupervisionResult<RuntimeSupervisorBinding> {
        policy
            .validate()
            .map_err(RuntimeSupervisionError::Invalid)?;
        if assignment_id.is_nil() {
            return Err(RuntimeSupervisionError::Invalid(
                "assignment_id cannot be nil".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let assignment = sqlx::query(
            "SELECT assignment.member_pubkey, assignment.ended_at, \
                    actor.agent_owner_pubkey IS NOT NULL AS managed_agent \
             FROM project_role_assignments assignment \
             LEFT JOIN users actor \
               ON actor.community_id = assignment.community_id \
              AND actor.pubkey = decode(assignment.member_pubkey, 'hex') \
             JOIN communities community ON community.id = assignment.community_id \
             JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = assignment.community_id \
             WHERE assignment.community_id = $1 AND assignment.assignment_id = $2 \
               AND community.project_view_schema_version IN (2, 3) \
               AND community.project_view_enabled \
               AND community.archived_at IS NULL \
               AND maintenance.state = 'normal' \
             FOR UPDATE OF assignment, maintenance",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RuntimeSupervisionError::AssignmentEnded)?;
        if assignment
            .try_get::<Option<DateTime<Utc>>, _>("ended_at")?
            .is_some()
        {
            return Err(RuntimeSupervisionError::AssignmentEnded);
        }
        if !assignment.try_get::<bool, _>("managed_agent")? {
            return Err(RuntimeSupervisionError::Invalid(
                "only a known managed-Agent Assignment can be supervised".to_owned(),
            ));
        }

        if let Some(row) = sqlx::query(
            "SELECT binding_id, supervisor_pubkey, lease_seconds, \
                    recovery_window_seconds, max_recovery_attempts, \
                    recovery_backoff_seconds, \
                    monitor_timeout_seconds, monitor_grace_seconds, \
                    automatic_unrecoverable, registered_at \
             FROM project_runtime_supervisor_bindings \
             WHERE community_id = $1 AND assignment_id = $2 AND revoked_at IS NULL \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            let existing = binding_from_row(community_id, assignment_id, &row)?;
            if existing.supervisor_pubkey != supervisor_pubkey || existing.policy != policy {
                return Err(RuntimeSupervisionError::BindingConflict);
            }
            tx.rollback().await?;
            return Ok(existing);
        }

        let binding_id = Uuid::new_v4();
        let supervisor_bytes = supervisor_pubkey.to_bytes();
        let operator_bytes = registered_by.to_bytes();
        let registered_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO project_runtime_supervisor_bindings \
                (community_id, binding_id, assignment_id, supervisor_pubkey, \
                 lease_seconds, recovery_window_seconds, max_recovery_attempts, \
                 recovery_backoff_seconds, monitor_timeout_seconds, monitor_grace_seconds, \
                 automatic_unrecoverable, registered_by, registered_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$13)",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .bind(assignment_id)
        .bind(supervisor_bytes.as_slice())
        .bind(i32::try_from(policy.lease_seconds).map_err(|_| {
            RuntimeSupervisionError::Invalid("lease_seconds exceeds PostgreSQL INT".to_owned())
        })?)
        .bind(i32::try_from(policy.recovery_window_seconds).map_err(|_| {
            RuntimeSupervisionError::Invalid(
                "recovery_window_seconds exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(i32::try_from(policy.max_recovery_attempts).map_err(|_| {
            RuntimeSupervisionError::Invalid(
                "max_recovery_attempts exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(i32::try_from(policy.recovery_backoff_seconds).map_err(|_| {
            RuntimeSupervisionError::Invalid(
                "recovery_backoff_seconds exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(i32::try_from(policy.monitor_timeout_seconds).map_err(|_| {
            RuntimeSupervisionError::Invalid(
                "monitor_timeout_seconds exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(i32::try_from(policy.monitor_grace_seconds).map_err(|_| {
            RuntimeSupervisionError::Invalid(
                "monitor_grace_seconds exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(policy.automatic_unrecoverable)
        .bind(operator_bytes.as_slice())
        .bind(registered_at)
        .execute(&mut *tx)
        .await?;
        buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::RuntimeSupervisorRegistered,
                actor_pubkey: Some(operator_bytes.to_vec()),
                object_id: Some(assignment_id.to_string()),
                detail: json!({
                    "binding_id": binding_id,
                    "assignment_id": assignment_id,
                    "supervisor_pubkey": supervisor_pubkey,
                    "policy": policy,
                }),
            },
        )
        .await?;
        tx.commit().await?;
        Ok(RuntimeSupervisorBinding {
            community_id,
            binding_id,
            assignment_id,
            supervisor_pubkey,
            policy,
            registered_at,
        })
    }

    /// Revoke the current binding and fence all of its runtime epochs.
    pub async fn revoke_runtime_supervisor(
        &self,
        community_id: CommunityId,
        assignment_id: Uuid,
        revoked_by: PublicKey,
    ) -> RuntimeSupervisionResult<bool> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_normal_maintenance_in_tx(&mut tx, community_id).await?;
        let revoked_at: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let revoked_by = revoked_by.to_bytes();
        let binding_id: Option<Uuid> = sqlx::query_scalar(
            "UPDATE project_runtime_supervisor_bindings SET \
                 revoked_by = $3, revoked_at = $4, automatic_unrecoverable = FALSE, \
                 scheduler_claim_token = NULL, scheduler_claimed_until = NULL, \
                 updated_at = $4 \
             WHERE community_id = $1 AND assignment_id = $2 AND revoked_at IS NULL \
             RETURNING binding_id",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .bind(revoked_by.as_slice())
        .bind(revoked_at)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(binding_id) = binding_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        sqlx::query(
            "UPDATE project_runtime_leases SET \
                 lease_expires_at = NULL, recovery_attempt_in_flight = FALSE, \
                 next_recovery_at = NULL, ended_at = $3, updated_at = $3 \
             WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .bind(revoked_at)
        .execute(&mut *tx)
        .await?;
        buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id,
                action: AuditAction::RuntimeSupervisorRevoked,
                actor_pubkey: Some(revoked_by.to_vec()),
                object_id: Some(assignment_id.to_string()),
                detail: json!({
                    "binding_id": binding_id,
                    "assignment_id": assignment_id,
                }),
            },
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Record one NIP-98-authenticated supervisor observation.
    pub async fn record_runtime_evidence(
        &self,
        community_id: CommunityId,
        supervisor_pubkey: PublicKey,
        evidence_id: [u8; 32],
        request: &RuntimeEvidenceRequest,
    ) -> RuntimeSupervisionResult<RuntimeEvidenceReceipt> {
        request
            .validate()
            .map_err(RuntimeSupervisionError::Invalid)?;
        let idempotency_key_hash =
            buzz_project_view::v2::idempotency_key_hash(request.idempotency_key.as_bytes());
        let request_hash = runtime_evidence_request_hash(request)?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let maintenance = maintenance_observation_in_tx(&mut tx, community_id).await?;
        let row = sqlx::query(
            "SELECT binding_id, supervisor_pubkey, lease_seconds, \
                    recovery_window_seconds, max_recovery_attempts, \
                    recovery_backoff_seconds, \
                    monitor_timeout_seconds, monitor_grace_seconds, \
                    automatic_unrecoverable, registered_at, last_monitor_at, \
                    monitor_grace_until, system_change_id \
             FROM project_runtime_supervisor_bindings \
             WHERE community_id = $1 AND assignment_id = $2 AND revoked_at IS NULL \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(request.assignment_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(RuntimeSupervisionError::NotRegistered)?;
        let stored_supervisor = public_key(
            &row.try_get::<Vec<u8>, _>("supervisor_pubkey")?,
            "supervisor_pubkey",
        )?;
        if stored_supervisor != supervisor_pubkey {
            return Err(RuntimeSupervisionError::NotRegistered);
        }
        validate_evidence_maintenance_fence(
            &mut tx,
            community_id,
            maintenance,
            row.try_get("binding_id")?,
            supervisor_pubkey,
            request,
        )
        .await?;
        if row
            .try_get::<Option<Vec<u8>>, _>("system_change_id")?
            .is_some()
        {
            return Err(RuntimeSupervisionError::AssignmentEnded);
        }
        let assignment_active: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
                 SELECT 1 FROM project_role_assignments \
                 WHERE community_id = $1 AND assignment_id = $2 AND ended_at IS NULL \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(request.assignment_id)
        .fetch_one(&mut *tx)
        .await?;
        if !assignment_active {
            return Err(RuntimeSupervisionError::AssignmentEnded);
        }

        if let Some(replayed) = sqlx::query(
            "SELECT receipt, request_hash FROM project_runtime_evidence \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            if bytes32(replayed.try_get("request_hash")?, "request_hash")? != request_hash {
                return Err(RuntimeSupervisionError::Invalid(
                    "runtime evidence idempotency key was reused for another request".to_owned(),
                ));
            }
            let mut receipt: RuntimeEvidenceReceipt = serde_json::from_value(
                replayed.try_get::<Value, _>("receipt")?,
            )
            .map_err(|error| {
                RuntimeSupervisionError::Invalid(format!(
                    "stored runtime evidence receipt is invalid: {error}"
                ))
            })?;
            receipt.replayed = true;
            tx.rollback().await?;
            return Ok(receipt);
        }

        let policy = policy_from_row(&row)?;
        let binding_id: Uuid = row.try_get("binding_id")?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *tx)
            .await?;
        let existing = sqlx::query(
            "SELECT runtime_epoch, availability, lease_expires_at, \
                    recovery_started_at, recovery_deadline, recovery_attempts, \
                    recovery_attempt_in_flight, next_recovery_at, \
                    ended_at, created_at \
             FROM project_runtime_leases \
             WHERE community_id = $1 AND binding_id = $2 AND runtime_id = $3 \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .bind(request.runtime_id)
        .fetch_optional(&mut *tx)
        .await?;
        let transition = apply_evidence_transition(existing.as_ref(), request, policy, now)?;
        let receipt = RuntimeEvidenceReceipt {
            assignment_id: request.assignment_id,
            runtime_id: request.runtime_id,
            runtime_epoch: transition.runtime_epoch,
            availability: transition.availability,
            lease_expires_at: transition.lease_expires_at,
            recovery_deadline: transition.recovery_deadline,
            recovery_attempts: transition.recovery_attempts,
            recovery_attempt_in_flight: transition.recovery_attempt_in_flight,
            next_recovery_at: transition.next_recovery_at,
            max_recovery_attempts: policy.max_recovery_attempts,
            replayed: false,
        };
        let receipt_json = serde_json::to_value(&receipt).map_err(|error| {
            RuntimeSupervisionError::Invalid(format!("serialize runtime evidence receipt: {error}"))
        })?;
        let created_at = existing
            .as_ref()
            .map(|existing| existing.try_get("created_at"))
            .transpose()?
            .unwrap_or(now);
        sqlx::query(
            "INSERT INTO project_runtime_leases \
                (community_id, binding_id, assignment_id, runtime_id, runtime_epoch, \
                 availability, lease_expires_at, recovery_started_at, \
                 recovery_deadline, recovery_attempts, recovery_attempt_in_flight, \
                 next_recovery_at, last_evidence_id, \
                 last_evidence_at, ended_at, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$14) \
             ON CONFLICT (community_id, binding_id, runtime_id) DO UPDATE SET \
                 runtime_epoch = EXCLUDED.runtime_epoch, \
                 availability = EXCLUDED.availability, \
                 lease_expires_at = EXCLUDED.lease_expires_at, \
                 recovery_started_at = EXCLUDED.recovery_started_at, \
                 recovery_deadline = EXCLUDED.recovery_deadline, \
                 recovery_attempts = EXCLUDED.recovery_attempts, \
                 recovery_attempt_in_flight = EXCLUDED.recovery_attempt_in_flight, \
                 next_recovery_at = EXCLUDED.next_recovery_at, \
                 last_evidence_id = EXCLUDED.last_evidence_id, \
                 last_evidence_at = EXCLUDED.last_evidence_at, \
                 ended_at = EXCLUDED.ended_at, updated_at = EXCLUDED.updated_at",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .bind(request.assignment_id)
        .bind(request.runtime_id)
        .bind(i64::try_from(transition.runtime_epoch).map_err(|_| {
            RuntimeSupervisionError::Invalid("runtime_epoch exceeds PostgreSQL BIGINT".to_owned())
        })?)
        .bind(transition.availability.as_str())
        .bind(transition.lease_expires_at)
        .bind(transition.recovery_started_at)
        .bind(transition.recovery_deadline)
        .bind(i32::try_from(transition.recovery_attempts).map_err(|_| {
            RuntimeSupervisionError::Invalid("recovery_attempts exceeds PostgreSQL INT".to_owned())
        })?)
        .bind(transition.recovery_attempt_in_flight)
        .bind(transition.next_recovery_at)
        .bind(evidence_id.as_slice())
        .bind(now)
        .bind(transition.ended_at)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;

        let previous_monitor_at: Option<DateTime<Utc>> = row.try_get("last_monitor_at")?;
        let previous_grace: Option<DateTime<Utc>> = row.try_get("monitor_grace_until")?;
        let monitor_was_stale = previous_monitor_at.is_none_or(|last| {
            now.signed_duration_since(last)
                > Duration::seconds(i64::from(policy.monitor_timeout_seconds))
        });
        let fresh_grace = now + Duration::seconds(i64::from(policy.monitor_grace_seconds));
        let monitor_grace_until = if monitor_was_stale {
            fresh_grace
        } else {
            previous_grace.map_or(fresh_grace, |grace| grace.max(now))
        };
        sqlx::query(
            "UPDATE project_runtime_supervisor_bindings SET \
                 last_monitor_at = $3, monitor_grace_until = $4, updated_at = $3 \
             WHERE community_id = $1 AND binding_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .bind(now)
        .bind(monitor_grace_until)
        .execute(&mut *tx)
        .await?;

        let detail = serde_json::to_value(&request.evidence).map_err(|error| {
            RuntimeSupervisionError::Invalid(format!("serialize runtime evidence: {error}"))
        })?;
        let supervisor_bytes = supervisor_pubkey.to_bytes();
        sqlx::query(
            "INSERT INTO project_runtime_evidence \
                (community_id, evidence_id, idempotency_key_hash, request_hash, binding_id, \
                 assignment_id, runtime_id, runtime_epoch, supervisor_pubkey, \
                 evidence_type, detail, availability_after, receipt, recorded_at) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)",
        )
        .bind(community_id.as_uuid())
        .bind(evidence_id.as_slice())
        .bind(idempotency_key_hash.as_slice())
        .bind(request_hash.as_slice())
        .bind(binding_id)
        .bind(request.assignment_id)
        .bind(request.runtime_id)
        .bind(i64::try_from(transition.runtime_epoch).map_err(|_| {
            RuntimeSupervisionError::Invalid("runtime_epoch exceeds PostgreSQL BIGINT".to_owned())
        })?)
        .bind(supervisor_bytes.as_slice())
        .bind(request.evidence.as_str())
        .bind(detail)
        .bind(transition.availability.as_str())
        .bind(receipt_json)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(receipt)
    }

    /// Read one Assignment's current operational availability.
    pub async fn assignment_runtime_status(
        &self,
        community_id: CommunityId,
        assignment_id: Uuid,
    ) -> RuntimeSupervisionResult<AssignmentRuntimeStatus> {
        let binding_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT binding_id FROM project_runtime_supervisor_bindings \
             WHERE community_id = $1 AND assignment_id = $2 AND revoked_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some(binding_id) = binding_id else {
            return Ok(AssignmentRuntimeStatus {
                assignment_id,
                managed: false,
                availability: None,
                runtimes: Vec::new(),
            });
        };
        let now = Utc::now();
        let rows = sqlx::query(
            "SELECT runtime_id, runtime_epoch, availability, lease_expires_at, \
                    recovery_deadline, recovery_attempts, recovery_attempt_in_flight, \
                    next_recovery_at, last_evidence_at \
             FROM project_runtime_leases \
             WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL \
             ORDER BY runtime_id",
        )
        .bind(community_id.as_uuid())
        .bind(binding_id)
        .fetch_all(&self.pool)
        .await?;
        let mut runtimes = Vec::with_capacity(rows.len());
        for row in rows {
            runtimes.push(RuntimeLeaseStatus {
                runtime_id: row.try_get("runtime_id")?,
                runtime_epoch: db_u64(row.try_get("runtime_epoch")?, "runtime_epoch")?,
                availability: parse_availability(row.try_get("availability")?)?,
                lease_expires_at: row.try_get("lease_expires_at")?,
                recovery_deadline: row.try_get("recovery_deadline")?,
                recovery_attempts: db_u32(row.try_get("recovery_attempts")?, "recovery_attempts")?,
                recovery_attempt_in_flight: row.try_get("recovery_attempt_in_flight")?,
                next_recovery_at: row.try_get("next_recovery_at")?,
                last_evidence_at: row.try_get("last_evidence_at")?,
            });
        }
        let availability = if runtimes.iter().any(|runtime| {
            runtime.availability == RuntimeAvailability::Available
                && runtime
                    .lease_expires_at
                    .is_some_and(|deadline| deadline > now)
        }) {
            RuntimeAvailability::Available
        } else if runtimes
            .iter()
            .any(|runtime| runtime.availability == RuntimeAvailability::Recovering)
        {
            RuntimeAvailability::Recovering
        } else {
            RuntimeAvailability::Unavailable
        };
        Ok(AssignmentRuntimeStatus {
            assignment_id,
            managed: true,
            availability: Some(availability),
            runtimes,
        })
    }

    /// Claim exhausted Assignment candidates across Relay pods.
    pub async fn claim_unrecoverable_runtime_assignments(
        &self,
        limit: u16,
        claim_duration: std::time::Duration,
    ) -> RuntimeSupervisionResult<Vec<RuntimeUnrecoverableClaim>> {
        let claim_seconds = i64::try_from(claim_duration.as_secs()).map_err(|_| {
            RuntimeSupervisionError::Invalid("claim duration exceeds PostgreSQL range".to_owned())
        })?;
        if limit == 0 || claim_seconds <= 0 {
            return Err(RuntimeSupervisionError::Invalid(
                "scheduler limit and claim duration must be positive".to_owned(),
            ));
        }
        // Discovery is intentionally read-only. Every actual claim is made in
        // a per-Community transaction after taking that Community's shared
        // advisory lock and rechecking the maintenance pointer.
        let community_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT binding.community_id \
             FROM project_runtime_supervisor_bindings binding \
             JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = binding.community_id \
             WHERE binding.revoked_at IS NULL \
               AND binding.automatic_unrecoverable \
               AND binding.system_change_id IS NULL \
               AND maintenance.state = 'normal' \
             ORDER BY binding.community_id \
             LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        let mut claims = Vec::with_capacity(usize::from(limit));
        for raw_community_id in community_ids {
            if claims.len() >= usize::from(limit) {
                break;
            }
            let community_id = CommunityId::from_uuid(raw_community_id);
            let mut tx = self.pool.begin().await?;
            crate::community_lock::acquire(&mut tx, community_id, true).await?;
            require_normal_maintenance_in_tx(&mut tx, community_id).await?;
            let remaining = i64::try_from(usize::from(limit) - claims.len()).map_err(|_| {
                RuntimeSupervisionError::Invalid("scheduler claim limit overflow".to_owned())
            })?;
            let rows = sqlx::query(
                "WITH candidates AS ( \
                     SELECT binding.binding_id \
                     FROM project_runtime_supervisor_bindings binding \
                     JOIN project_role_assignments assignment \
                       ON assignment.community_id = binding.community_id \
                      AND assignment.assignment_id = binding.assignment_id \
                     WHERE binding.community_id = $1 \
                       AND binding.revoked_at IS NULL \
                       AND binding.automatic_unrecoverable \
                       AND binding.system_change_id IS NULL \
                       AND assignment.ended_at IS NULL \
                       AND binding.last_monitor_at IS NOT NULL \
                       AND binding.last_monitor_at >= clock_timestamp() \
                           - make_interval(secs => binding.monitor_timeout_seconds) \
                       AND binding.monitor_grace_until <= clock_timestamp() \
                       AND (binding.scheduler_claimed_until IS NULL \
                            OR binding.scheduler_claimed_until < clock_timestamp()) \
                       AND EXISTS ( \
                           SELECT 1 FROM project_runtime_leases runtime \
                           WHERE runtime.community_id = binding.community_id \
                             AND runtime.binding_id = binding.binding_id \
                             AND runtime.ended_at IS NULL \
                       ) \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM project_runtime_leases runtime \
                           WHERE runtime.community_id = binding.community_id \
                             AND runtime.binding_id = binding.binding_id \
                             AND runtime.ended_at IS NULL \
                             AND runtime.availability <> 'unavailable' \
                       ) \
                     ORDER BY binding.updated_at, binding.binding_id \
                     FOR UPDATE OF binding SKIP LOCKED \
                     LIMIT $2 \
                 ) \
                 UPDATE project_runtime_supervisor_bindings binding SET \
                     scheduler_claim_token = gen_random_uuid(), \
                     scheduler_claimed_until = clock_timestamp() \
                         + make_interval(secs => $3), \
                     updated_at = clock_timestamp() \
                 FROM candidates \
                 WHERE binding.community_id = $1 \
                   AND binding.binding_id = candidates.binding_id \
                 RETURNING binding.binding_id, binding.assignment_id, \
                           binding.scheduler_claim_token",
            )
            .bind(community_id.as_uuid())
            .bind(remaining)
            .bind(claim_seconds)
            .fetch_all(&mut *tx)
            .await?;
            let community_host: String =
                sqlx::query_scalar("SELECT host FROM communities WHERE id = $1")
                    .bind(community_id.as_uuid())
                    .fetch_one(&mut *tx)
                    .await?;
            let mut community_claims = Vec::with_capacity(rows.len());
            for row in rows {
                let binding_id: Uuid = row.try_get("binding_id")?;
                let assignment_id: Uuid = row.try_get("assignment_id")?;
                let claim_token: Uuid = row.try_get("scheduler_claim_token")?;
                let evidence_rows: Vec<Vec<u8>> = sqlx::query_scalar(
                    "SELECT evidence_id FROM project_runtime_evidence \
                     WHERE community_id = $1 AND binding_id = $2 \
                     ORDER BY recorded_at, evidence_id",
                )
                .bind(community_id.as_uuid())
                .bind(binding_id)
                .fetch_all(&mut *tx)
                .await?;
                let evidence_ids = evidence_rows
                    .into_iter()
                    .map(|bytes| bytes32(bytes, "evidence_id"))
                    .collect::<RuntimeSupervisionResult<Vec<_>>>()?;
                community_claims.push(RuntimeUnrecoverableClaim {
                    community_id,
                    community_host: community_host.clone(),
                    binding_id,
                    assignment_id,
                    claim_token,
                    idempotency_key_hash: runtime_unrecoverable_idempotency_hash(binding_id),
                    evidence_ids,
                });
            }
            tx.commit().await?;
            claims.extend(community_claims);
        }
        Ok(claims)
    }

    /// Release exactly one scheduler claim after a non-committing failure.
    pub async fn release_unrecoverable_runtime_claim(
        &self,
        claim: &RuntimeUnrecoverableClaim,
    ) -> RuntimeSupervisionResult<bool> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, claim.community_id, true).await?;
        let result = sqlx::query(
            "UPDATE project_runtime_supervisor_bindings SET \
                 scheduler_claim_token = NULL, scheduler_claimed_until = NULL, \
                 updated_at = clock_timestamp() \
             WHERE community_id = $1 AND binding_id = $2 \
               AND scheduler_claim_token = $3 AND system_change_id IS NULL",
        )
        .bind(claim.community_id.as_uuid())
        .bind(claim.binding_id)
        .bind(claim.claim_token)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }
}

#[derive(Debug, Clone)]
struct MaintenanceObservation {
    state: String,
    epoch: Option<i64>,
}

async fn maintenance_observation_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> RuntimeSupervisionResult<MaintenanceObservation> {
    let row = sqlx::query(
        "SELECT state, current_epoch FROM project_view_maintenance \
         WHERE community_id = $1 FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| {
        RuntimeSupervisionError::Invalid("Project View maintenance pointer is missing".to_owned())
    })?;
    Ok(MaintenanceObservation {
        state: row.try_get("state")?,
        epoch: row.try_get("current_epoch")?,
    })
}

async fn require_normal_maintenance_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> RuntimeSupervisionResult<()> {
    let maintenance = maintenance_observation_in_tx(tx, community_id).await?;
    if maintenance.state == "normal" && maintenance.epoch.is_none() {
        Ok(())
    } else {
        Err(RuntimeSupervisionError::Invalid(
            "Project View maintenance fences runtime binding changes".to_owned(),
        ))
    }
}

async fn validate_evidence_maintenance_fence(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    maintenance: MaintenanceObservation,
    binding_id: Uuid,
    supervisor_pubkey: PublicKey,
    request: &RuntimeEvidenceRequest,
) -> RuntimeSupervisionResult<()> {
    if maintenance.state == "normal" && maintenance.epoch.is_none() {
        return Ok(());
    }
    if maintenance.state != "draining" {
        return Err(RuntimeSupervisionError::Invalid(
            "Project View maintenance fences runtime evidence".to_owned(),
        ));
    }
    if !matches!(
        request.evidence,
        RuntimeEvidence::GracefulStop
            | RuntimeEvidence::AbnormalExit { .. }
            | RuntimeEvidence::RecoveryFailed { .. }
            | RuntimeEvidence::SupervisorHeartbeat
    ) {
        return Err(RuntimeSupervisionError::Invalid(
            "draining only accepts terminal evidence for an exact baseline runtime".to_owned(),
        ));
    }
    let maintenance_epoch = maintenance.epoch.ok_or_else(|| {
        RuntimeSupervisionError::Invalid(
            "draining maintenance pointer has no current epoch".to_owned(),
        )
    })?;
    let runtime_epoch = request
        .runtime_epoch
        .ok_or(RuntimeSupervisionError::StaleEpoch)?;
    let runtime_epoch = i64::try_from(runtime_epoch).map_err(|_| {
        RuntimeSupervisionError::Invalid("runtime_epoch exceeds PostgreSQL BIGINT".to_owned())
    })?;
    let supervisor = supervisor_pubkey.to_bytes();
    let is_baseline: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_view_maintenance_runtime_baselines \
             WHERE community_id = $1 AND maintenance_epoch = $2 \
               AND binding_id = $3 AND assignment_id = $4 \
               AND runtime_id = $5 AND runtime_epoch = $6 \
               AND supervisor_pubkey = $7 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(maintenance_epoch)
    .bind(binding_id)
    .bind(request.assignment_id)
    .bind(request.runtime_id)
    .bind(runtime_epoch)
    .bind(supervisor.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if is_baseline {
        Ok(())
    } else {
        Err(RuntimeSupervisionError::Invalid(
            "runtime evidence does not name the current maintenance baseline".to_owned(),
        ))
    }
}

/// Runtime supervision requirement chosen by the calling protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeCommandFencePolicy {
    /// Preserve Project View v2 behavior: only registered supervision is fenced.
    LegacyOptionalSupervision,
    /// Managed commands require a binding and an exact current leased Runtime.
    RequireSupervisedRuntime,
}

/// Reject a supervised managed command unless its signed runtime fence is
/// current and leased. Called after the Community lock and canonical Assignment
/// checks are held; the policy keeps legacy v2 and strict newer protocols
/// explicit at every call site.
pub(crate) async fn validate_runtime_command_fence_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    assignment_id: Option<Uuid>,
    runtime_fence: Option<RuntimeFence>,
    policy: RuntimeCommandFencePolicy,
) -> RuntimeSupervisionResult<()> {
    let Some(assignment_id) = assignment_id else {
        return Ok(());
    };
    let binding_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT binding_id FROM project_runtime_supervisor_bindings \
         WHERE community_id = $1 AND assignment_id = $2 \
           AND revoked_at IS NULL AND system_change_id IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(assignment_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(binding_id) = binding_id else {
        return match policy {
            RuntimeCommandFencePolicy::LegacyOptionalSupervision => Ok(()),
            RuntimeCommandFencePolicy::RequireSupervisedRuntime => {
                Err(RuntimeSupervisionError::CommandFence)
            }
        };
    };
    let Some(runtime_fence) = runtime_fence else {
        return Err(RuntimeSupervisionError::CommandFence);
    };
    let epoch = i64::try_from(runtime_fence.runtime_epoch)
        .map_err(|_| RuntimeSupervisionError::CommandFence)?;
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_runtime_leases \
             WHERE community_id = $1 AND binding_id = $2 AND runtime_id = $3 \
               AND runtime_epoch = $4 AND availability = 'available' \
               AND lease_expires_at > clock_timestamp() AND ended_at IS NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(binding_id)
    .bind(runtime_fence.runtime_id)
    .bind(epoch)
    .fetch_one(&mut **tx)
    .await?;
    if valid {
        Ok(())
    } else {
        Err(RuntimeSupervisionError::CommandFence)
    }
}

/// Fence supervisor bindings whose canonical Assignment was ended by an
/// ordinary accepted Project command.
pub(crate) async fn fence_ended_runtime_bindings_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    changes: &[RoleContinuityChange],
    actor: PublicKey,
    canonical_time: DateTime<Utc>,
) -> RuntimeSupervisionResult<()> {
    let assignment_ids = changes
        .iter()
        .filter_map(|change| match change {
            RoleContinuityChange::Assignment(assignment) if !assignment.is_active() => {
                Some(assignment.assignment_id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if assignment_ids.is_empty() {
        return Ok(());
    }
    let actor = actor.to_bytes();
    let binding_ids = sqlx::query_scalar::<_, Uuid>(
        "UPDATE project_runtime_supervisor_bindings SET \
             revoked_by = $3, revoked_at = $4, automatic_unrecoverable = FALSE, \
             scheduler_claim_token = NULL, scheduler_claimed_until = NULL, \
             updated_at = $4 \
         WHERE community_id = $1 AND assignment_id = ANY($2) \
           AND revoked_at IS NULL AND system_change_id IS NULL \
         RETURNING binding_id",
    )
    .bind(community_id.as_uuid())
    .bind(&assignment_ids)
    .bind(actor.as_slice())
    .bind(canonical_time)
    .fetch_all(&mut **tx)
    .await?;
    if !binding_ids.is_empty() {
        sqlx::query(
            "UPDATE project_runtime_leases SET \
                 lease_expires_at = NULL, recovery_attempt_in_flight = FALSE, \
                 next_recovery_at = NULL, ended_at = $3, updated_at = $3 \
             WHERE community_id = $1 AND binding_id = ANY($2) AND ended_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(&binding_ids)
        .bind(canonical_time)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Revalidate one scheduler claim under the same transaction that will commit
/// the terminal Project change.
///
/// An `available` runtime always blocks the transition, even if its lease has
/// expired: lease expiry alone is silence, not trusted failure evidence.
pub(crate) async fn validate_unrecoverable_claim_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    claim: &RuntimeUnrecoverableClaim,
) -> RuntimeSupervisionResult<Vec<[u8; 32]>> {
    let row = sqlx::query(
        "SELECT binding.assignment_id, binding.scheduler_claim_token, \
                binding.scheduler_claimed_until, binding.last_monitor_at, \
                binding.monitor_grace_until, binding.monitor_timeout_seconds, \
                binding.automatic_unrecoverable, \
                binding.revoked_at, binding.system_change_id, assignment.ended_at \
         FROM project_runtime_supervisor_bindings binding \
         JOIN project_role_assignments assignment \
           ON assignment.community_id = binding.community_id \
          AND assignment.assignment_id = binding.assignment_id \
         WHERE binding.community_id = $1 AND binding.binding_id = $2 \
         FOR UPDATE OF binding, assignment",
    )
    .bind(claim.community_id.as_uuid())
    .bind(claim.binding_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RuntimeSupervisionError::NotRegistered)?;
    if row.try_get::<Uuid, _>("assignment_id")? != claim.assignment_id
        || row.try_get::<Option<Uuid>, _>("scheduler_claim_token")? != Some(claim.claim_token)
    {
        return Err(RuntimeSupervisionError::Invalid(
            "runtime scheduler claim no longer owns this Assignment".to_owned(),
        ));
    }
    if row
        .try_get::<Option<DateTime<Utc>>, _>("revoked_at")?
        .is_some()
        || row
            .try_get::<Option<Vec<u8>>, _>("system_change_id")?
            .is_some()
        || row
            .try_get::<Option<DateTime<Utc>>, _>("ended_at")?
            .is_some()
    {
        return Err(RuntimeSupervisionError::AssignmentEnded);
    }
    if !row.try_get::<bool, _>("automatic_unrecoverable")? {
        return Err(RuntimeSupervisionError::Invalid(
            "automatic unrecoverable policy is disabled".to_owned(),
        ));
    }

    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    let claimed_until: Option<DateTime<Utc>> = row.try_get("scheduler_claimed_until")?;
    let monitor_at: Option<DateTime<Utc>> = row.try_get("last_monitor_at")?;
    let monitor_grace_until: Option<DateTime<Utc>> = row.try_get("monitor_grace_until")?;
    let monitor_timeout = i64::from(row.try_get::<i32, _>("monitor_timeout_seconds")?);
    if claimed_until.is_none_or(|deadline| deadline <= now) {
        return Err(RuntimeSupervisionError::Invalid(
            "runtime scheduler claim expired before commit".to_owned(),
        ));
    }
    if monitor_at.is_none_or(|observed| {
        now.signed_duration_since(observed) > Duration::seconds(monitor_timeout)
    }) || monitor_grace_until.is_none_or(|deadline| deadline > now)
    {
        return Err(RuntimeSupervisionError::Invalid(
            "runtime supervisor monitor is stale or still inside recovery grace".to_owned(),
        ));
    }

    let runtimes = sqlx::query(
        "SELECT availability, recovery_deadline, recovery_attempts \
         FROM project_runtime_leases \
         WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL \
         ORDER BY runtime_id FOR UPDATE",
    )
    .bind(claim.community_id.as_uuid())
    .bind(claim.binding_id)
    .fetch_all(&mut **tx)
    .await?;
    if runtimes.is_empty() {
        return Err(RuntimeSupervisionError::Invalid(
            "no trusted runtime evidence exists".to_owned(),
        ));
    }
    for runtime in &runtimes {
        let availability: String = runtime.try_get("availability")?;
        if availability == RuntimeAvailability::Available.as_str() {
            return Err(RuntimeSupervisionError::Invalid(
                "an available runtime still fences Assignment termination".to_owned(),
            ));
        }
        if availability == RuntimeAvailability::Recovering.as_str() {
            return Err(RuntimeSupervisionError::Invalid(
                "a runtime is still recovering and has no terminal failure evidence".to_owned(),
            ));
        }
    }

    let evidence = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT evidence_id FROM project_runtime_evidence \
         WHERE community_id = $1 AND binding_id = $2 \
         ORDER BY recorded_at, evidence_id",
    )
    .bind(claim.community_id.as_uuid())
    .bind(claim.binding_id)
    .fetch_all(&mut **tx)
    .await?
    .into_iter()
    .map(|bytes| bytes32(bytes, "evidence_id"))
    .collect::<RuntimeSupervisionResult<Vec<_>>>()?;
    if evidence.is_empty() {
        return Err(RuntimeSupervisionError::Invalid(
            "runtime claim has no immutable supporting evidence".to_owned(),
        ));
    }
    Ok(evidence)
}

fn runtime_unrecoverable_idempotency_hash(binding_id: Uuid) -> [u8; 32] {
    let mut key = b"buzz/runtime-unrecoverable/v1\0".to_vec();
    key.extend_from_slice(binding_id.as_bytes());
    buzz_project_view::v2::idempotency_key_hash(&key)
}

fn runtime_evidence_request_hash(
    request: &RuntimeEvidenceRequest,
) -> RuntimeSupervisionResult<[u8; 32]> {
    let canonical = serde_json::to_vec(request).map_err(|error| {
        RuntimeSupervisionError::Invalid(format!("serialize runtime evidence request: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"buzz/runtime-evidence-request/v1\0");
    hasher.update(canonical);
    Ok(hasher.finalize().into())
}

#[derive(Debug, Clone, Copy)]
struct RuntimeTransition {
    runtime_epoch: u64,
    availability: RuntimeAvailability,
    lease_expires_at: Option<DateTime<Utc>>,
    recovery_started_at: Option<DateTime<Utc>>,
    recovery_deadline: Option<DateTime<Utc>>,
    recovery_attempts: u32,
    recovery_attempt_in_flight: bool,
    next_recovery_at: Option<DateTime<Utc>>,
    ended_at: Option<DateTime<Utc>>,
}

fn apply_evidence_transition(
    existing: Option<&sqlx::postgres::PgRow>,
    request: &RuntimeEvidenceRequest,
    policy: RuntimeRecoveryPolicy,
    now: DateTime<Utc>,
) -> RuntimeSupervisionResult<RuntimeTransition> {
    let current = existing.map(current_runtime).transpose()?;
    apply_runtime_transition(current, request, policy, now)
}

fn apply_runtime_transition(
    current: Option<RuntimeTransition>,
    request: &RuntimeEvidenceRequest,
    policy: RuntimeRecoveryPolicy,
    now: DateTime<Utc>,
) -> RuntimeSupervisionResult<RuntimeTransition> {
    let lease_deadline = now + Duration::seconds(i64::from(policy.lease_seconds));
    let recovery_deadline = now + Duration::seconds(i64::from(policy.recovery_window_seconds));
    if current.is_some_and(|runtime| runtime.ended_at.is_some()) {
        return Err(RuntimeSupervisionError::Invalid(
            "logical runtime has already been retired".to_owned(),
        ));
    }
    match (&request.evidence, current) {
        (RuntimeEvidence::Start, None) => Ok(RuntimeTransition {
            runtime_epoch: 1,
            availability: RuntimeAvailability::Available,
            lease_expires_at: Some(lease_deadline),
            recovery_started_at: None,
            recovery_deadline: None,
            recovery_attempts: 0,
            recovery_attempt_in_flight: false,
            next_recovery_at: None,
            ended_at: None,
        }),
        (RuntimeEvidence::Start, Some(_)) => Err(RuntimeSupervisionError::Invalid(
            "start cannot replace an existing logical runtime; use the fenced recovery flow"
                .to_owned(),
        )),
        (_, None) => Err(RuntimeSupervisionError::StaleEpoch),
        (RuntimeEvidence::LeaseRenewed, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            if current.availability != RuntimeAvailability::Available {
                return Err(RuntimeSupervisionError::Invalid(
                    "only an available runtime can renew its lease".to_owned(),
                ));
            }
            Ok(RuntimeTransition {
                lease_expires_at: Some(lease_deadline),
                ..current
            })
        }
        (RuntimeEvidence::GracefulStop, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            if current.availability != RuntimeAvailability::Available
                || current.recovery_attempt_in_flight
            {
                return Err(RuntimeSupervisionError::Invalid(
                    "graceful_stop requires an available runtime without recovery in flight"
                        .to_owned(),
                ));
            }
            Ok(RuntimeTransition {
                lease_expires_at: None,
                recovery_attempt_in_flight: false,
                next_recovery_at: None,
                ended_at: Some(now),
                ..current
            })
        }
        (RuntimeEvidence::AbnormalExit { .. }, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            if current.availability != RuntimeAvailability::Available {
                return Err(RuntimeSupervisionError::Invalid(
                    "abnormal_exit requires an available runtime".to_owned(),
                ));
            }
            Ok(RuntimeTransition {
                runtime_epoch: current.runtime_epoch,
                availability: RuntimeAvailability::Recovering,
                lease_expires_at: None,
                recovery_started_at: Some(now),
                recovery_deadline: Some(recovery_deadline),
                recovery_attempts: 0,
                recovery_attempt_in_flight: false,
                next_recovery_at: Some(now),
                ended_at: None,
            })
        }
        (RuntimeEvidence::RecoveryAttempt, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            require_recovering(current, now)?;
            if current.recovery_attempt_in_flight {
                return Err(RuntimeSupervisionError::Invalid(
                    "the preceding recovery attempt is still in flight".to_owned(),
                ));
            }
            if current
                .next_recovery_at
                .is_none_or(|eligible_at| eligible_at > now)
            {
                return Err(RuntimeSupervisionError::Invalid(
                    "the next recovery attempt is still inside backoff".to_owned(),
                ));
            }
            if current.recovery_attempts >= policy.max_recovery_attempts {
                return Err(RuntimeSupervisionError::Invalid(
                    "recovery attempt limit is exhausted".to_owned(),
                ));
            }
            Ok(RuntimeTransition {
                runtime_epoch: next_epoch(current.runtime_epoch)?,
                recovery_attempts: current.recovery_attempts + 1,
                recovery_attempt_in_flight: true,
                next_recovery_at: None,
                ..current
            })
        }
        (RuntimeEvidence::RecoverySucceeded, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            require_recovering_state(current)?;
            if !current.recovery_attempt_in_flight {
                return Err(RuntimeSupervisionError::Invalid(
                    "recovery_succeeded requires an in-flight recovery attempt".to_owned(),
                ));
            }
            Ok(RuntimeTransition {
                runtime_epoch: current.runtime_epoch,
                availability: RuntimeAvailability::Available,
                lease_expires_at: Some(lease_deadline),
                recovery_started_at: None,
                recovery_deadline: None,
                recovery_attempts: 0,
                recovery_attempt_in_flight: false,
                next_recovery_at: None,
                ended_at: None,
            })
        }
        (RuntimeEvidence::RecoveryFailed { .. }, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            require_recovering_state(current)?;
            if !current.recovery_attempt_in_flight {
                return Err(RuntimeSupervisionError::Invalid(
                    "recovery_failed requires an in-flight recovery_attempt".to_owned(),
                ));
            }
            let exhausted = current.recovery_attempts >= policy.max_recovery_attempts
                || current
                    .recovery_deadline
                    .is_some_and(|deadline| deadline <= now);
            Ok(RuntimeTransition {
                availability: if exhausted {
                    RuntimeAvailability::Unavailable
                } else {
                    RuntimeAvailability::Recovering
                },
                recovery_attempt_in_flight: false,
                next_recovery_at: if exhausted {
                    None
                } else {
                    Some(now + recovery_backoff(policy, current.recovery_attempts))
                },
                ..current
            })
        }
        (RuntimeEvidence::SupervisorHeartbeat, Some(current)) => {
            require_epoch(request, current.runtime_epoch)?;
            if current.availability == RuntimeAvailability::Available {
                return Err(RuntimeSupervisionError::Invalid(
                    "available runtimes must use lease_renewed".to_owned(),
                ));
            }
            let exhausted = current.availability == RuntimeAvailability::Unavailable
                || (!current.recovery_attempt_in_flight
                    && current.recovery_attempts > 0
                    && (current.recovery_attempts >= policy.max_recovery_attempts
                        || current
                            .recovery_deadline
                            .is_some_and(|deadline| deadline <= now)));
            Ok(RuntimeTransition {
                availability: if exhausted {
                    RuntimeAvailability::Unavailable
                } else {
                    RuntimeAvailability::Recovering
                },
                recovery_attempt_in_flight: current.recovery_attempt_in_flight,
                next_recovery_at: if exhausted {
                    None
                } else {
                    current.next_recovery_at
                },
                ..current
            })
        }
    }
}

fn current_runtime(row: &sqlx::postgres::PgRow) -> RuntimeSupervisionResult<RuntimeTransition> {
    Ok(RuntimeTransition {
        runtime_epoch: db_u64(row.try_get("runtime_epoch")?, "runtime_epoch")?,
        availability: parse_availability(row.try_get("availability")?)?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        recovery_started_at: row.try_get("recovery_started_at")?,
        recovery_deadline: row.try_get("recovery_deadline")?,
        recovery_attempts: db_u32(row.try_get("recovery_attempts")?, "recovery_attempts")?,
        recovery_attempt_in_flight: row.try_get("recovery_attempt_in_flight")?,
        next_recovery_at: row.try_get("next_recovery_at")?,
        ended_at: row.try_get("ended_at")?,
    })
}

fn require_epoch(
    request: &RuntimeEvidenceRequest,
    current_epoch: u64,
) -> RuntimeSupervisionResult<()> {
    if request.runtime_epoch == Some(current_epoch) {
        Ok(())
    } else {
        Err(RuntimeSupervisionError::StaleEpoch)
    }
}

fn require_recovering(
    current: RuntimeTransition,
    now: DateTime<Utc>,
) -> RuntimeSupervisionResult<()> {
    require_recovering_state(current)?;
    if current
        .recovery_deadline
        .is_some_and(|deadline| deadline <= now)
    {
        return Err(RuntimeSupervisionError::Invalid(
            "recovery window is exhausted".to_owned(),
        ));
    }
    Ok(())
}

fn require_recovering_state(current: RuntimeTransition) -> RuntimeSupervisionResult<()> {
    if current.availability != RuntimeAvailability::Recovering {
        return Err(RuntimeSupervisionError::Invalid(
            "recovery transition requires a recovering runtime".to_owned(),
        ));
    }
    Ok(())
}

fn recovery_backoff(policy: RuntimeRecoveryPolicy, completed_attempts: u32) -> Duration {
    const MAX_BACKOFF_SECONDS: u64 = 300;

    let exponent = completed_attempts.saturating_sub(1).min(31);
    let multiplier = 1_u64 << exponent;
    let seconds = u64::from(policy.recovery_backoff_seconds)
        .saturating_mul(multiplier)
        .min(MAX_BACKOFF_SECONDS);
    Duration::seconds(i64::try_from(seconds).unwrap_or(i64::MAX))
}

fn next_epoch(current: u64) -> RuntimeSupervisionResult<u64> {
    current
        .checked_add(1)
        .filter(|epoch| *epoch <= buzz_project_view::MAX_SAFE_REVISION)
        .ok_or_else(|| RuntimeSupervisionError::Invalid("runtime epoch overflow".to_owned()))
}

fn binding_from_row(
    community_id: CommunityId,
    assignment_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> RuntimeSupervisionResult<RuntimeSupervisorBinding> {
    Ok(RuntimeSupervisorBinding {
        community_id,
        binding_id: row.try_get("binding_id")?,
        assignment_id,
        supervisor_pubkey: public_key(
            &row.try_get::<Vec<u8>, _>("supervisor_pubkey")?,
            "supervisor_pubkey",
        )?,
        policy: policy_from_row(row)?,
        registered_at: row.try_get("registered_at")?,
    })
}

fn policy_from_row(row: &sqlx::postgres::PgRow) -> RuntimeSupervisionResult<RuntimeRecoveryPolicy> {
    Ok(RuntimeRecoveryPolicy {
        lease_seconds: db_u32(row.try_get("lease_seconds")?, "lease_seconds")?,
        recovery_window_seconds: db_u32(
            row.try_get("recovery_window_seconds")?,
            "recovery_window_seconds",
        )?,
        max_recovery_attempts: db_u32(
            row.try_get("max_recovery_attempts")?,
            "max_recovery_attempts",
        )?,
        recovery_backoff_seconds: db_u32(
            row.try_get("recovery_backoff_seconds")?,
            "recovery_backoff_seconds",
        )?,
        monitor_timeout_seconds: db_u32(
            row.try_get("monitor_timeout_seconds")?,
            "monitor_timeout_seconds",
        )?,
        monitor_grace_seconds: db_u32(
            row.try_get("monitor_grace_seconds")?,
            "monitor_grace_seconds",
        )?,
        automatic_unrecoverable: row.try_get("automatic_unrecoverable")?,
    })
}

fn parse_availability(value: String) -> RuntimeSupervisionResult<RuntimeAvailability> {
    match value.as_str() {
        "available" => Ok(RuntimeAvailability::Available),
        "recovering" => Ok(RuntimeAvailability::Recovering),
        "unavailable" => Ok(RuntimeAvailability::Unavailable),
        _ => Err(RuntimeSupervisionError::Invalid(format!(
            "unknown stored runtime availability {value:?}"
        ))),
    }
}

fn public_key(bytes: &[u8], field: &str) -> RuntimeSupervisionResult<PublicKey> {
    PublicKey::from_slice(bytes).map_err(|error| {
        RuntimeSupervisionError::Invalid(format!("invalid stored {field}: {error}"))
    })
}

fn db_u64(value: i64, field: &str) -> RuntimeSupervisionResult<u64> {
    u64::try_from(value).map_err(|_| {
        RuntimeSupervisionError::Invalid(format!("stored {field} must be non-negative"))
    })
}

fn db_u32(value: i32, field: &str) -> RuntimeSupervisionResult<u32> {
    u32::try_from(value).map_err(|_| {
        RuntimeSupervisionError::Invalid(format!("stored {field} must be non-negative"))
    })
}

fn bytes32(bytes: Vec<u8>, field: &str) -> RuntimeSupervisionResult<[u8; 32]> {
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        RuntimeSupervisionError::Invalid(format!(
            "stored {field} must be 32 bytes, got {}",
            bytes.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_project_view::v2::RUNTIME_SUPERVISION_SCHEMA_VERSION;

    fn request(evidence: RuntimeEvidence, epoch: Option<u64>) -> RuntimeEvidenceRequest {
        RuntimeEvidenceRequest {
            schema_version: RUNTIME_SUPERVISION_SCHEMA_VERSION,
            assignment_id: Uuid::new_v4(),
            runtime_id: Uuid::new_v4(),
            idempotency_key: Uuid::new_v4(),
            runtime_epoch: epoch,
            evidence,
        }
    }

    #[test]
    fn start_allocates_first_epoch_and_healthy_lease() {
        let policy = RuntimeRecoveryPolicy::default();
        let now = Utc::now();
        let started =
            apply_evidence_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        assert_eq!(started.runtime_epoch, 1);
        assert_eq!(started.availability, RuntimeAvailability::Available);
        assert!(!started.recovery_attempt_in_flight);
        assert!(started.next_recovery_at.is_none());
        assert!(started
            .lease_expires_at
            .is_some_and(|deadline| deadline > now));
    }

    #[test]
    fn graceful_stop_retires_only_the_runtime_without_failure_evidence() {
        let policy = RuntimeRecoveryPolicy::default();
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let stopped = apply_runtime_transition(
            Some(started),
            &request(RuntimeEvidence::GracefulStop, Some(1)),
            policy,
            now + Duration::seconds(1),
        )
        .expect("graceful stop");
        assert_eq!(stopped.availability, RuntimeAvailability::Available);
        assert!(stopped.lease_expires_at.is_none());
        assert_eq!(stopped.ended_at, Some(now + Duration::seconds(1)));
        assert!(stopped.recovery_started_at.is_none());
        assert_eq!(stopped.recovery_attempts, 0);

        let late_renewal = apply_runtime_transition(
            Some(stopped),
            &request(RuntimeEvidence::LeaseRenewed, Some(1)),
            policy,
            now + Duration::seconds(2),
        );
        assert!(matches!(
            late_renewal,
            Err(RuntimeSupervisionError::Invalid(message))
                if message.contains("already been retired")
        ));

        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: None,
                    exit_code: None,
                },
                Some(1),
            ),
            policy,
            now + Duration::seconds(1),
        )
        .expect("abnormal exit");
        let bypass_recovery = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::GracefulStop, Some(1)),
            policy,
            now + Duration::seconds(2),
        );
        assert!(matches!(
            bypass_recovery,
            Err(RuntimeSupervisionError::Invalid(message))
                if message.contains("requires an available runtime")
        ));
    }

    #[test]
    fn policy_defaults_are_safe_and_automation_is_off() {
        let policy = RuntimeRecoveryPolicy::default();
        assert!(policy.validate().is_ok());
        assert!(!policy.automatic_unrecoverable);
    }

    #[test]
    fn recovery_only_becomes_terminal_after_a_recorded_failed_attempt() {
        let policy = RuntimeRecoveryPolicy {
            max_recovery_attempts: 1,
            ..RuntimeRecoveryPolicy::default()
        };
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: Some("process exited".to_owned()),
                    exit_code: Some(1),
                },
                Some(1),
            ),
            policy,
            now,
        )
        .expect("abnormal exit");
        assert_eq!(recovering.availability, RuntimeAvailability::Recovering);

        let attempting = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::RecoveryAttempt, Some(1)),
            policy,
            now,
        )
        .expect("recovery attempt");
        assert_eq!(attempting.availability, RuntimeAvailability::Recovering);
        assert_eq!(attempting.recovery_attempts, 1);
        assert_eq!(attempting.runtime_epoch, 2);

        let failed = apply_runtime_transition(
            Some(attempting),
            &request(
                RuntimeEvidence::RecoveryFailed {
                    summary: Some("replacement did not start".to_owned()),
                },
                Some(2),
            ),
            policy,
            now,
        )
        .expect("terminal failure evidence");
        assert_eq!(failed.availability, RuntimeAvailability::Unavailable);
    }

    #[test]
    fn successful_recovery_keeps_assignment_runtime_but_allocates_a_new_epoch() {
        let policy = RuntimeRecoveryPolicy::default();
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: None,
                    exit_code: None,
                },
                Some(1),
            ),
            policy,
            now,
        )
        .expect("abnormal exit");
        let attempting = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::RecoveryAttempt, Some(1)),
            policy,
            now,
        )
        .expect("recovery attempt");
        let recovered = apply_runtime_transition(
            Some(attempting),
            &request(RuntimeEvidence::RecoverySucceeded, Some(2)),
            policy,
            now,
        )
        .expect("recovery");
        assert_eq!(recovered.runtime_epoch, 2);
        assert_eq!(recovered.availability, RuntimeAvailability::Available);

        let stale = apply_runtime_transition(
            Some(recovered),
            &request(RuntimeEvidence::LeaseRenewed, Some(1)),
            policy,
            now,
        );
        assert!(matches!(stale, Err(RuntimeSupervisionError::StaleEpoch)));
        assert!(apply_runtime_transition(
            Some(recovered),
            &request(RuntimeEvidence::LeaseRenewed, Some(2)),
            policy,
            now,
        )
        .is_ok());
    }

    #[test]
    fn trusted_heartbeat_only_closes_an_expired_window_after_a_failed_attempt() {
        let policy = RuntimeRecoveryPolicy::default();
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: None,
                    exit_code: None,
                },
                Some(1),
            ),
            policy,
            now,
        )
        .expect("abnormal exit");
        let without_attempt = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::SupervisorHeartbeat, Some(1)),
            policy,
            now + Duration::seconds(i64::from(policy.recovery_window_seconds) + 1),
        )
        .expect("heartbeat without recovery evidence");
        assert_eq!(
            without_attempt.availability,
            RuntimeAvailability::Recovering
        );
        let attempting = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::RecoveryAttempt, Some(1)),
            policy,
            now,
        )
        .expect("recovery attempt");
        let failed = apply_runtime_transition(
            Some(attempting),
            &request(RuntimeEvidence::RecoveryFailed { summary: None }, Some(2)),
            policy,
            now,
        )
        .expect("failed recovery result");
        let after_deadline = now + Duration::seconds(i64::from(policy.recovery_window_seconds) + 1);
        let unavailable = apply_runtime_transition(
            Some(failed),
            &request(RuntimeEvidence::SupervisorHeartbeat, Some(2)),
            policy,
            after_deadline,
        )
        .expect("trusted monitor observation");
        assert_eq!(unavailable.availability, RuntimeAvailability::Unavailable);
    }

    #[test]
    fn in_flight_attempt_blocks_terminal_heartbeat_past_the_deadline() {
        let policy = RuntimeRecoveryPolicy {
            max_recovery_attempts: 1,
            ..RuntimeRecoveryPolicy::default()
        };
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: None,
                    exit_code: Some(1),
                },
                Some(1),
            ),
            policy,
            now,
        )
        .expect("abnormal exit");
        let attempting = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::RecoveryAttempt, Some(1)),
            policy,
            now,
        )
        .expect("last recovery attempt");
        let after_deadline = now + Duration::seconds(i64::from(policy.recovery_window_seconds) + 1);
        let still_recovering = apply_runtime_transition(
            Some(attempting),
            &request(RuntimeEvidence::SupervisorHeartbeat, Some(2)),
            policy,
            after_deadline,
        )
        .expect("heartbeat while attempt remains in flight");
        assert_eq!(
            still_recovering.availability,
            RuntimeAvailability::Recovering
        );
        assert!(still_recovering.recovery_attempt_in_flight);
    }

    #[test]
    fn failed_attempt_enforces_exponential_backoff_and_result_ordering() {
        let policy = RuntimeRecoveryPolicy {
            max_recovery_attempts: 3,
            recovery_backoff_seconds: 5,
            ..RuntimeRecoveryPolicy::default()
        };
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let recovering = apply_runtime_transition(
            Some(started),
            &request(
                RuntimeEvidence::AbnormalExit {
                    summary: None,
                    exit_code: Some(1),
                },
                Some(1),
            ),
            policy,
            now,
        )
        .expect("abnormal exit");
        let attempting = apply_runtime_transition(
            Some(recovering),
            &request(RuntimeEvidence::RecoveryAttempt, Some(1)),
            policy,
            now,
        )
        .expect("first attempt");
        assert!(apply_runtime_transition(
            Some(attempting),
            &request(RuntimeEvidence::RecoveryAttempt, Some(2)),
            policy,
            now,
        )
        .is_err());
        let failed = apply_runtime_transition(
            Some(attempting),
            &request(RuntimeEvidence::RecoveryFailed { summary: None }, Some(2)),
            policy,
            now,
        )
        .expect("first failed result");
        assert!(!failed.recovery_attempt_in_flight);
        assert_eq!(failed.next_recovery_at, Some(now + Duration::seconds(5)));
        assert!(apply_runtime_transition(
            Some(failed),
            &request(RuntimeEvidence::RecoveryAttempt, Some(2)),
            policy,
            now + Duration::seconds(4),
        )
        .is_err());
        let second_attempt = apply_runtime_transition(
            Some(failed),
            &request(RuntimeEvidence::RecoveryAttempt, Some(2)),
            policy,
            now + Duration::seconds(5),
        )
        .expect("second attempt after backoff");
        assert_eq!(second_attempt.runtime_epoch, 3);
        assert!(second_attempt.recovery_attempt_in_flight);
    }

    #[test]
    fn logical_runtime_cannot_be_restarted_without_epoch_transition() {
        let policy = RuntimeRecoveryPolicy::default();
        let now = Utc::now();
        let started =
            apply_runtime_transition(None, &request(RuntimeEvidence::Start, None), policy, now)
                .expect("start");
        let duplicate = apply_runtime_transition(
            Some(started),
            &request(RuntimeEvidence::Start, None),
            policy,
            now,
        );
        assert!(matches!(
            duplicate,
            Err(RuntimeSupervisionError::Invalid(message))
                if message.contains("recovery flow")
        ));
    }

    #[test]
    fn idempotency_digest_commits_to_the_complete_evidence_request() {
        let first = request(RuntimeEvidence::LeaseRenewed, Some(1));
        let mut changed = first.clone();
        changed.evidence = RuntimeEvidence::AbnormalExit {
            summary: None,
            exit_code: Some(1),
        };
        assert_eq!(
            runtime_evidence_request_hash(&first).expect("first digest"),
            runtime_evidence_request_hash(&first).expect("stable digest")
        );
        assert_ne!(
            runtime_evidence_request_hash(&first).expect("first digest"),
            runtime_evidence_request_hash(&changed).expect("changed digest")
        );
    }
}
