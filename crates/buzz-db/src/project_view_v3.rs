//! Atomic Project View v3 coordinator.
//!
//! The v3 transaction owns the Community advisory lock across current-state
//! loading, sparse Document proof, pure reduction, Relay signing, normalized
//! Context/provenance persistence, and projection-pointer publication.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::{
    GeneratedRoleContinuityIds, ProposalStatus, ProposalType, RoleAssignment,
    RoleAssignmentProposal, RoleContinuityChange, RoleContinuityEntity, RoleContinuityError,
    RoleLevel, WorkResponsibility,
};
use buzz_project_view::v3::{
    DocumentCoordinate, DocumentReferenceMode, DocumentTargetState, ProjectContextReference,
    ProjectObjectCommandV3, ProjectObjectOutcomeV3, ProjectViewEntryV3, ProjectViewInitializeV3,
    ProjectViewInitializeV3Request, ProjectViewObjectDataV3, ProjectViewObjectV3,
    ProjectViewStateV3, ProjectViewTombstoneV3, ProjectedHeadV3, ProjectionPlanV3,
    ReferenceTargetProof, RoleCommandV3, RoleDefinitionV3, V3ContractError, V3ProjectObjectError,
    V3ReducerCapabilities,
};
use buzz_project_view::{
    ObjectRef, ProjectRole, ProjectViewObjectType, ProjectViewRelations, WorkStatus,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::project_view::PreparedObjectProjection;
use crate::project_view_v2::{
    PreparedV2EntityProjection, ProjectViewV2Receipt, V2CanonicalCounts, V2MembershipEntry,
};
use crate::{Db, DbError};

/// Stable failures from a Project View v3 write transaction.
#[derive(Debug, thiserror::Error)]
pub enum ProjectViewV3WriteError {
    /// Database abstraction failure.
    #[error(transparent)]
    Database(#[from] DbError),
    /// Direct SQL failure.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Shared v2/v3 continuity persistence failure.
    #[error(transparent)]
    ContinuityStorage(#[from] crate::project_view_v2::ProjectViewV2WriteError),
    /// Pure v3 ordinary-object failure.
    #[error(transparent)]
    ObjectDomain(#[from] V3ProjectObjectError),
    /// Closed greenfield/bootstrap contract failure.
    #[error(transparent)]
    Contract(#[from] V3ContractError),
    /// Pure continuity failure.
    #[error(transparent)]
    RoleDomain(#[from] RoleContinuityError),
    /// Managed Runtime fence failure.
    #[error(transparent)]
    RuntimeSupervision(#[from] crate::project_runtime::RuntimeSupervisionError),
    /// Community is not an advertised writable v3 Project View.
    #[error("Project View v3 is unavailable for community {community_id}")]
    Unavailable {
        /// Unavailable Community.
        community_id: CommunityId,
    },
    /// Relay-signed material did not match the prepared canonical change.
    #[error("invalid prepared Project View v3 commit: {0}")]
    InvalidCommit(String),
}

/// Convenient v3 write result.
pub type ProjectViewV3WriteResult<T> = Result<T, ProjectViewV3WriteError>;

/// Operator-facing readiness for the staged Project Context capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextFeatureStatus {
    /// Community identity.
    pub community_id: CommunityId,
    /// Normalized Community host.
    pub host: String,
    /// Whether the Community is archived.
    pub archived: bool,
    /// Stored Project View schema major.
    pub project_view_schema_version: i16,
    /// Main Project View capability flag.
    pub project_view_enabled: bool,
    /// Project Context sub-capability flag.
    pub context_enabled: bool,
    /// Project Document capability flag.
    pub document_enabled: bool,
    /// Durable maintenance state.
    pub maintenance_state: String,
    /// Current Project revision when initialized.
    pub project_revision: Option<u64>,
    /// Current Project View projection generation.
    pub projection_generation: Option<u64>,
    /// Stable projection signer.
    pub projection_pubkey: Option<PublicKey>,
    /// Current Document catalog revision.
    pub document_catalog_revision: Option<u64>,
    /// Number of normalized Resource Context coordinates.
    pub resource_reference_count: u64,
    /// Number of normalized Document Context coordinates.
    pub document_reference_count: u64,
    /// Project View v3 structure, projections, and normalized parity are valid.
    pub project_view_ready: bool,
    /// Enabled Document catalog and projection parity are valid.
    pub document_ready: bool,
    /// Exact state currently eligible for NIP-11 advertisement.
    pub advertised_ready: bool,
}

/// One canonical v3 head awaiting Relay signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedV3ProjectObjectHead {
    /// Unified active RoleDefinitionV3 entity head.
    Role(RoleDefinitionV3),
    /// Ordinary active object or tombstone, including Role tombstones.
    Object {
        /// Complete canonical entry.
        entry: ProjectViewEntryV3,
        /// Stable responsible Role for active Work.
        responsible_role_id: Option<Uuid>,
    },
}

impl PreparedV3ProjectObjectHead {
    /// Stable object identity represented by the head.
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        match self {
            Self::Role(role) => role.role_id,
            Self::Object { entry, .. } => entry.id(),
        }
    }

    /// Stable Work responsibility carried outside the closed business body.
    #[must_use]
    pub const fn responsible_role_id(&self) -> Option<Uuid> {
        match self {
            Self::Role(_) => None,
            Self::Object {
                responsible_role_id,
                ..
            } => *responsible_role_id,
        }
    }
}

/// Canonical state returned to the Relay for one ordinary-object write.
#[derive(Debug, Clone)]
pub struct PreparedV3ProjectObjectChange {
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// Allocated Project revision.
    pub project_revision: u64,
    /// Existing active projection generation.
    pub projection_generation: u64,
    /// Expected stable Relay signer.
    pub projection_pubkey: PublicKey,
    /// Relay canonical change time.
    pub canonical_time: DateTime<Utc>,
    /// One head per changed object identity.
    pub heads: Vec<PreparedV3ProjectObjectHead>,
    /// Continuity heads ended by a terminal Work transition.
    pub entity_changes: Vec<RoleContinuityChange>,
    /// Complete post-change metadata counts.
    pub counts: V2CanonicalCounts,
    /// Exact current NIP-43 snapshot pointer.
    pub membership_snapshot_event_id: EventId,
    /// Stable successful response.
    pub receipt_result: Value,
}

/// Signed material completing one staged v3 object transaction.
#[derive(Debug, Clone)]
pub struct PreparedV3ProjectObjectCommit {
    /// Original accepted command.
    pub command_event: Event,
    /// One signed head per changed object.
    pub object_projections: Vec<PreparedObjectProjection>,
    /// Commitment heads ended by a terminal Work transition.
    pub entity_projections: Vec<PreparedV2EntityProjection>,
    /// Signed schema-v3 metadata head.
    pub meta_projection: Event,
}

/// Canonical state returned to the Relay for a continuity-only v3 command.
#[derive(Debug, Clone)]
pub struct PreparedV3RoleChange {
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// Allocated Project revision.
    pub project_revision: u64,
    /// Existing active projection generation.
    pub projection_generation: u64,
    /// Expected stable Relay signer.
    pub projection_pubkey: PublicKey,
    /// Relay canonical change time.
    pub canonical_time: DateTime<Utc>,
    /// Changed continuity entity heads.
    pub changes: Vec<RoleContinuityChange>,
    /// Work heads whose stable responsibility changed.
    pub work_heads: Vec<PreparedV3ProjectObjectHead>,
    /// Complete post-change metadata counts.
    pub counts: V2CanonicalCounts,
    /// Membership before the staged transition.
    pub membership_before: Vec<V2MembershipEntry>,
    /// Membership after the staged transition.
    pub membership_after: Vec<V2MembershipEntry>,
    /// Existing snapshot pointer, absent only during bootstrap/cutover.
    pub membership_snapshot_event_id: Option<EventId>,
    /// Stable successful response.
    pub receipt_result: Value,
}

impl PreparedV3RoleChange {
    /// Whether a replacement NIP-43 snapshot must be signed.
    #[must_use]
    pub fn membership_changed(&self) -> bool {
        self.membership_before != self.membership_after
            || self.membership_snapshot_event_id.is_none()
    }
}

/// Signed material completing one staged continuity-only v3 command.
#[derive(Debug, Clone)]
pub struct PreparedV3RoleCommit {
    /// Original accepted command.
    pub command_event: Event,
    /// One signed head per changed continuity entity.
    pub entity_projections: Vec<PreparedV2EntityProjection>,
    /// One signed head per changed Work responsibility.
    pub object_projections: Vec<PreparedObjectProjection>,
    /// Signed schema-v3 metadata head.
    pub meta_projection: Event,
    /// Replacement NIP-43 snapshot only when membership changed.
    pub membership_projection: Option<Event>,
}

/// Successful v3 commit result.
#[derive(Debug, Clone)]
pub struct ProjectViewV3CommitOutcome {
    /// Durable idempotency receipt.
    pub receipt: ProjectViewV2Receipt,
    /// Newly stored events in dispatch order.
    pub events: Vec<Event>,
}

/// Atomic result of the owner-signed prepared schema-v3 bootstrap path.
#[derive(Debug, Clone)]
pub struct ProjectViewV3InitializeOutcome {
    /// First canonical Project revision.
    pub project_revision: u64,
    /// First projection generation.
    pub projection_generation: u64,
    /// Durable response body.
    pub result: Value,
    /// Newly stored command and Relay projections; empty for replay.
    pub events: Vec<Event>,
    /// Whether the exact accepted command event was replayed.
    pub replayed: bool,
}

/// Preparation either finds an existing receipt or stages a new transition.
#[derive(Debug, Clone)]
pub enum ProjectViewV3ProjectObjectPrepareOutcome {
    /// Exact accepted event replay.
    Replayed(ProjectViewV2Receipt),
    /// New staged canonical change.
    Prepared(PreparedV3ProjectObjectChange),
}

/// Continuity preparation replay/new result.
#[derive(Debug, Clone)]
pub enum ProjectViewV3PrepareOutcome {
    /// Exact accepted event replay.
    Replayed(ProjectViewV2Receipt),
    /// New staged continuity transition.
    Prepared(PreparedV3RoleChange),
}

/// Caller-owned v3 transaction holding the exclusive Community lock.
pub struct ProjectViewV3WriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    object_basis: Option<V3PreparedProjectObjectBasis>,
    role_basis: Option<V3PreparedRoleBasis>,
}

#[derive(Debug, Clone)]
struct V3PreparedProjectObjectBasis {
    command: ProjectObjectCommandV3,
    command_event_id: [u8; 32],
    actor: PublicKey,
    preparation: PreparedV3ProjectObjectChange,
    outcome: ProjectObjectOutcomeV3,
    role_levels: BTreeMap<Uuid, RoleLevel>,
    old_meta_projection_id: [u8; 32],
    old_projection_ids: BTreeMap<Uuid, [u8; 32]>,
    old_entity_projection_ids: BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>,
}

#[derive(Debug, Clone)]
struct V3PreparedRoleBasis {
    command: RoleCommandV3,
    command_event_id: [u8; 32],
    actor: PublicKey,
    preparation: PreparedV3RoleChange,
    old_meta_projection_id: [u8; 32],
    old_projection_ids: BTreeMap<(RoleContinuityEntity, Uuid), [u8; 32]>,
    old_object_projection_ids: BTreeMap<Uuid, [u8; 32]>,
}

impl std::fmt::Debug for ProjectViewV3WriteTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectViewV3WriteTx")
            .field("community_id", &self.community_id)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Validate schema-v3 canonical and projection structure without requiring
    /// operational enablement or a normal maintenance pointer.
    pub async fn project_view_v3_structural_ready(
        &self,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        let mut tx = self.pool.begin().await?;
        let ready =
            Self::project_view_v3_structural_ready_in_tx(&mut tx, community_id, relay_pubkey)
                .await?;
        tx.rollback().await?;
        Ok(ready)
    }

    /// Validate schema-v3 readiness inside a caller-owned Community lock.
    pub(crate) async fn project_view_v3_structural_ready_in_tx(
        tx: &mut Transaction<'_, Postgres>,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        let relay = relay_pubkey.to_bytes();
        let ready: Option<bool> = sqlx::query_scalar(
            "SELECT c.archived_at IS NULL \
                    AND c.project_view_schema_version = 3 \
                    AND s.schema_version = 3 \
                    AND s.projection_pubkey = $2 \
                    AND s.membership_snapshot_event_id IS NOT NULL \
                    AND s.active_object_count = ( \
                        SELECT count(*)::integer FROM project_view_objects object \
                        WHERE object.community_id = c.id AND object.deleted_at IS NULL \
                    ) \
                    AND s.open_proposal_count = ( \
                        SELECT count(*)::integer FROM project_role_assignment_proposals proposal \
                        WHERE proposal.community_id = c.id AND proposal.status = 'open' \
                    ) \
                    AND NOT EXISTS ( \
                        SELECT 1 \
                        FROM project_role_assignment_proposals proposal \
                        LEFT JOIN project_view_objects role \
                          ON role.community_id = proposal.community_id \
                         AND role.object_id = proposal.role_id \
                        WHERE proposal.community_id = c.id \
                          AND proposal.status = 'open' \
                          AND (role.object_id IS NULL \
                               OR role.object_type <> 'role' \
                               OR role.schema_version <> 3 \
                               OR role.deleted_at IS NOT NULL \
                               OR role.body->'active' IS DISTINCT FROM 'true'::jsonb) \
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
                    AND EXISTS ( \
                        SELECT 1 FROM events meta \
                        WHERE meta.community_id = c.id \
                          AND meta.id = s.meta_projection_event_id \
                          AND meta.kind = $3 AND meta.pubkey = $2 \
                          AND meta.deleted_at IS NULL \
                          AND meta.content::jsonb->>'schema_version' = '3' \
                          AND (meta.content::jsonb->>'project_revision')::bigint = s.project_revision \
                          AND (meta.content::jsonb->>'projection_generation')::bigint = s.projection_generation \
                    ) \
                    AND EXISTS ( \
                        SELECT 1 FROM events membership \
                        WHERE membership.community_id = c.id \
                          AND membership.id = s.membership_snapshot_event_id \
                          AND membership.kind = $4 AND membership.pubkey = $2 \
                          AND membership.deleted_at IS NULL \
                    ) \
                    AND NOT EXISTS ( \
                        SELECT 1 FROM project_view_objects object \
                        WHERE object.community_id = c.id \
                          AND (object.schema_version <> 3 OR object.source_provenance_id IS NULL) \
                    ) \
                    AND NOT EXISTS ( \
                        SELECT 1 \
                        FROM ( \
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
                                 AND role.object_type = 'role' \
                                 AND role.deleted_at IS NULL \
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
                                 AND role.object_type = 'role' \
                                 AND role.deleted_at IS NULL \
                                WHERE handoff.community_id = $1 \
                            ) current_handoff WHERE history_rank <= 3 \
                        ) head \
                        LEFT JOIN events projection \
                          ON projection.community_id = c.id \
                         AND projection.id = head.projection_event_id \
                         AND projection.kind = $5 \
                         AND projection.pubkey = $2 \
                         AND projection.deleted_at IS NULL \
                        WHERE head.projection_event_id IS NULL \
                           OR projection.id IS NULL \
                           OR projection.content::jsonb->>'schema_version' IS DISTINCT FROM '3' \
                           OR (projection.content::jsonb->>'projection_generation')::bigint \
                              IS DISTINCT FROM s.projection_generation \
                    ) \
             FROM communities c \
             JOIN project_view_state s ON s.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(relay.as_slice())
        .bind(kind_i32(KIND_PROJECT_VIEW_META)?)
        .bind(kind_i32(KIND_NIP43_MEMBERSHIP_LIST)?)
        .bind(kind_i32(KIND_PROJECT_VIEW_OBJECT)?)
        .fetch_optional(&mut **tx)
        .await?;
        if ready != Some(true) {
            return Ok(false);
        }
        sqlx::query("SELECT project_view_v3_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut **tx)
            .await?;
        sqlx::query("SELECT project_role_open_proposal_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut **tx)
            .await?;
        Ok(true)
    }

    /// Structural readiness plus the deployed v3 implementation seam.
    pub async fn project_view_v3_pre_enable_ready(
        &self,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        self.project_view_v3_structural_ready(community_id, relay_pubkey)
            .await
    }

    /// Read/write readiness advertised to ordinary members and Agents.
    pub async fn project_view_v3_advertised_write_ready(
        &self,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        if !self
            .project_view_v3_pre_enable_ready(community_id, relay_pubkey)
            .await?
        {
            return Ok(false);
        }
        let operational: Option<bool> = sqlx::query_scalar(
            "SELECT c.project_view_enabled AND maintenance.state = 'normal' \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        Ok(operational == Some(true))
    }

    /// Read the complete staged Context capability status and independently
    /// recompute Project View and Document readiness.
    pub async fn project_context_feature_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<Option<ProjectContextFeatureStatus>> {
        let row = sqlx::query(
            "SELECT community.host, community.archived_at IS NOT NULL AS archived, \
                    community.project_view_schema_version, community.project_view_enabled, \
                    community.project_context_enabled, community.project_document_enabled, \
                    maintenance.state AS maintenance_state, view_state.project_revision, \
                    view_state.projection_generation, view_state.projection_pubkey, \
                    document_state.catalog_revision AS document_catalog_revision, \
                    (SELECT count(*)::bigint FROM project_view_resource_context_references \
                     WHERE community_id = community.id) AS resource_reference_count, \
                    (SELECT count(*)::bigint FROM project_view_document_context_references \
                     WHERE community_id = community.id) AS document_reference_count \
             FROM communities community \
             LEFT JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = community.id \
             LEFT JOIN project_view_state view_state ON view_state.community_id = community.id \
             LEFT JOIN project_document_state document_state \
               ON document_state.community_id = community.id \
             WHERE community.id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let projection_pubkey_bytes: Option<Vec<u8>> = row.try_get("projection_pubkey")?;
        let projection_pubkey = projection_pubkey_bytes
            .as_deref()
            .map(|bytes| {
                PublicKey::from_slice(bytes).map_err(|error| {
                    DbError::InvalidData(format!(
                        "stored Project View projection_pubkey is invalid: {error}"
                    ))
                })
            })
            .transpose()?;
        let project_view_ready = if let Some(relay_pubkey) = projection_pubkey {
            self.project_view_v3_structural_ready(community_id, &relay_pubkey)
                .await?
        } else {
            false
        };
        let document_enabled: bool = row.try_get("project_document_enabled")?;
        let document_ready = if document_enabled {
            if let Some(relay_pubkey) = projection_pubkey {
                self.project_document_preflight(community_id, &relay_pubkey)
                    .await?
                    .ready
            } else {
                false
            }
        } else {
            false
        };
        let archived: bool = row.try_get("archived")?;
        let project_view_schema_version: i16 = row.try_get("project_view_schema_version")?;
        let project_view_enabled: bool = row.try_get("project_view_enabled")?;
        let context_enabled: bool = row.try_get("project_context_enabled")?;
        let maintenance_state: Option<String> = row.try_get("maintenance_state")?;
        let maintenance_state = maintenance_state.unwrap_or_else(|| "missing".to_owned());
        let advertised_ready = !archived
            && project_view_schema_version == 3
            && project_view_enabled
            && context_enabled
            && maintenance_state == "normal"
            && project_view_ready
            && document_ready;
        Ok(Some(ProjectContextFeatureStatus {
            community_id,
            host: row.try_get("host")?,
            archived,
            project_view_schema_version,
            project_view_enabled,
            context_enabled,
            document_enabled,
            maintenance_state,
            project_revision: optional_db_u64(
                row.try_get("project_revision")?,
                "project_revision",
            )?,
            projection_generation: optional_db_u64(
                row.try_get("projection_generation")?,
                "projection_generation",
            )?,
            projection_pubkey,
            document_catalog_revision: optional_db_u64(
                row.try_get("document_catalog_revision")?,
                "document_catalog_revision",
            )?,
            resource_reference_count: db_count_u64(
                row.try_get("resource_reference_count")?,
                "resource_reference_count",
            )?,
            document_reference_count: db_count_u64(
                row.try_get("document_reference_count")?,
                "document_reference_count",
            )?,
            project_view_ready,
            document_ready,
            advertised_ready,
        }))
    }

    /// Independently advertised non-empty Context write readiness.
    pub async fn project_context_v1_advertised_ready(
        &self,
        community_id: CommunityId,
        relay_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        let status = self.project_context_feature_status(community_id).await?;
        Ok(status.is_some_and(|status| {
            status.projection_pubkey == Some(*relay_pubkey) && status.advertised_ready
        }))
    }

    /// Atomically initialize one explicitly prepared, disabled schema-v3
    /// Community from an owner-signed command. Current owner/ban/archive
    /// authorization is checked before exact event replay.
    #[allow(clippy::too_many_lines)]
    pub async fn initialize_project_view_v3(
        &self,
        community_id: CommunityId,
        command_event: &Event,
        command: &ProjectViewInitializeV3,
        relay_keys: &Keys,
    ) -> ProjectViewV3WriteResult<ProjectViewV3InitializeOutcome> {
        command.validate()?;
        if command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION
            || ProjectViewInitializeV3::from_json(&command_event.content)? != *command
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "command event does not carry the supplied ProjectViewInitializeV3".to_owned(),
            ));
        }
        command_event.verify().map_err(|error| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "initialize command signature is invalid: {error}"
            ))
        })?;
        let command_tags = command_event
            .tags
            .iter()
            .map(Tag::as_slice)
            .collect::<Vec<_>>();
        if command_tags
            != [
                vec!["-".to_owned()],
                vec!["t".to_owned(), "buzz-project-view-mutation".to_owned()],
            ]
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize command tags are not the exact protected v3 shape".to_owned(),
            ));
        }

        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_current_initialize_owner_in_tx(&mut tx, community_id, command_event.pubkey).await?;

        if let Some(row) = sqlx::query(
            "SELECT change.project_revision, change.result, state.projection_generation \
             FROM project_view_changes change \
             JOIN project_view_state state ON state.community_id = change.community_id \
             WHERE change.community_id = $1 AND change.change_id = $2 \
               AND change.source_type = 'nostr_event' \
               AND change.source_event_id = $2 AND change.operation = 'initialize_v3'",
        )
        .bind(community_id.as_uuid())
        .bind(command_event.id.as_bytes())
        .fetch_optional(&mut *tx)
        .await?
        {
            let result = row.try_get("result")?;
            let project_revision =
                revision_u64(row.try_get("project_revision")?, "project_revision")?;
            let projection_generation = revision_u64(
                row.try_get("projection_generation")?,
                "projection_generation",
            )?;
            tx.rollback().await?;
            return Ok(ProjectViewV3InitializeOutcome {
                project_revision,
                projection_generation,
                result,
                events: Vec::new(),
                replayed: true,
            });
        }

        let ProjectViewInitializeV3Request::Initialize {
            preparation_operation_id,
            profile,
            goals,
            initial_roles,
            initial_governance_assignments,
        } = &command.request;
        let prepared: Option<bool> = sqlx::query_scalar(
            "SELECT community.project_view_schema_version = 3 \
                    AND NOT community.project_view_enabled \
                    AND NOT community.project_context_enabled \
                    AND community.archived_at IS NULL \
                    AND community.project_view_preparation_operation_id = $2 \
                    AND maintenance.state = 'normal' \
                    AND preparation.target_schema_version = 3 \
                    AND preparation.operation = 'prepare_v3' \
                    AND preparation.consumed_by_change_id IS NULL \
                    AND NOT EXISTS (SELECT 1 FROM project_view_state WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_objects WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_mutations WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_view_changes WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_role_assignment_proposals WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_role_assignments WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_work_commitments WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_role_checkpoints WHERE community_id = $1) \
                    AND NOT EXISTS (SELECT 1 FROM project_role_handoffs WHERE community_id = $1) \
             FROM communities community \
             JOIN project_view_maintenance maintenance \
               ON maintenance.community_id = community.id \
             JOIN project_view_provisioning_operations preparation \
               ON preparation.community_id = community.id \
              AND preparation.operation_id = $2 \
             WHERE community.id = $1 \
             FOR UPDATE OF community, maintenance, preparation",
        )
        .bind(community_id.as_uuid())
        .bind(preparation_operation_id)
        .fetch_optional(&mut *tx)
        .await?;
        if prepared != Some(true) {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize requires an exact unconsumed prepare-v3 receipt and empty state"
                    .to_owned(),
            ));
        }

        let governors = load_initialize_governors_in_tx(&mut tx, community_id).await?;
        if governors.owner != command_event.pubkey {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "ProjectViewInitializeV3 must be signed by the current direct Human owner"
                    .to_owned(),
            ));
        }
        let supplied_governors = initial_governance_assignments
            .iter()
            .map(|assignment| assignment.member_pubkey)
            .collect::<BTreeSet<_>>();
        if supplied_governors != governors.members {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initial governance assignments must map the exact current Human owner/admin set"
                    .to_owned(),
            ));
        }

        let canonical_time: DateTime<Utc> = sqlx::query_scalar(
            "SELECT GREATEST( \
                 clock_timestamp(), \
                 COALESCE(( \
                     SELECT max(created_at) + interval '1 second' FROM events \
                     WHERE community_id = $1 AND kind = $2 AND deleted_at IS NULL \
                 ), '-infinity'::timestamptz) \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(kind_i32(KIND_NIP43_MEMBERSHIP_LIST)?)
        .fetch_one(&mut *tx)
        .await?;
        let project_revision = 1_u64;
        let projection_generation = 1_u64;
        let actor = command_event.pubkey;
        let mut entries = Vec::with_capacity(1 + goals.len() + initial_roles.len());
        entries.push(ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
            id: *community_id.as_uuid(),
            object_type: ProjectViewObjectType::ProjectProfile,
            object_revision: 1,
            project_revision,
            created_at: canonical_time,
            updated_at: canonical_time,
            created_by: actor,
            updated_by: actor,
            data: ProjectViewObjectDataV3::ProjectProfile(profile.clone()),
            relations: ProjectViewRelations::default(),
            context_references: Vec::new(),
        })));
        for goal in goals {
            entries.push(ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                id: goal.id,
                object_type: ProjectViewObjectType::Goal,
                object_revision: 1,
                project_revision,
                created_at: canonical_time,
                updated_at: canonical_time,
                created_by: actor,
                updated_by: actor,
                data: ProjectViewObjectDataV3::Goal(goal.clone().into_goal()),
                relations: ProjectViewRelations::default(),
                context_references: Vec::new(),
            })));
        }
        for role in initial_roles {
            entries.push(ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                id: role.role_id,
                object_type: ProjectViewObjectType::Role,
                object_revision: 1,
                project_revision,
                created_at: canonical_time,
                updated_at: canonical_time,
                created_by: actor,
                updated_by: actor,
                data: ProjectViewObjectDataV3::Role(ProjectRole {
                    name: role.name.clone(),
                    purpose: role.purpose.clone(),
                    responsibilities: role.responsibilities.clone(),
                    boundaries: role.boundaries.clone(),
                    active: role.active,
                }),
                relations: ProjectViewRelations::default(),
                context_references: role.context_references.clone(),
            })));
        }
        entries.sort_by_key(ProjectViewEntryV3::id);
        for entry in &entries {
            if let ProjectViewEntryV3::Active(object) = entry {
                buzz_project_view::v3::validate_projected_object_v3(object)?;
            }
        }

        let mut continuity = Vec::with_capacity(initial_governance_assignments.len() * 2);
        for assignment in initial_governance_assignments {
            continuity.push(RoleContinuityChange::Proposal(RoleAssignmentProposal {
                proposal_id: assignment.proposal_id,
                role_id: assignment.role_id,
                candidate_pubkey: assignment.member_pubkey,
                proposal_type: ProposalType::Offer,
                candidate_accepted_at: Some(canonical_time),
                authorized_by: Some(actor),
                authorized_at: Some(canonical_time),
                expected_target_assignment_id: None,
                expected_candidate_assignment_id: None,
                expires_at: canonical_time + chrono::Duration::days(1),
                status: ProposalStatus::Consumed,
                reason: Some("greenfield_v3_initialize".to_owned()),
                created_by: actor,
                created_at: canonical_time,
                resolved_at: Some(canonical_time),
                entity_revision: 1,
                project_revision,
            }));
            continuity.push(RoleContinuityChange::Assignment(RoleAssignment {
                assignment_id: assignment.assignment_id,
                role_id: assignment.role_id,
                member_pubkey: assignment.member_pubkey,
                proposal_id: assignment.proposal_id,
                started_at: canonical_time,
                started_by: actor,
                replacement_requested_at: None,
                replacement_request_reason: None,
                unable_reported_at: None,
                unable_report_reason: None,
                ended_at: None,
                ended_by: None,
                ended_reason: None,
                replaced_by_assignment_id: None,
                entity_revision: 1,
                project_revision,
            }));
        }
        continuity.sort_by_key(|change| (change.entity_type(), change.entity_id()));

        let source = buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
            change_id: command_event.id,
            event_id: command_event.id,
        };
        let context = buzz_sdk::project_view_v3::V3ProjectionContext {
            project_id: community_id,
            projection_generation,
            project_revision,
            source,
            updated_at: canonical_time,
        };
        let membership = crate::project_view_v2::load_membership(&mut tx, community_id).await?;
        let membership_event = build_v3_membership_event(&membership, canonical_time, relay_keys)?;
        let mut object_events = BTreeMap::new();
        for entry in &entries {
            let builder = match entry {
                ProjectViewEntryV3::Active(object)
                    if object.object_type == ProjectViewObjectType::Role =>
                {
                    let role = object.role_definition(RoleLevel::Admin)?;
                    buzz_sdk::project_view_v3::build_entity_projection(
                        &context,
                        &buzz_sdk::project_view_v3::V3EntityChange::Role(role),
                    )
                }
                ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => {
                    buzz_sdk::project_view_v3::build_project_object_projection(
                        &context, entry, None,
                    )
                }
            }
            .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
            let event = builder.sign_with_keys(relay_keys).map_err(|error| {
                ProjectViewV3WriteError::InvalidCommit(format!(
                    "sign initialize object projection: {error}"
                ))
            })?;
            object_events.insert(entry.id(), event);
        }
        let mut continuity_events = BTreeMap::new();
        for change in &continuity {
            let entity = v3_entity_change(change)?;
            let event = buzz_sdk::project_view_v3::build_entity_projection(&context, &entity)
                .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?
                .sign_with_keys(relay_keys)
                .map_err(|error| {
                    ProjectViewV3WriteError::InvalidCommit(format!(
                        "sign initialize continuity projection: {error}"
                    ))
                })?;
            continuity_events.insert((change.entity_type(), change.entity_id()), event);
        }
        let counts = crate::project_view_v2::V2CanonicalCounts {
            active_objects: u32::try_from(entries.len()).map_err(|_| {
                ProjectViewV3WriteError::InvalidCommit(
                    "initial active object count exceeds u32".to_owned(),
                )
            })?,
            open_proposals: 0,
            active_assignments: u32::try_from(initial_governance_assignments.len()).map_err(
                |_| {
                    ProjectViewV3WriteError::InvalidCommit(
                        "initial Assignment count exceeds u32".to_owned(),
                    )
                },
            )?,
            active_commitments: 0,
            checkpoints: 0,
            handoffs: 0,
        };
        let meta_event = buzz_sdk::project_view_v3::build_meta_projection(
            &context,
            buzz_sdk::project_view_v3::V3EntityCounts {
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
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "sign initialize metadata projection: {error}"
            ))
        })?;

        let result = json!({
            "schema_version": 3,
            "operation": "initialize_v3",
            "preparation_operation_id": preparation_operation_id,
            "project_revision": project_revision,
            "projection_generation": projection_generation,
            "object_ids": entries.iter().map(ProjectViewEntryV3::id).collect::<Vec<_>>(),
            "governance_assignments": initial_governance_assignments,
        });
        let actor_bytes = actor.to_bytes();
        let relay_bytes = relay_keys.public_key().to_bytes();
        sqlx::query(
            "INSERT INTO project_view_state \
                (community_id, project_revision, active_object_count, initialized_at, \
                 updated_at, last_event_id, last_actor_pubkey, meta_projection_event_id, \
                 projection_pubkey, projection_generation, schema_version, last_change_id, \
                 last_source_event_id, open_proposal_count, active_assignment_count, \
                 active_commitment_count, checkpoint_count, handoff_count, \
                 membership_snapshot_event_id) \
             VALUES ($1,1,$2,$3,$3,$4,$5,$6,$7,1,3,$4,$4,0,$8,0,0,0,$9)",
        )
        .bind(community_id.as_uuid())
        // `project_view_objects_adjust_active_count` advances this baseline as
        // the initial Profile, Goals, and Roles are inserted below.
        .bind(0_i32)
        .bind(canonical_time)
        .bind(command_event.id.as_bytes())
        .bind(actor_bytes.as_slice())
        .bind(meta_event.id.as_bytes())
        .bind(relay_bytes.as_slice())
        .bind(count_i32(
            counts.active_assignments,
            "active_assignment_count",
        )?)
        .bind(membership_event.id.as_bytes())
        .execute(&mut *tx)
        .await?;
        let subject = serde_json::to_value(&command.request).map_err(DbError::from)?;
        sqlx::query(
            "INSERT INTO project_view_changes \
                (community_id, change_id, source_type, source_event_id, actor_pubkey, \
                 acting_assignment_id, operation, subject, project_revision, result, accepted_at) \
             VALUES ($1,$2,'nostr_event',$2,$3,NULL,'initialize_v3',$4,1,$5,$6)",
        )
        .bind(community_id.as_uuid())
        .bind(command_event.id.as_bytes())
        .bind(actor_bytes.as_slice())
        .bind(subject)
        .bind(&result)
        .bind(canonical_time)
        .execute(&mut *tx)
        .await?;
        let (_, command_inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, command_event, None).await?;
        if !command_inserted {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize command event exists without its typed receipt".to_owned(),
            ));
        }
        crate::project_view_v2::retire_membership_heads(
            &mut tx,
            community_id,
            relay_keys.public_key(),
        )
        .await?;
        let (_, membership_inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, &membership_event, None)
                .await?;
        if !membership_inserted {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize membership projection already exists".to_owned(),
            ));
        }

        let mut events = vec![command_event.clone(), membership_event];
        for entry in &entries {
            let projection = object_events.get(&entry.id()).ok_or_else(|| {
                ProjectViewV3WriteError::InvalidCommit(
                    "initialize object projection set is incomplete".to_owned(),
                )
            })?;
            let role_level =
                (entry.object_type() == ProjectViewObjectType::Role).then_some(RoleLevel::Admin);
            write_v3_entry(
                &mut tx,
                community_id,
                &command_event.id.to_bytes(),
                projection.id.as_bytes(),
                actor,
                entry,
                role_level,
                None,
            )
            .await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut tx, community_id, projection, None).await?;
            if !inserted {
                return Err(ProjectViewV3WriteError::InvalidCommit(
                    "initialize object projection already exists".to_owned(),
                ));
            }
            events.push(projection.clone());
        }
        crate::project_view_v2::persist_changes(
            &mut tx,
            community_id,
            command_event.id.as_bytes(),
            canonical_time,
            &continuity,
        )
        .await?;
        for change in &continuity {
            let key = (change.entity_type(), change.entity_id());
            let projection = continuity_events.get(&key).ok_or_else(|| {
                ProjectViewV3WriteError::InvalidCommit(
                    "initialize continuity projection set is incomplete".to_owned(),
                )
            })?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut tx, community_id, projection, None).await?;
            if !inserted {
                return Err(ProjectViewV3WriteError::InvalidCommit(
                    "initialize continuity projection already exists".to_owned(),
                ));
            }
            crate::project_view_v2::update_projection_pointer(
                &mut tx,
                community_id,
                change.entity_type(),
                change.entity_id(),
                projection.id.as_bytes(),
            )
            .await?;
            events.push(projection.clone());
        }
        let (_, meta_inserted) =
            crate::event::insert_event_in_tx(&mut tx, community_id, &meta_event, None).await?;
        if !meta_inserted {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize metadata projection already exists".to_owned(),
            ));
        }
        let consumed = sqlx::query(
            "UPDATE project_view_provisioning_operations SET \
                 consumed_by_change_id = $3, consumed_at = $4 \
             WHERE community_id = $1 AND operation_id = $2 \
               AND consumed_by_change_id IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(preparation_operation_id)
        .bind(command_event.id.as_bytes())
        .bind(canonical_time)
        .execute(&mut *tx)
        .await?;
        if consumed.rows_affected() != 1 {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize did not consume exactly one prepare-v3 receipt".to_owned(),
            ));
        }
        let cleared = sqlx::query(
            "UPDATE communities SET project_view_preparation_operation_id = NULL \
             WHERE id = $1 AND project_view_preparation_operation_id = $2 \
               AND project_view_schema_version = 3 AND NOT project_view_enabled",
        )
        .bind(community_id.as_uuid())
        .bind(preparation_operation_id)
        .execute(&mut *tx)
        .await?;
        if cleared.rows_affected() != 1 {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "initialize did not clear its exact prepare-v3 pointer".to_owned(),
            ));
        }
        assert_counts_in_tx(&mut tx, community_id, counts).await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        events.push(meta_event);
        Ok(ProjectViewV3InitializeOutcome {
            project_revision,
            projection_generation,
            result,
            events,
            replayed: false,
        })
    }

    /// Begin one ordinary v3 write under the shared Community coordination
    /// lock and the durable maintenance fence.
    pub async fn begin_project_view_v3_write(
        &self,
        community_id: CommunityId,
    ) -> ProjectViewV3WriteResult<ProjectViewV3WriteTx> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let available: Option<bool> = sqlx::query_scalar(
            "SELECT c.project_view_schema_version = 3 \
                    AND c.project_view_enabled \
                    AND c.archived_at IS NULL \
                    AND maintenance.state = 'normal' \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             WHERE c.id = $1 FOR UPDATE OF c, maintenance",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if available != Some(true) {
            return Err(ProjectViewV3WriteError::Unavailable { community_id });
        }
        Ok(ProjectViewV3WriteTx {
            tx,
            community_id,
            object_basis: None,
            role_basis: None,
        })
    }
}

impl ProjectViewV3WriteTx {
    /// Explicitly roll back the staged transaction.
    pub async fn rollback(self) -> ProjectViewV3WriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Reauthorize, replay-check, and stage one continuity-only v3 command.
    #[allow(clippy::too_many_lines)]
    pub async fn prepare_role_command(
        &mut self,
        command_event: &Event,
        command: &RoleCommandV3,
    ) -> ProjectViewV3WriteResult<ProjectViewV3PrepareOutcome> {
        if self.object_basis.is_some() || self.role_basis.is_some() {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "this transaction already has a prepared v3 change".to_owned(),
            ));
        }
        if command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION
            || RoleCommandV3::from_json(&command_event.content)? != *command
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "command event does not carry the supplied continuity-only v3 command".to_owned(),
            ));
        }
        sqlx::query("SELECT project_view_v3_validate_community($1)")
            .bind(self.community_id.as_uuid())
            .execute(&mut *self.tx)
            .await?;
        let loaded =
            crate::project_view_v2::load_continuity_state(&mut self.tx, self.community_id, 3)
                .await?;
        buzz_project_view::v3::validate_role_actor_for_v3_replay(
            &loaded.state,
            command,
            command_event.pubkey,
        )?;
        validate_v3_actor_in_tx(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command.acting_assignment_id,
            command.runtime_fence,
        )
        .await?;
        if let Some(receipt) = crate::project_view_v2::find_receipt(
            &mut self.tx,
            self.community_id,
            command_event.id.as_bytes(),
        )
        .await?
        {
            return Ok(ProjectViewV3PrepareOutcome::Replayed(receipt));
        }
        let generated_ids = GeneratedRoleContinuityIds {
            assignment_id: Uuid::new_v4(),
            handoff_ids: vec![Uuid::new_v4(), Uuid::new_v4()],
        };
        let (_next_state, outcome) = buzz_project_view::v3::reduce_role_command_v3(
            &loaded.state,
            command,
            command_event.pubkey,
            loaded.canonical_time,
            &generated_ids,
        )?;
        let (work_heads, old_object_projection_ids) = prepare_v3_work_responsibility_heads(
            &mut self.tx,
            self.community_id,
            &outcome.work_changes,
        )
        .await?;
        let receipt_result = role_receipt_v3(
            command,
            &outcome.changes,
            &outcome.work_changes,
            outcome.project_revision,
        );
        insert_role_change_v3(
            &mut self.tx,
            self.community_id,
            command_event,
            command,
            outcome.project_revision,
            loaded.canonical_time,
            &receipt_result,
        )
        .await?;
        let old_projection_ids = crate::project_view_v2::load_old_projection_ids(
            &mut self.tx,
            self.community_id,
            &outcome.changes,
        )
        .await?;
        crate::project_view_v2::persist_changes(
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
        let membership_before =
            crate::project_view_v2::load_membership(&mut self.tx, self.community_id).await?;
        crate::project_view_v2::apply_membership_roles(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            &outcome.membership_roles,
            loaded.canonical_time,
        )
        .await?;
        let membership_after =
            crate::project_view_v2::load_membership(&mut self.tx, self.community_id).await?;
        let counts = crate::project_view_v2::load_counts(&mut self.tx, self.community_id).await?;
        let preparation = PreparedV3RoleChange {
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
        self.role_basis = Some(V3PreparedRoleBasis {
            command: command.clone(),
            command_event_id: command_event.id.to_bytes(),
            actor: command_event.pubkey,
            preparation: preparation.clone(),
            old_meta_projection_id: loaded.meta_projection_event_id,
            old_projection_ids,
            old_object_projection_ids,
        });
        Ok(ProjectViewV3PrepareOutcome::Prepared(preparation))
    }

    /// Reauthorize, replay-check, sparsely prove new Document targets, and
    /// stage one pure v3 ordinary-object transition.
    #[allow(clippy::too_many_lines)]
    pub async fn prepare_project_object_command(
        &mut self,
        command_event: &Event,
        command: &ProjectObjectCommandV3,
    ) -> ProjectViewV3WriteResult<ProjectViewV3ProjectObjectPrepareOutcome> {
        if self.object_basis.is_some() || self.role_basis.is_some() {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "this transaction already has a prepared v3 change".to_owned(),
            ));
        }
        if command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION
            || ProjectObjectCommandV3::from_json(&command_event.content)? != *command
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "command event does not carry the supplied typed v3 object command".to_owned(),
            ));
        }

        let loaded = load_v3_project_object_state(&mut self.tx, self.community_id).await?;
        validate_v3_actor_in_tx(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command.acting_assignment_id,
            command.runtime_fence,
        )
        .await?;
        if let Some(receipt) = crate::project_view_v2::find_receipt(
            &mut self.tx,
            self.community_id,
            command_event.id.as_bytes(),
        )
        .await?
        {
            return Ok(ProjectViewV3ProjectObjectPrepareOutcome::Replayed(receipt));
        }

        let capabilities = V3ReducerCapabilities {
            project_context_enabled: loaded.project_context_enabled,
            document_capability_available: loaded.document_capability_available,
        };
        let delta = loaded.state.document_target_delta(command, capabilities)?;
        let proof = load_document_target_proof(
            &mut self.tx,
            self.community_id,
            &delta.required_coordinates(),
        )
        .await?;
        let (_next_state, outcome) = loaded.state.reduce(
            command,
            command_event.pubkey,
            loaded.canonical_time,
            capabilities,
            &proof,
        )?;

        let deactivated_roles = deactivated_role_ids(&outcome.changed_entries);
        crate::project_view_v2::reject_role_ids_with_active_authority(
            &mut self.tx,
            self.community_id,
            &deactivated_roles,
        )
        .await?;
        let terminal_work = terminal_work_ids(&outcome.changed_entries);
        let continuity_changes = crate::project_view_v2::close_commitments_for_work_ids(
            &mut self.tx,
            self.community_id,
            &terminal_work,
            command_event.pubkey,
            loaded.canonical_time,
            outcome.project_revision,
        )
        .await?;

        let plan = ProjectionPlanV3::for_object_outcome(&outcome, |role_id| {
            loaded.role_levels.get(&role_id).copied().or_else(|| {
                outcome
                    .changed_entries
                    .iter()
                    .any(|entry| {
                        entry.id() == role_id && entry.object_type() == ProjectViewObjectType::Role
                    })
                    .then_some(RoleLevel::Member)
            })
        })?;
        plan.validate_single_head_per_object()?;
        let mut role_levels = loaded.role_levels;
        for entry in &outcome.changed_entries {
            if entry.object_type() == ProjectViewObjectType::Role {
                role_levels.entry(entry.id()).or_insert(RoleLevel::Member);
            }
        }
        let heads = plan
            .heads
            .into_iter()
            .map(|head| match head {
                ProjectedHeadV3::Role(role) => PreparedV3ProjectObjectHead::Role(role),
                ProjectedHeadV3::Object(entry) => {
                    let responsible_role_id = match &entry {
                        ProjectViewEntryV3::Active(object)
                            if object.object_type == ProjectViewObjectType::Work =>
                        {
                            loaded.work_responsibilities.get(&object.id).copied()
                        }
                        ProjectViewEntryV3::Active(_) | ProjectViewEntryV3::Tombstone(_) => None,
                    };
                    PreparedV3ProjectObjectHead::Object {
                        entry,
                        responsible_role_id,
                    }
                }
            })
            .collect::<Vec<_>>();
        let changed_ids = outcome
            .changed_entries
            .iter()
            .map(ProjectViewEntryV3::id)
            .collect::<Vec<_>>();
        let old_projection_ids =
            load_object_projection_ids(&mut self.tx, self.community_id, &changed_ids).await?;
        let old_entity_projection_ids = crate::project_view_v2::load_old_projection_ids(
            &mut self.tx,
            self.community_id,
            &continuity_changes,
        )
        .await?;

        let receipt_result = project_object_receipt_v3(
            command,
            &outcome.changed_entries,
            &continuity_changes,
            outcome.project_revision,
        );
        insert_project_object_change_v3(
            &mut self.tx,
            self.community_id,
            command_event,
            command,
            outcome.project_revision,
            loaded.canonical_time,
            &receipt_result,
        )
        .await?;
        crate::project_view_v2::persist_changes(
            &mut self.tx,
            self.community_id,
            command_event.id.as_bytes(),
            loaded.canonical_time,
            &continuity_changes,
        )
        .await?;
        let mut counts =
            crate::project_view_v2::load_counts(&mut self.tx, self.community_id).await?;
        counts.active_objects = u32::try_from(
            loaded.state.active_objects().count() as i64
                + active_count_delta(&outcome.changed_entries),
        )
        .map_err(|_| {
            ProjectViewV3WriteError::InvalidCommit(
                "active Project View object count exceeds u32".to_owned(),
            )
        })?;
        let membership_snapshot_event_id =
            loaded.membership_snapshot_event_id.ok_or_else(|| {
                ProjectViewV3WriteError::InvalidCommit(
                    "v3 state has no membership snapshot pointer".to_owned(),
                )
            })?;
        let preparation = PreparedV3ProjectObjectChange {
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
        self.object_basis = Some(V3PreparedProjectObjectBasis {
            command: command.clone(),
            command_event_id: command_event.id.to_bytes(),
            actor: command_event.pubkey,
            preparation: preparation.clone(),
            outcome,
            role_levels,
            old_meta_projection_id: loaded.meta_projection_event_id,
            old_projection_ids,
            old_entity_projection_ids,
        });
        Ok(ProjectViewV3ProjectObjectPrepareOutcome::Prepared(
            preparation,
        ))
    }

    /// Validate and atomically publish every signed v3 head and canonical row.
    #[allow(clippy::too_many_lines)]
    pub async fn commit_project_object_command(
        mut self,
        commit: PreparedV3ProjectObjectCommit,
    ) -> ProjectViewV3WriteResult<ProjectViewV3CommitOutcome> {
        let basis = self.object_basis.take().ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
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
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "command event exists without its v3 receipt".to_owned(),
            ));
        }

        for old_event_id in basis.old_projection_ids.values() {
            retire_required_head(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
                "v3 object",
            )
            .await?;
        }
        for old_event_id in basis.old_entity_projection_ids.values() {
            retire_required_head(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
                "v3 continuity entity",
            )
            .await?;
        }
        retire_required_head(
            &mut self.tx,
            self.community_id,
            &basis.old_meta_projection_id,
            KIND_PROJECT_VIEW_META,
            "v3 metadata",
        )
        .await?;

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
                ProjectViewV3WriteError::InvalidCommit(format!(
                    "missing signed head for changed v3 object {}",
                    entry.id()
                ))
            })?;
            let role_level = basis.role_levels.get(&entry.id()).copied();
            let responsible_role_id = prepared_heads
                .get(&entry.id())
                .and_then(|head| head.responsible_role_id());
            write_v3_entry(
                &mut self.tx,
                self.community_id,
                &basis.command_event_id,
                projection.id.as_bytes(),
                basis.actor,
                entry,
                role_level,
                responsible_role_id,
            )
            .await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, projection, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                    "v3 object projection {} already exists",
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
                return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                    "v3 continuity projection {} already exists",
                    projection.entity_id
                )));
            }
            crate::project_view_v2::update_projection_pointer(
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
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "v3 metadata projection already exists".to_owned(),
            ));
        }
        let actor = basis.actor.to_bytes();
        let update = sqlx::query(
            "UPDATE project_view_state SET \
                 project_revision = $2, updated_at = $3, last_event_id = $4, \
                 last_actor_pubkey = $5, meta_projection_event_id = $6, \
                 active_object_count = $7, active_commitment_count = $8, schema_version = 3, \
                 last_change_id = $4, last_source_event_id = $4 \
             WHERE community_id = $1 AND project_revision = $9 \
               AND schema_version = 3",
        )
        .bind(self.community_id.as_uuid())
        .bind(revision_i64(
            basis.preparation.project_revision,
            "project_revision",
        )?)
        .bind(basis.preparation.canonical_time)
        .bind(basis.command_event_id.as_slice())
        .bind(actor.as_slice())
        .bind(commit.meta_projection.id.as_bytes().as_slice())
        .bind(count_i32(
            basis.preparation.counts.active_objects,
            "active_object_count",
        )?)
        .bind(count_i32(
            basis.preparation.counts.active_commitments,
            "active_commitment_count",
        )?)
        .bind(revision_i64(
            basis.command.expected_project_revision,
            "expected_project_revision",
        )?)
        .execute(&mut *self.tx)
        .await?;
        if update.rows_affected() != 1 {
            return Err(ProjectViewV3WriteError::ObjectDomain(
                buzz_project_view::DomainError::RevisionConflict {
                    expected: basis.command.expected_project_revision,
                    actual: basis.preparation.project_revision.saturating_sub(1),
                }
                .into(),
            ));
        }

        assert_counts_in_tx(&mut self.tx, self.community_id, basis.preparation.counts).await?;
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
        Ok(ProjectViewV3CommitOutcome { receipt, events })
    }

    /// Validate and atomically publish one continuity-only v3 transition.
    #[allow(clippy::too_many_lines)]
    pub async fn commit_role_command(
        mut self,
        commit: PreparedV3RoleCommit,
    ) -> ProjectViewV3WriteResult<ProjectViewV3CommitOutcome> {
        let basis = self.role_basis.take().ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
                "commit requires prepare_role_command on the same transaction".to_owned(),
            )
        })?;
        validate_role_commit_bundle(&basis, &commit)?;
        let (_, command_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &commit.command_event,
            None,
        )
        .await?;
        if !command_inserted {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "command event exists without its v3 receipt".to_owned(),
            ));
        }

        let mut events = vec![commit.command_event.clone()];
        let membership_event_id = if basis.preparation.membership_changed() {
            let event = commit.membership_projection.as_ref().ok_or_else(|| {
                ProjectViewV3WriteError::InvalidCommit(
                    "changed membership requires a signed NIP-43 snapshot".to_owned(),
                )
            })?;
            crate::project_view_v2::verify_membership_projection(
                event,
                basis.preparation.projection_pubkey,
                &basis.preparation.membership_after,
                basis.preparation.canonical_time,
            )?;
            crate::project_view_v2::retire_membership_heads(
                &mut self.tx,
                self.community_id,
                basis.preparation.projection_pubkey,
            )
            .await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, event, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV3WriteError::InvalidCommit(
                    "membership projection already exists".to_owned(),
                ));
            }
            events.push(event.clone());
            event.id
        } else {
            if commit.membership_projection.is_some() {
                return Err(ProjectViewV3WriteError::InvalidCommit(
                    "unchanged membership must reuse its existing snapshot".to_owned(),
                ));
            }
            basis
                .preparation
                .membership_snapshot_event_id
                .ok_or_else(|| {
                    ProjectViewV3WriteError::InvalidCommit(
                        "v3 state has no membership snapshot pointer".to_owned(),
                    )
                })?
        };

        for old_event_id in basis.old_projection_ids.values() {
            retire_required_head(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
                "v3 continuity entity",
            )
            .await?;
        }
        for old_event_id in basis.old_object_projection_ids.values() {
            retire_required_head(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
                "v3 Work responsibility",
            )
            .await?;
        }
        retire_required_head(
            &mut self.tx,
            self.community_id,
            &basis.old_meta_projection_id,
            KIND_PROJECT_VIEW_META,
            "v3 metadata",
        )
        .await?;

        let work_projections = commit
            .object_projections
            .iter()
            .map(|projection| (projection.object_id(), projection.event()))
            .collect::<BTreeMap<_, _>>();
        for head in &basis.preparation.work_heads {
            let PreparedV3ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } = head
            else {
                return Err(ProjectViewV3WriteError::InvalidCommit(
                    "continuity command prepared a non-Work object head".to_owned(),
                ));
            };
            let projection = work_projections.get(&entry.id()).ok_or_else(|| {
                ProjectViewV3WriteError::InvalidCommit(format!(
                    "missing signed v3 Work head {}",
                    entry.id()
                ))
            })?;
            write_v3_entry(
                &mut self.tx,
                self.community_id,
                &basis.command_event_id,
                projection.id.as_bytes(),
                basis.actor,
                entry,
                None,
                *responsible_role_id,
            )
            .await?;
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, projection, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                    "v3 Work projection {} already exists",
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
                return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                    "v3 entity projection {} already exists",
                    projection.entity_id
                )));
            }
            crate::project_view_v2::update_projection_pointer(
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
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "v3 metadata projection already exists".to_owned(),
            ));
        }

        let actor = basis.actor.to_bytes();
        let update = sqlx::query(
            "UPDATE project_view_state SET \
                 project_revision = $2, updated_at = $3, last_event_id = $4, \
                 last_actor_pubkey = $5, meta_projection_event_id = $6, \
                 schema_version = 3, last_change_id = $4, last_source_event_id = $4, \
                 open_proposal_count = $7, active_assignment_count = $8, \
                 active_commitment_count = $9, checkpoint_count = $10, \
                 handoff_count = $11, membership_snapshot_event_id = $12 \
             WHERE community_id = $1 AND project_revision = $13 \
               AND schema_version = 3",
        )
        .bind(self.community_id.as_uuid())
        .bind(revision_i64(
            basis.preparation.project_revision,
            "project_revision",
        )?)
        .bind(basis.preparation.canonical_time)
        .bind(basis.command_event_id.as_slice())
        .bind(actor.as_slice())
        .bind(commit.meta_projection.id.as_bytes().as_slice())
        .bind(count_i32(
            basis.preparation.counts.open_proposals,
            "open_proposal_count",
        )?)
        .bind(count_i32(
            basis.preparation.counts.active_assignments,
            "active_assignment_count",
        )?)
        .bind(count_i32(
            basis.preparation.counts.active_commitments,
            "active_commitment_count",
        )?)
        .bind(count_i32(
            basis.preparation.counts.checkpoints,
            "checkpoint_count",
        )?)
        .bind(count_i32(
            basis.preparation.counts.handoffs,
            "handoff_count",
        )?)
        .bind(membership_event_id.as_bytes().as_slice())
        .bind(revision_i64(
            basis.command.expected_project_revision,
            "expected_project_revision",
        )?)
        .execute(&mut *self.tx)
        .await?;
        if update.rows_affected() != 1 {
            return Err(ProjectViewV3WriteError::RoleDomain(
                RoleContinuityError::RevisionConflict {
                    expected: basis.command.expected_project_revision,
                    current: basis.preparation.project_revision.saturating_sub(1),
                },
            ));
        }
        assert_counts_in_tx(&mut self.tx, self.community_id, basis.preparation.counts).await?;
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
        Ok(ProjectViewV3CommitOutcome { receipt, events })
    }
}

#[derive(Debug)]
struct LoadedV3ProjectObjectState {
    state: ProjectViewStateV3,
    canonical_time: DateTime<Utc>,
    projection_generation: u64,
    projection_pubkey: PublicKey,
    meta_projection_event_id: [u8; 32],
    membership_snapshot_event_id: Option<EventId>,
    role_levels: BTreeMap<Uuid, RoleLevel>,
    work_responsibilities: BTreeMap<Uuid, Uuid>,
    project_context_enabled: bool,
    document_capability_available: bool,
}

async fn prepare_v3_work_responsibility_heads(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    changes: &[WorkResponsibility],
) -> ProjectViewV3WriteResult<(Vec<PreparedV3ProjectObjectHead>, BTreeMap<Uuid, [u8; 32]>)> {
    if changes.is_empty() {
        return Ok((Vec::new(), BTreeMap::new()));
    }
    let ids = changes.iter().map(|work| work.work_id).collect::<Vec<_>>();
    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, \
                updated_by, deleted_at, role_level, responsible_role_id, \
                projection_event_id \
         FROM project_view_objects \
         WHERE community_id = $1 AND schema_version = 3 AND object_id = ANY($2) \
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
        let entry = v3_entry_from_row(row)?;
        let work = by_id.get(&object_id).ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "loaded unexpected v3 responsibility Work {object_id}"
            ))
        })?;
        let ProjectViewEntryV3::Active(mut object) = entry else {
            return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                "v3 responsibility Work {object_id} is deleted"
            )));
        };
        let current_status = match &object.data {
            ProjectViewObjectDataV3::Work(value) => value.status,
            _ => {
                return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                    "v3 responsibility target {object_id} is not Work"
                )));
            }
        };
        if work.status != Some(current_status)
            || object.object_revision.checked_add(1) != Some(work.object_revision)
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                "v3 responsibility Work {object_id} disagrees with canonical state"
            )));
        }
        object.object_revision = work.object_revision;
        object.project_revision = work.project_revision;
        object.updated_at = work.updated_at;
        object.updated_by = work.updated_by;
        heads.push(PreparedV3ProjectObjectHead::Object {
            entry: ProjectViewEntryV3::Active(object),
            responsible_role_id: work.responsible_role_id,
        });
        old_projection_ids.insert(object_id, projection_event_id);
    }
    if heads.len() != changes.len() {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "one or more v3 responsibility Work rows are missing".to_owned(),
        ));
    }
    Ok((heads, old_projection_ids))
}

async fn insert_role_change_v3(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    command: &RoleCommandV3,
    project_revision: u64,
    accepted_at: DateTime<Utc>,
    result: &Value,
) -> ProjectViewV3WriteResult<()> {
    let actor = event.pubkey.to_bytes();
    let subject = serde_json::to_value(&command.request).map_err(DbError::from)?;
    sqlx::query(
        "INSERT INTO project_view_changes \
            (community_id, change_id, source_type, source_event_id, \
             source_request_hash, source_audit_seq, idempotency_key_hash, \
             actor_pubkey, acting_assignment_id, operation, subject, \
             project_revision, result, accepted_at) \
         VALUES ($1,$2,'nostr_event',$2,NULL,NULL,NULL,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes())
    .bind(actor.as_slice())
    .bind(command.acting_assignment_id)
    .bind(command.operation())
    .bind(subject)
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(result)
    .bind(accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn role_receipt_v3(
    command: &RoleCommandV3,
    changes: &[RoleContinuityChange],
    work_changes: &[WorkResponsibility],
    project_revision: u64,
) -> Value {
    json!({
        "schema_version": 3,
        "operation": command.operation(),
        "project_revision": project_revision,
        "entities": changes.iter().map(|change| json!({
            "entity_type": change.entity_type().as_str(),
            "entity_id": change.entity_id(),
            "entity_revision": change.entity_revision(),
        })).collect::<Vec<_>>(),
        "work_objects": work_changes.iter().map(|work| json!({
            "object_id": work.work_id,
            "object_revision": work.object_revision,
            "responsible_role_id": work.responsible_role_id,
        })).collect::<Vec<_>>(),
    })
}

#[allow(clippy::too_many_lines)]
fn validate_role_commit_bundle(
    basis: &V3PreparedRoleBasis,
    commit: &PreparedV3RoleCommit,
) -> ProjectViewV3WriteResult<()> {
    if commit.command_event.id.to_bytes() != basis.command_event_id
        || commit.command_event.pubkey != basis.actor
        || RoleCommandV3::from_json(&commit.command_event.content)? != basis.command
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "committed continuity command differs from the prepared v3 command".to_owned(),
        ));
    }
    commit.command_event.verify().map_err(|error| {
        ProjectViewV3WriteError::InvalidCommit(format!(
            "committed v3 command signature is invalid: {error}"
        ))
    })?;
    let tags = commit
        .command_event
        .tags
        .iter()
        .map(Tag::as_slice)
        .collect::<Vec<_>>();
    if tags
        != [
            vec!["-".to_owned()],
            vec!["t".to_owned(), "buzz-project-view-mutation".to_owned()],
        ]
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "committed command tags are not the exact protected v3 shape".to_owned(),
        ));
    }
    let expected_source = buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
        change_id: commit.command_event.id,
        event_id: commit.command_event.id,
    };
    let context = buzz_sdk::project_view_v3::V3ProjectionContext {
        project_id: basis.preparation.community_id,
        projection_generation: basis.preparation.projection_generation,
        project_revision: basis.preparation.project_revision,
        source: expected_source.clone(),
        updated_at: basis.preparation.canonical_time,
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
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "projection counts do not match prepared v3 continuity changes".to_owned(),
        ));
    }
    let mut expected_heads = BTreeMap::new();
    let mut seen_entities = BTreeSet::new();
    for projection in &commit.entity_projections {
        let key = (projection.entity_type, projection.entity_id);
        if !seen_entities.insert(key) {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "duplicate v3 continuity projection".to_owned(),
            ));
        }
        let change = expected_entities.get(&key).ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit("unexpected v3 continuity projection".to_owned())
        })?;
        let entity = v3_entity_change(change)?;
        let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
            &projection.event,
            &basis.preparation.projection_pubkey,
            basis.preparation.community_id,
        )
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if parsed.project_revision != basis.preparation.project_revision
            || parsed.projection_generation != basis.preparation.projection_generation
            || parsed.source != expected_source
            || parsed.updated_at != basis.preparation.canonical_time
            || parsed.entity != entity
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "signed v3 entity differs from canonical continuity change".to_owned(),
            ));
        }
        let changed = buzz_sdk::project_view_v3::changed_head_for_entity(
            &context,
            &entity,
            &projection.event,
        )
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        expected_heads.insert(changed.coordinate().to_owned(), changed);
    }
    let work_projections = commit
        .object_projections
        .iter()
        .map(|projection| (projection.object_id(), projection.event()))
        .collect::<BTreeMap<_, _>>();
    if work_projections.len() != commit.object_projections.len() {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "duplicate v3 Work projection".to_owned(),
        ));
    }
    for head in &basis.preparation.work_heads {
        let PreparedV3ProjectObjectHead::Object {
            entry,
            responsible_role_id,
        } = head
        else {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "continuity command prepared a non-Work v3 head".to_owned(),
            ));
        };
        let event = work_projections.get(&entry.id()).ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "missing v3 Work projection {}",
                entry.id()
            ))
        })?;
        let parsed = buzz_sdk::project_view_v3::parse_project_object_projection(
            event,
            &basis.preparation.projection_pubkey,
            basis.preparation.community_id,
        )
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if parsed.project_revision != basis.preparation.project_revision
            || parsed.projection_generation != basis.preparation.projection_generation
            || parsed.source != expected_source
            || parsed.updated_at != basis.preparation.canonical_time
            || parsed.responsible_role_id != *responsible_role_id
            || !projected_v3_object_matches_entry(&parsed.object, entry)
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "signed v3 Work head differs from canonical responsibility change".to_owned(),
            ));
        }
        let changed =
            buzz_sdk::project_view_v3::changed_head_for_project_object(&context, entry, event)
                .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed.coordinate().to_owned(), changed)
            .is_some()
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "duplicate v3 changed-head coordinate".to_owned(),
            ));
        }
    }
    let membership_event_id = commit
        .membership_projection
        .as_ref()
        .map(|event| event.id)
        .or(basis.preparation.membership_snapshot_event_id)
        .ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
                "prepared v3 transition has no membership snapshot".to_owned(),
            )
        })?;
    let meta = buzz_sdk::project_view_v3::parse_meta_projection(
        &commit.meta_projection,
        &basis.preparation.projection_pubkey,
    )
    .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
    let expected_counts = buzz_sdk::project_view_v3::V3EntityCounts {
        active_objects: basis.preparation.counts.active_objects,
        open_proposals: basis.preparation.counts.open_proposals,
        active_assignments: basis.preparation.counts.active_assignments,
        active_commitments: basis.preparation.counts.active_commitments,
        checkpoints: basis.preparation.counts.checkpoints,
        handoffs: basis.preparation.counts.handoffs,
    };
    if meta.project_id != basis.preparation.community_id
        || meta.project_revision != basis.preparation.project_revision
        || meta.projection_generation != basis.preparation.projection_generation
        || meta.entity_counts != expected_counts
        || meta.membership_snapshot_event_id != membership_event_id
        || meta.reset
        || meta.source != expected_source
        || meta.updated_at != basis.preparation.canonical_time
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "signed v3 metadata differs from canonical continuity change".to_owned(),
        ));
    }
    let actual_heads = meta
        .changed_heads
        .iter()
        .map(|head| (head.coordinate().to_owned(), head.clone()))
        .collect::<BTreeMap<_, _>>();
    if actual_heads.len() != meta.changed_heads.len() || actual_heads != expected_heads {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "v3 metadata changed heads do not exactly bind continuity heads".to_owned(),
        ));
    }
    Ok(())
}

async fn load_v3_project_object_state(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV3WriteResult<LoadedV3ProjectObjectState> {
    sqlx::query("SELECT project_view_v3_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    let continuity = crate::project_view_v2::load_continuity_state(tx, community_id, 3).await?;
    let state_row = sqlx::query(
        "SELECT s.initialized_at, s.updated_at, c.project_context_enabled, \
                c.project_document_enabled, \
                document_state.projection_pubkey AS document_projection_pubkey, \
                document_state.meta_projection_event_id AS document_meta_event_id \
         FROM project_view_state s \
         JOIN communities c ON c.id = s.community_id \
         LEFT JOIN project_document_state document_state \
           ON document_state.community_id = c.id \
         WHERE s.community_id = $1 AND s.schema_version = 3 FOR SHARE OF s, c",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    let initialized_at: DateTime<Utc> = state_row.try_get("initialized_at")?;
    let updated_at: DateTime<Utc> = state_row.try_get("updated_at")?;
    let project_context_enabled: bool = state_row.try_get("project_context_enabled")?;
    let project_document_enabled: bool = state_row.try_get("project_document_enabled")?;
    let document_projection_pubkey: Option<Vec<u8>> =
        state_row.try_get("document_projection_pubkey")?;
    let document_meta_event_id: Option<Vec<u8>> = state_row.try_get("document_meta_event_id")?;
    let document_capability_available = project_document_enabled
        && document_projection_pubkey.as_deref() == Some(continuity.projection_pubkey.as_bytes())
        && document_meta_event_id
            .as_ref()
            .is_some_and(|id| id.len() == 32);

    let rows = sqlx::query(
        "SELECT object_id, object_type, object_revision, project_revision, body, \
                under_goal_id, under_plan_id, planned_in_stage_id, \
                about_object_id, about_object_type, handles_object_id, \
                handles_object_type, created_at, updated_at, created_by, \
                updated_by, deleted_at, role_level, responsible_role_id \
         FROM project_view_objects \
         WHERE community_id = $1 AND schema_version = 3 \
         ORDER BY object_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut role_levels = BTreeMap::new();
    let mut work_responsibilities = BTreeMap::new();
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        let object_id: Uuid = row.try_get("object_id")?;
        let role_level: Option<String> = row.try_get("role_level")?;
        if let Some(level) = role_level {
            role_levels.insert(object_id, parse_role_level(&level)?);
        }
        if let Some(role_id) = row.try_get::<Option<Uuid>, _>("responsible_role_id")? {
            work_responsibilities.insert(object_id, role_id);
        }
        entries.push(v3_entry_from_row(row)?);
    }
    let state = ProjectViewStateV3::from_snapshot(
        community_id,
        continuity.state.project_revision(),
        Some(initialized_at),
        Some(updated_at),
        entries,
        role_levels.iter().map(|(id, level)| (*id, *level)),
    )?;
    Ok(LoadedV3ProjectObjectState {
        state,
        canonical_time: continuity.canonical_time,
        projection_generation: continuity.projection_generation,
        projection_pubkey: continuity.projection_pubkey,
        meta_projection_event_id: continuity.meta_projection_event_id,
        membership_snapshot_event_id: continuity.membership_snapshot_event_id,
        role_levels,
        work_responsibilities,
        project_context_enabled,
        document_capability_available,
    })
}

async fn validate_v3_actor_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    acting_assignment_id: Option<Uuid>,
    runtime_fence: Option<buzz_core::RuntimeFence>,
) -> ProjectViewV3WriteResult<()> {
    let actor_bytes = actor.to_bytes();
    let owner: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT agent_owner_pubkey FROM users \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community_id.as_uuid())
    .bind(actor_bytes.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let direct_member: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2)",
    )
    .bind(community_id.as_uuid())
    .bind(actor.to_hex())
    .fetch_one(&mut **tx)
    .await?;
    if active_write_restriction(tx, community_id, actor_bytes.as_slice()).await? {
        return Err(ProjectViewV3WriteError::RoleDomain(
            RoleContinuityError::NotAuthorized,
        ));
    }
    let managed = owner.is_some();
    if let Some(owner) = owner {
        let owner_pubkey = public_key(&owner, "managed Agent owner")?;
        let owner_is_member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2)",
        )
        .bind(community_id.as_uuid())
        .bind(owner_pubkey.to_hex())
        .fetch_one(&mut **tx)
        .await?;
        if !owner_is_member || active_write_restriction(tx, community_id, &owner).await? {
            return Err(ProjectViewV3WriteError::RoleDomain(
                RoleContinuityError::NotAuthorized,
            ));
        }
        if acting_assignment_id.is_none() {
            return Err(ProjectViewV3WriteError::RoleDomain(
                RoleContinuityError::ActingAssignmentRequired,
            ));
        }
    } else if !direct_member {
        return Err(ProjectViewV3WriteError::RoleDomain(
            RoleContinuityError::NotAuthorized,
        ));
    }
    if let Some(assignment_id) = acting_assignment_id {
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_role_assignments \
             WHERE community_id = $1 AND assignment_id = $2 \
               AND member_pubkey = $3 AND ended_at IS NULL)",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .bind(actor.to_hex())
        .fetch_one(&mut **tx)
        .await?;
        if !valid {
            return Err(ProjectViewV3WriteError::RoleDomain(
                RoleContinuityError::ActingAssignmentInvalid,
            ));
        }
    }
    if managed {
        crate::project_runtime::validate_runtime_command_fence_in_tx(
            tx,
            community_id,
            acting_assignment_id,
            runtime_fence,
            crate::project_runtime::RuntimeCommandFencePolicy::RequireSupervisedRuntime,
        )
        .await?;
    }
    Ok(())
}

async fn active_write_restriction(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: &[u8],
) -> ProjectViewV3WriteResult<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM community_bans \
         WHERE community_id = $1 AND pubkey = $2 \
           AND ((banned AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp())) \
                OR muted_until > clock_timestamp()))",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await?)
}

async fn load_document_target_proof(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    coordinates: &BTreeSet<DocumentCoordinate>,
) -> ProjectViewV3WriteResult<ReferenceTargetProof> {
    let mut facts = Vec::with_capacity(coordinates.len());
    for coordinate in coordinates {
        let state = match coordinate.mode {
            DocumentReferenceMode::Live => sqlx::query(
                "SELECT current_revision, state FROM project_documents \
                     WHERE community_id = $1 AND document_id = $2 FOR SHARE",
            )
            .bind(community_id.as_uuid())
            .bind(coordinate.document_id)
            .fetch_optional(&mut **tx)
            .await?
            .map(|row| -> ProjectViewV3WriteResult<DocumentTargetState> {
                let state: String = row.try_get("state")?;
                if state == "active" {
                    Ok(DocumentTargetState::CurrentActive {
                        current_revision: revision_u64(
                            row.try_get("current_revision")?,
                            "document.current_revision",
                        )?,
                    })
                } else {
                    Ok(DocumentTargetState::CurrentTombstone)
                }
            })
            .transpose()?,
            DocumentReferenceMode::Pinned => {
                let revision = coordinate.document_revision.ok_or_else(|| {
                    ProjectViewV3WriteError::InvalidCommit(
                        "pinned Document coordinate has no revision".to_owned(),
                    )
                })?;
                sqlx::query_scalar::<_, String>(
                    "SELECT state FROM project_document_revisions \
                     WHERE community_id = $1 AND document_id = $2 \
                       AND document_revision = $3 FOR SHARE",
                )
                .bind(community_id.as_uuid())
                .bind(coordinate.document_id)
                .bind(revision_i64(revision, "document_revision")?)
                .fetch_optional(&mut **tx)
                .await?
                .map(|state| {
                    if state == "active" {
                        DocumentTargetState::ActiveContentRevision
                    } else {
                        DocumentTargetState::TombstoneRevision
                    }
                })
            }
        };
        if let Some(state) = state {
            facts.push((*coordinate, state));
        }
    }
    ReferenceTargetProof::from_documents(facts)
        .map_err(V3ProjectObjectError::from)
        .map_err(Into::into)
}

pub(crate) fn v3_entry_from_row(
    row: sqlx::postgres::PgRow,
) -> ProjectViewV3WriteResult<ProjectViewEntryV3> {
    let object_id: Uuid = row.try_get("object_id")?;
    let object_type = parse_object_type(&row.try_get::<String, _>("object_type")?)?;
    let object_revision = revision_u64(row.try_get("object_revision")?, "object_revision")?;
    let project_revision = revision_u64(row.try_get("project_revision")?, "project_revision")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let created_by = public_key(&row.try_get::<Vec<u8>, _>("created_by")?, "created_by")?;
    let updated_by = public_key(&row.try_get::<Vec<u8>, _>("updated_by")?, "updated_by")?;
    if let Some(deleted_at) = row.try_get::<Option<DateTime<Utc>>, _>("deleted_at")? {
        return Ok(ProjectViewEntryV3::Tombstone(ProjectViewTombstoneV3 {
            id: object_id,
            object_type,
            object_revision,
            project_revision,
            created_at,
            deleted_at,
            created_by,
            deleted_by: updated_by,
        }));
    }
    let mut body: Value = row.try_get("body")?;
    let context_references = body
        .as_object_mut()
        .and_then(|object| object.remove("context_references"))
        .ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
                "active v3 body has no context_references".to_owned(),
            )
        })?;
    let context_references: Vec<ProjectContextReference> =
        serde_json::from_value(context_references).map_err(DbError::from)?;
    if object_type == ProjectViewObjectType::Role {
        body.as_object_mut().map(|object| object.remove("level"));
    }
    let data: ProjectViewObjectDataV3 = serde_json::from_value(json!({
        "object_type": object_type.as_str(),
        "data": body,
    }))
    .map_err(DbError::from)?;
    let relations = ProjectViewRelations {
        under_goal_id: row.try_get("under_goal_id")?,
        under_plan_id: row.try_get("under_plan_id")?,
        planned_in_stage_id: row.try_get("planned_in_stage_id")?,
        about: typed_reference(
            row.try_get("about_object_id")?,
            row.try_get("about_object_type")?,
            "about",
        )?,
        handles: typed_reference(
            row.try_get("handles_object_id")?,
            row.try_get("handles_object_type")?,
            "handles",
        )?,
    };
    Ok(ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
        id: object_id,
        object_type,
        object_revision,
        project_revision,
        created_at,
        updated_at,
        created_by,
        updated_by,
        data,
        relations,
        context_references,
    })))
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn write_v3_entry(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8; 32],
    projection_event_id: &[u8],
    actor: PublicKey,
    entry: &ProjectViewEntryV3,
    role_level: Option<RoleLevel>,
    responsible_role_id: Option<Uuid>,
) -> ProjectViewV3WriteResult<()> {
    let provenance_id = Uuid::new_v4();
    let (
        object_id,
        object_type,
        object_revision,
        project_revision,
        body,
        relations,
        created_at,
        updated_at,
        created_by,
        updated_by,
        deleted_at,
        context_references,
        guide_document_id,
    ) = match entry {
        ProjectViewEntryV3::Active(object) => {
            let mut body = v3_object_body(&object.data, &object.context_references)?;
            if object.object_type == ProjectViewObjectType::Role {
                let level = role_level.ok_or_else(|| {
                    ProjectViewV3WriteError::InvalidCommit(
                        "active v3 Role is missing its governance level".to_owned(),
                    )
                })?;
                body.as_object_mut()
                    .ok_or_else(|| {
                        ProjectViewV3WriteError::InvalidCommit(
                            "serialized v3 Role body is not an object".to_owned(),
                        )
                    })?
                    .insert("level".to_owned(), Value::String(level.as_str().to_owned()));
            }
            let guide_document_id = match &object.data {
                ProjectViewObjectDataV3::Resource(resource) => Some(resource.guide_document_id),
                _ => None,
            };
            (
                object.id,
                object.object_type,
                object.object_revision,
                object.project_revision,
                Some(body),
                object.relations,
                object.created_at,
                object.updated_at,
                object.created_by,
                object.updated_by,
                None,
                object.context_references.as_slice(),
                guide_document_id,
            )
        }
        ProjectViewEntryV3::Tombstone(tombstone) => (
            tombstone.id,
            tombstone.object_type,
            tombstone.object_revision,
            tombstone.project_revision,
            None,
            ProjectViewRelations::default(),
            tombstone.created_at,
            tombstone.deleted_at,
            tombstone.created_by,
            tombstone.deleted_by,
            Some(tombstone.deleted_at),
            &[][..],
            None,
        ),
    };
    if object_type != ProjectViewObjectType::Role && role_level.is_some() {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "only v3 Roles may carry role_level".to_owned(),
        ));
    }
    if responsible_role_id.is_some()
        && !(object_type == ProjectViewObjectType::Work && deleted_at.is_none())
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "only active v3 Work may carry responsible_role_id".to_owned(),
        ));
    }
    let created_by_bytes = created_by.to_bytes();
    let updated_by_bytes = updated_by.to_bytes();
    let actor_bytes = actor.to_bytes();
    sqlx::query(
        "INSERT INTO project_view_object_provenance \
            (community_id, provenance_id, object_id, object_type, source_type, \
             source_change_id, source_event_id, source_project_revision, \
             source_actor_pubkey, legacy_mutation_event_id, project_view_change_id) \
         VALUES ($1,$2,$3,$4,'nostr_event',$5,$5,$6,$7,NULL,$5)",
    )
    .bind(community_id.as_uuid())
    .bind(provenance_id)
    .bind(object_id)
    .bind(object_type.as_str())
    .bind(change_id.as_slice())
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(actor_bytes.as_slice())
    .execute(&mut **tx)
    .await?;

    let about_id = relations.about.map(|reference| reference.object_id);
    let about_type = relations
        .about
        .map(|reference| reference.object_type.as_str());
    let handles_id = relations.handles.map(|reference| reference.object_id);
    let handles_type = relations
        .handles
        .map(|reference| reference.object_type.as_str());
    let stored = sqlx::query(
        "INSERT INTO project_view_objects \
            (community_id, object_id, object_type, schema_version, object_revision, \
             project_revision, body, under_goal_id, under_plan_id, planned_in_stage_id, \
             about_object_id, about_object_type, handles_object_id, handles_object_type, \
             created_at, updated_at, created_by, updated_by, source_event_id, \
             projection_event_id, deleted_at, role_level, responsible_role_id, \
             guide_document_id, source_type, source_change_id, source_provenance_id) \
         VALUES ($1,$2,$3,3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17, \
                 $18,$19,$20,$21,$22,$23,'nostr_event',$18,$24) \
         ON CONFLICT (community_id, object_id) DO UPDATE SET \
             object_type = EXCLUDED.object_type, schema_version = 3, \
             object_revision = EXCLUDED.object_revision, \
             project_revision = EXCLUDED.project_revision, body = EXCLUDED.body, \
             under_goal_id = EXCLUDED.under_goal_id, under_plan_id = EXCLUDED.under_plan_id, \
             planned_in_stage_id = EXCLUDED.planned_in_stage_id, \
             about_object_id = EXCLUDED.about_object_id, about_object_type = EXCLUDED.about_object_type, \
             handles_object_id = EXCLUDED.handles_object_id, handles_object_type = EXCLUDED.handles_object_type, \
             created_at = EXCLUDED.created_at, updated_at = EXCLUDED.updated_at, \
             created_by = EXCLUDED.created_by, updated_by = EXCLUDED.updated_by, \
             source_event_id = EXCLUDED.source_event_id, \
             projection_event_id = EXCLUDED.projection_event_id, deleted_at = EXCLUDED.deleted_at, \
             role_level = EXCLUDED.role_level, responsible_role_id = EXCLUDED.responsible_role_id, \
             guide_document_id = EXCLUDED.guide_document_id, source_type = EXCLUDED.source_type, \
             source_change_id = EXCLUDED.source_change_id, \
             source_provenance_id = EXCLUDED.source_provenance_id \
         WHERE project_view_objects.schema_version = 3 \
           AND project_view_objects.deleted_at IS NULL \
           AND project_view_objects.object_type = EXCLUDED.object_type \
           AND project_view_objects.object_revision + 1 = EXCLUDED.object_revision \
           AND project_view_objects.project_revision < EXCLUDED.project_revision",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .bind(object_type.as_str())
    .bind(revision_i64(object_revision, "object_revision")?)
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(body)
    .bind(relations.under_goal_id)
    .bind(relations.under_plan_id)
    .bind(relations.planned_in_stage_id)
    .bind(about_id)
    .bind(about_type)
    .bind(handles_id)
    .bind(handles_type)
    .bind(created_at)
    .bind(updated_at)
    .bind(created_by_bytes.as_slice())
    .bind(updated_by_bytes.as_slice())
    .bind(change_id.as_slice())
    .bind(projection_event_id)
    .bind(deleted_at)
    .bind(role_level.map(RoleLevel::as_str))
    .bind(responsible_role_id)
    .bind(guide_document_id)
    .bind(provenance_id)
    .execute(&mut **tx)
    .await?;
    if stored.rows_affected() != 1 {
        return Err(ProjectViewV3WriteError::InvalidCommit(format!(
            "canonical v3 object {object_id} did not advance exactly one revision"
        )));
    }

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
    for reference in context_references {
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
                    DocumentReferenceMode::Live => "live",
                    DocumentReferenceMode::Pinned => "pinned",
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

pub(crate) fn v3_object_body(
    data: &ProjectViewObjectDataV3,
    references: &[ProjectContextReference],
) -> ProjectViewV3WriteResult<Value> {
    let mut envelope = serde_json::to_value(data).map_err(DbError::from)?;
    let mut body = envelope
        .get_mut("data")
        .map(Value::take)
        .and_then(|value| value.as_object().cloned())
        .ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
                "serialized v3 object body is missing data".to_owned(),
            )
        })?;
    body.insert(
        "context_references".to_owned(),
        serde_json::to_value(references).map_err(DbError::from)?,
    );
    Ok(Value::Object(body))
}

async fn insert_project_object_change_v3(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    command: &ProjectObjectCommandV3,
    project_revision: u64,
    accepted_at: DateTime<Utc>,
    result: &Value,
) -> ProjectViewV3WriteResult<()> {
    let actor = event.pubkey.to_bytes();
    let subject = serde_json::to_value(&command.request).map_err(DbError::from)?;
    sqlx::query(
        "INSERT INTO project_view_changes \
            (community_id, change_id, source_type, source_event_id, \
             source_request_hash, source_audit_seq, idempotency_key_hash, \
             actor_pubkey, acting_assignment_id, operation, subject, \
             project_revision, result, accepted_at) \
         VALUES ($1,$2,'nostr_event',$2,NULL,NULL,NULL,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes())
    .bind(actor.as_slice())
    .bind(command.acting_assignment_id)
    .bind(command.operation())
    .bind(subject)
    .bind(revision_i64(project_revision, "project_revision")?)
    .bind(result)
    .bind(accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn project_object_receipt_v3(
    command: &ProjectObjectCommandV3,
    entries: &[ProjectViewEntryV3],
    continuity_changes: &[RoleContinuityChange],
    project_revision: u64,
) -> Value {
    json!({
        "schema_version": 3,
        "operation": command.operation(),
        "project_revision": project_revision,
        "objects": entries.iter().map(|entry| json!({
            "object_id": entry.id(),
            "object_type": entry.object_type().as_str(),
            "object_revision": entry.object_revision(),
            "deleted": matches!(entry, ProjectViewEntryV3::Tombstone(_)),
        })).collect::<Vec<_>>(),
        "continuity_entities": continuity_changes.iter().map(|change| json!({
            "entity_type": change.entity_type().as_str(),
            "entity_id": change.entity_id(),
            "entity_revision": change.entity_revision(),
        })).collect::<Vec<_>>(),
    })
}

fn validate_project_object_commit_bundle(
    basis: &V3PreparedProjectObjectBasis,
    commit: &PreparedV3ProjectObjectCommit,
) -> ProjectViewV3WriteResult<()> {
    if commit.command_event.id.to_bytes() != basis.command_event_id
        || commit.command_event.pubkey != basis.actor
        || ProjectObjectCommandV3::from_json(&commit.command_event.content)? != basis.command
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "committed command differs from the prepared v3 command".to_owned(),
        ));
    }
    commit.command_event.verify().map_err(|error| {
        ProjectViewV3WriteError::InvalidCommit(format!(
            "committed v3 command signature is invalid: {error}"
        ))
    })?;
    let tags = commit
        .command_event
        .tags
        .iter()
        .map(Tag::as_slice)
        .collect::<Vec<_>>();
    if tags
        != [
            vec!["-".to_owned()],
            vec!["t".to_owned(), "buzz-project-view-mutation".to_owned()],
        ]
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "committed command tags are not the exact protected v3 shape".to_owned(),
        ));
    }
    let expected_source = buzz_sdk::project_view_v3::V3ProjectionSource::NostrEvent {
        change_id: commit.command_event.id,
        event_id: commit.command_event.id,
    };
    let context = buzz_sdk::project_view_v3::V3ProjectionContext {
        project_id: basis.preparation.community_id,
        projection_generation: basis.preparation.projection_generation,
        project_revision: basis.preparation.project_revision,
        source: expected_source.clone(),
        updated_at: basis.preparation.canonical_time,
    };
    let projection_map = commit
        .object_projections
        .iter()
        .map(|projection| (projection.object_id(), projection.event()))
        .collect::<BTreeMap<_, _>>();
    if projection_map.len() != commit.object_projections.len()
        || projection_map.len() != basis.preparation.heads.len()
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "projection set does not exactly cover changed v3 objects".to_owned(),
        ));
    }
    let mut expected_heads = BTreeMap::new();
    for head in &basis.preparation.heads {
        let event = projection_map.get(&head.object_id()).ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "missing v3 projection for {}",
                head.object_id()
            ))
        })?;
        let changed = match head {
            PreparedV3ProjectObjectHead::Role(role) => {
                let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
                    event,
                    &basis.preparation.projection_pubkey,
                    basis.preparation.community_id,
                )
                .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
                if parsed.project_revision != basis.preparation.project_revision
                    || parsed.projection_generation != basis.preparation.projection_generation
                    || parsed.source != expected_source
                    || parsed.updated_at != basis.preparation.canonical_time
                    || parsed.entity
                        != buzz_sdk::project_view_v3::V3EntityChange::Role(role.clone())
                {
                    return Err(ProjectViewV3WriteError::InvalidCommit(
                        "signed RoleDefinitionV3 differs from canonical Role".to_owned(),
                    ));
                }
                buzz_sdk::project_view_v3::changed_head_for_entity(
                    &context,
                    &buzz_sdk::project_view_v3::V3EntityChange::Role(role.clone()),
                    event,
                )
            }
            PreparedV3ProjectObjectHead::Object {
                entry,
                responsible_role_id,
            } => {
                let parsed = buzz_sdk::project_view_v3::parse_project_object_projection(
                    event,
                    &basis.preparation.projection_pubkey,
                    basis.preparation.community_id,
                )
                .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
                if parsed.project_revision != basis.preparation.project_revision
                    || parsed.projection_generation != basis.preparation.projection_generation
                    || parsed.source != expected_source
                    || parsed.updated_at != basis.preparation.canonical_time
                    || parsed.responsible_role_id != *responsible_role_id
                    || !projected_v3_object_matches_entry(&parsed.object, entry)
                {
                    return Err(ProjectViewV3WriteError::InvalidCommit(
                        "signed v3 object head differs from canonical entry".to_owned(),
                    ));
                }
                buzz_sdk::project_view_v3::changed_head_for_project_object(&context, entry, event)
            }
        }
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed.coordinate().to_owned(), changed)
            .is_some()
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "prepared v3 object change has duplicate head coordinates".to_owned(),
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
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "v3 continuity projection count differs from prepared changes".to_owned(),
        ));
    }
    let mut actual_entities = BTreeSet::new();
    for projection in &commit.entity_projections {
        let key = (projection.entity_type, projection.entity_id);
        if !actual_entities.insert(key) {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "duplicate v3 continuity entity projection".to_owned(),
            ));
        }
        let expected = expected_entities.get(&key).ok_or_else(|| {
            ProjectViewV3WriteError::InvalidCommit(
                "unexpected v3 continuity entity projection".to_owned(),
            )
        })?;
        let entity = v3_entity_change(expected)?;
        let parsed = buzz_sdk::project_view_v3::parse_entity_projection(
            &projection.event,
            &basis.preparation.projection_pubkey,
            basis.preparation.community_id,
        )
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if parsed.project_revision != basis.preparation.project_revision
            || parsed.projection_generation != basis.preparation.projection_generation
            || parsed.source != expected_source
            || parsed.updated_at != basis.preparation.canonical_time
            || parsed.entity != entity
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "signed v3 continuity head differs from canonical change".to_owned(),
            ));
        }
        let changed = buzz_sdk::project_view_v3::changed_head_for_entity(
            &context,
            &entity,
            &projection.event,
        )
        .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
        if expected_heads
            .insert(changed.coordinate().to_owned(), changed)
            .is_some()
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "duplicate v3 changed-head coordinate".to_owned(),
            ));
        }
    }
    let meta = buzz_sdk::project_view_v3::parse_meta_projection(
        &commit.meta_projection,
        &basis.preparation.projection_pubkey,
    )
    .map_err(|error| ProjectViewV3WriteError::InvalidCommit(error.to_string()))?;
    let expected_counts = buzz_sdk::project_view_v3::V3EntityCounts {
        active_objects: basis.preparation.counts.active_objects,
        open_proposals: basis.preparation.counts.open_proposals,
        active_assignments: basis.preparation.counts.active_assignments,
        active_commitments: basis.preparation.counts.active_commitments,
        checkpoints: basis.preparation.counts.checkpoints,
        handoffs: basis.preparation.counts.handoffs,
    };
    if meta.project_id != basis.preparation.community_id
        || meta.project_revision != basis.preparation.project_revision
        || meta.projection_generation != basis.preparation.projection_generation
        || meta.entity_counts != expected_counts
        || meta.membership_snapshot_event_id != basis.preparation.membership_snapshot_event_id
        || meta.reset
        || meta.source != expected_source
        || meta.updated_at != basis.preparation.canonical_time
    {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "signed v3 metadata differs from canonical change".to_owned(),
        ));
    }
    let actual_heads = meta
        .changed_heads
        .iter()
        .map(|head| (head.coordinate().to_owned(), head.clone()))
        .collect::<BTreeMap<_, _>>();
    if actual_heads.len() != meta.changed_heads.len() || actual_heads != expected_heads {
        return Err(ProjectViewV3WriteError::InvalidCommit(
            "v3 metadata changed heads do not exactly bind prepared heads".to_owned(),
        ));
    }
    Ok(())
}

fn projected_v3_object_matches_entry(
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

fn v3_entity_change(
    change: &RoleContinuityChange,
) -> ProjectViewV3WriteResult<buzz_sdk::project_view_v3::V3EntityChange> {
    Ok(match change {
        RoleContinuityChange::Role(_) => {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "continuity-only v3 path cannot project a legacy Role head".to_owned(),
            ));
        }
        RoleContinuityChange::Proposal(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Proposal(value.clone())
        }
        RoleContinuityChange::Assignment(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Assignment(value.clone())
        }
        RoleContinuityChange::Commitment(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Commitment(value.clone())
        }
        RoleContinuityChange::Checkpoint(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Checkpoint(value.clone())
        }
        RoleContinuityChange::Handoff(value) => {
            buzz_sdk::project_view_v3::V3EntityChange::Handoff(value.clone())
        }
    })
}

async fn load_object_projection_ids(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    object_ids: &[Uuid],
) -> ProjectViewV3WriteResult<BTreeMap<Uuid, [u8; 32]>> {
    if object_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let rows = sqlx::query(
        "SELECT object_id, projection_event_id FROM project_view_objects \
         WHERE community_id = $1 AND object_id = ANY($2) FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(object_ids)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok((
                row.try_get("object_id")?,
                bytes32(row.try_get("projection_event_id")?, "projection_event_id")?,
            ))
        })
        .collect()
}

async fn retire_required_head(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8; 32],
    kind: u32,
    label: &str,
) -> ProjectViewV3WriteResult<()> {
    if !crate::event::retire_projection_head_in_tx(tx, community_id, event_id, kind).await? {
        return Err(ProjectViewV3WriteError::InvalidCommit(format!(
            "stored {label} projection pointer is not live"
        )));
    }
    Ok(())
}

async fn assert_counts_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    expected: V2CanonicalCounts,
) -> ProjectViewV3WriteResult<()> {
    let row = sqlx::query(
        "SELECT active_object_count, open_proposal_count, active_assignment_count, \
                active_commitment_count, checkpoint_count, handoff_count \
         FROM project_view_state WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    for (field, expected) in [
        ("active_object_count", expected.active_objects),
        ("open_proposal_count", expected.open_proposals),
        ("active_assignment_count", expected.active_assignments),
        ("active_commitment_count", expected.active_commitments),
        ("checkpoint_count", expected.checkpoints),
        ("handoff_count", expected.handoffs),
    ] {
        let actual: i32 = row.try_get(field)?;
        if u32::try_from(actual).ok() != Some(expected) {
            return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                "{field} differs from the prepared v3 state"
            )));
        }
    }
    Ok(())
}

fn deactivated_role_ids(entries: &[ProjectViewEntryV3]) -> Vec<Uuid> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            ProjectViewEntryV3::Active(object)
                if matches!(&object.data, ProjectViewObjectDataV3::Role(role) if !role.active) =>
            {
                Some(object.id)
            }
            ProjectViewEntryV3::Tombstone(tombstone)
                if tombstone.object_type == ProjectViewObjectType::Role =>
            {
                Some(tombstone.id)
            }
            _ => None,
        })
        .collect()
}

fn terminal_work_ids(entries: &[ProjectViewEntryV3]) -> Vec<Uuid> {
    entries
        .iter()
        .filter(|entry| match entry {
            ProjectViewEntryV3::Active(object)
                if object.object_type == ProjectViewObjectType::Work =>
            {
                matches!(
                    &object.data,
                    ProjectViewObjectDataV3::Work(work)
                        if matches!(work.status, WorkStatus::Completed | WorkStatus::Cancelled)
                )
            }
            ProjectViewEntryV3::Tombstone(tombstone) => {
                tombstone.object_type == ProjectViewObjectType::Work
            }
            ProjectViewEntryV3::Active(_) => false,
        })
        .map(ProjectViewEntryV3::id)
        .collect()
}

#[derive(Debug)]
struct InitializeGovernors {
    owner: PublicKey,
    members: BTreeSet<PublicKey>,
}

async fn require_current_initialize_owner_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
) -> ProjectViewV3WriteResult<()> {
    let actor_bytes = actor.to_bytes();
    let eligible: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM communities community \
             JOIN relay_members member ON member.community_id = community.id \
             LEFT JOIN users identity \
               ON identity.community_id = community.id AND identity.pubkey = $3 \
             LEFT JOIN community_bans restriction \
               ON restriction.community_id = community.id AND restriction.pubkey = $3 \
             WHERE community.id = $1 AND community.archived_at IS NULL \
               AND member.pubkey = $2 AND member.role = 'owner' \
               AND identity.agent_owner_pubkey IS NULL \
               AND NOT COALESCE( \
                   restriction.banned \
                   AND (restriction.ban_expires_at IS NULL \
                        OR restriction.ban_expires_at > clock_timestamp()), FALSE) \
               AND NOT COALESCE(restriction.muted_until > clock_timestamp(), FALSE) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(actor.to_hex())
    .bind(actor_bytes.as_slice())
    .fetch_one(&mut **tx)
    .await?;
    if eligible {
        Ok(())
    } else {
        Err(ProjectViewV3WriteError::InvalidCommit(
            "current eligible direct Human owner is required".to_owned(),
        ))
    }
}

async fn load_initialize_governors_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> ProjectViewV3WriteResult<InitializeGovernors> {
    let rows = sqlx::query(
        "SELECT member.pubkey, member.role, identity.agent_owner_pubkey, \
                COALESCE( \
                    restriction.banned \
                    AND (restriction.ban_expires_at IS NULL \
                         OR restriction.ban_expires_at > clock_timestamp()), FALSE \
                ) AS banned, \
                COALESCE(restriction.muted_until > clock_timestamp(), FALSE) AS timed_out \
         FROM relay_members member \
         LEFT JOIN users identity \
           ON identity.community_id = member.community_id \
          AND encode(identity.pubkey, 'hex') = member.pubkey \
         LEFT JOIN community_bans restriction \
           ON restriction.community_id = member.community_id \
          AND encode(restriction.pubkey, 'hex') = member.pubkey \
         WHERE member.community_id = $1 AND member.role IN ('owner', 'admin') \
         ORDER BY member.pubkey FOR UPDATE OF member",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut **tx)
    .await?;
    let mut owner = None;
    let mut members = BTreeSet::new();
    for row in rows {
        let pubkey_text: String = row.try_get("pubkey")?;
        let pubkey = PublicKey::parse(&pubkey_text).map_err(|error| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "invalid governor pubkey {pubkey_text}: {error}"
            ))
        })?;
        if row
            .try_get::<Option<Vec<u8>>, _>("agent_owner_pubkey")?
            .is_some()
            || row.try_get::<bool, _>("banned")?
            || row.try_get::<bool, _>("timed_out")?
        {
            return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                "governor {pubkey_text} is not an eligible direct Human"
            )));
        }
        if row.try_get::<String, _>("role")? == "owner" && owner.replace(pubkey).is_some() {
            return Err(ProjectViewV3WriteError::InvalidCommit(
                "greenfield initialization requires exactly one Community owner".to_owned(),
            ));
        }
        members.insert(pubkey);
    }
    let owner = owner.ok_or_else(|| {
        ProjectViewV3WriteError::InvalidCommit(
            "greenfield initialization requires exactly one Community owner".to_owned(),
        )
    })?;
    Ok(InitializeGovernors { owner, members })
}

fn build_v3_membership_event(
    members: &[V2MembershipEntry],
    canonical_time: DateTime<Utc>,
    relay_keys: &Keys,
) -> ProjectViewV3WriteResult<Event> {
    let mut tags = Vec::with_capacity(members.len() + 1);
    tags.push(Tag::parse(["-"]).map_err(|error| {
        ProjectViewV3WriteError::InvalidCommit(format!(
            "build initialize membership protection tag: {error}"
        ))
    })?);
    for member in members {
        tags.push(
            Tag::parse(["member", member.pubkey.as_str(), member.role.as_str()]).map_err(
                |error| {
                    ProjectViewV3WriteError::InvalidCommit(format!(
                        "build initialize membership member tag: {error}"
                    ))
                },
            )?,
        );
    }
    let seconds = u64::try_from(canonical_time.timestamp()).map_err(|_| {
        ProjectViewV3WriteError::InvalidCommit(
            "initialize canonical time precedes Unix epoch".to_owned(),
        )
    })?;
    EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
        .tags(tags)
        .custom_created_at(Timestamp::from(seconds))
        .sign_with_keys(relay_keys)
        .map_err(|error| {
            ProjectViewV3WriteError::InvalidCommit(format!(
                "sign initialize membership snapshot: {error}"
            ))
        })
}

fn active_count_delta(entries: &[ProjectViewEntryV3]) -> i64 {
    entries.iter().fold(0_i64, |delta, entry| match entry {
        // A create has revision one. Updates remain active and tombstones were
        // active before this transaction.
        ProjectViewEntryV3::Active(object) if object.object_revision == 1 => delta + 1,
        ProjectViewEntryV3::Active(_) => delta,
        ProjectViewEntryV3::Tombstone(_) => delta - 1,
    })
}

fn typed_reference(
    object_id: Option<Uuid>,
    object_type: Option<String>,
    field: &str,
) -> ProjectViewV3WriteResult<Option<ObjectRef>> {
    match (object_id, object_type) {
        (None, None) => Ok(None),
        (Some(object_id), Some(object_type)) => Ok(Some(ObjectRef {
            object_type: parse_object_type(&object_type)?,
            object_id,
        })),
        _ => Err(ProjectViewV3WriteError::InvalidCommit(format!(
            "stored {field} relation has an incomplete id/type pair"
        ))),
    }
}

fn parse_object_type(value: &str) -> ProjectViewV3WriteResult<ProjectViewObjectType> {
    Ok(match value {
        "project_profile" => ProjectViewObjectType::ProjectProfile,
        "goal" => ProjectViewObjectType::Goal,
        "role" => ProjectViewObjectType::Role,
        "plan" => ProjectViewObjectType::Plan,
        "stage" => ProjectViewObjectType::Stage,
        "requirement" => ProjectViewObjectType::Requirement,
        "issue" => ProjectViewObjectType::Issue,
        "work" => ProjectViewObjectType::Work,
        "resource" => ProjectViewObjectType::Resource,
        other => {
            return Err(ProjectViewV3WriteError::InvalidCommit(format!(
                "unknown Project View object type {other}"
            )));
        }
    })
}

fn parse_role_level(value: &str) -> ProjectViewV3WriteResult<RoleLevel> {
    match value {
        "admin" => Ok(RoleLevel::Admin),
        "member" => Ok(RoleLevel::Member),
        other => Err(ProjectViewV3WriteError::InvalidCommit(format!(
            "unknown Role level {other}"
        ))),
    }
}

fn public_key(bytes: &[u8], field: &str) -> ProjectViewV3WriteResult<PublicKey> {
    PublicKey::from_slice(bytes).map_err(|error| {
        ProjectViewV3WriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn bytes32(bytes: Vec<u8>, field: &str) -> ProjectViewV3WriteResult<[u8; 32]> {
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        ProjectViewV3WriteError::InvalidCommit(format!(
            "{field} must contain 32 bytes, got {}",
            bytes.len()
        ))
    })
}

fn revision_u64(value: i64, field: &str) -> ProjectViewV3WriteResult<u64> {
    u64::try_from(value).map_err(|_| {
        ProjectViewV3WriteError::InvalidCommit(format!("{field} must be non-negative, got {value}"))
    })
}

fn revision_i64(value: u64, field: &str) -> ProjectViewV3WriteResult<i64> {
    i64::try_from(value).map_err(|_| {
        ProjectViewV3WriteError::InvalidCommit(format!("{field} exceeds PostgreSQL BIGINT"))
    })
}

fn optional_db_u64(value: Option<i64>, field: &str) -> crate::Result<Option<u64>> {
    value.map(|value| db_count_u64(value, field)).transpose()
}

fn db_count_u64(value: i64, field: &str) -> crate::Result<u64> {
    u64::try_from(value)
        .map_err(|_| DbError::InvalidData(format!("stored {field} must be non-negative")))
}

fn count_i32(value: u32, field: &str) -> ProjectViewV3WriteResult<i32> {
    i32::try_from(value).map_err(|_| {
        ProjectViewV3WriteError::InvalidCommit(format!("{field} exceeds PostgreSQL INTEGER"))
    })
}

fn kind_i32(kind: u32) -> crate::Result<i32> {
    i32::try_from(kind)
        .map_err(|_| DbError::InvalidData(format!("event kind {kind} exceeds PostgreSQL INT")))
}
