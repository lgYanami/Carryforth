//! Atomic Project View v2 Role Proposal and Assignment coordinator.
//!
//! A caller holds [`ProjectViewV2WriteTx`] across pure reduction and Relay
//! signing. All canonical rows are staged in the same SQL transaction; the
//! final commit also stores the command, receipt, entity heads, metadata head,
//! membership role changes, and the exact NIP-43 snapshot.

use std::collections::{BTreeMap, BTreeSet};

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::ChangeSource;
use buzz_project_view::v2::{
    AssignmentEndReason, CommitmentEndReason, CommunityMemberRole, GeneratedRoleContinuityIds,
    HandoffCause, MemberGovernance, ProjectObjectCommand, ProposalStatus, ProposalType,
    RoleAssignment, RoleAssignmentProposal, RoleCheckpoint, RoleCheckpointContent, RoleCommand,
    RoleContinuityChange, RoleContinuityEntity, RoleContinuityError, RoleContinuityReference,
    RoleContinuityState, RoleDefinition, RoleHandoff, RoleHandoffContent, RoleLevel, RoleSlot,
    WorkCommitment, WorkResponsibility,
};
use buzz_project_view::{
    DomainError, MutationOutcome, ProjectViewEntry, ProjectViewObject, ProjectViewObjectData,
    ProjectViewObjectType, ProjectViewState, WorkStatus,
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
    /// Tamper-evident audit append failed.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// Pure v2 state machine rejected the command.
    #[error(transparent)]
    Domain(#[from] RoleContinuityError),
    /// Shared ordinary-object reducer rejected a schema-v2 command.
    #[error(transparent)]
    ObjectDomain(#[from] DomainError),
    /// Trusted runtime fencing rejected a supervised managed command.
    #[error(transparent)]
    RuntimeSupervision(#[from] crate::project_runtime::RuntimeSupervisionError),
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
    /// Work object heads whose stable responsible Role changed.
    pub work_heads: Vec<PreparedV2ProjectObjectHead>,
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

/// One changed head produced by an ordinary-object command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedV2ProjectObjectHead {
    /// An ordinary active object or tombstone.
    Object {
        /// Complete canonical object or tombstone.
        entry: ProjectViewEntry,
        /// Stable Role responsible for an active Work.
        responsible_role_id: Option<Uuid>,
    },
    /// An active Role carrying its v2 governance level.
    Role(RoleDefinition),
}

impl PreparedV2ProjectObjectHead {
    /// Stable canonical object ID whose projection pointer will be replaced.
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        match self {
            Self::Object { entry, .. } => entry.id(),
            Self::Role(role) => role.role_id,
        }
    }

    /// Stable Work responsibility carried outside the closed v1 object body.
    #[must_use]
    pub const fn responsible_role_id(&self) -> Option<Uuid> {
        match self {
            Self::Object {
                responsible_role_id,
                ..
            } => *responsible_role_id,
            Self::Role(_) => None,
        }
    }
}

/// Prepared state returned to the Relay for signing an ordinary-object change.
#[derive(Debug, Clone)]
pub struct PreparedV2ProjectObjectChange {
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
    /// Changed object/entity heads.
    pub heads: Vec<PreparedV2ProjectObjectHead>,
    /// Commitment heads ended by a terminal Work transition.
    pub entity_changes: Vec<RoleContinuityChange>,
    /// Counts after the staged change.
    pub counts: V2CanonicalCounts,
    /// Existing exact NIP-43 snapshot pointer.
    pub membership_snapshot_event_id: EventId,
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
    /// One signed head per changed Work responsibility.
    pub object_projections: Vec<crate::project_view::PreparedObjectProjection>,
    /// Signed kind `40904` metadata head.
    pub meta_projection: Event,
    /// New NIP-43 snapshot when canonical membership changed.
    pub membership_projection: Option<Event>,
}

/// Relay-signed material completing a staged ordinary-object change.
#[derive(Debug, Clone)]
pub struct PreparedV2ProjectObjectCommit {
    /// Original accepted member command.
    pub command_event: Event,
    /// One signed head per changed canonical object.
    pub object_projections: Vec<crate::project_view::PreparedObjectProjection>,
    /// Commitment heads ended by a terminal Work transition.
    pub entity_projections: Vec<PreparedV2EntityProjection>,
    /// Signed kind `40904` metadata head.
    pub meta_projection: Event,
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

/// Result of one trusted internal Project View system change.
#[derive(Debug, Clone)]
pub struct ProjectViewV2SystemOutcome {
    /// New or replayed project revision.
    pub project_revision: u64,
    /// Stable successful receipt.
    pub result: Value,
    /// Newly stored Relay-signed projections in dispatch order.
    pub events: Vec<Event>,
    /// Whether the idempotent system change was already committed.
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

/// Ordinary-object preparation may discover an accepted event after fencing.
#[derive(Debug, Clone)]
pub enum ProjectViewV2ProjectObjectPrepareOutcome {
    /// Existing receipt; no revision was allocated.
    Replayed(ProjectViewV2Receipt),
    /// New change staged inside the still-open transaction.
    Prepared(PreparedV2ProjectObjectChange),
}

/// Caller-owned v2 write transaction holding the Community Project lock.
pub struct ProjectViewV2WriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    basis: Option<V2PreparedBasis>,
    object_basis: Option<V2PreparedProjectObjectBasis>,
}

#[derive(Debug, Clone)]
struct V2PreparedBasis {
    command: RoleCommand,
    command_event_id: [u8; 32],
    actor: PublicKey,
    preparation: PreparedV2RoleChange,
    old_meta_projection_id: [u8; 32],
    old_projection_ids: BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>,
    old_object_projection_ids: BTreeMap<Uuid, [u8; 32]>,
    meeting_action: Option<crate::meeting_v2_actions::PreparedActionProjectEvent>,
}

#[derive(Debug, Clone)]
struct V2PreparedProjectObjectBasis {
    command: ProjectObjectCommand,
    command_event_id: [u8; 32],
    actor: PublicKey,
    preparation: PreparedV2ProjectObjectChange,
    next_state: ProjectViewState,
    outcome: MutationOutcome,
    continuity_changes: Vec<RoleContinuityChange>,
    role_levels: BTreeMap<Uuid, RoleLevel>,
    old_meta_projection_id: [u8; 32],
    old_projection_ids: BTreeMap<Uuid, [u8; 32]>,
    old_entity_projection_ids: BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>,
    meeting_action: Option<crate::meeting_v2_actions::PreparedActionProjectEvent>,
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
        let cutover_context = buzz_sdk::project_view_v2::V2ProjectionContext {
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
        let mut verified_entity_projections = Vec::with_capacity(changes.len());
        for change in &changes {
            let projection_context = match change {
                RoleContinuityChange::Role(role) => {
                    buzz_sdk::project_view_v2::V2ProjectionContext {
                        project_revision: role.project_revision,
                        updated_at: role.updated_at,
                        ..cutover_context.clone()
                    }
                }
                _ => cutover_context.clone(),
            };
            let event =
                buzz_sdk::project_view_v2::build_entity_projection(&projection_context, change)
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
                || parsed.project_revision != projection_context.project_revision
                || parsed.projection_generation != next_generation
                || parsed.source != projection_context.source
                || parsed.updated_at != projection_context.updated_at
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover entity projection differs from canonical state".to_owned(),
                ));
            }
            verified_entity_projections.push(parsed);
            projections.push(PreparedV2EntityProjection {
                entity_type: change.entity_type(),
                entity_id: change.entity_id(),
                event,
            });
        }
        let mut project_object_projections = Vec::with_capacity(project_entries.len());
        let mut verified_object_projections = Vec::with_capacity(project_entries.len());
        for entry in &project_entries {
            let updated_at = match entry {
                ProjectViewEntry::Active(object) => object.updated_at,
                ProjectViewEntry::Tombstone(tombstone) => tombstone.deleted_at,
            };
            let projection_context = buzz_sdk::project_view_v2::V2ProjectionContext {
                project_revision: entry.project_revision(),
                updated_at,
                ..cutover_context.clone()
            };
            let event = buzz_sdk::project_view_v2::build_project_object_projection(
                &projection_context,
                entry,
            )
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
            if !projected_object_matches_entry(&parsed.object, entry)
                || parsed.project_revision != projection_context.project_revision
                || parsed.projection_generation != next_generation
                || parsed.source != projection_context.source
                || parsed.updated_at != projection_context.updated_at
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "cutover Project object projection differs from canonical state".to_owned(),
                ));
            }
            verified_object_projections.push(parsed);
            project_object_projections.push((entry.id(), event));
        }
        let meta_event = buzz_sdk::project_view_v2::build_meta_projection(
            &cutover_context,
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
            || verified_meta.source != cutover_context.source
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
        let verified_membership = buzz_sdk::project_view_v2::parse_membership_projection(
            &membership_event,
            &relay_keys.public_key(),
        )
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "verify cutover membership projection: {error}"
            ))
        })?;
        buzz_sdk::role_brief::VerifiedRoleBriefSnapshot::new(
            verified_meta,
            verified_membership,
            verified_object_projections,
            verified_entity_projections,
        )
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "assemble cutover verified snapshot: {error}"
            ))
        })?;

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

    /// End one exhaustively recovered managed-Agent Assignment as a trusted
    /// system action.
    ///
    /// Runtime claim revalidation, audit append, canonical Role continuity,
    /// Community membership, signed projections, and supervisor fencing commit
    /// in one transaction. Presence and lease expiry are never accepted as the
    /// terminal evidence.
    pub async fn end_unrecoverable_assignment(
        &self,
        claim: &crate::project_runtime::RuntimeUnrecoverableClaim,
        relay_keys: &Keys,
    ) -> ProjectViewV2WriteResult<ProjectViewV2SystemOutcome> {
        const OPERATION: &str = "end_unrecoverable_assignment";

        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, claim.community_id, false).await?;

        if let Some(row) = sqlx::query(
            "SELECT operation, subject, project_revision, result \
             FROM project_view_changes \
             WHERE community_id = $1 AND idempotency_key_hash = $2",
        )
        .bind(claim.community_id.as_uuid())
        .bind(claim.idempotency_key_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await?
        {
            let operation: String = row.try_get("operation")?;
            let subject: Value = row.try_get("subject")?;
            if operation != OPERATION
                || subject.get("binding_id").and_then(Value::as_str)
                    != Some(claim.binding_id.to_string().as_str())
                || subject.get("assignment_id").and_then(Value::as_str)
                    != Some(claim.assignment_id.to_string().as_str())
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "runtime system idempotency key belongs to another change".to_owned(),
                ));
            }
            let project_revision =
                db_revision(row.try_get("project_revision")?, "project_revision")?;
            let result: Value = row.try_get("result")?;
            tx.rollback().await?;
            return Ok(ProjectViewV2SystemOutcome {
                project_revision,
                result,
                events: Vec::new(),
                replayed: true,
            });
        }

        let evidence_ids =
            crate::project_runtime::validate_unrecoverable_claim_in_tx(&mut tx, claim).await?;
        let loaded = load_v2_state(&mut tx, claim.community_id).await?;
        let relay_pubkey = relay_keys.public_key();
        if loaded.projection_pubkey != relay_pubkey {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "runtime system action is fenced during projection signer rotation".to_owned(),
            ));
        }
        let current_revision = loaded.state.project_revision();
        let handoff_id = Uuid::new_v4();
        let (next_state, outcome) = loaded.state.reduce_unrecoverable_assignment(
            claim.assignment_id,
            relay_pubkey,
            loaded.canonical_time,
            handoff_id,
        )?;
        if !outcome.work_changes.is_empty() {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "unrecoverable Assignment unexpectedly rewrote Project Work".to_owned(),
            ));
        }
        let old_projection_ids =
            load_old_projection_ids(&mut tx, claim.community_id, &outcome.changes).await?;
        let membership_before = load_membership(&mut tx, claim.community_id).await?;
        let previous_membership_id = loaded.membership_snapshot_event_id.ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(
                "v2 Project View has no membership snapshot".to_owned(),
            )
        })?;

        let evidence_hex = evidence_ids.iter().map(hex::encode).collect::<Vec<_>>();
        let audit_entry = buzz_audit::append_in_transaction(
            &mut tx,
            NewAuditEntry {
                community_id: claim.community_id,
                action: AuditAction::RuntimeAssignmentUnrecoverable,
                actor_pubkey: None,
                object_id: Some(claim.assignment_id.to_string()),
                detail: json!({
                    "binding_id": claim.binding_id,
                    "assignment_id": claim.assignment_id,
                    "handoff_id": handoff_id,
                    "evidence_ids": evidence_hex,
                    "idempotency_key_hash": hex::encode(claim.idempotency_key_hash),
                }),
            },
        )
        .await?;
        let source =
            ChangeSource::system(audit_entry.seq, claim.idempotency_key_hash).map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "invalid runtime system source: {error}"
                ))
            })?;
        let change_id = source.change_id();
        let change_event_id = EventId::from_slice(&change_id).map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "invalid runtime system change ID: {error}"
            ))
        })?;
        let result = json!({
            "operation": OPERATION,
            "project_revision": outcome.project_revision,
            "assignment_id": claim.assignment_id,
            "handoff_id": handoff_id,
            "changed_entities": outcome.changes.iter().map(|change| json!({
                "entity_type": change.entity_type().as_str(),
                "entity_id": change.entity_id(),
                "entity_revision": change.entity_revision(),
            })).collect::<Vec<_>>(),
            "evidence_ids": evidence_hex,
        });
        let subject = json!({
            "binding_id": claim.binding_id,
            "assignment_id": claim.assignment_id,
            "evidence_ids": evidence_hex,
        });
        sqlx::query(
            "INSERT INTO project_view_changes \
                (community_id, change_id, source_type, source_audit_seq, \
                 idempotency_key_hash, actor_pubkey, acting_assignment_id, \
                 operation, subject, project_revision, result, accepted_at) \
             VALUES ($1,$2,'system',$3,$4,NULL,NULL,$5,$6,$7,$8,$9)",
        )
        .bind(claim.community_id.as_uuid())
        .bind(change_id.as_slice())
        .bind(audit_entry.seq)
        .bind(claim.idempotency_key_hash.as_slice())
        .bind(OPERATION)
        .bind(&subject)
        .bind(revision_i64(outcome.project_revision, "project_revision")?)
        .bind(&result)
        .bind(loaded.canonical_time)
        .execute(&mut *tx)
        .await?;

        persist_changes(
            &mut tx,
            claim.community_id,
            &change_id,
            loaded.canonical_time,
            &outcome.changes,
        )
        .await?;
        apply_membership_roles(
            &mut tx,
            claim.community_id,
            relay_pubkey,
            &outcome.membership_roles,
            loaded.canonical_time,
        )
        .await?;
        let membership_after = load_membership(&mut tx, claim.community_id).await?;
        let counts = load_counts(&mut tx, claim.community_id).await?;
        if counts.active_assignments
            != u32::try_from(
                next_state
                    .assignments()
                    .filter(|assignment| assignment.is_active())
                    .count(),
            )
            .map_err(|_| {
                ProjectViewV2WriteError::InvalidCommit(
                    "active Assignment count exceeds u32".to_owned(),
                )
            })?
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "runtime reducer and stored active Assignment count disagree".to_owned(),
            ));
        }

        let membership_event = if membership_before != membership_after {
            Some(build_cutover_membership_event(
                &membership_after,
                loaded.canonical_time,
                relay_keys,
            )?)
        } else {
            None
        };
        let membership_event_id = membership_event
            .as_ref()
            .map_or(previous_membership_id, |event| event.id);
        let audit_seq = u64::try_from(audit_entry.seq).map_err(|_| {
            ProjectViewV2WriteError::InvalidCommit(
                "runtime system audit sequence must be positive".to_owned(),
            )
        })?;
        let projection_source = buzz_sdk::project_view_v2::V2ProjectionSource::System {
            change_id: change_event_id,
            audit_seq,
        };
        let context = buzz_sdk::project_view_v2::V2ProjectionContext {
            project_id: claim.community_id,
            projection_generation: loaded.projection_generation,
            project_revision: outcome.project_revision,
            source: projection_source.clone(),
            updated_at: loaded.canonical_time,
        };
        let mut entity_projections = Vec::with_capacity(outcome.changes.len());
        let mut expected_heads = BTreeMap::new();
        for change in &outcome.changes {
            let event = buzz_sdk::project_view_v2::build_entity_projection(&context, change)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "build runtime system entity projection: {error}"
                    ))
                })?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "sign runtime system entity projection: {error}"
                    ))
                })?;
            let parsed = buzz_sdk::project_view_v2::parse_entity_projection(
                &event,
                &relay_pubkey,
                claim.community_id,
            )
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "verify runtime system entity projection: {error}"
                ))
            })?;
            if parsed.entity != *change
                || parsed.project_revision != outcome.project_revision
                || parsed.projection_generation != loaded.projection_generation
                || parsed.source != projection_source
                || parsed.updated_at != loaded.canonical_time
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "runtime system entity projection differs from canonical state".to_owned(),
                ));
            }
            let changed_head = buzz_sdk::project_view_v2::changed_head_for(
                &context, change, &event,
            )
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "bind runtime system changed head: {error}"
                ))
            })?;
            if expected_heads
                .insert(changed_head.coordinate().to_owned(), changed_head)
                .is_some()
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "runtime system change generated duplicate head coordinates".to_owned(),
                ));
            }
            entity_projections.push(PreparedV2EntityProjection {
                entity_type: change.entity_type(),
                entity_id: change.entity_id(),
                event,
            });
        }
        let changed_heads = expected_heads.values().cloned().collect::<Vec<_>>();
        let entity_counts = buzz_sdk::project_view_v2::V2EntityCounts {
            active_objects: counts.active_objects,
            open_proposals: counts.open_proposals,
            active_assignments: counts.active_assignments,
            active_commitments: counts.active_commitments,
            checkpoints: counts.checkpoints,
            handoffs: counts.handoffs,
        };
        let meta_event = buzz_sdk::project_view_v2::build_meta_projection(
            &context,
            entity_counts,
            membership_event_id,
            false,
            &changed_heads,
        )
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "build runtime system metadata projection: {error}"
            ))
        })?
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "sign runtime system metadata projection: {error}"
            ))
        })?;
        let meta = buzz_sdk::project_view_v2::parse_meta_projection(&meta_event, &relay_pubkey)
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "verify runtime system metadata projection: {error}"
                ))
            })?;
        let actual_heads = meta
            .changed_heads
            .iter()
            .map(|head| (head.coordinate().to_owned(), head.clone()))
            .collect::<BTreeMap<_, _>>();
        if meta.project_id != claim.community_id
            || meta.project_revision != outcome.project_revision
            || meta.projection_generation != loaded.projection_generation
            || meta.entity_counts != entity_counts
            || meta.membership_snapshot_event_id != membership_event_id
            || meta.reset
            || meta.source != projection_source
            || meta.updated_at != loaded.canonical_time
            || actual_heads != expected_heads
            || actual_heads.len() != meta.changed_heads.len()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "runtime system metadata projection differs from canonical state".to_owned(),
            ));
        }
        if let Some(event) = &membership_event {
            verify_membership_projection(
                event,
                relay_pubkey,
                &membership_after,
                loaded.canonical_time,
            )?;
        }

        for old_event_id in old_projection_ids.values() {
            if !crate::event::retire_projection_head_in_tx(
                &mut tx,
                claim.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "stored runtime-affected entity projection is not live".to_owned(),
                ));
            }
        }
        if !crate::event::retire_projection_head_in_tx(
            &mut tx,
            claim.community_id,
            &loaded.meta_projection_event_id,
            KIND_PROJECT_VIEW_META,
        )
        .await?
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "stored v2 metadata projection is not live".to_owned(),
            ));
        }

        let mut events = Vec::with_capacity(entity_projections.len() + 2);
        if let Some(event) = &membership_event {
            retire_membership_heads(&mut tx, claim.community_id, relay_pubkey).await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut tx, claim.community_id, event, None).await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "runtime membership projection already exists".to_owned(),
                ));
            }
            events.push(event.clone());
        }
        for projection in &entity_projections {
            let (_, inserted) = crate::event::insert_event_in_tx(
                &mut tx,
                claim.community_id,
                &projection.event,
                None,
            )
            .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "runtime entity projection {} already exists",
                    projection.entity_id
                )));
            }
            update_projection_pointer(
                &mut tx,
                claim.community_id,
                projection.entity_type,
                projection.entity_id,
                projection.event.id.as_bytes(),
            )
            .await?;
            events.push(projection.event.clone());
        }
        let (_, meta_inserted) =
            crate::event::insert_event_in_tx(&mut tx, claim.community_id, &meta_event, None)
                .await?;
        if !meta_inserted {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "runtime metadata projection already exists".to_owned(),
            ));
        }

        let relay_bytes = relay_pubkey.to_bytes();
        let state_update = sqlx::query(
            "UPDATE project_view_state SET \
                 project_revision = $2, updated_at = $3, last_event_id = $4, \
                 last_actor_pubkey = $5, meta_projection_event_id = $6, \
                 schema_version = 2, last_change_id = $4, \
                 last_source_event_id = NULL, open_proposal_count = $7, \
                 active_assignment_count = $8, active_commitment_count = $9, \
                 checkpoint_count = $10, handoff_count = $11, \
                 membership_snapshot_event_id = $12 \
             WHERE community_id = $1 AND project_revision = $13 \
               AND schema_version = 2",
        )
        .bind(claim.community_id.as_uuid())
        .bind(revision_i64(outcome.project_revision, "project_revision")?)
        .bind(loaded.canonical_time)
        .bind(change_id.as_slice())
        .bind(relay_bytes.as_slice())
        .bind(meta_event.id.as_bytes().as_slice())
        .bind(count_i32(counts.open_proposals, "open_proposals")?)
        .bind(count_i32(counts.active_assignments, "active_assignments")?)
        .bind(count_i32(counts.active_commitments, "active_commitments")?)
        .bind(count_i32(counts.checkpoints, "checkpoints")?)
        .bind(count_i32(counts.handoffs, "handoffs")?)
        .bind(membership_event_id.as_bytes().as_slice())
        .bind(revision_i64(current_revision, "current_revision")?)
        .execute(&mut *tx)
        .await?;
        if state_update.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "Project View state changed during runtime system action".to_owned(),
            ));
        }
        let binding_update = sqlx::query(
            "UPDATE project_runtime_supervisor_bindings SET \
                 system_change_id = $4, system_audit_seq = $5, \
                 automatic_unrecoverable = FALSE, scheduler_claim_token = NULL, \
                 scheduler_claimed_until = NULL, updated_at = $6 \
             WHERE community_id = $1 AND binding_id = $2 \
               AND scheduler_claim_token = $3 AND system_change_id IS NULL",
        )
        .bind(claim.community_id.as_uuid())
        .bind(claim.binding_id)
        .bind(claim.claim_token)
        .bind(change_id.as_slice())
        .bind(audit_entry.seq)
        .bind(loaded.canonical_time)
        .execute(&mut *tx)
        .await?;
        if binding_update.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "runtime scheduler claim changed during system action".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE project_runtime_leases SET \
                 lease_expires_at = NULL, recovery_attempt_in_flight = FALSE, \
                 next_recovery_at = NULL, ended_at = $3, updated_at = $3 \
             WHERE community_id = $1 AND binding_id = $2 AND ended_at IS NULL",
        )
        .bind(claim.community_id.as_uuid())
        .bind(claim.binding_id)
        .bind(loaded.canonical_time)
        .execute(&mut *tx)
        .await?;

        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        events.push(meta_event);
        Ok(ProjectViewV2SystemOutcome {
            project_revision: outcome.project_revision,
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
            object_basis: None,
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
        if self.basis.is_some() || self.object_basis.is_some() {
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
        crate::project_runtime::validate_runtime_command_fence_in_tx(
            &mut self.tx,
            self.community_id,
            command.acting_assignment_id,
            command.runtime_fence,
        )
        .await?;
        if let Some(receipt) =
            find_receipt(&mut self.tx, self.community_id, command_event.id.as_bytes()).await?
        {
            return Ok(ProjectViewV2PrepareOutcome::Replayed(receipt));
        }
        let meeting_action = crate::meeting_v2_actions::fence_prepared_project_event_tx(
            &mut self.tx,
            self.community_id,
            command_event,
            command.expected_project_revision,
        )
        .await?;
        let generated_ids = GeneratedRoleContinuityIds {
            assignment_id: Uuid::new_v4(),
            handoff_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        };
        let (_next_state, outcome) = loaded.state.reduce(
            command,
            command_event.pubkey,
            loaded.canonical_time,
            &generated_ids,
        )?;
        let (work_heads, old_object_projection_ids) = prepare_work_responsibility_heads(
            &mut self.tx,
            self.community_id,
            &outcome.work_changes,
        )
        .await?;

        let receipt_result = role_receipt(
            command,
            &outcome.changes,
            &outcome.work_changes,
            outcome.project_revision,
        );
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
        crate::project_runtime::fence_ended_runtime_bindings_in_tx(
            &mut self.tx,
            self.community_id,
            &outcome.changes,
            command_event.pubkey,
            loaded.canonical_time,
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
            work_heads,
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
            old_object_projection_ids,
            meeting_action,
        });
        Ok(ProjectViewV2PrepareOutcome::Prepared(preparation))
    }

    /// Validate actor fencing and stage one ordinary Project View object
    /// transition under the same v2 revision and Community lock.
    pub async fn prepare_project_object_command(
        &mut self,
        command_event: &Event,
        command: &ProjectObjectCommand,
    ) -> ProjectViewV2WriteResult<ProjectViewV2ProjectObjectPrepareOutcome> {
        if self.basis.is_some() || self.object_basis.is_some() {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "this transaction already has a prepared v2 change".to_owned(),
            ));
        }
        if command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION
            || ProjectObjectCommand::from_json(&command_event.content)? != *command
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "command event does not carry the supplied typed v2 object command".to_owned(),
            ));
        }

        let loaded = load_v2_project_object_state(&mut self.tx, self.community_id).await?;
        validate_project_object_actor_fence(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command.acting_assignment_id,
            command.runtime_fence,
        )
        .await?;
        if let Some(receipt) =
            find_receipt(&mut self.tx, self.community_id, command_event.id.as_bytes()).await?
        {
            return Ok(ProjectViewV2ProjectObjectPrepareOutcome::Replayed(receipt));
        }
        let meeting_action = crate::meeting_v2_actions::fence_prepared_project_event_tx(
            &mut self.tx,
            self.community_id,
            command_event,
            command.expected_project_revision,
        )
        .await?;

        let mutation = command.as_reducer_mutation();
        let (next_state, outcome) =
            loaded
                .state
                .reduce(&mutation, command_event.pubkey, loaded.canonical_time)?;
        reject_assigned_role_deactivation(
            &mut self.tx,
            self.community_id,
            &outcome.changed_entries,
        )
        .await?;
        let continuity_changes = close_commitments_for_terminal_work(
            &mut self.tx,
            self.community_id,
            &outcome.changed_entries,
            command_event.pubkey,
            loaded.canonical_time,
            outcome.project_revision,
        )
        .await?;

        let mut role_levels = loaded.role_levels;
        for entry in &outcome.changed_entries {
            if entry.object_type() == ProjectViewObjectType::Role {
                role_levels.entry(entry.id()).or_insert(RoleLevel::Member);
            }
        }
        let heads = outcome
            .changed_entries
            .iter()
            .map(|entry| match entry {
                ProjectViewEntry::Active(object)
                    if object.object_type == ProjectViewObjectType::Role =>
                {
                    let level = role_levels.get(&object.id).copied().ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "active v2 Role has no governance level".to_owned(),
                        )
                    })?;
                    role_definition_from_object(object, level)
                        .map(PreparedV2ProjectObjectHead::Role)
                }
                _ => {
                    let responsible_role_id = match entry {
                        ProjectViewEntry::Active(object)
                            if object.object_type == ProjectViewObjectType::Work =>
                        {
                            loaded.work_responsibilities.get(&object.id).copied()
                        }
                        ProjectViewEntry::Active(_) | ProjectViewEntry::Tombstone(_) => None,
                    };
                    Ok(PreparedV2ProjectObjectHead::Object {
                        entry: entry.clone(),
                        responsible_role_id,
                    })
                }
            })
            .collect::<ProjectViewV2WriteResult<Vec<_>>>()?;

        let changed_ids = outcome
            .changed_entries
            .iter()
            .map(ProjectViewEntry::id)
            .collect::<Vec<_>>();
        let old_rows = sqlx::query(
            "SELECT object_id, projection_event_id \
             FROM project_view_objects \
             WHERE community_id = $1 AND object_id = ANY($2) FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(&changed_ids)
        .fetch_all(&mut *self.tx)
        .await?;
        let old_projection_ids = old_rows
            .into_iter()
            .map(|row| {
                Ok((
                    row.try_get::<Uuid, _>("object_id")?,
                    bytes32(
                        row.try_get("projection_event_id")?,
                        "object.projection_event_id",
                    )?,
                ))
            })
            .collect::<ProjectViewV2WriteResult<BTreeMap<_, _>>>()?;
        let old_entity_projection_ids =
            load_old_projection_ids(&mut self.tx, self.community_id, &continuity_changes).await?;

        let receipt_result = project_object_receipt(
            command,
            &outcome.changed_entries,
            &continuity_changes,
            outcome.project_revision,
        );
        insert_project_object_change(
            &mut self.tx,
            self.community_id,
            command_event,
            command,
            outcome.project_revision,
            loaded.canonical_time,
            &receipt_result,
        )
        .await?;
        persist_changes(
            &mut self.tx,
            self.community_id,
            command_event.id.as_bytes(),
            loaded.canonical_time,
            &continuity_changes,
        )
        .await?;
        let mut counts = load_counts(&mut self.tx, self.community_id).await?;
        counts.active_objects =
            u32::try_from(next_state.active_objects().count()).map_err(|_| {
                ProjectViewV2WriteError::InvalidCommit(
                    "active Project View object count exceeds u32".to_owned(),
                )
            })?;
        let membership_snapshot_event_id =
            loaded.membership_snapshot_event_id.ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(
                    "v2 state has no membership snapshot pointer".to_owned(),
                )
            })?;
        let preparation = PreparedV2ProjectObjectChange {
            community_id: self.community_id,
            project_revision: outcome.project_revision,
            projection_generation: loaded.projection_generation,
            projection_pubkey: loaded.projection_pubkey,
            canonical_time: loaded.canonical_time,
            heads,
            entity_changes: continuity_changes.clone(),
            counts,
            membership_snapshot_event_id,
            receipt_result,
        };
        self.object_basis = Some(V2PreparedProjectObjectBasis {
            command: command.clone(),
            command_event_id: command_event.id.to_bytes(),
            actor: command_event.pubkey,
            preparation: preparation.clone(),
            next_state,
            outcome,
            continuity_changes,
            role_levels,
            old_meta_projection_id: loaded.meta_projection_event_id,
            old_projection_ids,
            old_entity_projection_ids,
            meeting_action,
        });
        Ok(ProjectViewV2ProjectObjectPrepareOutcome::Prepared(
            preparation,
        ))
    }

    /// Commit a staged ordinary-object change and every signed v2 head.
    pub async fn commit_project_object_command(
        mut self,
        commit: PreparedV2ProjectObjectCommit,
    ) -> ProjectViewV2WriteResult<ProjectViewV2CommitOutcome> {
        let basis = self.object_basis.take().ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(
                "commit requires prepare_project_object_command on the same transaction".to_owned(),
            )
        })?;
        validate_project_object_commit_bundle(&basis, &commit)?;

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
                    "stored v2 object projection pointer is not live".to_owned(),
                ));
            }
        }
        for old_event_id in basis.old_entity_projection_ids.values() {
            if !crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "stored Work Commitment projection pointer is not live".to_owned(),
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

        let projections = commit
            .object_projections
            .iter()
            .map(|projection| (projection.object_id(), projection.event()))
            .collect::<BTreeMap<_, _>>();
        let prepared_heads = basis
            .preparation
            .heads
            .iter()
            .map(|head| (head.object_id(), head))
            .collect::<BTreeMap<_, _>>();
        let mut events = vec![commit.command_event.clone()];
        for entry in &basis.outcome.changed_entries {
            let projection = projections.get(&entry.id()).ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "missing signed head for changed object {}",
                    entry.id()
                ))
            })?;
            let role_level = basis
                .role_levels
                .get(&entry.id())
                .map(|level| level.as_str());
            let responsible_role_id = prepared_heads
                .get(&entry.id())
                .and_then(|head| head.responsible_role_id());
            crate::project_view::write_project_view_entry(
                &mut self.tx,
                self.community_id,
                basis.command_event_id.as_slice(),
                projection.id.as_bytes(),
                entry,
                crate::project_view::ProjectViewEntryStorageMetadata {
                    schema_version: 2,
                    role_level,
                    responsible_role_id,
                },
            )
            .await
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "persist schema-v2 Project object: {error}"
                ))
            })?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, projection, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "v2 object projection {} already exists",
                    entry.id()
                )));
            }
            events.push((*projection).clone());
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
                    "v2 Commitment projection {} already exists",
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
                 active_commitment_count = $7, \
                 schema_version = 2, last_change_id = $4, \
                 last_source_event_id = $4 \
             WHERE community_id = $1 AND project_revision = $8 \
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
        .bind(
            i32::try_from(basis.preparation.counts.active_commitments).map_err(|_| {
                ProjectViewV2WriteError::InvalidCommit(
                    "active Commitment count exceeds the storage range".to_owned(),
                )
            })?,
        )
        .bind(revision_i64(
            basis.command.expected_project_revision,
            "expected_project_revision",
        )?)
        .execute(&mut *self.tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::ObjectDomain(
                DomainError::RevisionConflict {
                    expected: basis.command.expected_project_revision,
                    actual: basis.preparation.project_revision.saturating_sub(1),
                },
            ));
        }
        let (active_object_count, active_commitment_count): (i32, i32) = sqlx::query_as(
            "SELECT active_object_count, active_commitment_count \
             FROM project_view_state WHERE community_id = $1",
        )
        .bind(self.community_id.as_uuid())
        .fetch_one(&mut *self.tx)
        .await?;
        if u32::try_from(active_object_count).ok() != Some(basis.preparation.counts.active_objects)
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "active-object count differs from the prepared v2 state".to_owned(),
            ));
        }
        if u32::try_from(active_commitment_count).ok()
            != Some(basis.preparation.counts.active_commitments)
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "active Commitment count differs from the prepared v2 state".to_owned(),
            ));
        }
        if let Some(meeting_action) = basis.meeting_action.as_ref() {
            crate::meeting_v2_actions::accept_prepared_project_event_tx(
                &mut self.tx,
                meeting_action,
                basis.preparation.project_revision,
                basis.preparation.canonical_time,
            )
            .await?;
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
        for old_event_id in basis.old_object_projection_ids.values() {
            if !crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?
            {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "stored v2 Work projection pointer is not live".to_owned(),
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

        let work_projections = commit
            .object_projections
            .iter()
            .map(|projection| (projection.object_id(), projection.event()))
            .collect::<BTreeMap<_, _>>();
        for head in &basis.preparation.work_heads {
            let PreparedV2ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } = head
            else {
                return Err(ProjectViewV2WriteError::InvalidCommit(
                    "Role command prepared a non-Work object head".to_owned(),
                ));
            };
            let projection = work_projections.get(&entry.id()).ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "missing signed Work responsibility head {}",
                    entry.id()
                ))
            })?;
            crate::project_view::write_project_view_entry(
                &mut self.tx,
                self.community_id,
                basis.command_event_id.as_slice(),
                projection.id.as_bytes(),
                entry,
                crate::project_view::ProjectViewEntryStorageMetadata {
                    schema_version: 2,
                    role_level: None,
                    responsible_role_id: *responsible_role_id,
                },
            )
            .await
            .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "persist Work responsibility {}: {error}",
                    entry.id()
                ))
            })?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, projection, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "v2 Work projection {} already exists",
                    entry.id()
                )));
            }
            events.push((*projection).clone());
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
        if let Some(meeting_action) = basis.meeting_action.as_ref() {
            crate::meeting_v2_actions::accept_prepared_project_event_tx(
                &mut self.tx,
                meeting_action,
                basis.preparation.project_revision,
                basis.preparation.canonical_time,
            )
            .await?;
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
    let works = load_work_responsibilities(tx, community_id).await?;
    let proposals = load_proposals(tx, community_id).await?;
    let assignments = load_assignments(tx, community_id).await?;
    let commitments = load_commitments(tx, community_id).await?;
    let references = load_continuity_references(tx, community_id).await?;
    let checkpoints = load_checkpoints(tx, community_id, &references).await?;
    let handoffs = load_handoffs(tx, community_id, &references).await?;
    let referenceable_object_ids = sqlx::query_scalar(
        "SELECT object_id FROM project_view_objects \
         WHERE community_id = $1 ORDER BY object_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let state = RoleContinuityState::from_complete_snapshot(
        project_revision,
        roles,
        works,
        members,
        proposals,
        assignments,
        commitments,
        checkpoints,
        handoffs,
        referenceable_object_ids,
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

async fn load_work_responsibilities(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<WorkResponsibility>> {
    let rows = sqlx::query(
        "SELECT object_id, body->>'status' AS status, responsible_role_id, \
                object_revision, project_revision, updated_at, updated_by, deleted_at \
         FROM project_view_objects \
         WHERE community_id = $1 AND object_type = 'work' \
         ORDER BY object_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let deleted_at: Option<DateTime<Utc>> = row.try_get("deleted_at")?;
            let status = if deleted_at.is_some() {
                None
            } else {
                let value: String = row.try_get("status")?;
                Some(parse_work_status(&value)?)
            };
            Ok(WorkResponsibility {
                work_id: row.try_get("object_id")?,
                status,
                responsible_role_id: row.try_get("responsible_role_id")?,
                object_revision: db_revision(
                    row.try_get("object_revision")?,
                    "work.object_revision",
                )?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "work.project_revision",
                )?,
                updated_at: row.try_get("updated_at")?,
                updated_by: public_key(
                    &row.try_get::<Vec<u8>, _>("updated_by")?,
                    "work.updated_by",
                )?,
            })
        })
        .collect()
}

#[derive(Debug)]
struct LoadedV2ProjectObjectState {
    state: ProjectViewState,
    canonical_time: DateTime<Utc>,
    projection_generation: u64,
    projection_pubkey: PublicKey,
    meta_projection_event_id: [u8; 32],
    membership_snapshot_event_id: Option<EventId>,
    role_levels: BTreeMap<Uuid, RoleLevel>,
    work_responsibilities: BTreeMap<Uuid, Uuid>,
}

async fn load_v2_project_object_state(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<LoadedV2ProjectObjectState> {
    let continuity = load_v2_state(tx, community_id).await?;
    let state_times = sqlx::query(
        "SELECT initialized_at, updated_at \
         FROM project_view_state \
         WHERE community_id = $1 AND schema_version = 2",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    let initialized_at: DateTime<Utc> = state_times.try_get("initialized_at")?;
    let updated_at: DateTime<Utc> = state_times.try_get("updated_at")?;
    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, \
                updated_by, deleted_at, responsible_role_id \
         FROM project_view_objects \
         WHERE community_id = $1 ORDER BY object_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut work_responsibilities = BTreeMap::new();
    let entries = rows
        .into_iter()
        .map(|row| {
            let object_id: Uuid = row.try_get("object_id")?;
            let responsible_role_id: Option<Uuid> = row.try_get("responsible_role_id")?;
            if let Some(role_id) = responsible_role_id {
                work_responsibilities.insert(object_id, role_id);
            }
            crate::project_view::entry_from_row(row).map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "load schema-v2 Project object: {error}"
                ))
            })
        })
        .collect::<ProjectViewV2WriteResult<Vec<_>>>()?;
    let state = ProjectViewState::from_snapshot(
        community_id,
        continuity.state.project_revision(),
        Some(initialized_at),
        Some(updated_at),
        entries,
    )?;
    let role_rows = sqlx::query(
        "SELECT object_id, role_level \
         FROM project_view_objects \
         WHERE community_id = $1 AND object_type = 'role'",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let role_levels = role_rows
        .into_iter()
        .map(|row| {
            let role_id: Uuid = row.try_get("object_id")?;
            let level: String = row.try_get("role_level")?;
            Ok((role_id, parse_role_level(&level)?))
        })
        .collect::<ProjectViewV2WriteResult<BTreeMap<_, _>>>()?;
    Ok(LoadedV2ProjectObjectState {
        state,
        canonical_time: continuity.canonical_time,
        projection_generation: continuity.projection_generation,
        projection_pubkey: continuity.projection_pubkey,
        meta_projection_event_id: continuity.meta_projection_event_id,
        membership_snapshot_event_id: continuity.membership_snapshot_event_id,
        role_levels,
        work_responsibilities,
    })
}

async fn validate_project_object_actor_fence(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    acting_assignment_id: Option<Uuid>,
    runtime_fence: Option<buzz_project_view::v2::RuntimeFence>,
) -> ProjectViewV2WriteResult<()> {
    let actor_bytes = actor.to_bytes();
    let managed: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM users \
             WHERE community_id = $1 AND pubkey = $2 \
               AND agent_owner_pubkey IS NOT NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(actor_bytes.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if managed && acting_assignment_id.is_none() {
        return Err(ProjectViewV2WriteError::Domain(
            RoleContinuityError::ActingAssignmentRequired,
        ));
    }
    let Some(assignment_id) = acting_assignment_id else {
        return Ok(());
    };
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_role_assignments \
             WHERE community_id = $1 AND assignment_id = $2 \
               AND member_pubkey = $3 AND ended_at IS NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(assignment_id)
    .bind(actor.to_hex())
    .fetch_one(&mut **tx)
    .await?;
    if !valid {
        return Err(ProjectViewV2WriteError::Domain(
            RoleContinuityError::ActingAssignmentInvalid,
        ));
    }
    crate::project_runtime::validate_runtime_command_fence_in_tx(
        tx,
        community_id,
        Some(assignment_id),
        runtime_fence,
    )
    .await?;
    Ok(())
}

async fn reject_assigned_role_deactivation(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    entries: &[ProjectViewEntry],
) -> ProjectViewV2WriteResult<()> {
    let role_ids = entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectViewEntry::Active(object)
                if matches!(
                    &object.data,
                    ProjectViewObjectData::Role(role) if !role.active
                ) =>
            {
                Some(object.id)
            }
            ProjectViewEntry::Tombstone(tombstone)
                if tombstone.object_type == ProjectViewObjectType::Role =>
            {
                Some(tombstone.id)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if role_ids.is_empty() {
        return Ok(());
    }
    let assigned_role: Option<Uuid> = sqlx::query_scalar(
        "SELECT role_id FROM project_role_assignments \
         WHERE community_id = $1 AND role_id = ANY($2) AND ended_at IS NULL \
         ORDER BY role_id LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(&role_ids)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(role_id) = assigned_role {
        return Err(ProjectViewV2WriteError::ObjectDomain(
            DomainError::InvalidField {
                field: "active",
                reason: format!(
                    "Role {role_id} has an active Assignment; end the Assignment before deactivating or deleting the Role"
                ),
            },
        ));
    }
    let responsible_role: Option<Uuid> = sqlx::query_scalar(
        "SELECT responsible_role_id FROM project_view_objects \
         WHERE community_id = $1 \
           AND responsible_role_id = ANY($2) \
           AND object_type = 'work' AND deleted_at IS NULL \
         ORDER BY responsible_role_id LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(&role_ids)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(role_id) = responsible_role {
        return Err(ProjectViewV2WriteError::ObjectDomain(
            DomainError::InvalidField {
                field: "active",
                reason: format!(
                    "Role {role_id} still owns Work; clear or reassign that responsibility before deactivating or deleting the Role"
                ),
            },
        ));
    }
    Ok(())
}

fn role_definition_from_object(
    object: &ProjectViewObject,
    level: RoleLevel,
) -> ProjectViewV2WriteResult<RoleDefinition> {
    let ProjectViewObjectData::Role(role) = &object.data else {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "Role head was prepared from a non-Role object".to_owned(),
        ));
    };
    Ok(RoleDefinition {
        role_id: object.id,
        name: role.name.clone(),
        purpose: role.purpose.clone(),
        responsibilities: role.responsibilities.clone(),
        boundaries: role.boundaries.clone(),
        level,
        active: role.active,
        object_revision: object.object_revision,
        project_revision: object.project_revision,
        created_at: object.created_at,
        updated_at: object.updated_at,
        created_by: object.created_by,
        updated_by: object.updated_by,
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

async fn load_commitments(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<Vec<WorkCommitment>> {
    let rows = sqlx::query(
        "SELECT commitment_id, work_id, assignment_id, member_pubkey, \
                accepted_at, accepted_by, ended_at, ended_by, ended_reason, \
                entity_revision, project_revision \
         FROM project_work_commitments \
         WHERE community_id = $1 ORDER BY commitment_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let ended_reason: Option<String> = row.try_get("ended_reason")?;
            Ok(WorkCommitment {
                commitment_id: row.try_get("commitment_id")?,
                work_id: row.try_get("work_id")?,
                assignment_id: row.try_get("assignment_id")?,
                member_pubkey: parse_pubkey(
                    &row.try_get::<String, _>("member_pubkey")?,
                    "commitment.member_pubkey",
                )?,
                started_at: row.try_get("accepted_at")?,
                started_by: public_key(
                    &row.try_get::<Vec<u8>, _>("accepted_by")?,
                    "commitment.accepted_by",
                )?,
                ended_at: row.try_get("ended_at")?,
                ended_by: row
                    .try_get::<Option<Vec<u8>>, _>("ended_by")?
                    .map(|bytes| public_key(&bytes, "commitment.ended_by"))
                    .transpose()?,
                ended_reason: ended_reason
                    .map(|reason| parse_commitment_end_reason(&reason))
                    .transpose()?,
                entity_revision: db_revision(
                    row.try_get("entity_revision")?,
                    "commitment.entity_revision",
                )?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "commitment.project_revision",
                )?,
            })
        })
        .collect()
}

type ContinuityReferences = BTreeMap<(RoleContinuityEntity, Uuid), Vec<RoleContinuityReference>>;

async fn load_continuity_references(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV2WriteResult<ContinuityReferences> {
    let rows = sqlx::query(
        "SELECT owner_type, owner_id, reference_type, object_id, assignment_id, \
                commitment_id, nostr_event_id, label \
         FROM project_role_continuity_references \
         WHERE community_id = $1 ORDER BY owner_type, owner_id, position",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut references = ContinuityReferences::new();
    for row in rows {
        let owner_type: String = row.try_get("owner_type")?;
        let entity_type = match owner_type.as_str() {
            "checkpoint" => RoleContinuityEntity::RoleCheckpoint,
            "handoff" => RoleContinuityEntity::RoleHandoff,
            _ => {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "invalid Role continuity reference owner type {owner_type}"
                )));
            }
        };
        let reference_type: String = row.try_get("reference_type")?;
        let label: Option<String> = row.try_get("label")?;
        let reference = match reference_type.as_str() {
            "object" => RoleContinuityReference::Object {
                object_id: row
                    .try_get::<Option<Uuid>, _>("object_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "object reference has no object_id".to_owned(),
                        )
                    })?,
                label,
            },
            "assignment" => RoleContinuityReference::Assignment {
                assignment_id: row
                    .try_get::<Option<Uuid>, _>("assignment_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "Assignment reference has no assignment_id".to_owned(),
                        )
                    })?,
                label,
            },
            "commitment" => RoleContinuityReference::Commitment {
                commitment_id: row
                    .try_get::<Option<Uuid>, _>("commitment_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "Commitment reference has no commitment_id".to_owned(),
                        )
                    })?,
                label,
            },
            "nostr_event" => RoleContinuityReference::NostrEvent {
                event_id: event_id(
                    row.try_get::<Option<Vec<u8>>, _>("nostr_event_id")?
                        .ok_or_else(|| {
                            ProjectViewV2WriteError::InvalidCommit(
                                "Nostr reference has no event_id".to_owned(),
                            )
                        })?,
                    "continuity_reference.nostr_event_id",
                )?,
                label,
            },
            _ => {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "invalid Role continuity reference type {reference_type}"
                )));
            }
        };
        references
            .entry((entity_type, row.try_get("owner_id")?))
            .or_default()
            .push(reference);
    }
    Ok(references)
}

async fn load_checkpoints(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    references: &ContinuityReferences,
) -> ProjectViewV2WriteResult<Vec<RoleCheckpoint>> {
    let rows = sqlx::query(
        "SELECT checkpoint_id, role_id, assignment_id, based_on_project_revision, \
                body, supersedes_checkpoint_id, created_by, created_at, \
                entity_revision, project_revision \
         FROM project_role_checkpoints WHERE community_id = $1 ORDER BY checkpoint_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let checkpoint_id: Uuid = row.try_get("checkpoint_id")?;
            let mut content: RoleCheckpointContent = serde_json::from_value(row.try_get("body")?)
                .map_err(|error| {
                ProjectViewV2WriteError::InvalidCommit(format!(
                    "stored Checkpoint content is invalid: {error}"
                ))
            })?;
            content.references = references
                .get(&(RoleContinuityEntity::RoleCheckpoint, checkpoint_id))
                .cloned()
                .unwrap_or_default();
            Ok(RoleCheckpoint {
                checkpoint_id,
                role_id: row.try_get("role_id")?,
                assignment_id: row.try_get("assignment_id")?,
                based_on_project_revision: db_revision(
                    row.try_get("based_on_project_revision")?,
                    "checkpoint.based_on_project_revision",
                )?,
                content,
                supersedes_checkpoint_id: row.try_get("supersedes_checkpoint_id")?,
                created_by: public_key(
                    &row.try_get::<Vec<u8>, _>("created_by")?,
                    "checkpoint.created_by",
                )?,
                created_at: row.try_get("created_at")?,
                entity_revision: db_revision(
                    row.try_get("entity_revision")?,
                    "checkpoint.entity_revision",
                )?,
                project_revision: db_revision(
                    row.try_get("project_revision")?,
                    "checkpoint.project_revision",
                )?,
            })
        })
        .collect()
}

async fn load_handoffs(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    references: &ContinuityReferences,
) -> ProjectViewV2WriteResult<Vec<RoleHandoff>> {
    let rows = sqlx::query(
        "SELECT handoff_id, role_id, from_assignment_id, to_assignment_id, \
                checkpoint_id, body, system_generated, created_by, created_at, \
                entity_revision, project_revision \
         FROM project_role_handoffs WHERE community_id = $1 ORDER BY handoff_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let handoff_id: Uuid = row.try_get("handoff_id")?;
            let body: Value = row.try_get("body")?;
            let cause = body
                .get("cause")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    ProjectViewV2WriteError::InvalidCommit(
                        "stored Handoff body has no cause".to_owned(),
                    )
                })
                .and_then(parse_handoff_cause)?;
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
            let mut content = body
                .get("content")
                .cloned()
                .map(serde_json::from_value::<RoleHandoffContent>)
                .transpose()
                .map_err(|error| {
                    ProjectViewV2WriteError::InvalidCommit(format!(
                        "stored Handoff content is invalid: {error}"
                    ))
                })?
                .unwrap_or_default();
            content.references = references
                .get(&(RoleContinuityEntity::RoleHandoff, handoff_id))
                .cloned()
                .unwrap_or_default();
            Ok(RoleHandoff {
                handoff_id,
                role_id: row.try_get("role_id")?,
                from_assignment_id: row
                    .try_get::<Option<Uuid>, _>("from_assignment_id")?
                    .ok_or_else(|| {
                        ProjectViewV2WriteError::InvalidCommit(
                            "system Handoff is missing from_assignment_id".to_owned(),
                        )
                    })?,
                to_assignment_id: row.try_get("to_assignment_id")?,
                checkpoint_id: row.try_get("checkpoint_id")?,
                affected_commitment_ids,
                content,
                cause,
                system_generated: row.try_get("system_generated")?,
                created_by: row
                    .try_get::<Option<Vec<u8>>, _>("created_by")?
                    .map(|bytes| public_key(&bytes, "handoff.created_by"))
                    .transpose()?,
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

fn project_object_receipt(
    command: &ProjectObjectCommand,
    entries: &[ProjectViewEntry],
    continuity_changes: &[RoleContinuityChange],
    project_revision: u64,
) -> Value {
    let mut result = serde_json::Map::new();
    result.insert("project_revision".to_owned(), Value::from(project_revision));
    result.insert(
        "operation".to_owned(),
        Value::String(command.operation().to_owned()),
    );
    if let [entry] = entries {
        result.insert(
            "object_id".to_owned(),
            Value::String(entry.id().to_string()),
        );
        result.insert(
            "object_revision".to_owned(),
            Value::from(entry.object_revision()),
        );
        result.insert(
            "deleted".to_owned(),
            Value::Bool(matches!(entry, ProjectViewEntry::Tombstone(_))),
        );
    }
    if !continuity_changes.is_empty() {
        result.insert(
            "changed_entities".to_owned(),
            Value::Array(
                continuity_changes
                    .iter()
                    .map(|change| {
                        json!({
                            "entity_type": change.entity_type().as_str(),
                            "entity_id": change.entity_id(),
                            "entity_revision": change.entity_revision(),
                        })
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(result)
}

#[allow(clippy::too_many_arguments)]
async fn insert_project_object_change(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    command: &ProjectObjectCommand,
    project_revision: u64,
    canonical_time: DateTime<Utc>,
    result: &Value,
) -> ProjectViewV2WriteResult<()> {
    let actor = event.pubkey.to_bytes();
    let subject = serde_json::to_value(&command.request).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "serialize v2 object command subject: {error}"
        ))
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
            RoleContinuityEntity::WorkCommitment => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_work_commitments \
                 WHERE community_id = $1 AND commitment_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(change.entity_id())
            .fetch_optional(&mut **tx)
            .await?
            .flatten(),
            RoleContinuityEntity::RoleCheckpoint => sqlx::query_scalar(
                "SELECT projection_event_id FROM project_role_checkpoints \
                 WHERE community_id = $1 AND checkpoint_id = $2 FOR UPDATE",
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

async fn prepare_work_responsibility_heads(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    changes: &[WorkResponsibility],
) -> ProjectViewV2WriteResult<(Vec<PreparedV2ProjectObjectHead>, BTreeMap<Uuid, [u8; 32]>)> {
    if changes.is_empty() {
        return Ok((Vec::new(), BTreeMap::new()));
    }
    let ids = changes.iter().map(|work| work.work_id).collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, \
                updated_by, deleted_at, projection_event_id \
         FROM project_view_objects \
         WHERE community_id = $1 AND object_id = ANY($2) \
         ORDER BY object_id FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(&ids)
    .fetch_all(&mut **tx)
    .await?;
    let by_id = changes
        .iter()
        .map(|work| (work.work_id, work))
        .collect::<BTreeMap<_, _>>();
    let mut heads = Vec::with_capacity(rows.len());
    let mut old_projection_ids = BTreeMap::new();
    for row in rows {
        let object_id: Uuid = row.try_get("object_id")?;
        let projection_event_id = bytes32(
            row.try_get("projection_event_id")?,
            "work.projection_event_id",
        )?;
        let entry = crate::project_view::entry_from_row(row).map_err(|error| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "load responsibility Work {object_id}: {error}"
            ))
        })?;
        let work = by_id.get(&object_id).ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "loaded unexpected responsibility Work {object_id}"
            ))
        })?;
        let ProjectViewEntry::Active(mut object) = entry else {
            return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                "responsibility Work {object_id} is deleted"
            )));
        };
        let current_status = match &object.data {
            ProjectViewObjectData::Work(work) => work.status,
            _ => {
                return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                    "responsibility target {object_id} is not Work"
                )));
            }
        };
        if work.status != Some(current_status)
            || object.object_revision.checked_add(1) != Some(work.object_revision)
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                "responsibility Work {object_id} disagrees with canonical object state"
            )));
        }
        object.object_revision = work.object_revision;
        object.project_revision = work.project_revision;
        object.updated_at = work.updated_at;
        object.updated_by = work.updated_by;
        heads.push(PreparedV2ProjectObjectHead::Object {
            entry: ProjectViewEntry::Active(object),
            responsible_role_id: work.responsible_role_id,
        });
        old_projection_ids.insert(object_id, projection_event_id);
    }
    if heads.len() != changes.len() {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "one or more responsibility Work rows are missing".to_owned(),
        ));
    }
    Ok((heads, old_projection_ids))
}

async fn close_commitments_for_terminal_work(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    changed_entries: &[ProjectViewEntry],
    actor: PublicKey,
    canonical_time: DateTime<Utc>,
    project_revision: u64,
) -> ProjectViewV2WriteResult<Vec<RoleContinuityChange>> {
    let work_ids = changed_entries
        .iter()
        .filter(|entry| match entry {
            ProjectViewEntry::Active(object)
                if object.object_type == ProjectViewObjectType::Work =>
            {
                matches!(
                    &object.data,
                    ProjectViewObjectData::Work(work)
                        if matches!(work.status, WorkStatus::Completed | WorkStatus::Cancelled)
                )
            }
            ProjectViewEntry::Tombstone(tombstone) => {
                tombstone.object_type == ProjectViewObjectType::Work
            }
            ProjectViewEntry::Active(_) => false,
        })
        .map(ProjectViewEntry::id)
        .collect::<Vec<_>>();
    if work_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT commitment_id, work_id, assignment_id, member_pubkey, \
                accepted_at, accepted_by, entity_revision \
         FROM project_work_commitments \
         WHERE community_id = $1 AND work_id = ANY($2) AND ended_at IS NULL \
         ORDER BY commitment_id FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(&work_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            let entity_revision = db_revision(
                row.try_get("entity_revision")?,
                "commitment.entity_revision",
            )?
            .checked_add(1)
            .ok_or_else(|| {
                ProjectViewV2WriteError::InvalidCommit(
                    "Commitment entity revision overflow".to_owned(),
                )
            })?;
            Ok(RoleContinuityChange::Commitment(WorkCommitment {
                commitment_id: row.try_get("commitment_id")?,
                work_id: row.try_get("work_id")?,
                assignment_id: row.try_get("assignment_id")?,
                member_pubkey: parse_pubkey(
                    &row.try_get::<String, _>("member_pubkey")?,
                    "commitment.member_pubkey",
                )?,
                started_at: row.try_get("accepted_at")?,
                started_by: public_key(
                    &row.try_get::<Vec<u8>, _>("accepted_by")?,
                    "commitment.accepted_by",
                )?,
                ended_at: Some(canonical_time),
                ended_by: Some(actor),
                ended_reason: Some(CommitmentEndReason::WorkClosed),
                entity_revision,
                project_revision,
            }))
        })
        .collect()
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
    // End old Commitments and seats before inserting their replacements so
    // partial unique indexes remain satisfied throughout the transaction.
    for commitment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Commitment(commitment) if !commitment.is_active() => Some(commitment),
        _ => None,
    }) {
        persist_commitment(tx, community_id, change_id, canonical_time, commitment).await?;
    }
    for assignment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Assignment(assignment) if !assignment.is_active() => Some(assignment),
        _ => None,
    }) {
        persist_assignment(tx, community_id, change_id, canonical_time, assignment).await?;
    }
    for commitment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Commitment(commitment) if commitment.is_active() => Some(commitment),
        _ => None,
    }) {
        persist_commitment(tx, community_id, change_id, canonical_time, commitment).await?;
    }
    for assignment in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Assignment(assignment) if assignment.is_active() => Some(assignment),
        _ => None,
    }) {
        persist_assignment(tx, community_id, change_id, canonical_time, assignment).await?;
    }
    for checkpoint in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Checkpoint(checkpoint) => Some(checkpoint),
        _ => None,
    }) {
        persist_checkpoint(tx, community_id, change_id, checkpoint).await?;
    }
    for handoff in changes.iter().filter_map(|change| match change {
        RoleContinuityChange::Handoff(handoff) => Some(handoff),
        _ => None,
    }) {
        persist_handoff(tx, community_id, change_id, handoff).await?;
    }
    Ok(())
}

async fn persist_checkpoint(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    checkpoint: &RoleCheckpoint,
) -> ProjectViewV2WriteResult<()> {
    let mut stored_content = checkpoint.content.clone();
    stored_content.references.clear();
    let body = serde_json::to_value(stored_content).map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "cannot serialize Checkpoint content: {error}"
        ))
    })?;
    let created_by = checkpoint.created_by.to_bytes();
    let result = sqlx::query(
        "INSERT INTO project_role_checkpoints \
            (community_id, checkpoint_id, role_id, assignment_id, \
             based_on_project_revision, body, supersedes_checkpoint_id, \
             created_by, created_at, source_change_id, last_change_id, \
             entity_revision, project_revision) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10,$11,$12) \
         ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(checkpoint.checkpoint_id)
    .bind(checkpoint.role_id)
    .bind(checkpoint.assignment_id)
    .bind(revision_i64(
        checkpoint.based_on_project_revision,
        "checkpoint.based_on_project_revision",
    )?)
    .bind(body)
    .bind(checkpoint.supersedes_checkpoint_id)
    .bind(created_by.as_slice())
    .bind(checkpoint.created_at)
    .bind(change_id)
    .bind(revision_i64(
        checkpoint.entity_revision,
        "checkpoint.entity_revision",
    )?)
    .bind(revision_i64(
        checkpoint.project_revision,
        "checkpoint.project_revision",
    )?)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "Checkpoint {} already exists",
            checkpoint.checkpoint_id
        )));
    }
    persist_continuity_references(
        tx,
        community_id,
        change_id,
        RoleContinuityEntity::RoleCheckpoint,
        checkpoint.checkpoint_id,
        &checkpoint.content.references,
    )
    .await
}

async fn persist_commitment(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    canonical_time: DateTime<Utc>,
    commitment: &WorkCommitment,
) -> ProjectViewV2WriteResult<()> {
    let member = commitment.member_pubkey.to_hex();
    let started_by = commitment.started_by.to_bytes();
    let ended_by = commitment.ended_by.map(PublicKey::to_bytes);
    let ended_change_id = commitment.ended_at.map(|_| change_id);
    let result = sqlx::query(
        "INSERT INTO project_work_commitments \
            (community_id, commitment_id, work_id, assignment_id, member_pubkey, \
             accepted_at, accepted_by, ended_at, ended_by, ended_reason, \
             source_change_id, ended_source_change_id, last_change_id, \
             entity_revision, project_revision, updated_at) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$11,$13,$14,$15) \
         ON CONFLICT (community_id, commitment_id) DO UPDATE SET \
             ended_at = EXCLUDED.ended_at, ended_by = EXCLUDED.ended_by, \
             ended_reason = EXCLUDED.ended_reason, \
             ended_source_change_id = EXCLUDED.ended_source_change_id, \
             last_change_id = EXCLUDED.last_change_id, \
             entity_revision = EXCLUDED.entity_revision, \
             project_revision = EXCLUDED.project_revision, \
             updated_at = EXCLUDED.updated_at \
         WHERE project_work_commitments.work_id = EXCLUDED.work_id \
           AND project_work_commitments.assignment_id = EXCLUDED.assignment_id \
           AND project_work_commitments.member_pubkey = EXCLUDED.member_pubkey \
           AND project_work_commitments.accepted_at = EXCLUDED.accepted_at \
           AND project_work_commitments.accepted_by = EXCLUDED.accepted_by \
           AND project_work_commitments.ended_at IS NULL \
           AND project_work_commitments.entity_revision + 1 = EXCLUDED.entity_revision",
    )
    .bind(community_id.as_uuid())
    .bind(commitment.commitment_id)
    .bind(commitment.work_id)
    .bind(commitment.assignment_id)
    .bind(member)
    .bind(commitment.started_at)
    .bind(started_by.as_slice())
    .bind(commitment.ended_at)
    .bind(ended_by.as_ref().map(<[u8; 32]>::as_slice))
    .bind(commitment.ended_reason.map(CommitmentEndReason::as_str))
    .bind(change_id)
    .bind(ended_change_id)
    .bind(revision_i64(
        commitment.entity_revision,
        "commitment.entity_revision",
    )?)
    .bind(revision_i64(
        commitment.project_revision,
        "commitment.project_revision",
    )?)
    .bind(canonical_time)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "Commitment {} did not advance exactly one entity revision",
            commitment.commitment_id
        )));
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
    let mut stored_content = handoff.content.clone();
    stored_content.references.clear();
    let body = json!({
        "cause": handoff.cause.as_str(),
        "affected_commitment_ids": handoff.affected_commitment_ids,
        "content": stored_content,
    });
    let created_by = handoff.created_by.map(PublicKey::to_bytes);
    let result = sqlx::query(
        "INSERT INTO project_role_handoffs \
            (community_id, handoff_id, role_id, from_assignment_id, to_assignment_id, \
             checkpoint_id, body, system_generated, created_by, created_at, \
             source_change_id, last_change_id, entity_revision, project_revision) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11,$12,$13) \
         ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(handoff.handoff_id)
    .bind(handoff.role_id)
    .bind(handoff.from_assignment_id)
    .bind(handoff.to_assignment_id)
    .bind(handoff.checkpoint_id)
    .bind(body)
    .bind(handoff.system_generated)
    .bind(created_by.as_ref().map(<[u8; 32]>::as_slice))
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
    persist_continuity_references(
        tx,
        community_id,
        change_id,
        RoleContinuityEntity::RoleHandoff,
        handoff.handoff_id,
        &handoff.content.references,
    )
    .await
}

async fn persist_continuity_references(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
    owner_type: RoleContinuityEntity,
    owner_id: Uuid,
    references: &[RoleContinuityReference],
) -> ProjectViewV2WriteResult<()> {
    let owner_type = match owner_type {
        RoleContinuityEntity::RoleCheckpoint => "checkpoint",
        RoleContinuityEntity::RoleHandoff => "handoff",
        _ => {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "only Checkpoints and Handoffs own continuity references".to_owned(),
            ));
        }
    };
    for (position, reference) in references.iter().enumerate() {
        let (reference_type, object_id, assignment_id, commitment_id, nostr_event_id, label) =
            match reference {
                RoleContinuityReference::Object { object_id, label } => (
                    "object",
                    Some(*object_id),
                    None,
                    None,
                    None,
                    label.as_deref(),
                ),
                RoleContinuityReference::Assignment {
                    assignment_id,
                    label,
                } => (
                    "assignment",
                    None,
                    Some(*assignment_id),
                    None,
                    None,
                    label.as_deref(),
                ),
                RoleContinuityReference::Commitment {
                    commitment_id,
                    label,
                } => (
                    "commitment",
                    None,
                    None,
                    Some(*commitment_id),
                    None,
                    label.as_deref(),
                ),
                RoleContinuityReference::NostrEvent { event_id, label } => (
                    "nostr_event",
                    None,
                    None,
                    None,
                    Some(event_id.to_bytes()),
                    label.as_deref(),
                ),
            };
        let result = sqlx::query(
            "INSERT INTO project_role_continuity_references \
                (community_id, owner_type, owner_id, position, reference_type, \
                 object_id, assignment_id, commitment_id, nostr_event_id, label, \
                 source_change_id) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) \
             ON CONFLICT DO NOTHING",
        )
        .bind(community_id.as_uuid())
        .bind(owner_type)
        .bind(owner_id)
        .bind(i32::try_from(position).map_err(|_| {
            ProjectViewV2WriteError::InvalidCommit(
                "continuity reference position exceeds PostgreSQL INT".to_owned(),
            )
        })?)
        .bind(reference_type)
        .bind(object_id)
        .bind(assignment_id)
        .bind(commitment_id)
        .bind(nostr_event_id.as_ref().map(<[u8; 32]>::as_slice))
        .bind(label)
        .bind(change_id)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectViewV2WriteError::InvalidCommit(format!(
                "continuity reference {owner_type}/{owner_id}/{position} already exists"
            )));
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
    let expected_entities = basis
        .preparation
        .changes
        .iter()
        .map(|change| ((change.entity_type(), change.entity_id()), change))
        .collect::<BTreeMap<_, _>>();
    if commit.entity_projections.len() != expected_entities.len()
        || commit.object_projections.len() != basis.preparation.work_heads.len()
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "projection counts do not match changed Role entities and Work objects".to_owned(),
        ));
    }
    let context = buzz_sdk::project_view_v2::V2ProjectionContext {
        project_id: basis.preparation.community_id,
        projection_generation: basis.preparation.projection_generation,
        project_revision: basis.preparation.project_revision,
        source: expected_source.clone(),
        updated_at: basis.preparation.canonical_time,
    };
    let mut expected_heads = BTreeMap::new();
    let mut actual_entities = BTreeSet::new();
    for projection in &commit.entity_projections {
        let key = (projection.entity_type, projection.entity_id);
        if !actual_entities.insert(key) || !expected_entities.contains_key(&key) {
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
            || Some(&parsed.entity) != expected_entities.get(&key).copied()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "signed entity projection differs from canonical change".to_owned(),
            ));
        }
        let changed_head = buzz_sdk::project_view_v2::changed_head_for(
            &context,
            &parsed.entity,
            &projection.event,
        )
        .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        expected_heads.insert(changed_head.coordinate().to_owned(), changed_head);
    }
    let object_projections = commit
        .object_projections
        .iter()
        .map(|projection| (projection.object_id(), projection.event()))
        .collect::<BTreeMap<_, _>>();
    if object_projections.len() != commit.object_projections.len() {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "Role command contains duplicate Work object projections".to_owned(),
        ));
    }
    for head in &basis.preparation.work_heads {
        let PreparedV2ProjectObjectHead::Object {
            entry,
            responsible_role_id,
        } = head
        else {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "Role command prepared a non-Work object head".to_owned(),
            ));
        };
        let event = object_projections.get(&entry.id()).ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "missing Work projection for responsibility change {}",
                entry.id()
            ))
        })?;
        let parsed = buzz_sdk::project_view_v2::parse_project_object_projection(
            event,
            &basis.preparation.projection_pubkey,
            basis.preparation.community_id,
        )
        .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        if parsed.project_revision != basis.preparation.project_revision
            || parsed.projection_generation != basis.preparation.projection_generation
            || parsed.source != expected_source
            || parsed.updated_at != basis.preparation.canonical_time
            || parsed.responsible_role_id != *responsible_role_id
            || !projected_object_matches_entry(&parsed.object, entry)
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "signed Work projection differs from canonical responsibility change".to_owned(),
            ));
        }
        let changed_head =
            buzz_sdk::project_view_v2::changed_head_for_project_object(&context, entry, event)
                .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed_head.coordinate().to_owned(), changed_head)
            .is_some()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "Role command contains duplicate changed-head coordinates".to_owned(),
            ));
        }
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
        || meta.changed_heads.len() != expected_heads.len()
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
        .map(|head| (head.coordinate().to_owned(), head.clone()))
        .collect::<BTreeMap<_, _>>();
    if actual_heads.len() != meta.changed_heads.len() || actual_heads != expected_heads {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "metadata changed heads do not exactly bind Role entities and Work objects".to_owned(),
        ));
    }
    Ok(())
}

fn validate_project_object_commit_bundle(
    basis: &V2PreparedProjectObjectBasis,
    commit: &PreparedV2ProjectObjectCommit,
) -> ProjectViewV2WriteResult<()> {
    if commit.command_event.id.to_bytes() != basis.command_event_id
        || commit.command_event.pubkey != basis.actor
        || ProjectObjectCommand::from_json(&commit.command_event.content)? != basis.command
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "committed object command differs from the prepared command".to_owned(),
        ));
    }
    commit.command_event.verify().map_err(|error| {
        ProjectViewV2WriteError::InvalidCommit(format!(
            "committed object command signature is invalid: {error}"
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
            "committed object command tags are not the exact protected v2 shape".to_owned(),
        ));
    }
    if basis.next_state.project_revision() != basis.preparation.project_revision
        || basis.outcome.project_revision != basis.preparation.project_revision
        || basis.continuity_changes != basis.preparation.entity_changes
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "prepared object state and outcome revisions disagree".to_owned(),
        ));
    }

    let projection_map = commit
        .object_projections
        .iter()
        .map(|projection| (projection.object_id(), projection.event()))
        .collect::<BTreeMap<_, _>>();
    if projection_map.len() != commit.object_projections.len()
        || projection_map.len() != basis.preparation.heads.len()
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "projection set does not exactly cover changed Project objects".to_owned(),
        ));
    }
    let expected_source = buzz_sdk::project_view_v2::V2ProjectionSource::NostrEvent {
        change_id: commit.command_event.id,
        event_id: commit.command_event.id,
    };
    let context = buzz_sdk::project_view_v2::V2ProjectionContext {
        project_id: basis.preparation.community_id,
        projection_generation: basis.preparation.projection_generation,
        project_revision: basis.preparation.project_revision,
        source: expected_source.clone(),
        updated_at: basis.preparation.canonical_time,
    };
    let mut expected_heads = BTreeMap::new();
    for head in &basis.preparation.heads {
        let event = projection_map.get(&head.object_id()).ok_or_else(|| {
            ProjectViewV2WriteError::InvalidCommit(format!(
                "missing projection for prepared object {}",
                head.object_id()
            ))
        })?;
        let changed_head = match head {
            PreparedV2ProjectObjectHead::Role(role) => {
                let parsed = buzz_sdk::project_view_v2::parse_entity_projection(
                    event,
                    &basis.preparation.projection_pubkey,
                    basis.preparation.community_id,
                )
                .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
                if parsed.project_revision != basis.preparation.project_revision
                    || parsed.projection_generation != basis.preparation.projection_generation
                    || parsed.source != expected_source
                    || parsed.updated_at != basis.preparation.canonical_time
                    || parsed.entity != RoleContinuityChange::Role(role.clone())
                {
                    return Err(ProjectViewV2WriteError::InvalidCommit(
                        "signed Role head differs from the prepared object change".to_owned(),
                    ));
                }
                buzz_sdk::project_view_v2::changed_head_for(
                    &context,
                    &RoleContinuityChange::Role(role.clone()),
                    event,
                )
            }
            PreparedV2ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } => {
                let parsed = buzz_sdk::project_view_v2::parse_project_object_projection(
                    event,
                    &basis.preparation.projection_pubkey,
                    basis.preparation.community_id,
                )
                .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
                if parsed.project_revision != basis.preparation.project_revision
                    || parsed.projection_generation != basis.preparation.projection_generation
                    || parsed.source != expected_source
                    || parsed.updated_at != basis.preparation.canonical_time
                    || parsed.responsible_role_id != *responsible_role_id
                    || !projected_object_matches_entry(&parsed.object, entry)
                {
                    return Err(ProjectViewV2WriteError::InvalidCommit(
                        "signed ordinary-object head differs from the prepared change".to_owned(),
                    ));
                }
                buzz_sdk::project_view_v2::changed_head_for_project_object(&context, entry, event)
            }
        }
        .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed_head.coordinate().to_owned(), changed_head)
            .is_some()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "prepared object change contains duplicate coordinates".to_owned(),
            ));
        }
    }
    let expected_entities = basis
        .preparation
        .entity_changes
        .iter()
        .map(|change| ((change.entity_type(), change.entity_id()), change))
        .collect::<BTreeMap<_, _>>();
    if commit.entity_projections.len() != expected_entities.len() {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "entity projections do not exactly cover terminal Work side effects".to_owned(),
        ));
    }
    let mut actual_entities = BTreeSet::new();
    for projection in &commit.entity_projections {
        let key = (projection.entity_type, projection.entity_id);
        if !actual_entities.insert(key) || !expected_entities.contains_key(&key) {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "unexpected entity projection in ordinary-object command".to_owned(),
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
            || Some(&parsed.entity) != expected_entities.get(&key).copied()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "signed Commitment head differs from terminal Work side effect".to_owned(),
            ));
        }
        let changed_head = buzz_sdk::project_view_v2::changed_head_for(
            &context,
            &parsed.entity,
            &projection.event,
        )
        .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed_head.coordinate().to_owned(), changed_head)
            .is_some()
        {
            return Err(ProjectViewV2WriteError::InvalidCommit(
                "ordinary-object change contains duplicate changed-head coordinates".to_owned(),
            ));
        }
    }

    let meta = buzz_sdk::project_view_v2::parse_meta_projection(
        &commit.meta_projection,
        &basis.preparation.projection_pubkey,
    )
    .map_err(|error| ProjectViewV2WriteError::InvalidCommit(error.to_string()))?;
    let expected_counts = buzz_sdk::project_view_v2::V2EntityCounts {
        active_objects: basis.preparation.counts.active_objects,
        open_proposals: basis.preparation.counts.open_proposals,
        active_assignments: basis.preparation.counts.active_assignments,
        active_commitments: basis.preparation.counts.active_commitments,
        checkpoints: basis.preparation.counts.checkpoints,
        handoffs: basis.preparation.counts.handoffs,
    };
    let actual_heads = meta
        .changed_heads
        .iter()
        .map(|head| (head.coordinate().to_owned(), head.clone()))
        .collect::<BTreeMap<_, _>>();
    if meta.project_id != basis.preparation.community_id
        || meta.project_revision != basis.preparation.project_revision
        || meta.projection_generation != basis.preparation.projection_generation
        || meta.entity_counts != expected_counts
        || meta.membership_snapshot_event_id != basis.preparation.membership_snapshot_event_id
        || meta.reset
        || meta.source != expected_source
        || meta.updated_at != basis.preparation.canonical_time
        || actual_heads.len() != meta.changed_heads.len()
        || actual_heads != expected_heads
    {
        return Err(ProjectViewV2WriteError::InvalidCommit(
            "signed v2 metadata differs from the prepared ordinary-object change".to_owned(),
        ));
    }
    Ok(())
}

fn projected_object_matches_entry(
    projected: &buzz_sdk::project_view_v2::V2ProjectedObject,
    entry: &ProjectViewEntry,
) -> bool {
    match (projected, entry) {
        (
            buzz_sdk::project_view_v2::V2ProjectedObject::Active(projected),
            ProjectViewEntry::Active(entry),
        ) => projected.as_ref() == entry,
        (
            buzz_sdk::project_view_v2::V2ProjectedObject::Tombstone(projected),
            ProjectViewEntry::Tombstone(entry),
        ) => {
            projected.object_id == entry.id
                && projected.object_type == entry.object_type
                && projected.object_revision == entry.object_revision
                && projected.project_revision == entry.project_revision
                && projected.created_at == entry.created_at
                && projected.deleted_at == entry.deleted_at
                && projected.created_by == entry.created_by
                && projected.deleted_by == entry.deleted_by
        }
        _ => false,
    }
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
        RoleContinuityEntity::WorkCommitment => {
            sqlx::query(
                "UPDATE project_work_commitments SET projection_event_id = $3 \
             WHERE community_id = $1 AND commitment_id = $2",
            )
            .bind(community_id.as_uuid())
            .bind(entity_id)
            .bind(event_id)
            .execute(&mut **tx)
            .await?
        }
        RoleContinuityEntity::RoleCheckpoint => {
            sqlx::query(
                "UPDATE project_role_checkpoints SET projection_event_id = $3 \
             WHERE community_id = $1 AND checkpoint_id = $2",
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
    work_changes: &[WorkResponsibility],
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
    let changed_objects = work_changes
        .iter()
        .map(|work| {
            json!({
                "object_type": "work",
                "object_id": work.work_id,
                "object_revision": work.object_revision,
                "responsible_role_id": work.responsible_role_id,
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
    result.insert("changed_objects".to_owned(), Value::Array(changed_objects));
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
        buzz_project_view::v2::RoleCommandRequest::SetWorkResponsibility {
            work_id,
            responsible_role_id,
        } => {
            result.insert("work_id".to_owned(), Value::String(work_id.to_string()));
            result.insert(
                "responsible_role_id".to_owned(),
                responsible_role_id
                    .map_or(Value::Null, |role_id| Value::String(role_id.to_string())),
            );
        }
        buzz_project_view::v2::RoleCommandRequest::AcceptWork {
            commitment_id,
            work_id,
        }
        | buzz_project_view::v2::RoleCommandRequest::ReplaceCommitment {
            commitment_id,
            work_id,
            ..
        } => {
            result.insert("work_id".to_owned(), Value::String(work_id.to_string()));
            result.insert(
                "commitment_id".to_owned(),
                Value::String(commitment_id.to_string()),
            );
        }
        buzz_project_view::v2::RoleCommandRequest::EndCommitment { commitment_id, .. } => {
            result.insert(
                "commitment_id".to_owned(),
                Value::String(commitment_id.to_string()),
            );
        }
        buzz_project_view::v2::RoleCommandRequest::AppendCheckpoint { checkpoint_id, .. } => {
            result.insert(
                "checkpoint_id".to_owned(),
                Value::String(checkpoint_id.to_string()),
            );
        }
        buzz_project_view::v2::RoleCommandRequest::AppendHandoff { handoff_id, .. } => {
            result.insert(
                "handoff_id".to_owned(),
                Value::String(handoff_id.to_string()),
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

fn parse_commitment_end_reason(value: &str) -> ProjectViewV2WriteResult<CommitmentEndReason> {
    match value {
        "released" => Ok(CommitmentEndReason::Released),
        "replaced" => Ok(CommitmentEndReason::Replaced),
        "assignment_ended" => Ok(CommitmentEndReason::AssignmentEnded),
        "work_closed" => Ok(CommitmentEndReason::WorkClosed),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Commitment end reason {value}"
        ))),
    }
}

fn parse_handoff_cause(value: &str) -> ProjectViewV2WriteResult<HandoffCause> {
    match value {
        "planned" => Ok(HandoffCause::Planned),
        "revoked" => Ok(HandoffCause::Revoked),
        "replaced" => Ok(HandoffCause::Replaced),
        "membership_ended" => Ok(HandoffCause::MembershipEnded),
        "unrecoverable" => Ok(HandoffCause::Unrecoverable),
        "role_deactivated" => Ok(HandoffCause::RoleDeactivated),
        "other" => Ok(HandoffCause::Other),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Handoff cause {value}"
        ))),
    }
}

fn parse_work_status(value: &str) -> ProjectViewV2WriteResult<WorkStatus> {
    match value {
        "pending" => Ok(WorkStatus::Pending),
        "in_progress" => Ok(WorkStatus::InProgress),
        "paused" => Ok(WorkStatus::Paused),
        "submitted" => Ok(WorkStatus::Submitted),
        "completed" => Ok(WorkStatus::Completed),
        "cancelled" => Ok(WorkStatus::Cancelled),
        _ => Err(ProjectViewV2WriteError::InvalidCommit(format!(
            "invalid Work status {value}"
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
