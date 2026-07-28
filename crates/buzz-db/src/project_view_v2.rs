//! Atomic Project View v2 Role Proposal and Assignment coordinator.
//!
//! A caller holds [`ProjectViewV2WriteTx`] across pure reduction and Relay
//! signing. All canonical rows are staged in the same SQL transaction; the
//! final commit also stores the command, receipt, entity heads, metadata head,
//! membership role changes, and the exact NIP-43 snapshot.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::ChangeSource;
use buzz_project_view::v2::{
    AssignmentEndReason, CommunityMemberRole, GeneratedRoleContinuityIds, MemberGovernance,
    ProposalStatus, ProposalType, RoleAssignment, RoleAssignmentProposal, RoleCommand,
    RoleContinuityChange, RoleContinuityEntity, RoleContinuityError, RoleContinuityState,
    RoleDefinition, RoleHandoff, RoleLevel, RoleSlot,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

/// Errors returned by the v2 Role continuity transaction.
#[derive(Debug, thiserror::Error)]
pub enum ProjectViewV2WriteError {
    /// Database abstraction failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// SQL execution failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Pure v2 state machine rejected the command.
    #[error(transparent)]
    Domain(#[from] RoleContinuityError),
    /// Community is not an initialized, enabled v2 Project View.
    #[error("Project View v2 is unavailable for community {community_id}")]
    Unavailable {
        /// Unavailable Community.
        community_id: CommunityId,
    },
    /// Relay supplied a projection bundle that does not match the staged
    /// canonical change.
    #[error("invalid prepared Project View v2 commit: {0}")]
    InvalidCommit(String),
}

/// Convenient v2 write result.
pub type ProjectViewV2WriteResult<T> = Result<T, ProjectViewV2WriteError>;

/// Current NIP-43 member tuple used to build and verify one atomic snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2MembershipEntry {
    /// Lowercase hex public key.
    pub pubkey: String,
    /// `owner`, `admin`, or `member`.
    pub role: String,
}

/// Canonical entity counts written into v2 metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V2CanonicalCounts {
    /// Active Project View objects.
    pub active_objects: u32,
    /// Durably open Proposals.
    pub open_proposals: u32,
    /// Active Assignments.
    pub active_assignments: u32,
    /// Active Commitments.
    pub active_commitments: u32,
    /// Checkpoints.
    pub checkpoints: u32,
    /// Handoffs.
    pub handoffs: u32,
}

/// Stored v2 idempotency receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectViewV2Receipt {
    /// Stable change/event ID.
    pub change_id: [u8; 32],
    /// Accepted project revision.
    pub project_revision: u64,
    /// Verified actor.
    pub actor_pubkey: PublicKey,
    /// Stable operation.
    pub operation: String,
    /// Stable successful response.
    pub result: Value,
    /// Relay canonical acceptance time.
    pub accepted_at: DateTime<Utc>,
}

/// Prepared state returned to the Relay for signing.
#[derive(Debug, Clone)]
pub struct PreparedV2RoleChange {
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// New project revision.
    pub project_revision: u64,
    /// Existing projection generation.
    pub projection_generation: u64,
    /// Expected stable Relay signer.
    pub projection_pubkey: PublicKey,
    /// Relay canonical change time.
    pub canonical_time: DateTime<Utc>,
    /// Exact changed entity heads.
    pub changes: Vec<RoleContinuityChange>,
    /// Counts after the staged change.
    pub counts: V2CanonicalCounts,
    /// NIP-43 rows before the staged change.
    pub membership_before: Vec<V2MembershipEntry>,
    /// NIP-43 rows after the staged change.
    pub membership_after: Vec<V2MembershipEntry>,
    /// Existing snapshot pointer, absent only during controlled cutover.
    pub membership_snapshot_event_id: Option<EventId>,
    /// Stable receipt response.
    pub receipt_result: Value,
}

impl PreparedV2RoleChange {
    /// Return whether the transaction must publish a replacement membership
    /// snapshot.
    #[must_use]
    pub fn membership_changed(&self) -> bool {
        self.membership_before != self.membership_after
            || self.membership_snapshot_event_id.is_none()
    }
}

/// Signed projection associated with one changed continuity entity.
#[derive(Debug, Clone)]
pub struct PreparedV2EntityProjection {
    /// Entity type.
    pub entity_type: RoleContinuityEntity,
    /// Stable entity ID.
    pub entity_id: Uuid,
    /// Signed kind `40903` event.
    pub event: Event,
}

/// Relay-signed material completing a staged v2 transaction.
#[derive(Debug, Clone)]
pub struct PreparedV2RoleCommit {
    /// Original accepted member command.
    pub command_event: Event,
    /// One signed head per changed entity.
    pub entity_projections: Vec<PreparedV2EntityProjection>,
    /// Signed kind `40904` metadata head.
    pub meta_projection: Event,
    /// New NIP-43 snapshot when canonical membership changed.
    pub membership_projection: Option<Event>,
}

/// Result of committing a new v2 role change.
#[derive(Debug, Clone)]
pub struct ProjectViewV2CommitOutcome {
    /// Durable receipt.
    pub receipt: ProjectViewV2Receipt,
    /// Newly stored events in dispatch order: command, optional membership,
    /// entity heads, then metadata.
    pub events: Vec<Event>,
}

/// Explicit mapping of one existing v1 Community admin into one Leader Role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewV2AdminAssignment {
    /// Existing non-owner Community admin.
    pub member_pubkey: PublicKey,
    /// Existing active Role that becomes an admin-level Leader Role.
    pub role_id: Uuid,
}

/// Explicit, audited v1-to-v2 cutover plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewV2CutoverPlan {
    /// Existing admins that retain admin through a Leader Assignment.
    pub admin_assignments: Vec<ProjectViewV2AdminAssignment>,
    /// Existing admins explicitly downgraded to ordinary members.
    pub downgraded_admins: Vec<PublicKey>,
    /// Positive Community audit sequence allocated to this operator action.
    pub audit_seq: i64,
    /// Domain-separated hash of the operator idempotency key.
    pub idempotency_key_hash: [u8; 32],
}

/// Result of an explicit v1-to-v2 cutover.
#[derive(Debug, Clone)]
pub struct ProjectViewV2CutoverOutcome {
    /// New project revision.
    pub project_revision: u64,
    /// New projection generation.
    pub projection_generation: u64,
    /// Stable operator receipt.
    pub result: Value,
    /// Relay-signed events to fan out after commit.
    pub events: Vec<Event>,
    /// Whether an existing idempotency receipt was returned.
    pub replayed: bool,
}

/// Preparation may discover an already accepted event after current security
/// fencing.
#[derive(Debug, Clone)]
pub enum ProjectViewV2PrepareOutcome {
    /// Existing receipt; no revision was allocated.
    Replayed(ProjectViewV2Receipt),
    /// New change staged inside the still-open transaction.
    Prepared(PreparedV2RoleChange),
}

/// Caller-owned v2 write transaction holding the Community Project lock.
pub struct ProjectViewV2WriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    basis: Option<V2PreparedBasis>,
}

#[derive(Debug, Clone)]
struct V2PreparedBasis {
    command: RoleCommand,
    command_event_id: [u8; 32],
    actor: PublicKey,
    preparation: PreparedV2RoleChange,
    old_meta_projection_id: [u8; 32],
    old_projection_ids: BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>,
}

impl std::fmt::Debug for ProjectViewV2WriteTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectViewV2WriteTx")
            .field("community_id", &self.community_id)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Return one Community's configured Project View schema version.
    pub async fn project_view_schema_version(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<i16> {
        crate::relay_members::project_view_schema_version(&self.pool, community_id).await
    }

    /// Return whether an enabled v2 Community has a complete, live projection
    /// root for the expected stable Relay signer.
    pub async fn project_view_v2_capability_ready(
        &self,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        if crate::relay_members::project_view_schema_version(&self.pool, community_id).await? != 2 {
            return Ok(false);
        }
        let relay_pubkey = relay_pubkey.to_bytes();
        let row: Option<bool> = sqlx::query_scalar(
            "SELECT c.project_view_enabled \
                    AND c.archived_at IS NULL \
                    AND s.schema_version = 2 \
                    AND s.projection_pubkey = $2 \
                    AND s.membership_snapshot_event_id IS NOT NULL \
                    AND EXISTS ( \
                        SELECT 1 FROM events meta \
                        WHERE meta.community_id = c.id \
                          AND meta.id = s.meta_projection_event_id \
                          AND meta.kind = $3 AND meta.pubkey = $2 \
                          AND meta.deleted_at IS NULL \
                          AND meta.content::jsonb->>'schema_version' = '2' \
                          AND (meta.content::jsonb->>'project_revision')::bigint = s.project_revision \
                          AND (meta.content::jsonb->>'projection_generation')::bigint = s.projection_generation \
                          AND decode(meta.content::jsonb->>'membership_snapshot_event_id', 'hex') \
                              = s.membership_snapshot_event_id \
                    ) \
                    AND EXISTS ( \
                        SELECT 1 FROM events membership \
                        WHERE membership.community_id = c.id \
                          AND membership.id = s.membership_snapshot_event_id \
                          AND membership.kind = $4 AND membership.pubkey = $2 \
                          AND membership.deleted_at IS NULL \
                          AND membership.content = '' \
                          AND membership.tags = ( \
                              SELECT jsonb_build_array(jsonb_build_array('-')) \
                                  || COALESCE(jsonb_agg( \
                                      jsonb_build_array('member', member.pubkey, member.role) \
                                      ORDER BY member.pubkey \
                                  ), '[]'::jsonb) \
                              FROM relay_members member \
                              WHERE member.community_id = c.id \
                          ) \
                    ) \
                    AND s.active_object_count = ( \
                        SELECT count(*)::integer FROM project_view_objects object \
                        WHERE object.community_id = c.id AND object.deleted_at IS NULL \
                    ) \
                    AND s.open_proposal_count = ( \
                        SELECT count(*)::integer FROM project_role_assignment_proposals proposal \
                        WHERE proposal.community_id = c.id AND proposal.status = 'open' \
                    ) \
                    AND s.active_assignment_count = ( \
                        SELECT count(*)::integer FROM project_role_assignments assignment \
                        WHERE assignment.community_id = c.id AND assignment.ended_at IS NULL \
                    ) \
                    AND s.active_commitment_count = ( \
                        SELECT count(*)::integer FROM project_work_commitments commitment \
                        WHERE commitment.community_id = c.id AND commitment.ended_at IS NULL \
                    ) \
                    AND s.checkpoint_count = ( \
                        SELECT count(*)::integer FROM project_role_checkpoints checkpoint \
                        WHERE checkpoint.community_id = c.id \
                    ) \
                    AND s.handoff_count = ( \
                        SELECT count(*)::integer FROM project_role_handoffs handoff \
                        WHERE handoff.community_id = c.id \
                    ) \
                    AND NOT EXISTS ( \
                        SELECT 1 FROM project_view_objects object \
                        WHERE object.community_id = c.id AND object.schema_version <> 2 \
                    ) \
                    AND NOT EXISTS ( \
                        SELECT 1 \
                        FROM ( \
                            SELECT projection_event_id FROM project_view_objects WHERE community_id = c.id \
                            UNION ALL \
                            SELECT projection_event_id FROM project_role_assignment_proposals WHERE community_id = c.id \
                            UNION ALL \
                            SELECT projection_event_id FROM project_role_assignments WHERE community_id = c.id \
                            UNION ALL \
                            SELECT projection_event_id FROM project_work_commitments WHERE community_id = c.id \
                            UNION ALL \
                            SELECT projection_event_id FROM project_role_checkpoints WHERE community_id = c.id \
                            UNION ALL \
                            SELECT projection_event_id FROM project_role_handoffs WHERE community_id = c.id \
                        ) head \
                        LEFT JOIN events projection \
                          ON projection.community_id = c.id \
                         AND projection.id = head.projection_event_id \
                         AND projection.kind = $5 \
                         AND projection.pubkey = $2 \
                         AND projection.deleted_at IS NULL \
                        WHERE head.projection_event_id IS NULL \
                           OR projection.id IS NULL \
                           OR projection.content::jsonb->>'schema_version' IS DISTINCT FROM '2' \
                           OR (projection.content::jsonb->>'projection_generation')::bigint \
                              IS DISTINCT FROM s.projection_generation \
                    ) \
             FROM communities c \
             JOIN project_view_state s ON s.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(relay_pubkey.as_slice())
        .bind(i32::try_from(KIND_PROJECT_VIEW_META).map_err(|_| {
            DbError::InvalidData("Project View meta kind exceeds PostgreSQL INT".to_owned())
        })?)
        .bind(i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
            DbError::InvalidData("membership kind exceeds PostgreSQL INT".to_owned())
        })?)
        .bind(i32::try_from(KIND_PROJECT_VIEW_OBJECT).map_err(|_| {
            DbError::InvalidData("Project View object kind exceeds PostgreSQL INT".to_owned())
        })?)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row == Some(true))
    }

    /// Look up an already accepted operator change by its hashed idempotency
    /// key. Control-plane callers use this before allocating another audit
    /// entry on retry.
    pub async fn project_view_v2_operator_receipt(
        &self,
        community_id: CommunityId,
        idempotency_key_hash: &[u8; 32],
    ) -> crate::Result<Option<Value>> {
        sqlx::query_scalar(
            "SELECT result FROM project_view_changes \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(idempotency_key_hash.as_slice())
        .fetch_optional(&self.pool)
        .await
        .map_err(Into::into)
    }

    /// Explicitly cut one disabled, initialized v1 Project View over to v2.
    ///
    /// Every existing non-owner admin must appear exactly once in either
    /// `admin_assignments` or `downgraded_admins`; no Role or Member mapping
    /// is inferred. The cutover, initial Leader tenures, Community roles,
    /// NIP-43 snapshot, v2 entity heads, reset metadata, and operator receipt
    /// commit atomically under the Community Project lock.
    pub async fn cutover_project_view_v2(
        &self,
        community_id: CommunityId,
        plan: &ProjectViewV2CutoverPlan,
        relay_keys: &Keys,
    ) -> ProjectViewV2WriteResult<ProjectViewV2CutoverOutcome> {
        let source =
            ChangeSource::operator(plan.audit_seq, plan.idempotency_key_hash).map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "invalid operator cutover source: {error}"
                ))
            })?;
        let change_id = source.change_id();
        let change_event_id = EventId::from_slice(&change_id).map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!("invalid cutover change ID: {error}"))
        })?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;

        if let Some(row) = sqlx::query(
            "SELECT c.result, c.project_revision, s.projection_generation \
             FROM project_view_changes c \
             JOIN project_view_state s ON s.community_id = c.community_id \
             WHERE c.community_id = $1 AND c.idempotency_key_hash = $2",
        )
        .bind(community_id.as_uuid())
        .bind(plan.idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            let result: Value = row.try_get("result")?;
            let project_revision =
                db_revision(row.try_get("project_revision")?, "project_revision")?;
            let projection_generation = db_revision(
                row.try_get("projection_generation")?,
                "projection_generation",
            )?;
            tx.rollback().await?;
            return Ok(ProjectViewV2CutoverOutcome {
                project_revision,
                projection_generation,
                result,
                events: Vec::new(),
                replayed: true,
            });
        }

        let audit_action: Option<String> = sqlx::query_scalar(
            "SELECT action FROM audit_log \
             WHERE community_id = $1 AND seq = $2 FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(plan.audit_seq)
        .fetch_optional(&mut *tx)
        .await?;
        if audit_action.as_deref() != Some("project_view_cutover") {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover source must reference a project_view_cutover audit entry".to_owned(),
            ));
        }

        let community = sqlx::query(
            "SELECT project_view_enabled, archived_at IS NOT NULL AS archived, \
                    project_view_schema_version \
             FROM communities WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(ProjectViewV2WriteError::Unavailable { community_id })?;
        if community.try_get::<bool, _>("archived")?
            || community.try_get::<bool, _>("project_view_enabled")?
            || community.try_get::<i16, _>("project_view_schema_version")? != 1
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover requires a disabled, non-archived schema-v1 Project View".to_owned(),
            ));
        }
        let state = sqlx::query(
            "SELECT project_revision, updated_at, projection_generation, \
                    meta_projection_event_id \
             FROM project_view_state \
             WHERE community_id = $1 AND schema_version = 1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(
                "cutover requires an initialized v1 Project View".to_owned(),
            )
        })?;
        let current_revision = db_revision(state.try_get("project_revision")?, "project_revision")?;
        let current_generation = db_revision(
            state.try_get("projection_generation")?,
            "projection_generation",
        )?;
        let next_revision = current_revision
            .checked_add(1)
            .filter(|revision| *revision <= buzz_project_view::MAX_SAFE_REVISION)
            .ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(
                    "project revision overflow during cutover".to_owned(),
                )
            })?;
        let next_generation = current_generation.checked_add(1).ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(
                "projection generation overflow during cutover".to_owned(),
            )
        })?;
        let previous_updated_at: DateTime<Utc> = state.try_get("updated_at")?;
        let canonical_time: DateTime<Utc> = sqlx::query_scalar(
            "SELECT GREATEST( \
                 clock_timestamp(), \
                 $2::timestamptz + interval '1 microsecond', \
                 COALESCE(( \
                     SELECT max(created_at) + interval '1 second' FROM events \
                     WHERE community_id = $1 AND kind = $3 AND deleted_at IS NULL \
                 ), '-infinity'::timestamptz) \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(previous_updated_at)
        .bind(i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
            ProjectViewV2WriteError::InvalidCommit(
                "membership kind exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .fetch_one(&mut *tx)
        .await?;

        let owner_text: String = sqlx::query_scalar(
            "SELECT pubkey FROM relay_members \
             WHERE community_id = $1 AND role = 'owner'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        let owner = parse_pubkey(&owner_text, "Community owner")?;
        let owner_bytes = owner.to_bytes();
        let owner_is_ineligible: bool = sqlx::query_scalar(
            "SELECT \
                EXISTS ( \
                    SELECT 1 FROM users \
                    WHERE community_id = $1 AND pubkey = $2 \
                      AND agent_owner_pubkey IS NOT NULL \
                ) \
                OR EXISTS ( \
                    SELECT 1 FROM community_bans \
                    WHERE community_id = $1 AND pubkey = $2 AND banned \
                      AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp()) \
                )",
        )
        .bind(community_id.as_uuid())
        .bind(owner_bytes.as_slice())
        .fetch_one(&mut *tx)
        .await?;
        if owner_is_ineligible {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover requires an eligible non-managed Community owner".to_owned(),
            ));
        }
        let current_admin_text: Vec<String> = sqlx::query_scalar(
            "SELECT pubkey FROM relay_members \
             WHERE community_id = $1 AND role = 'admin' ORDER BY pubkey FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        let current_admins = current_admin_text
            .iter()
            .map(|pubkey| parse_pubkey(pubkey, "Community admin"))
            .collect::<ProjectViewV2WriteResult<BTreeSet<_>>>()?;
        validate_cutover_plan(plan, &current_admins)?;

        let active_role_ids: BTreeSet<Uuid> = sqlx::query_scalar(
            "SELECT object_id FROM project_view_objects \
             WHERE community_id = $1 AND object_type = 'role' \
               AND deleted_at IS NULL AND body->'active' = 'true'::jsonb",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .collect();
        for assignment in &plan.admin_assignments {
            if !active_role_ids.contains(&assignment.role_id) {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "Leader Role {} is missing or inactive",
                    assignment.role_id
                )));
            }
        }
        let occupied_v2_rows: i64 = sqlx::query_scalar(
            "SELECT \
                (SELECT count(*) FROM project_role_assignment_proposals WHERE community_id = $1) \
              + (SELECT count(*) FROM project_role_assignments WHERE community_id = $1) \
              + (SELECT count(*) FROM project_work_commitments WHERE community_id = $1) \
              + (SELECT count(*) FROM project_role_checkpoints WHERE community_id = $1) \
              + (SELECT count(*) FROM project_role_handoffs WHERE community_id = $1)",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if occupied_v2_rows != 0 {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "v1 cutover target already contains role-continuity rows".to_owned(),
            ));
        }

        let leader_role_ids = plan
            .admin_assignments
            .iter()
            .map(|assignment| assignment.role_id)
            .collect::<Vec<_>>();
        sqlx::query(
            "UPDATE project_view_objects SET \
                 schema_version = 2, \
                 role_level = CASE \
                     WHEN object_type = 'role' AND object_id = ANY($2) THEN 'admin' \
                     WHEN object_type = 'role' THEN 'member' \
                     ELSE NULL \
                 END, \
                 body = CASE \
                     WHEN object_type = 'role' THEN body || jsonb_build_object( \
                         'level', CASE WHEN object_id = ANY($2) THEN 'admin' ELSE 'member' END \
                     ) \
                     ELSE body \
                 END \
             WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(&leader_role_ids)
        .execute(&mut *tx)
        .await?;
        let downgraded = plan
            .downgraded_admins
            .iter()
            .map(PublicKey::to_hex)
            .collect::<Vec<_>>();
        if !downgraded.is_empty() {
            sqlx::query(
                "UPDATE relay_members SET role = 'member', updated_at = $3 \
                 WHERE community_id = $1 AND pubkey = ANY($2) AND role = 'admin'",
            )
            .bind(community_id.as_uuid())
            .bind(&downgraded)
            .bind(canonical_time)
            .execute(&mut *tx)
            .await?;
        }

        let assignment_seeds = plan
            .admin_assignments
            .iter()
            .map(|mapping| (mapping.clone(), Uuid::new_v4(), Uuid::new_v4()))
            .collect::<Vec<_>>();
        let result = json!({
            "operation": "cutover_v2",
            "project_revision": next_revision,
            "projection_generation": next_generation,
            "admin_assignments": assignment_seeds.iter().map(
                |(mapping, proposal_id, assignment_id)| json!({
                    "member_pubkey": mapping.member_pubkey,
                    "role_id": mapping.role_id,
                    "proposal_id": proposal_id,
                    "assignment_id": assignment_id,
                })
            ).collect::<Vec<_>>(),
            "downgraded_admins": plan.downgraded_admins,
        });
        let subject = json!({
            "from_schema_version": 1,
            "to_schema_version": 2,
            "admin_assignments": plan.admin_assignments.iter().map(|mapping| json!({
                "member_pubkey": mapping.member_pubkey,
                "role_id": mapping.role_id,
            })).collect::<Vec<_>>(),
            "downgraded_admins": plan.downgraded_admins,
        });
        sqlx::query(
            "INSERT INTO project_view_changes \
                (community_id, change_id, source_type, source_audit_seq, \
                 idempotency_key_hash, actor_pubkey, acting_assignment_id, \
                 operation, subject, project_revision, result, accepted_at) \
             VALUES ($1,$2,'operator',$3,$4,NULL,NULL,'cutover_v2',$5,$6,$7,$8)",
        )
        .bind(community_id.as_uuid())
        .bind(change_id.as_slice())
        .bind(plan.audit_seq)
        .bind(plan.idempotency_key_hash.as_slice())
        .bind(subject)
        .bind(revision_i64(next_revision, "project_revision")?)
        .bind(&result)
        .bind(canonical_time)
        .execute(&mut *tx)
        .await?;

        let mut changes = Vec::new();
        for (mapping, proposal_id, assignment_id) in &assignment_seeds {
            changes.push(RoleContinuityChange::Proposal(RoleAssignmentProposal {
                proposal_id: *proposal_id,
                role_id: mapping.role_id,
                candidate_pubkey: mapping.member_pubkey,
                proposal_type: ProposalType::Offer,
                candidate_accepted_at: Some(canonical_time),
                authorized_by: Some(owner),
                authorized_at: Some(canonical_time),
                expected_target_assignment_id: None,
                expected_candidate_assignment_id: None,
                expires_at: canonical_time + chrono::Duration::days(1),
                status: ProposalStatus::Consumed,
                reason: Some("v1_to_v2_cutover".to_owned()),
                created_by: owner,
                created_at: canonical_time,
                resolved_at: Some(canonical_time),
                entity_revision: 1,
                project_revision: next_revision,
            }));
            changes.push(RoleContinuityChange::Assignment(RoleAssignment {
                assignment_id: *assignment_id,
                role_id: mapping.role_id,
                member_pubkey: mapping.member_pubkey,
                proposal_id: *proposal_id,
                started_at: canonical_time,
                started_by: owner,
                replacement_requested_at: None,
                replacement_request_reason: None,
                unable_reported_at: None,
                unable_report_reason: None,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                replaced_by_assignment_id: None,
                entity_revision: 1,
                project_revision: next_revision,
            }));
        }
        persist_changes(&mut tx, community_id, &change_id, canonical_time, &changes).await?;

        let role_rows = sqlx::query(
            "SELECT object_id, object_revision, project_revision, body, \
                    created_at, updated_at, created_by, updated_by, \
                    projection_event_id \
             FROM project_view_objects \
             WHERE community_id = $1 AND object_type = 'role' \
               AND deleted_at IS NULL ORDER BY object_id FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        for row in role_rows {
            let body: CutoverRoleBody =
                serde_json::from_value(row.try_get("body")?).map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "invalid Role body during cutover: {error}"
                    ))
                })?;
            let role_id: Uuid = row.try_get("object_id")?;
            changes.push(RoleContinuityChange::Role(RoleDefinition {
                role_id,
                name: body.name,
                purpose: body.purpose,
                responsibilities: body.responsibilities,
                boundaries: body.boundaries,
                level: body.level,
                active: body.active,
                object_revision: db_revision(
                    row.try_get("object_revision")?,
                    "role.object_revision",
                )?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "role.project_revision",
                )?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                created_by: public_key(
                    &row.try_get::<Vec<u8>, _>("created_by")?,
                    "role.created_by",
                )?,
                updated_by: public_key(
                    &row.try_get::<Vec<u8>, _>("updated_by")?,
                    "role.updated_by",
                )?,
            }));
        }
        changes.sort_by_key(|change| (change.entity_type(), change.entity_id()));

        let object_rows = sqlx::query(
            "SELECT object_id, object_type, object_revision, project_revision, body, \
                    under_goal_id, under_plan_id, planned_in_stage_id, \
                    about_object_id, about_object_type, handles_object_id, \
                    handles_object_type, created_at, updated_at, created_by, \
                    updated_by, deleted_at, projection_event_id \
             FROM project_view_objects \
             WHERE community_id = $1 ORDER BY object_id FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        let mut old_object_projection_ids = Vec::with_capacity(object_rows.len());
        let mut project_entries = Vec::new();
        for row in object_rows {
            old_object_projection_ids.push(bytes32(
                row.try_get("projection_event_id")?,
                "Project object projection_event_id",
            )?);
            let object_type: String = row.try_get("object_type")?;
            let deleted_at: Option<DateTime<Utc>> = row.try_get("deleted_at")?;
            if object_type != "role" || deleted_at.is_some() {
                project_entries.push(crate::project_view::entry_from_row(row).map_err(
                    |error| {
                        ProjectViewV2WriteError::InvalidCommit(format!(
                            "load Project object during cutover: {error}"
                        ))
                    },
                )?);
            }
        }

        let membership = load_membership(&mut tx, community_id).await?;
        let membership_event =
            build_cutover_membership_event(&membership, canonical_time, relay_keys)?;
        let counts = load_counts(&mut tx, community_id).await?;
        let context = buzz_sdk::project_view_v2::V2ProjectionContext {
            project_id: community_id,
            projection_generation: next_generation,
            project_revision: next_revision,
            source: buzz_sdk::project_view_v2::V2ProjectionSource::Operator {
                change_id: change_event_id,
                audit_seq: u64::try_from(plan.audit_seq).map_err(|_| {
                    ProjectViewV2WriteError::InvalidCommit(
                        "audit sequence must be positive".to_owned(),
                    )
                })?,
            },
            updated_at: canonical_time,
        };
        let mut projections = Vec::with_capacity(changes.len());
        for change in &changes {
            let event = buzz_sdk::project_view_v2::build_entity_projection(&context, change)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "build cutover entity projection: {error}"
                    ))
                })?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "sign cutover entity projection: {error}"
                    ))
                })?;
            let parsed = buzz_sdk::project_view_v2::parse_entity_projection(
                &event,
                &relay_keys.public_key(),
                community_id,
            )
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "verify cutover entity projection: {error}"
                ))
            })?;
            if parsed.entity != *change
                || parsed.project_revision != next_revision
                || parsed.projection_generation != next_generation
                || parsed.source != context.source
                || parsed.updated_at != canonical_time
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover entity projection differs from canonical state".to_owned(),
                ));
            }
            projections.push(PreparedV2EntityProjection {
                entity_type: change.entity_type(),
                entity_id: change.entity_id(),
                event,
            });
        }
        let mut project_object_projections = Vec::with_capacity(project_entries.len());
        for entry in &project_entries {
            let event = buzz_sdk::project_view_v2::build_project_object_projection(&context, entry)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "build cutover Project object projection: {error}"
                    ))
                })?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "sign cutover Project object projection: {error}"
                    ))
                })?;
            let parsed = buzz_sdk::project_view_v2::parse_project_object_projection(
                &event,
                &relay_keys.public_key(),
                community_id,
            )
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "verify cutover Project object projection: {error}"
                ))
            })?;
            if parsed.object.id() != entry.id()
                || parsed.project_revision != next_revision
                || parsed.projection_generation != next_generation
                || parsed.source != context.source
                || parsed.updated_at != canonical_time
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover Project object projection differs from canonical state".to_owned(),
                ));
            }
            project_object_projections.push((entry.id(), event));
        }
        let meta_event = buzz_sdk::project_view_v2::build_meta_projection(
            &context,
            buzz_sdk::project_view_v2::V2EntityCounts {
                active_objects: counts.active_objects,
                open_proposals: counts.open_proposals,
                active_assignments: counts.active_assignments,
                active_commitments: counts.active_commitments,
                checkpoints: counts.checkpoints,
                handoffs: counts.handoffs,
            },
            membership_event.id,
            true,
            &[],
        )
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "build cutover metadata projection: {error}"
            ))
        })?
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "sign cutover metadata projection: {error}"
            ))
        })?;
        let verified_meta =
            buzz_sdk::project_view_v2::parse_meta_projection(&meta_event, &relay_keys.public_key())
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "verify cutover metadata projection: {error}"
                    ))
                })?;
        if verified_meta.project_id != community_id
            || verified_meta.project_revision != next_revision
            || verified_meta.projection_generation != next_generation
            || verified_meta.entity_counts
                != (buzz_sdk::project_view_v2::V2EntityCounts {
                    active_objects: counts.active_objects,
                    open_proposals: counts.open_proposals,
                    active_assignments: counts.active_assignments,
                    active_commitments: counts.active_commitments,
                    checkpoints: counts.checkpoints,
                    handoffs: counts.handoffs,
                })
            || verified_meta.membership_snapshot_event_id != membership_event.id
            || !verified_meta.reset
            || !verified_meta.changed_heads.is_empty()
            || verified_meta.source != context.source
            || verified_meta.updated_at != canonical_time
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover metadata projection differs from canonical state".to_owned(),
            ));
        }
        verify_membership_projection(
            &membership_event,
            relay_keys.public_key(),
            &membership,
            canonical_time,
        )?;

        for event_id in old_object_projection_ids {
            if !crate::event::retire_projection_head_in_tx(
                &mut tx,
                community_id,
                &event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "v1 Project object projection pointer is not live".to_owned(),
                ));
            }
        }
        let old_meta_id = bytes32(
            state.try_get("meta_projection_event_id")?,
            "meta_projection_event_id",
        )?;
        if !crate::event::retire_projection_head_in_tx(
            &mut tx,
            community_id,
            &old_meta_id,
            KIND_PROJECT_VIEW_META,
        )
        .await?
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "v1 metadata projection pointer is not live".to_owned(),
            ));
        }
        retire_membership_heads(&mut tx, community_id, relay_keys.public_key()).await?;
        let (_, membership_inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, &membership_event, None)
                .await?;
        if !membership_inserted {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover membership snapshot already exists".to_owned(),
            ));
        }
        for projection in &projections {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut tx, community_id, &projection.event, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover entity projection already exists".to_owned(),
                ));
            }
            update_projection_pointer(
                &mut tx,
                community_id,
                projection.entity_type,
                projection.entity_id,
                projection.event.id.as_bytes(),
            )
            .await?;
        }
        for (object_id, event) in &project_object_projections {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut tx, community_id, event, None).await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover Project object projection already exists".to_owned(),
                ));
            }
            let updated = sqlx::query(
                "UPDATE project_view_objects SET projection_event_id = $3 \
                 WHERE community_id = $1 AND object_id = $2",
            )
            .bind(community_id.as_uuid())
            .bind(object_id)
            .bind(event.id.as_bytes().as_slice())
            .execute(&mut *tx)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "Project object {object_id} disappeared during cutover"
                )));
            }
        }
        let (_, meta_inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, &meta_event, None).await?;
        if !meta_inserted {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "cutover metadata projection already exists".to_owned(),
            ));
        }

        let relay_pubkey = relay_keys.public_key().to_bytes();
        let state_update = sqlx::query(
            "UPDATE project_view_state SET \
                 project_revision = $2, updated_at = $3, last_event_id = $4, \
                 last_actor_pubkey = $5, meta_projection_event_id = $6, \
                 projection_pubkey = $7, projection_generation = $8, \
                 schema_version = 2, last_change_id = $4, \
                 last_source_event_id = NULL, open_proposal_count = $9, \
                 active_assignment_count = $10, active_commitment_count = $11, \
                 checkpoint_count = $12, handoff_count = $13, \
                 membership_snapshot_event_id = $14 \
             WHERE community_id = $1 AND project_revision = $15 AND schema_version = 1",
        )
        .bind(community_id.as_uuid())
        .bind(revision_i64(next_revision, "project_revision")?)
        .bind(canonical_time)
        .bind(change_id.as_slice())
        .bind(owner_bytes.as_slice())
        .bind(meta_event.id.as_bytes().as_slice())
        .bind(relay_pubkey.as_slice())
        .bind(revision_i64(next_generation, "projection_generation")?)
        .bind(count_i32(counts.open_proposals, "open_proposals")?)
        .bind(count_i32(counts.active_assignments, "active_assignments")?)
        .bind(count_i32(counts.active_commitments, "active_commitments")?)
        .bind(count_i32(counts.checkpoints, "checkpoints")?)
        .bind(count_i32(counts.handoffs, "handoffs")?)
        .bind(membership_event.id.as_bytes().as_slice())
        .bind(revision_i64(current_revision, "current_revision")?)
        .execute(&mut *tx)
        .await?;
        if state_update.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "Project View state changed during cutover".to_owned(),
            ));
        }
        let community_update =
            sqlx::query("UPDATE communities SET project_view_schema_version = 2 WHERE id = $1")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
        if community_update.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::Unavailable { community_id });
        }
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        let mut events = vec![membership_event];
        events.extend(projections.into_iter().map(|projection| projection.event));
        events.extend(
            project_object_projections
                .into_iter()
                .map(|(_, event)| event),
        );
        events.push(meta_event);
        Ok(ProjectViewV2CutoverOutcome {
            project_revision: next_revision,
            projection_generation: next_generation,
            result,
            events,
            replayed: false,
        })
    }

    /// Begin a v2 write under the same Community advisory lock used by all
    /// membership writers.
    pub async fn begin_project_view_v2_write(
        &self,
        community_id: CommunityId,
    ) -> ProjectViewV2WriteResult<ProjectViewV2WriteTx> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        if crate::relay_members::project_view_schema_version_in_tx(&mut tx, community_id).await?
            != 2
        {
            return Err(ProjectViewV2WriteError::Unavailable { community_id });
        }
        let available: Option<bool> = sqlx::query_scalar(
            "SELECT project_view_enabled AND archived_at IS NULL \
             FROM communities WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if available != Some(true) {
            return Err(ProjectViewV2WriteError::Unavailable { community_id });
        }
        Ok(ProjectViewV2WriteTx {
            tx,
            community_id,
            basis: None,
        })
    }
}

impl ProjectViewV2WriteTx {
    /// Explicitly roll back and release the Community lock.
    pub async fn rollback(self) -> ProjectViewV2WriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Validate current actor fencing, return a receipt if present, or stage
    /// one complete pure Role transition and its canonical SQL rows.
    pub async fn prepare_role_command(
        &mut self,
        command_event: &Event,
        command: &RoleCommand,
    ) -> ProjectViewV2WriteResult<ProjectViewV2PrepareOutcome> {
        if self.basis.is_some() {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "this transaction already has a prepared v2 change".to_owned(),
            ));
        }
        if command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION
            || RoleCommand::from_json(&command_event.content)? != *command
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "command event does not carry the supplied typed Role command".to_owned(),
            ));
        }

        let loaded = load_v2_state(&mut self.tx, self.community_id).await?;
        loaded
            .state
            .validate_actor_for_replay(command, command_event.pubkey)?;
        if let Some(receipt) =
            find_receipt(&mut self.tx, self.community_id, command_event.id.as_bytes()).await?
        {
            return Ok(ProjectViewV2PrepareOutcome::Replayed(receipt));
        }
        let generated_ids = GeneratedRoleContinuityIds {
            assignment_id: Uuid::new_v4(),
            handoff_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        };
        let (_next_state, mut outcome) = loaded.state.reduce(
            command,
            command_event.pubkey,
            loaded.canonical_time,
            &generated_ids,
        )?;
        end_commitments_for_changed_assignments(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command_event.id.as_bytes(),
            outcome.project_revision,
            loaded.canonical_time,
            &mut outcome.changes,
            &mut outcome.ended_commitments,
        )
        .await?;

        let receipt_result = role_receipt(command, &outcome.changes, outcome.project_revision);
        insert_change(
            &mut self.tx,
            self.community_id,
            command_event,
            command,
            outcome.project_revision,
            loaded.canonical_time,
            &receipt_result,
        )
        .await?;
        let old_projection_ids =
            load_old_projection_ids(&mut self.tx, self.community_id, &outcome.changes).await?;
        persist_changes(
            &mut self.tx,
            self.community_id,
            command_event.id.as_bytes(),
            loaded.canonical_time,
            &outcome.changes,
        )
        .await?;

        let membership_before = load_membership(&mut self.tx, self.community_id).await?;
        apply_membership_roles(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            &outcome.membership_roles,
            loaded.canonical_time,
        )
        .await?;
        let membership_after = load_membership(&mut self.tx, self.community_id).await?;
        let counts = load_counts(&mut self.tx, self.community_id).await?;
        let preparation = PreparedV2RoleChange {
            community_id: self.community_id,
            project_revision: outcome.project_revision,
            projection_generation: loaded.projection_generation,
            projection_pubkey: loaded.projection_pubkey,
            canonical_time: loaded.canonical_time,
            changes: outcome.changes,
            counts,
            membership_before,
            membership_after,
            membership_snapshot_event_id: loaded.membership_snapshot_event_id,
            receipt_result,
        };
        self.basis = Some(V2PreparedBasis {
            command: command.clone(),
            command_event_id: command_event.id.to_bytes(),
            actor: command_event.pubkey,
            preparation: preparation.clone(),
            old_meta_projection_id: loaded.meta_projection_event_id,
            old_projection_ids,
        });
        Ok(ProjectViewV2PrepareOutcome::Prepared(preparation))
    }

    /// Commit a staged canonical change and every signed projection.
    pub async fn commit_role_command(
        mut self,
        commit: PreparedV2RoleCommit,
    ) -> ProjectViewV2WriteResult<ProjectViewV2CommitOutcome> {
        let basis = self.basis.take().ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(
                "commit requires prepare_role_command on the same transaction".to_owned(),
            )
        })?;
        validate_commit_bundle(&basis, &commit)?;

        let (_, command_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &commit.command_event,
            None,
        )
        .await?;
        if !command_inserted {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "command event exists without its v2 receipt".to_owned(),
            ));
        }

        let mut events = vec![commit.command_event.clone()];
        let membership_event_id = if basis.preparation.membership_changed() {
            let event = commit.membership_projection.as_ref().ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(
                    "changed membership requires a signed NIP-43 snapshot".to_owned(),
                )
            })?;
            verify_membership_projection(
                event,
                basis.preparation.projection_pubkey,
                &basis.preparation.membership_after,
                basis.preparation.canonical_time,
            )?;
            retire_membership_heads(
                &mut self.tx,
                self.community_id,
                basis.preparation.projection_pubkey,
            )
            .await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, event, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "membership projection already exists".to_owned(),
                ));
            }
            events.push(event.clone());
            event.id
        } else {
            if commit.membership_projection.is_some() {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "unchanged membership must reuse its existing snapshot".to_owned(),
                ));
            }
            basis
                .preparation
                .membership_snapshot_event_id
                .ok_or_else(|| {
                    ProjectViewV2WriteError::InvalidCommit(
                        "v2 state has no membership snapshot pointer".to_owned(),
                    )
                })?
        };

        for old_event_id in basis.old_projection_ids.values() {
            if !crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "stored v2 entity projection pointer is not live".to_owned(),
                ));
            }
        }
        if !crate::event::retire_projection_head_in_tx(
            &mut self.tx,
            self.community_id,
            &basis.old_meta_projection_id,
            KIND_PROJECT_VIEW_META,
        )
        .await?
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "stored v2 metadata pointer is not live".to_owned(),
            ));
        }

        for projection in &commit.entity_projections {
            let (_, inserted) = crate::event::insert_event_in_tx(
                &mut self.tx,
                self.community_id,
                &projection.event,
                None,
            )
            .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "v2 entity projection {} already exists",
                    projection.entity_id
                )));
            }
            update_projection_pointer(
                &mut self.tx,
                self.community_id,
                projection.entity_type,
                projection.entity_id,
                projection.event.id.as_bytes(),
            )
            .await?;
            events.push(projection.event.clone());
        }
        let (_, meta_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &commit.meta_projection,
            None,
        )
        .await?;
        if !meta_inserted {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "v2 metadata projection already exists".to_owned(),
            ));
        }

        let actor_bytes = basis.actor.to_bytes();
        let result = sqlx::query(
            "UPDATE project_view_state SET \
                 project_revision = $2, updated_at = $3, last_event_id = $4, \
                 last_actor_pubkey = $5, meta_projection_event_id = $6, \
                 schema_version = 2, last_change_id = $4, \
                 last_source_event_id = $4, open_proposal_count = $7, \
                 active_assignment_count = $8, active_commitment_count = $9, \
                 checkpoint_count = $10, handoff_count = $11, \
                 membership_snapshot_event_id = $12 \
             WHERE community_id = $1 AND project_revision = $13 \
               AND schema_version = 2",
        )
        .bind(self.community_id.as_uuid())
        .bind(revision_i64(
            basis.preparation.project_revision,
            "project_revision",
        )?)
        .bind(basis.preparation.canonical_time)
        .bind(basis.command_event_id.as_slice())
        .bind(actor_bytes.as_slice())
        .bind(commit.meta_projection.id.as_bytes().as_slice())
        .bind(count_i32(
            basis.preparation.counts.open_proposals,
            "open_proposals",
        )?)
        .bind(count_i32(
            basis.preparation.counts.active_assignments,
            "active_assignments",
        )?)
        .bind(count_i32(
            basis.preparation.counts.active_commitments,
            "active_commitments",
        )?)
        .bind(count_i32(
            basis.preparation.counts.checkpoints,
            "checkpoints",
        )?)
        .bind(count_i32(basis.preparation.counts.handoffs, "handoffs")?)
        .bind(membership_event_id.as_bytes().as_slice())
        .bind(revision_i64(
            basis.command.expected_project_revision,
            "expected_project_revision",
        )?)
        .execute(&mut *self.tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::Domain(
                RoleContinuityError::RevisionConflict {
                    expected: basis.command.expected_project_revision,
                    current: basis.preparation.project_revision.saturating_sub(1),
                },
            ));
        }

        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *self.tx)
            .await?;
        self.tx.commit().await?;
        events.push(commit.meta_projection);
        let receipt = ProjectViewV2Receipt {
            change_id: basis.command_event_id,
            project_revision: basis.preparation.project_revision,
            actor_pubkey: basis.actor,
            operation: basis.command.operation().to_owned(),
            result: basis.preparation.receipt_result,
            accepted_at: basis.preparation.canonical_time,
        };
        Ok(ProjectViewV2CommitOutcome { receipt, events })
    }
}

pub(crate) async fn project_view_v2_enable_ready_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    relay_pubkey: &PublicKey,
) -> ProjectViewV2WriteResult<bool> {
    if crate::relay_members::project_view_schema_version_in_tx(tx, community_id).await? != 2 {
        return Ok(false);
    }
    let relay_pubkey = relay_pubkey.to_bytes();
    let ready: Option<bool> = sqlx::query_scalar(
        "SELECT c.archived_at IS NULL \
                AND s.schema_version = 2 \
                AND s.projection_pubkey = $2 \
                AND s.membership_snapshot_event_id IS NOT NULL \
                AND EXISTS ( \
                    SELECT 1 FROM events meta \
                    WHERE meta.community_id = c.id \
                      AND meta.id = s.meta_projection_event_id \
                      AND meta.kind = $3 AND meta.pubkey = $2 \
                      AND meta.deleted_at IS NULL \
                      AND meta.content::jsonb->>'schema_version' = '2' \
                      AND (meta.content::jsonb->>'project_revision')::bigint = s.project_revision \
                      AND (meta.content::jsonb->>'projection_generation')::bigint = s.projection_generation \
                      AND decode(meta.content::jsonb->>'membership_snapshot_event_id', 'hex') \
                          = s.membership_snapshot_event_id \
                ) \
                AND EXISTS ( \
                    SELECT 1 FROM events membership \
                    WHERE membership.community_id = c.id \
                      AND membership.id = s.membership_snapshot_event_id \
                      AND membership.kind = $4 AND membership.pubkey = $2 \
                      AND membership.deleted_at IS NULL \
                      AND membership.content = '' \
                      AND membership.tags = ( \
                          SELECT jsonb_build_array(jsonb_build_array('-')) \
                              || COALESCE(jsonb_agg( \
                                  jsonb_build_array('member', member.pubkey, member.role) \
                                  ORDER BY member.pubkey \
                              ), '[]'::jsonb) \
                          FROM relay_members member \
                          WHERE member.community_id = c.id \
                      ) \
                ) \
                AND s.active_object_count = ( \
                    SELECT count(*)::integer FROM project_view_objects object \
                    WHERE object.community_id = c.id AND object.deleted_at IS NULL \
                ) \
                AND s.open_proposal_count = ( \
                    SELECT count(*)::integer FROM project_role_assignment_proposals proposal \
                    WHERE proposal.community_id = c.id AND proposal.status = 'open' \
                ) \
                AND s.active_assignment_count = ( \
                    SELECT count(*)::integer FROM project_role_assignments assignment \
                    WHERE assignment.community_id = c.id AND assignment.ended_at IS NULL \
                ) \
                AND s.active_commitment_count = ( \
                    SELECT count(*)::integer FROM project_work_commitments commitment \
                    WHERE commitment.community_id = c.id AND commitment.ended_at IS NULL \
                ) \
                AND s.checkpoint_count = ( \
                    SELECT count(*)::integer FROM project_role_checkpoints checkpoint \
                    WHERE checkpoint.community_id = c.id \
                ) \
                AND s.handoff_count = ( \
                    SELECT count(*)::integer FROM project_role_handoffs handoff \
                    WHERE handoff.community_id = c.id \
                ) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM project_view_objects object \
                    WHERE object.community_id = c.id AND object.schema_version <> 2 \
                ) \
                AND NOT EXISTS ( \
                    SELECT 1 \
                    FROM ( \
                        SELECT projection_event_id FROM project_view_objects WHERE community_id = c.id \
                        UNION ALL \
                        SELECT projection_event_id FROM project_role_assignment_proposals WHERE community_id = c.id \
                        UNION ALL \
                        SELECT projection_event_id FROM project_role_assignments WHERE community_id = c.id \
                        UNION ALL \
                        SELECT projection_event_id FROM project_work_commitments WHERE community_id = c.id \
                        UNION ALL \
                        SELECT projection_event_id FROM project_role_checkpoints WHERE community_id = c.id \
                        UNION ALL \
                        SELECT projection_event_id FROM project_role_handoffs WHERE community_id = c.id \
                    ) head \
                    LEFT JOIN events projection \
                      ON projection.community_id = c.id \
                     AND projection.id = head.projection_event_id \
                     AND projection.kind = $5 \
                     AND projection.pubkey = $2 \
                     AND projection.deleted_at IS NULL \
                    WHERE head.projection_event_id IS NULL \
                       OR projection.id IS NULL \
                       OR projection.content::jsonb->>'schema_version' IS DISTINCT FROM '2' \
                       OR (projection.content::jsonb->>'projection_generation')::bigint \
                          IS DISTINCT FROM s.projection_generation \
                ) \
         FROM communities c \
         JOIN project_view_state s ON s.community_id = c.id \
         WHERE c.id = $1",
    )
    .bind(community_id.as_uuid())
    .bind(relay_pubkey.as_slice())
    .bind(i32::try_from(KIND_PROJECT_VIEW_META).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(
            "Project View meta kind exceeds PostgreSQL INT".to_owned(),
        )
    })?)
    .bind(i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(
            "membership kind exceeds PostgreSQL INT".to_owned(),
        )
    })?)
    .bind(i32::try_from(KIND_PROJECT_VIEW_OBJECT).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(
            "Project View object kind exceeds PostgreSQL INT".to_owned(),
        )
    })?)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(ready == Some(true))
}

#[derive(Debug)]
struct LoadedV2State {
    state: RoleContinuityState,
    canonical_time: DateTime<Utc>,
    projection_generation: u64,
    projection_pubkey: PublicKey,
    meta_projection_event_id: [u8; 32],
    membership_snapshot_event_id: Option<EventId>,
}

async fn load_v2_state(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<LoadedV2State> {
    let state_row = sqlx::query(
        "SELECT project_revision, updated_at, projection_generation, \
                projection_pubkey, meta_projection_event_id, \
                membership_snapshot_event_id \
         FROM project_view_state \
         WHERE community_id = $1 AND schema_version = 2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ProjectViewV2WriteError::Unavailable { community_id })?;
    let project_revision = db_revision(state_row.try_get("project_revision")?, "project_revision")?;
    let updated_at: DateTime<Utc> = state_row.try_get("updated_at")?;
    let membership_kind = i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit("membership kind exceeds PostgreSQL INT".to_owned())
    })?;
    let canonical_time: DateTime<Utc> = sqlx::query_scalar(
        "SELECT GREATEST( \
             clock_timestamp(), \
             $2::timestamptz + interval '1 microsecond', \
             COALESCE(( \
                 SELECT max(created_at) + interval '1 second' FROM events \
                 WHERE community_id = $1 AND kind = $3 AND deleted_at IS NULL \
             ), '-infinity'::timestamptz) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(updated_at)
    .bind(membership_kind)
    .fetch_one(&mut **tx)
    .await?;
    let projection_generation = db_revision(
        state_row.try_get("projection_generation")?,
        "projection_generation",
    )?;
    let projection_pubkey = public_key(
        &state_row.try_get::<Vec<u8>, _>("projection_pubkey")?,
        "projection_pubkey",
    )?;
    let meta_projection_event_id = bytes32(
        state_row.try_get("meta_projection_event_id")?,
        "meta_projection_event_id",
    )?;
    let membership_snapshot_event_id = state_row
        .try_get::<Option<Vec<u8>>, _>("membership_snapshot_event_id")?
        .map(|bytes| event_id(bytes, "membership_snapshot_event_id"))
        .transpose()?;

    let role_rows = sqlx::query(
        "SELECT object_id, role_level, \
                COALESCE((body->>'active')::boolean, FALSE) AS active \
         FROM project_view_objects \
         WHERE community_id = $1 AND object_type = 'role' AND schema_version = 2",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let roles = role_rows
        .into_iter()
        .map(|row| {
            let level: String = row.try_get("role_level")?;
            Ok(RoleSlot {
                role_id: row.try_get("object_id")?,
                level: parse_role_level(&level)?,
                active: row.try_get("active")?,
            })
        })
        .collect::<ProjectViewV2WriteResult<Vec<_>>>()?;
    let members = load_member_governance(tx, community_id).await?;
    let proposals = load_proposals(tx, community_id).await?;
    let assignments = load_assignments(tx, community_id).await?;
    let handoffs = load_handoffs(tx, community_id).await?;
    let state = RoleContinuityState::from_snapshot(
        project_revision,
        roles,
        members,
        proposals,
        assignments,
        handoffs,
    )?;
    Ok(LoadedV2State {
        state,
        canonical_time,
        projection_generation,
        projection_pubkey,
        meta_projection_event_id,
        membership_snapshot_event_id,
    })
}

async fn load_member_governance(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<MemberGovernance>> {
    let banned_rows: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT pubkey FROM community_bans \
         WHERE community_id = $1 AND banned \
           AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp())",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let banned = banned_rows
        .into_iter()
        .map(|bytes| public_key(&bytes, "community_bans.pubkey"))
        .collect::<ProjectViewV2WriteResult<BTreeSet<_>>>()?;

    let owner_rows =
        sqlx::query("SELECT pubkey, agent_owner_pubkey FROM users WHERE community_id = $1")
            .bind(community_id.as_uuid())
            .fetch_all(&mut **tx)
            .await?;
    let mut managed_owners = BTreeMap::new();
    for row in owner_rows {
        let pubkey = public_key(&row.try_get::<Vec<u8>, _>("pubkey")?, "users.pubkey")?;
        let owner = row
            .try_get::<Option<Vec<u8>>, _>("agent_owner_pubkey")?
            .map(|bytes| public_key(&bytes, "users.agent_owner_pubkey"))
            .transpose()?;
        managed_owners.insert(pubkey, owner);
    }

    let member_rows = sqlx::query("SELECT pubkey, role FROM relay_members WHERE community_id = $1")
        .bind(community_id.as_uuid())
        .fetch_all(&mut **tx)
        .await?;
    let mut members = BTreeMap::new();
    for row in member_rows {
        let pubkey_text: String = row.try_get("pubkey")?;
        let pubkey = parse_pubkey(&pubkey_text, "relay_members.pubkey")?;
        let community_role = parse_community_role(&row.try_get::<String, _>("role")?)?;
        members.insert(
            pubkey,
            MemberGovernance {
                pubkey,
                community_role: Some(community_role),
                eligible: !banned.contains(&pubkey),
                managed_agent_owner: managed_owners.get(&pubkey).copied().flatten(),
            },
        );
    }
    let direct_snapshot = members.clone();
    for (agent, owner) in managed_owners {
        let Some(owner) = owner else {
            continue;
        };
        let owner_eligible = direct_snapshot
            .get(&owner)
            .is_some_and(|member| member.eligible && member.managed_agent_owner.is_none());
        let eligible = owner_eligible && !banned.contains(&agent) && !banned.contains(&owner);
        members
            .entry(agent)
            .and_modify(|member| {
                member.eligible &= eligible;
                member.managed_agent_owner = Some(owner);
            })
            .or_insert(MemberGovernance {
                pubkey: agent,
                community_role: None,
                eligible,
                managed_agent_owner: Some(owner),
            });
    }
    Ok(members.into_values().collect())
}

async fn load_proposals(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<RoleAssignmentProposal>> {
    let rows = sqlx::query(
        "SELECT proposal_id, role_id, candidate_pubkey, proposal_type, \
                candidate_accepted_at, authorized_by, authorized_at, \
                expected_target_assignment_id, expected_candidate_assignment_id, \
                expires_at, status, reason, created_by, created_at, resolved_at, \
                entity_revision, project_revision \
         FROM project_role_assignment_proposals \
         WHERE community_id = $1 ORDER BY proposal_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let proposal_type: String = row.try_get("proposal_type")?;
            let status: String = row.try_get("status")?;
            Ok(RoleAssignmentProposal {
                proposal_id: row.try_get("proposal_id")?,
                role_id: row.try_get("role_id")?,
                candidate_pubkey: parse_pubkey(
                    &row.try_get::<String, _>("candidate_pubkey")?,
                    "candidate_pubkey",
                )?,
                proposal_type: parse_proposal_type(&proposal_type)?,
                candidate_accepted_at: row.try_get("candidate_accepted_at")?,
                authorized_by: row
                    .try_get::<Option<Vec<u8>>, _>("authorized_by")?
                    .map(|bytes| public_key(&bytes, "authorized_by"))
                    .transpose()?,
                authorized_at: row.try_get("authorized_at")?,
                expected_target_assignment_id: row.try_get("expected_target_assignment_id")?,
                expected_candidate_assignment_id: row
                    .try_get("expected_candidate_assignment_id")?,
                expires_at: row.try_get("expires_at")?,
                status: parse_proposal_status(&status)?,
                reason: row.try_get("reason")?,
                created_by: public_key(&row.try_get::<Vec<u8>, _>("created_by")?, "created_by")?,
                created_at: row.try_get("created_at")?,
                resolved_at: row.try_get("resolved_at")?,
                entity_revision: db_revision(row.try_get("entity_revision")?, "entity_revision")?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "project_revision",
                )?,
            })
        })
        .collect()
}

async fn load_assignments(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<RoleAssignment>> {
    let rows = sqlx::query(
        "SELECT assignment_id, role_id, member_pubkey, proposal_id, started_at, \
                started_by, replacement_requested_at, replacement_request_reason, \
                unable_reported_at, unable_report_reason, ended_at, ended_by, \
                ended_reason, replaced_by_assignment_id, entity_revision, project_revision \
         FROM project_role_assignments \
         WHERE community_id = $1 ORDER BY assignment_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let ended_reason: Option<String> = row.try_get("ended_reason")?;
            Ok(RoleAssignment {
                assignment_id: row.try_get("assignment_id")?,
                role_id: row.try_get("role_id")?,
                member_pubkey: parse_pubkey(
                    &row.try_get::<String, _>("member_pubkey")?,
                    "member_pubkey",
                )?,
                proposal_id: row
                    .try_get::<Option<Uuid>, _>("proposal_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "v2 Assignment is missing proposal_id".to_owned(),
                        )
                    })?,
                started_at: row.try_get("started_at")?,
                started_by: public_key(&row.try_get::<Vec<u8>, _>("started_by")?, "started_by")?,
                replacement_requested_at: row.try_get("replacement_requested_at")?,
                replacement_request_reason: row.try_get("replacement_request_reason")?,
                unable_reported_at: row.try_get("unable_reported_at")?,
                unable_report_reason: row.try_get("unable_report_reason")?,
                ended_at: row.try_get("ended_at")?,
                ended_by: row
                    .try_get::<Option<Vec<u8>>, _>("ended_by")?
                    .map(|bytes| public_key(&bytes, "ended_by"))
                    .transpose()?,
                ended_reason: ended_reason
                    .map(|reason| parse_end_reason(&reason))
                    .transpose()?,
                replaced_by_assignment_id: row.try_get("replaced_by_assignment_id")?,
                entity_revision: db_revision(row.try_get("entity_revision")?, "entity_revision")?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "project_revision",
                )?,
            })
        })
        .collect()
}

async fn load_handoffs(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<RoleHandoff>> {
    let rows = sqlx::query(
        "SELECT handoff_id, role_id, from_assignment_id, to_assignment_id, \
                body, created_at, entity_revision, project_revision \
         FROM project_role_handoffs WHERE community_id = $1 ORDER BY handoff_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let body: Value = row.try_get("body")?;
            let cause = body
                .get("cause")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProjectViewV2WriteError::InvalidCommit(
                        "stored Handoff body has no cause".to_owned(),
                    )
                })
                .and_then(parse_end_reason)?;
            let affected_commitment_ids = body
                .get("affected_commitment_ids")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "stored Handoff commitments are invalid: {error}"
                    ))
                })?
                .unwrap_or_default();
            Ok(RoleHandoff {
                handoff_id: row.try_get("handoff_id")?,
                role_id: row.try_get("role_id")?,
                from_assignment_id: row
                    .try_get::<Option<Uuid>, _>("from_assignment_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "system Handoff is missing from_assignment_id".to_owned(),
                        )
                    })?,
                to_assignment_id: row.try_get("to_assignment_id")?,
                affected_commitment_ids,
                cause,
                created_at: row.try_get("created_at")?,
                entity_revision: db_revision(row.try_get("entity_revision")?, "entity_revision")?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "project_revision",
                )?,
            })
        })
        .collect()
}

async fn find_receipt(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
) -> ProjectViewV2WriteResult<Option<ProjectViewV2Receipt>> {
    let row = sqlx::query(
        "SELECT change_id, project_revision, actor_pubkey, operation, result, accepted_at \
         FROM project_view_changes \
         WHERE community_id = $1 AND change_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(change_id)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|row| {
        Ok(ProjectViewV2Receipt {
            change_id: bytes32(row.try_get("change_id")?, "change_id")?,
            project_revision: db_revision(row.try_get("project_revision")?, "project_revision")?,
            actor_pubkey: public_key(
                &row.try_get::<Option<Vec<u8>>, _>("actor_pubkey")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "member command receipt has no actor".to_owned(),
                        )
                    })?,
                "actor_pubkey",
            )?,
            operation: row.try_get("operation")?,
            result: row.try_get("result")?,
            accepted_at: row.try_get("accepted_at")?,
        })
    })
    .transpose()
}

#[allow(clippy::too_many_arguments)]
async fn insert_change(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    command: &RoleCommand,
    project_revision: u64,
    canonical_time: DateTime<Utc>,
    result: &Value,
) -> ProjectViewV2WriteResult<()> {
    let actor = event.pubkey.to_bytes();
    let subject = serde_json::to_value(&command.request).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!("serialize command subject: {error}"))
    })?;
    sqlx::query(
        "INSERT INTO project_view_changes \
            (community_id, change_id, source_type, source_event_id, actor_pubkey, \
             acting_assignment_id, operation, subject, project_revision, result, accepted_at) \
         VALUES ($1, $2, 'nostr_event', $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .bind(actor.as_slice())
    .bind(command.acting_assignment_id)
    .bind(command.operation())
    .bind(subject)
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(result)
    .bind(canonical_time)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn load_old_projection_ids(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    changes: &[RoleContinuityChange],
) -> ProjectViewV2WriteResult<BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>> {
    let mut result = BTreeMap::new();
    for change in changes {
        let pointer: Option<Vec<u8>> = match change.entity_type() {
            RoleContinuityEntity::Role => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_view_objects \
                 WHERE community_id = $1 AND object_id = $2 \
                   AND object_type = 'role' FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(change.entity_id())
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
            RoleContinuityEntity::RoleAssignmentProposal => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_role_assignment_proposals \
                 WHERE community_id = $1 AND proposal_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(change.entity_id())
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
            RoleContinuityEntity::RoleAssignment => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_role_assignments \
                 WHERE community_id = $1 AND assignment_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(change.entity_id())
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
            RoleContinuityEntity::RoleHandoff => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_role_handoffs \
                 WHERE community_id = $1 AND handoff_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(change.entity_id())
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
        };
        if let Some(pointer) = pointer {
            result.insert(
                (change.entity_type(), change.entity_id()),
                bytes32(pointer, "projection_event_id")?,
            );
        } else if change.entity_revision() > 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                "existing v2 entity {} has no projection pointer",
                change.entity_id()
            )));
        }
    }
    Ok(result)
}

async fn persist_changes(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    canonical_time: DateTime<Utc>,
    changes: &[RoleContinuityChange],
) -> ProjectViewV2WriteResult<()> {
    for proposal in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Proposal(proposal) => Some(proposal),
        _ => None,
    }) {
        persist_proposal(tx, community_id, change_id, canonical_time, proposal).await?;
    }
    // End old seats before inserting the new seat so both partial unique
    // indexes remain satisfied throughout the transaction, not only at commit.
    for assignment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Assignment(assignment) if !assignment.is_active() => Some(assignment),
        _ => None,
    }) {
        persist_assignment(tx, community_id, change_id, canonical_time, assignment).await?;
    }
    for assignment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Assignment(assignment) if assignment.is_active() => Some(assignment),
        _ => None,
    }) {
        persist_assignment(tx, community_id, change_id, canonical_time, assignment).await?;
    }
    for handoff in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Handoff(handoff) => Some(handoff),
        _ => None,
    }) {
        persist_handoff(tx, community_id, change_id, handoff).await?;
    }
    Ok(())
}

async fn persist_proposal(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    canonical_time: DateTime<Utc>,
    proposal: &RoleAssignmentProposal,
) -> ProjectViewV2WriteResult<()> {
    let candidate = proposal.candidate_pubkey.to_hex();
    let authorized_by = proposal.authorized_by.map(PublicKey::to_bytes);
    let created_by = proposal.created_by.to_bytes();
    let result = sqlx::query(
        "INSERT INTO project_role_assignment_proposals \
            (community_id, proposal_id, role_id, candidate_pubkey, proposal_type, \
             candidate_accepted_at, authorized_by, authorized_at, \
             expected_target_assignment_id, expected_candidate_assignment_id, \
             expires_at, status, reason, created_by, created_at, resolved_at, \
             source_change_id, last_change_id, entity_revision, project_revision, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$17,$18,$19,$20) \
         ON CONFLICT (community_id, proposal_id) DO UPDATE SET \
             candidate_accepted_at = EXCLUDED.candidate_accepted_at, \
             authorized_by = EXCLUDED.authorized_by, \
             authorized_at = EXCLUDED.authorized_at, status = EXCLUDED.status, \
             reason = EXCLUDED.reason, resolved_at = EXCLUDED.resolved_at, \
             last_change_id = EXCLUDED.last_change_id, \
             entity_revision = EXCLUDED.entity_revision, \
             project_revision = EXCLUDED.project_revision, updated_at = EXCLUDED.updated_at \
         WHERE project_role_assignment_proposals.role_id = EXCLUDED.role_id \
           AND project_role_assignment_proposals.candidate_pubkey = EXCLUDED.candidate_pubkey \
           AND project_role_assignment_proposals.proposal_type = EXCLUDED.proposal_type \
           AND project_role_assignment_proposals.entity_revision + 1 = EXCLUDED.entity_revision",
    )
    .bind(community_id.as_uuid())
    .bind(proposal.proposal_id)
    .bind(proposal.role_id)
    .bind(candidate)
    .bind(proposal.proposal_type.as_str())
    .bind(proposal.candidate_accepted_at)
    .bind(authorized_by.as_ref().map(<[u8; 32]>::as_slice))
    .bind(proposal.authorized_at)
    .bind(proposal.expected_target_assignment_id)
    .bind(proposal.expected_candidate_assignment_id)
    .bind(proposal.expires_at)
    .bind(proposal.status.as_str())
    .bind(&proposal.reason)
    .bind(created_by.as_slice())
    .bind(proposal.created_at)
    .bind(proposal.resolved_at)
    .bind(change_id)
    .bind(revision_i64(
        proposal.entity_revision,
        "proposal.entity_revision",
    )?)
    .bind(revision_i64(
        proposal.project_revision,
        "proposal.project_revision",
    )?)
    .bind(canonical_time)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "Proposal {} did not advance exactly one entity revision",
            proposal.proposal_id
        )));
    }
    Ok(())
}

async fn persist_assignment(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    canonical_time: DateTime<Utc>,
    assignment: &RoleAssignment,
) -> ProjectViewV2WriteResult<()> {
    let member = assignment.member_pubkey.to_hex();
    let started_by = assignment.started_by.to_bytes();
    let ended_by = assignment.ended_by.map(PublicKey::to_bytes);
    let ended_change_id = assignment.ended_at.map(|_| change_id);
    let result = sqlx::query(
        "INSERT INTO project_role_assignments \
            (community_id, assignment_id, role_id, member_pubkey, proposal_id, \
             started_at, started_by, replacement_requested_at, replacement_request_reason, \
             unable_reported_at, unable_report_reason, ended_at, ended_by, ended_reason, \
             replaced_by_assignment_id, source_change_id, ended_source_change_id, \
             last_change_id, entity_revision, project_revision, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$16,$18,$19,$20) \
         ON CONFLICT (community_id, assignment_id) DO UPDATE SET \
             replacement_requested_at = EXCLUDED.replacement_requested_at, \
             replacement_request_reason = EXCLUDED.replacement_request_reason, \
             unable_reported_at = EXCLUDED.unable_reported_at, \
             unable_report_reason = EXCLUDED.unable_report_reason, \
             ended_at = EXCLUDED.ended_at, ended_by = EXCLUDED.ended_by, \
             ended_reason = EXCLUDED.ended_reason, \
             replaced_by_assignment_id = EXCLUDED.replaced_by_assignment_id, \
             ended_source_change_id = EXCLUDED.ended_source_change_id, \
             last_change_id = EXCLUDED.last_change_id, \
             entity_revision = EXCLUDED.entity_revision, \
             project_revision = EXCLUDED.project_revision, updated_at = EXCLUDED.updated_at \
         WHERE project_role_assignments.role_id = EXCLUDED.role_id \
           AND project_role_assignments.member_pubkey = EXCLUDED.member_pubkey \
           AND project_role_assignments.proposal_id = EXCLUDED.proposal_id \
           AND project_role_assignments.ended_at IS NULL \
           AND project_role_assignments.entity_revision + 1 = EXCLUDED.entity_revision",
    )
    .bind(community_id.as_uuid())
    .bind(assignment.assignment_id)
    .bind(assignment.role_id)
    .bind(member)
    .bind(assignment.proposal_id)
    .bind(assignment.started_at)
    .bind(started_by.as_slice())
    .bind(assignment.replacement_requested_at)
    .bind(&assignment.replacement_request_reason)
    .bind(assignment.unable_reported_at)
    .bind(&assignment.unable_report_reason)
    .bind(assignment.ended_at)
    .bind(ended_by.as_ref().map(<[u8; 32]>::as_slice))
    .bind(assignment.ended_reason.map(AssignmentEndReason::as_str))
    .bind(assignment.replaced_by_assignment_id)
    .bind(change_id)
    .bind(ended_change_id)
    .bind(revision_i64(
        assignment.entity_revision,
        "assignment.entity_revision",
    )?)
    .bind(revision_i64(
        assignment.project_revision,
        "assignment.project_revision",
    )?)
    .bind(canonical_time)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "Assignment {} did not advance exactly one entity revision",
            assignment.assignment_id
        )));
    }
    Ok(())
}

async fn persist_handoff(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    handoff: &RoleHandoff,
) -> ProjectViewV2WriteResult<()> {
    let body = json!({
        "cause": handoff.cause.as_str(),
        "affected_commitment_ids": handoff.affected_commitment_ids,
    });
    let result = sqlx::query(
        "INSERT INTO project_role_handoffs \
            (community_id, handoff_id, role_id, from_assignment_id, to_assignment_id, \
             body, system_generated, created_by, created_at, source_change_id, \
             last_change_id, entity_revision, project_revision) \
         VALUES ($1,$2,$3,$4,$5,$6,TRUE,NULL,$7,$8,$8,$9,$10) \
         ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(handoff.handoff_id)
    .bind(handoff.role_id)
    .bind(handoff.from_assignment_id)
    .bind(handoff.to_assignment_id)
    .bind(body)
    .bind(handoff.created_at)
    .bind(change_id)
    .bind(revision_i64(
        handoff.entity_revision,
        "handoff.entity_revision",
    )?)
    .bind(revision_i64(
        handoff.project_revision,
        "handoff.project_revision",
    )?)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "Handoff {} already exists",
            handoff.handoff_id
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn end_commitments_for_changed_assignments(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    change_id: &[u8],
    project_revision: u64,
    canonical_time: DateTime<Utc>,
    changes: &mut [RoleContinuityChange],
    ended_commitments: &mut BTreeMap<Uuid, Vec<Uuid>>,
) -> ProjectViewV2WriteResult<()> {
    let ended_assignment_ids = changes
        .iter()
        .filter_map(|change| match change {
            RoleContinuityChange::Assignment(assignment)
                if assignment.ended_at == Some(canonical_time) =>
            {
                Some(assignment.assignment_id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let actor = actor.to_bytes();
    for assignment_id in ended_assignment_ids {
        let commitment_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT commitment_id FROM project_work_commitments \
             WHERE community_id = $1 AND assignment_id = $2 AND ended_at IS NULL \
             ORDER BY commitment_id FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .fetch_all(&mut **tx)
        .await?;
        if !commitment_ids.is_empty() {
            sqlx::query(
                "UPDATE project_work_commitments SET ended_at = $3, ended_by = $4, \
                     ended_reason = 'assignment_ended', ended_source_change_id = $5, \
                     project_revision = $6 \
                 WHERE community_id = $1 AND assignment_id = $2 AND ended_at IS NULL",
            )
            .bind(community_id.as_uuid())
            .bind(assignment_id)
            .bind(canonical_time)
            .bind(actor.as_slice())
            .bind(change_id)
            .bind(revision_i64(project_revision, "project_revision")?)
            .execute(&mut **tx)
            .await?;
        }
        ended_commitments.insert(assignment_id, commitment_ids);
    }
    for change in changes {
        if let RoleContinuityChange::Handoff(handoff) = change {
            handoff.affected_commitment_ids = ended_commitments
                .get(&handoff.from_assignment_id)
                .cloned()
                .unwrap_or_default();
        }
    }
    Ok(())
}

async fn apply_membership_roles(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    roles: &BTreeMap<PublicKey, CommunityMemberRole>,
    canonical_time: DateTime<Utc>,
) -> ProjectViewV2WriteResult<()> {
    let actor_hex = actor.to_hex();
    for (pubkey, role) in roles {
        let pubkey = pubkey.to_hex();
        let current: Option<String> = sqlx::query_scalar(
            "SELECT role FROM relay_members WHERE community_id = $1 AND pubkey = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(&pubkey)
        .fetch_optional(&mut **tx)
        .await?;
        if current.as_deref() == Some("owner") {
            if *role != CommunityMemberRole::Owner {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "Role coordinator attempted to demote the Community owner".to_owned(),
                ));
            }
            continue;
        }
        let result = sqlx::query(
            "INSERT INTO relay_members \
                (community_id, pubkey, role, added_by, created_at, updated_at) \
             VALUES ($1,$2,$3,$4,$5,$5) \
             ON CONFLICT (community_id, pubkey) DO UPDATE SET \
                 role = EXCLUDED.role, updated_at = EXCLUDED.updated_at \
             WHERE relay_members.role <> 'owner'",
        )
        .bind(community_id.as_uuid())
        .bind(&pubkey)
        .bind(role.as_str())
        .bind(&actor_hex)
        .bind(canonical_time)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                "failed to synchronize Community role for {pubkey}"
            )));
        }
    }
    Ok(())
}

async fn load_membership(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<V2MembershipEntry>> {
    let rows = sqlx::query(
        "SELECT pubkey, role FROM relay_members \
         WHERE community_id = $1 ORDER BY pubkey, role",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(V2MembershipEntry {
                pubkey: row.try_get("pubkey")?,
                role: row.try_get("role")?,
            })
        })
        .collect()
}

async fn load_counts(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<V2CanonicalCounts> {
    let row = sqlx::query(
        "SELECT \
            (SELECT count(*) FROM project_view_objects \
             WHERE community_id = $1 AND deleted_at IS NULL) AS active_objects, \
            (SELECT count(*) FROM project_role_assignment_proposals \
             WHERE community_id = $1 AND status = 'open') AS open_proposals, \
            (SELECT count(*) FROM project_role_assignments \
             WHERE community_id = $1 AND ended_at IS NULL) AS active_assignments, \
            (SELECT count(*) FROM project_work_commitments \
             WHERE community_id = $1 AND ended_at IS NULL) AS active_commitments, \
            (SELECT count(*) FROM project_role_checkpoints \
             WHERE community_id = $1) AS checkpoints, \
            (SELECT count(*) FROM project_role_handoffs \
             WHERE community_id = $1) AS handoffs",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    Ok(V2CanonicalCounts {
        active_objects: db_count(row.try_get("active_objects")?, "active_objects")?,
        open_proposals: db_count(row.try_get("open_proposals")?, "open_proposals")?,
        active_assignments: db_count(row.try_get("active_assignments")?, "active_assignments")?,
        active_commitments: db_count(row.try_get("active_commitments")?, "active_commitments")?,
        checkpoints: db_count(row.try_get("checkpoints")?, "checkpoints")?,
        handoffs: db_count(row.try_get("handoffs")?, "handoffs")?,
    })
}

fn validate_commit_bundle(
    basis: &V2PreparedBasis,
    commit: &PreparedV2RoleCommit,
) -> ProjectViewV2WriteResult<()> {
    if commit.command_event.id.to_bytes() != basis.command_event_id
        || commit.command_event.pubkey != basis.actor
        || RoleCommand::from_json(&commit.command_event.content)? != basis.command
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "committed command differs from the prepared command".to_owned(),
        ));
    }
    commit.command_event.verify().map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "committed command signature is invalid: {error}"
        ))
    })?;
    let command_tags = commit
        .command_event
        .tags
        .iter()
        .map(Tag::as_slice)
        .collect::<Vec<_>>();
    let expected_command_tags = [
        vec!["-".to_owned()],
        vec!["t".to_owned(), "buzz-project-view-mutation".to_owned()],
    ];
    if command_tags != expected_command_tags {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "committed command tags are not the exact protected v2 shape".to_owned(),
        ));
    }
    let expected_source = buzz_sdk::project_view_v2::V2ProjectionSource::NostrEvent {
        change_id: commit.command_event.id,
        event_id: commit.command_event.id,
    };
    let expected = basis
        .preparation
        .changes
        .iter()
        .map(|change| ((change.entity_type(), change.entity_id()), change))
        .collect::<BTreeMap<_, _>>();
    if commit.entity_projections.len() != expected.len() {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "projection count does not match changed entities".to_owned(),
        ));
    }
    let mut actual = BTreeSet::new();
    let mut expected_heads = BTreeMap::new();
    for projection in &commit.entity_projections {
        let key = (projection.entity_type, projection.entity_id);
        if !actual.insert(key) || !expected.contains_key(&key) {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "projection set does not exactly match changed entities".to_owned(),
            ));
        }
        let parsed = buzz_sdk::project_view_v2::parse_entity_projection(
            &projection.event,
            &basis.preparation.projection_pubkey,
            basis.preparation.community_id,
        )
        .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        if parsed.project_revision != basis.preparation.project_revision
            || parsed.projection_generation != basis.preparation.projection_generation
            || parsed.source != expected_source
            || parsed.updated_at != basis.preparation.canonical_time
            || Some(&parsed.entity) != expected.get(&key).copied()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "signed entity projection differs from canonical change".to_owned(),
            ));
        }
        expected_heads.insert(
            buzz_sdk::project_view_v2::entity_projection_coordinate(
                basis.preparation.community_id,
                projection.entity_type,
                projection.entity_id,
            ),
            (
                projection.event.id,
                projection.entity_type,
                parsed.entity_revision,
            ),
        );
    }
    let meta = buzz_sdk::project_view_v2::parse_meta_projection(
        &commit.meta_projection,
        &basis.preparation.projection_pubkey,
    )
    .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
    if meta.project_id != basis.preparation.community_id
        || meta.project_revision != basis.preparation.project_revision
        || meta.projection_generation != basis.preparation.projection_generation
        || meta.entity_counts
            != (buzz_sdk::project_view_v2::V2EntityCounts {
                active_objects: basis.preparation.counts.active_objects,
                open_proposals: basis.preparation.counts.open_proposals,
                active_assignments: basis.preparation.counts.active_assignments,
                active_commitments: basis.preparation.counts.active_commitments,
                checkpoints: basis.preparation.counts.checkpoints,
                handoffs: basis.preparation.counts.handoffs,
            })
        || meta.membership_snapshot_event_id
            != commit
                .membership_projection
                .as_ref()
                .map(|event| event.id)
                .or(basis.preparation.membership_snapshot_event_id)
                .ok_or_else(|| {
                    ProjectViewV2WriteError::InvalidCommit(
                        "prepared change has no membership snapshot".to_owned(),
                    )
                })?
        || meta.reset
        || meta.changed_heads.len() != expected.len()
        || meta.source != expected_source
        || meta.updated_at != basis.preparation.canonical_time
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "signed v2 metadata differs from canonical change".to_owned(),
        ));
    }
    let actual_heads = meta
        .changed_heads
        .iter()
        .map(|head| {
            (
                head.coordinate.clone(),
                (head.event_id, head.entity_type, head.entity_revision),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if actual_heads.len() != meta.changed_heads.len() || actual_heads != expected_heads {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "metadata changed heads do not exactly bind the signed canonical entity heads"
                .to_owned(),
        ));
    }
    Ok(())
}

fn verify_membership_projection(
    event: &Event,
    relay_pubkey: PublicKey,
    members: &[V2MembershipEntry],
    canonical_time: DateTime<Utc>,
) -> ProjectViewV2WriteResult<()> {
    event.verify().map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid membership projection signature: {error}"
        ))
    })?;
    if event.pubkey != relay_pubkey
        || event.kind.as_u16() as u32 != KIND_NIP43_MEMBERSHIP_LIST
        || !event.content.is_empty()
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "membership projection signer, kind, or content is invalid".to_owned(),
        ));
    }
    let canonical_seconds = u64::try_from(canonical_time.timestamp()).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(
            "membership canonical time precedes Unix epoch".to_owned(),
        )
    })?;
    if event.created_at.as_secs() != canonical_seconds {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "membership projection timestamp differs from the accepted change".to_owned(),
        ));
    }
    let mut expected = Vec::with_capacity(members.len() + 1);
    expected.push(vec!["-".to_owned()]);
    expected.extend(members.iter().map(|member| {
        vec![
            "member".to_owned(),
            member.pubkey.clone(),
            member.role.clone(),
        ]
    }));
    let actual = event.tags.iter().map(Tag::as_slice).collect::<Vec<_>>();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(&expected)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "membership projection tags do not match canonical membership".to_owned(),
        ));
    }
    Ok(())
}

async fn retire_membership_heads(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    relay_pubkey: PublicKey,
) -> ProjectViewV2WriteResult<()> {
    let pubkey = relay_pubkey.to_bytes();
    sqlx::query(
        "UPDATE events SET deleted_at = clock_timestamp() \
         WHERE community_id = $1 AND kind = $2 AND pubkey = $3 \
           AND deleted_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(i32::try_from(KIND_NIP43_MEMBERSHIP_LIST).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit("membership kind exceeds PostgreSQL INT".to_owned())
    })?)
    .bind(pubkey.as_slice())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn update_projection_pointer(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    entity_type: RoleContinuityEntity,
    entity_id: Uuid,
    event_id: &[u8],
) -> ProjectViewV2WriteResult<()> {
    let result = match entity_type {
        RoleContinuityEntity::Role => {
            sqlx::query(
                "UPDATE project_view_objects SET projection_event_id = $3 \
             WHERE community_id = $1 AND object_id = $2 AND object_type = 'role'",
            )
            .bind(community_id.as_uuid())
            .bind(entity_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?
        }
        RoleContinuityEntity::RoleAssignmentProposal => {
            sqlx::query(
                "UPDATE project_role_assignment_proposals SET projection_event_id = $3 \
             WHERE community_id = $1 AND proposal_id = $2",
            )
            .bind(community_id.as_uuid())
            .bind(entity_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?
        }
        RoleContinuityEntity::RoleAssignment => {
            sqlx::query(
                "UPDATE project_role_assignments SET projection_event_id = $3 \
             WHERE community_id = $1 AND assignment_id = $2",
            )
            .bind(community_id.as_uuid())
            .bind(entity_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?
        }
        RoleContinuityEntity::RoleHandoff => {
            sqlx::query(
                "UPDATE project_role_handoffs SET projection_event_id = $3 \
             WHERE community_id = $1 AND handoff_id = $2",
            )
            .bind(community_id.as_uuid())
            .bind(entity_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?
        }
    };
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "cannot bind projection pointer for {entity_id}"
        )));
    }
    Ok(())
}

fn role_receipt(
    command: &RoleCommand,
    changes: &[RoleContinuityChange],
    project_revision: u64,
) -> Value {
    let changed_entities = changes
        .iter()
        .map(|change| {
            json!({
                "entity_type": change.entity_type().as_str(),
                "entity_id": change.entity_id(),
                "entity_revision": change.entity_revision(),
            })
        })
        .collect::<Vec<_>>();
    let new_assignment_id = changes.iter().find_map(|change| match change {
        RoleContinuityChange::Assignment(assignment)
            if assignment.entity_revision == 1 && assignment.is_active() =>
        {
            Some(assignment.assignment_id)
        }
        _ => None,
    });
    let mut result = serde_json::Map::new();
    result.insert("project_revision".to_owned(), Value::from(project_revision));
    result.insert(
        "operation".to_owned(),
        Value::String(command.operation().to_owned()),
    );
    result.insert(
        "changed_entities".to_owned(),
        Value::Array(changed_entities),
    );
    if let Some(assignment_id) = new_assignment_id {
        result.insert(
            "assignment_id".to_owned(),
            Value::String(assignment_id.to_string()),
        );
    }
    match &command.request {
        buzz_project_view::v2::RoleCommandRequest::RequestRole { proposal_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::OfferRole { proposal_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::AcceptProposal { proposal_id }
        | buzz_project_view::v2::RoleCommandRequest::RejectProposal { proposal_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::WithdrawProposal { proposal_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::ExpireProposal { proposal_id }
        | buzz_project_view::v2::RoleCommandRequest::AuthorizeProposal { proposal_id } => {
            result.insert(
                "proposal_id".to_owned(),
                Value::String(proposal_id.to_string()),
            );
        }
        buzz_project_view::v2::RoleCommandRequest::EndAssignment { assignment_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::RequestReplacement { assignment_id, .. }
        | buzz_project_view::v2::RoleCommandRequest::ReportUnableToContinue {
            assignment_id, ..
        } => {
            result.insert(
                "target_assignment_id".to_owned(),
                Value::String(assignment_id.to_string()),
            );
        }
    }
    Value::Object(result)
}

fn parse_role_level(value: &str) -> ProjectViewV2WriteResult<RoleLevel> {
    match value {
        "admin" => Ok(RoleLevel::Admin),
        "member" => Ok(RoleLevel::Member),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Role level {value}"
        ))),
    }
}

fn parse_community_role(value: &str) -> ProjectViewV2WriteResult<CommunityMemberRole> {
    match value {
        "owner" => Ok(CommunityMemberRole::Owner),
        "admin" => Ok(CommunityMemberRole::Admin),
        "member" => Ok(CommunityMemberRole::Member),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Community role {value}"
        ))),
    }
}

fn parse_proposal_type(value: &str) -> ProjectViewV2WriteResult<ProposalType> {
    match value {
        "request" => Ok(ProposalType::Request),
        "offer" => Ok(ProposalType::Offer),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Proposal type {value}"
        ))),
    }
}

fn parse_proposal_status(value: &str) -> ProjectViewV2WriteResult<ProposalStatus> {
    match value {
        "open" => Ok(ProposalStatus::Open),
        "consumed" => Ok(ProposalStatus::Consumed),
        "rejected" => Ok(ProposalStatus::Rejected),
        "withdrawn" => Ok(ProposalStatus::Withdrawn),
        "expired" => Ok(ProposalStatus::Expired),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Proposal status {value}"
        ))),
    }
}

fn parse_end_reason(value: &str) -> ProjectViewV2WriteResult<AssignmentEndReason> {
    match value {
        "revoked" => Ok(AssignmentEndReason::Revoked),
        "replaced" => Ok(AssignmentEndReason::Replaced),
        "unrecoverable" => Ok(AssignmentEndReason::Unrecoverable),
        "membership_ended" => Ok(AssignmentEndReason::MembershipEnded),
        "role_deactivated" => Ok(AssignmentEndReason::RoleDeactivated),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Assignment end reason {value}"
        ))),
    }
}

fn public_key(bytes: &[u8], field: &str) -> ProjectViewV2WriteResult<PublicKey> {
    PublicKey::from_slice(bytes).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn parse_pubkey(value: &str, field: &str) -> ProjectViewV2WriteResult<PublicKey> {
    let parsed = PublicKey::from_hex(value).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })?;
    if parsed.to_hex() != value {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "{field} is not canonical lowercase hex"
        )));
    }
    Ok(parsed)
}

fn bytes32(value: Vec<u8>, field: &str) -> ProjectViewV2WriteResult<[u8; 32]> {
    value.try_into().map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(format!("{field} must contain 32 bytes"))
    })
}

fn event_id(value: Vec<u8>, field: &str) -> ProjectViewV2WriteResult<EventId> {
    EventId::from_slice(&value).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn db_revision(value: i64, field: &str) -> ProjectViewV2WriteResult<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| (1..=buzz_project_view::MAX_SAFE_REVISION).contains(value))
        .ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(format!("{field} is outside the safe range"))
        })
}

fn revision_i64(value: u64, field: &str) -> ProjectViewV2WriteResult<i64> {
    if value == 0 || value > buzz_project_view::MAX_SAFE_REVISION {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "{field} is outside the safe range"
        )));
    }
    i64::try_from(value)
        .map_err(|_| ProjectViewV2WriteError::InvalidCommit(format!("{field} does not fit BIGINT")))
}

fn db_count(value: i64, field: &str) -> ProjectViewV2WriteResult<u32> {
    u32::try_from(value)
        .map_err(|_| ProjectViewV2WriteError::InvalidCommit(format!("{field} does not fit u32")))
}

fn count_i32(value: u32, field: &str) -> ProjectViewV2WriteResult<i32> {
    i32::try_from(value).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(format!("{field} does not fit INTEGER"))
    })
}

fn validate_cutover_plan(
    plan: &ProjectViewV2CutoverPlan,
    current_admins: &BTreeSet<PublicKey>,
) -> ProjectViewV2WriteResult<()> {
    let assigned_members = plan
        .admin_assignments
        .iter()
        .map(|assignment| assignment.member_pubkey)
        .collect::<BTreeSet<_>>();
    let assigned_roles = plan
        .admin_assignments
        .iter()
        .map(|assignment| assignment.role_id)
        .collect::<BTreeSet<_>>();
    let downgraded = plan
        .downgraded_admins
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if assigned_members.len() != plan.admin_assignments.len()
        || assigned_roles.len() != plan.admin_assignments.len()
        || downgraded.len() != plan.downgraded_admins.len()
        || !assigned_members.is_disjoint(&downgraded)
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "cutover admin mappings contain a duplicate Member or Role".to_owned(),
        ));
    }
    let classified = assigned_members
        .union(&downgraded)
        .copied()
        .collect::<BTreeSet<_>>();
    if &classified != current_admins {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "every existing non-owner admin must be explicitly assigned or downgraded".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CutoverRoleBody {
    name: String,
    purpose: String,
    responsibilities: Vec<String>,
    boundaries: Vec<String>,
    level: RoleLevel,
    active: bool,
}

fn build_cutover_membership_event(
    members: &[V2MembershipEntry],
    canonical_time: DateTime<Utc>,
    relay_keys: &Keys,
) -> ProjectViewV2WriteResult<Event> {
    let mut tags = Vec::with_capacity(members.len() + 1);
    tags.push(Tag::parse(["-"]).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "build cutover membership protection tag: {error}"
        ))
    })?);
    for member in members {
        tags.push(
            Tag::parse(["member", member.pubkey.as_str(), member.role.as_str()]).map_err(
                |error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "build cutover membership member tag: {error}"
                    ))
                },
            )?,
        );
    }
    let seconds = u64::try_from(canonical_time.timestamp()).map_err(|_| {
        ProjectViewV2WriteError::InvalidCommit(
            "cutover canonical time precedes Unix epoch".to_owned(),
        )
    })?;
    EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
        .tags(tags)
        .custom_created_at(Timestamp::from(seconds))
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "sign cutover membership snapshot: {error}"
            ))
        })
}
