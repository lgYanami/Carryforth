//! Project View v3 commands and strict Relay projection wire format.

use std::collections::HashSet;

use buzz_core::kind::{
    KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::{
    RoleAssignment, RoleAssignmentProposal, RoleCheckpoint, RoleContinuityEntity, RoleHandoff,
    WorkCommitment,
};
use buzz_project_view::v3::{
    validate_projected_object_v3, ProjectObjectCommandV3, ProjectViewEntryV3,
    ProjectViewInitializeV3, ProjectViewObjectV3, ProjectViewTombstoneV3, RoleCommandV3,
    RoleDefinitionV3, PROJECT_VIEW_V3_SCHEMA_VERSION,
};
use buzz_project_view::{ProjectViewObjectType, MAX_SAFE_REVISION};
use chrono::{DateTime, SecondsFormat, Utc};
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::SdkError;

const PROJECT_VIEW_TAG: &str = "buzz-project-view";
const PROJECT_VIEW_V3_ENTITY_TAG: &str = "buzz-project-view-v3-entity";
const PROJECT_VIEW_V3_OBJECT_TAG: &str = "buzz-project-view-v3-object";
const PROJECT_VIEW_META_TAG: &str = "buzz-project-view-meta";
const PROJECT_VIEW_MUTATION_TAG: &str = "buzz-project-view-mutation";

/// Build a protected kind `44300` schema-v3 ordinary-object command.
pub fn build_project_object_command(
    command: ProjectObjectCommandV3,
) -> Result<EventBuilder, SdkError> {
    command
        .validate_for_submission()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = serde_json::to_string(&command).map_err(|error| {
        SdkError::InvalidInput(format!("serialize v3 Project object command: {error}"))
    })?;
    ProjectObjectCommandV3::from_json(&content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    mutation_builder(content)
}

/// Build a protected kind `44300` schema-v3 continuity-only Role command.
pub fn build_role_command(command: RoleCommandV3) -> Result<EventBuilder, SdkError> {
    command
        .validate_for_submission()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = serde_json::to_string(&command)
        .map_err(|error| SdkError::InvalidInput(format!("serialize v3 Role command: {error}")))?;
    RoleCommandV3::from_json(&content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    mutation_builder(content)
}

/// Build the only owner-signed command accepted by the prepared, disabled,
/// uninitialized schema-v3 bootstrap path.
pub fn build_initialize_command(
    command: ProjectViewInitializeV3,
) -> Result<EventBuilder, SdkError> {
    command
        .validate()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = serde_json::to_string(&command)
        .map_err(|error| SdkError::InvalidInput(format!("serialize v3 initialize: {error}")))?;
    ProjectViewInitializeV3::from_json(&content)
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    mutation_builder(content)
}

fn mutation_builder(content: String) -> Result<EventBuilder, SdkError> {
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_MUTATION as u16), content)
            .tags([tag(["-"])?, tag(["t", PROJECT_VIEW_MUTATION_TAG])?]),
    )
}

/// Typed source carried by every v3 head and metadata projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum V3ProjectionSource {
    /// Verified member-signed Nostr command.
    NostrEvent {
        /// Stable change ID; equal to the event ID.
        change_id: EventId,
        /// Verified source event.
        event_id: EventId,
    },
    /// Audited operator transition such as cutover or repair.
    Operator {
        /// Stable domain-separated change ID.
        change_id: EventId,
        /// Positive Community audit sequence.
        audit_seq: u64,
    },
    /// Audited trusted-system transition.
    System {
        /// Stable domain-separated change ID.
        change_id: EventId,
        /// Positive Community audit sequence.
        audit_seq: u64,
    },
}

impl V3ProjectionSource {
    /// Stable source discriminator.
    #[must_use]
    pub const fn source_type(&self) -> &'static str {
        match self {
            Self::NostrEvent { .. } => "nostr_event",
            Self::Operator { .. } => "operator",
            Self::System { .. } => "system",
        }
    }

    /// Stable accepted change ID.
    #[must_use]
    pub const fn change_id(&self) -> EventId {
        match self {
            Self::NostrEvent { change_id, .. }
            | Self::Operator { change_id, .. }
            | Self::System { change_id, .. } => *change_id,
        }
    }

    /// Source event only for a member Nostr command.
    #[must_use]
    pub const fn source_event_id(&self) -> Option<EventId> {
        match self {
            Self::NostrEvent { event_id, .. } => Some(*event_id),
            Self::Operator { .. } | Self::System { .. } => None,
        }
    }
}

/// Projection-envelope input for one v3 head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3ProjectionContext {
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Active projection signer generation.
    pub projection_generation: u64,
    /// Project revision represented by this head.
    pub project_revision: u64,
    /// Exact accepted source.
    pub source: V3ProjectionSource,
    /// Relay canonical head time.
    pub updated_at: DateTime<Utc>,
}

/// Materialized counts inherited from the v2 observation boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V3EntityCounts {
    /// Active Project View objects; every non-tombstoned Role counts once.
    pub active_objects: u32,
    /// Open Proposals.
    pub open_proposals: u32,
    /// Active Assignments.
    pub active_assignments: u32,
    /// Active Work Commitments.
    pub active_commitments: u32,
    /// Append-only Checkpoints.
    pub checkpoints: u32,
    /// Append-only Handoffs.
    pub handoffs: u32,
}

/// Complete v3 Role-continuity head union.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V3EntityChange {
    /// Unified non-tombstoned Role definition head.
    Role(RoleDefinitionV3),
    /// Proposal head.
    Proposal(RoleAssignmentProposal),
    /// Assignment head.
    Assignment(RoleAssignment),
    /// Work Commitment head.
    Commitment(WorkCommitment),
    /// Checkpoint head.
    Checkpoint(RoleCheckpoint),
    /// Handoff head.
    Handoff(RoleHandoff),
}

impl V3EntityChange {
    /// Stable entity discriminator.
    #[must_use]
    pub const fn entity_type(&self) -> RoleContinuityEntity {
        match self {
            Self::Role(_) => RoleContinuityEntity::Role,
            Self::Proposal(_) => RoleContinuityEntity::RoleAssignmentProposal,
            Self::Assignment(_) => RoleContinuityEntity::RoleAssignment,
            Self::Commitment(_) => RoleContinuityEntity::WorkCommitment,
            Self::Checkpoint(_) => RoleContinuityEntity::RoleCheckpoint,
            Self::Handoff(_) => RoleContinuityEntity::RoleHandoff,
        }
    }

    /// Stable entity identity.
    #[must_use]
    pub const fn entity_id(&self) -> Uuid {
        match self {
            Self::Role(value) => value.role_id,
            Self::Proposal(value) => value.proposal_id,
            Self::Assignment(value) => value.assignment_id,
            Self::Commitment(value) => value.commitment_id,
            Self::Checkpoint(value) => value.checkpoint_id,
            Self::Handoff(value) => value.handoff_id,
        }
    }

    /// Per-entity revision.
    #[must_use]
    pub const fn entity_revision(&self) -> u64 {
        match self {
            Self::Role(value) => value.object_revision,
            Self::Proposal(value) => value.entity_revision,
            Self::Assignment(value) => value.entity_revision,
            Self::Commitment(value) => value.entity_revision,
            Self::Checkpoint(value) => value.entity_revision,
            Self::Handoff(value) => value.entity_revision,
        }
    }
}

/// One signed head referenced by incremental v3 metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "head_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum V3ChangedHead {
    /// Continuity entity, including the unified RoleDefinitionV3 head.
    Entity {
        /// Stable replaceable-event coordinate.
        coordinate: String,
        /// Signed head event.
        event_id: EventId,
        /// Closed entity discriminator.
        entity_type: RoleContinuityEntity,
        /// Per-entity revision.
        entity_revision: u64,
    },
    /// Ordinary object or tombstone head.
    Object {
        /// Stable replaceable-event coordinate.
        coordinate: String,
        /// Signed head event.
        event_id: EventId,
        /// Immutable object type.
        object_type: ProjectViewObjectType,
        /// Per-object revision.
        object_revision: u64,
    },
}

impl V3ChangedHead {
    /// Stable coordinate.
    #[must_use]
    pub fn coordinate(&self) -> &str {
        match self {
            Self::Entity { coordinate, .. } | Self::Object { coordinate, .. } => coordinate,
        }
    }

    /// Signed head event ID.
    #[must_use]
    pub const fn event_id(&self) -> EventId {
        match self {
            Self::Entity { event_id, .. } | Self::Object { event_id, .. } => *event_id,
        }
    }

    /// Per-head revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        match self {
            Self::Entity {
                entity_revision, ..
            } => *entity_revision,
            Self::Object {
                object_revision, ..
            } => *object_revision,
        }
    }
}

/// Verified active or deleted ordinary v3 object head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V3ProjectedObject {
    /// Complete active object; active Roles are forbidden here.
    Active(Box<ProjectViewObjectV3>),
    /// Bodyless tombstone, including Role tombstones.
    Tombstone(ProjectViewTombstoneV3),
}

impl V3ProjectedObject {
    /// Stable object identity.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Active(object) => object.id,
            Self::Tombstone(tombstone) => tombstone.id,
        }
    }

    /// Immutable object type.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::Active(object) => object.object_type,
            Self::Tombstone(tombstone) => tombstone.object_type,
        }
    }

    /// Current object-local revision.
    #[must_use]
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.object_revision,
            Self::Tombstone(tombstone) => tombstone.object_revision,
        }
    }
}

/// Strictly verified v3 ordinary object projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3ProjectObjectProjection {
    /// Signed event ID.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Projection signer generation.
    pub projection_generation: u64,
    /// Project revision represented by the head.
    pub project_revision: u64,
    /// Typed accepted source.
    pub source: V3ProjectionSource,
    /// Complete active object or tombstone.
    pub object: V3ProjectedObject,
    /// Stable Role responsible for an active Work.
    pub responsible_role_id: Option<Uuid>,
    /// Relay canonical head time.
    pub updated_at: DateTime<Utc>,
}

/// Strictly verified v3 continuity entity projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3EntityProjection {
    /// Signed event ID.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Projection signer generation.
    pub projection_generation: u64,
    /// Project revision represented by the head.
    pub project_revision: u64,
    /// Per-entity revision.
    pub entity_revision: u64,
    /// Typed accepted source.
    pub source: V3ProjectionSource,
    /// Complete current entity.
    pub entity: V3EntityChange,
    /// Relay canonical head time.
    pub updated_at: DateTime<Utc>,
}

/// Strictly verified v3 metadata projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V3MetaProjection {
    /// Signed event ID.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Projection signer generation.
    pub projection_generation: u64,
    /// Current Project revision.
    pub project_revision: u64,
    /// Complete inherited materialized counts.
    pub entity_counts: V3EntityCounts,
    /// Exact current NIP-43 membership snapshot.
    pub membership_snapshot_event_id: EventId,
    /// Whether clients must discard older generation heads.
    pub reset: bool,
    /// Incremental changed heads; empty exactly for reset metadata.
    pub changed_heads: Vec<V3ChangedHead>,
    /// Typed accepted source.
    pub source: V3ProjectionSource,
    /// Relay canonical metadata time.
    pub updated_at: DateTime<Utc>,
}

/// Stable v3 continuity entity coordinate.
#[must_use]
pub fn entity_projection_coordinate(
    project_id: CommunityId,
    entity_type: RoleContinuityEntity,
    entity_id: Uuid,
) -> String {
    format!(
        "project-view:{}:{}:{entity_id}",
        project_id.as_uuid(),
        entity_type.as_str()
    )
}

/// Build one Relay-authored ordinary object or tombstone head.
pub fn build_project_object_projection(
    context: &V3ProjectionContext,
    entry: &ProjectViewEntryV3,
    responsible_role_id: Option<Uuid>,
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    if entry.id().is_nil() {
        return Err(SdkError::InvalidInput(
            "v3 Project object ID cannot be nil".to_owned(),
        ));
    }
    if matches!(entry, ProjectViewEntryV3::Active(object) if object.object_type == ProjectViewObjectType::Role)
    {
        return Err(SdkError::InvalidInput(
            "active v3 Roles must use exactly one RoleDefinitionV3 entity head".to_owned(),
        ));
    }
    if let ProjectViewEntryV3::Active(object) = entry {
        validate_projected_object_v3(object)
            .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
        validate_object_identity(context.project_id, object.object_type, object.id)?;
    }
    if responsible_role_id.is_some_and(|role_id| role_id.is_nil())
        || (responsible_role_id.is_some()
            && !matches!(entry, ProjectViewEntryV3::Active(object) if object.object_type == ProjectViewObjectType::Work))
    {
        return Err(SdkError::InvalidInput(
            "responsible_role_id is only valid for one active Work".to_owned(),
        ));
    }
    let coordinate = crate::project_view::object_projection_coordinate(
        context.project_id,
        entry.object_type(),
        entry.id(),
    );
    let (deleted, object, tombstone) = match entry {
        ProjectViewEntryV3::Active(object) => (false, Some(object.as_ref()), None),
        ProjectViewEntryV3::Tombstone(tombstone) => (true, None, Some(tombstone)),
    };
    let content = serde_json::to_string(&ObjectProjectionContent {
        schema_version: PROJECT_VIEW_V3_SCHEMA_VERSION,
        projection_type: "object",
        project_id: *context.project_id.as_uuid(),
        projection_generation: context.projection_generation,
        project_revision: context.project_revision,
        object_revision: entry.object_revision(),
        source: &context.source,
        deleted,
        object,
        tombstone,
        responsible_role_id,
        updated_at: context.updated_at,
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize v3 object: {error}")))?;
    let tags = object_tags(
        &coordinate,
        entry.object_type(),
        context.projection_generation,
        entry.object_revision(),
        context.project_revision,
        &context.source,
    );
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16), content)
            .tags(parse_tags(&tags)?)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Strictly parse one Relay-authored ordinary object or tombstone head.
pub fn parse_project_object_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<V3ProjectObjectProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_OBJECT)?;
    let raw: RawObjectProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v3 object content: {error}")))?;
    if raw.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION || raw.projection_type != "object" {
        return Err(invalid_projection(
            "projection is not a schema-v3 Project object head",
        ));
    }
    if raw.project_id != *expected_project.as_uuid() {
        return Err(invalid_projection("v3 object belongs to another Project"));
    }
    validate_raw_common(
        raw.projection_generation,
        raw.project_revision,
        &raw.source,
        event,
        &raw.updated_at,
    )?;
    require_revision(raw.object_revision, "object_revision")?;
    let object = match (raw.deleted, raw.object, raw.tombstone) {
        (false, Some(object), None) => {
            if object.object_type == ProjectViewObjectType::Role {
                return Err(invalid_projection(
                    "active v3 Role must use a RoleDefinitionV3 entity head",
                ));
            }
            validate_projected_object_v3(&object)
                .map_err(|error| invalid_projection(error.to_string()))?;
            validate_object_identity(expected_project, object.object_type, object.id)?;
            V3ProjectedObject::Active(Box::new(object))
        }
        (true, None, Some(tombstone)) => {
            validate_tombstone(&tombstone, expected_project)?;
            V3ProjectedObject::Tombstone(tombstone)
        }
        _ => return Err(invalid_projection("invalid v3 active/tombstone shape")),
    };
    if object.object_revision() != raw.object_revision {
        return Err(invalid_projection(
            "v3 object revision disagrees with its payload",
        ));
    }
    if raw
        .responsible_role_id
        .is_some_and(|role_id| role_id.is_nil())
        || (raw.responsible_role_id.is_some()
            && !matches!(&object, V3ProjectedObject::Active(object) if object.object_type == ProjectViewObjectType::Work))
    {
        return Err(invalid_projection(
            "responsible_role_id is only valid for one active Work",
        ));
    }
    let coordinate = crate::project_view::object_projection_coordinate(
        expected_project,
        object.object_type(),
        object.id(),
    );
    require_exact_tags(
        event,
        &object_tags(
            &coordinate,
            object.object_type(),
            raw.projection_generation,
            raw.object_revision,
            raw.project_revision,
            &raw.source,
        ),
    )?;
    Ok(V3ProjectObjectProjection {
        event_id: event.id,
        project_id: expected_project,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        source: raw.source,
        object,
        responsible_role_id: raw.responsible_role_id,
        updated_at: parse_canonical_time(raw.updated_at, "updated_at")?,
    })
}

/// Build one Relay-authored v3 continuity entity head.
pub fn build_entity_projection(
    context: &V3ProjectionContext,
    entity: &V3EntityChange,
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    if entity.entity_id().is_nil() {
        return Err(SdkError::InvalidInput(
            "v3 entity ID cannot be nil".to_owned(),
        ));
    }
    if let V3EntityChange::Role(role) = entity {
        role.validate()
            .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    }
    require_revision(entity.entity_revision(), "entity_revision")?;
    let coordinate =
        entity_projection_coordinate(context.project_id, entity.entity_type(), entity.entity_id());
    let content = serde_json::to_string(&EntityProjectionContent {
        schema_version: PROJECT_VIEW_V3_SCHEMA_VERSION,
        projection_type: "entity",
        project_id: *context.project_id.as_uuid(),
        projection_generation: context.projection_generation,
        project_revision: context.project_revision,
        entity_revision: entity.entity_revision(),
        source: &context.source,
        entity_type: entity.entity_type(),
        entity: entity_value(entity)?,
        updated_at: context.updated_at,
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize v3 entity: {error}")))?;
    let tags = entity_tags(
        &coordinate,
        entity.entity_type(),
        context.projection_generation,
        entity.entity_revision(),
        context.project_revision,
        &context.source,
    );
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16), content)
            .tags(parse_tags(&tags)?)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Strictly parse one Relay-authored v3 continuity entity head.
pub fn parse_entity_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<V3EntityProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_OBJECT)?;
    let raw: RawEntityProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v3 entity content: {error}")))?;
    if raw.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION || raw.projection_type != "entity" {
        return Err(invalid_projection(
            "projection is not a schema-v3 entity head",
        ));
    }
    if raw.project_id != *expected_project.as_uuid() {
        return Err(invalid_projection("v3 entity belongs to another Project"));
    }
    validate_raw_common(
        raw.projection_generation,
        raw.project_revision,
        &raw.source,
        event,
        &raw.updated_at,
    )?;
    require_revision(raw.entity_revision, "entity_revision")?;
    let entity = parse_entity(raw.entity_type, raw.entity)?;
    if entity.entity_revision() != raw.entity_revision {
        return Err(invalid_projection(
            "v3 entity revision disagrees with its payload",
        ));
    }
    let coordinate =
        entity_projection_coordinate(expected_project, entity.entity_type(), entity.entity_id());
    require_exact_tags(
        event,
        &entity_tags(
            &coordinate,
            entity.entity_type(),
            raw.projection_generation,
            raw.entity_revision,
            raw.project_revision,
            &raw.source,
        ),
    )?;
    Ok(V3EntityProjection {
        event_id: event.id,
        project_id: expected_project,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        entity_revision: raw.entity_revision,
        source: raw.source,
        entity,
        updated_at: parse_canonical_time(raw.updated_at, "updated_at")?,
    })
}

/// Bind a signed entity projection into one metadata changed-head entry.
pub fn changed_head_for_entity(
    context: &V3ProjectionContext,
    entity: &V3EntityChange,
    event: &Event,
) -> Result<V3ChangedHead, SdkError> {
    let parsed = parse_entity_projection(event, &event.pubkey, context.project_id)?;
    if parsed.projection_generation != context.projection_generation
        || parsed.project_revision != context.project_revision
        || parsed.source != context.source
        || parsed.entity != *entity
    {
        return Err(SdkError::InvalidInput(
            "signed entity does not match its v3 projection context".to_owned(),
        ));
    }
    Ok(V3ChangedHead::Entity {
        coordinate: entity_projection_coordinate(
            context.project_id,
            entity.entity_type(),
            entity.entity_id(),
        ),
        event_id: event.id,
        entity_type: entity.entity_type(),
        entity_revision: entity.entity_revision(),
    })
}

/// Bind a signed ordinary object projection into metadata.
pub fn changed_head_for_project_object(
    context: &V3ProjectionContext,
    entry: &ProjectViewEntryV3,
    event: &Event,
) -> Result<V3ChangedHead, SdkError> {
    let parsed = parse_project_object_projection(event, &event.pubkey, context.project_id)?;
    if parsed.projection_generation != context.projection_generation
        || parsed.project_revision != context.project_revision
        || parsed.source != context.source
        || parsed.object.id() != entry.id()
        || parsed.object.object_revision() != entry.object_revision()
    {
        return Err(SdkError::InvalidInput(
            "signed object does not match its v3 projection context".to_owned(),
        ));
    }
    Ok(V3ChangedHead::Object {
        coordinate: crate::project_view::object_projection_coordinate(
            context.project_id,
            entry.object_type(),
            entry.id(),
        ),
        event_id: event.id,
        object_type: entry.object_type(),
        object_revision: entry.object_revision(),
    })
}

/// Build one Relay-authored v3 metadata head.
pub fn build_meta_projection(
    context: &V3ProjectionContext,
    counts: V3EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: &[V3ChangedHead],
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    if reset != changed_heads.is_empty() {
        return Err(SdkError::InvalidInput(
            "v3 reset metadata must have no changed heads and incremental metadata must have at least one"
                .to_owned(),
        ));
    }
    validate_changed_heads(context.project_id, changed_heads)?;
    let coordinate = crate::project_view::meta_projection_coordinate(context.project_id);
    let content = serde_json::to_string(&MetaProjectionContent {
        schema_version: PROJECT_VIEW_V3_SCHEMA_VERSION,
        projection_type: "meta",
        project_id: *context.project_id.as_uuid(),
        initialized: true,
        projection_generation: context.projection_generation,
        project_revision: context.project_revision,
        entity_counts: counts,
        membership_snapshot_event_id,
        reset,
        changed_heads,
        source: &context.source,
        updated_at: context.updated_at,
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize v3 metadata: {error}")))?;
    let tags = meta_tags(
        &coordinate,
        context.projection_generation,
        context.project_revision,
        &context.source,
        membership_snapshot_event_id,
    );
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_META as u16), content)
            .tags(parse_tags(&tags)?)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Strictly parse one Relay-authored v3 metadata head.
pub fn parse_meta_projection(
    event: &Event,
    expected_relay: &PublicKey,
) -> Result<V3MetaProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_META)?;
    let raw: RawMetaProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v3 metadata content: {error}")))?;
    if raw.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION
        || raw.projection_type != "meta"
        || !raw.initialized
    {
        return Err(invalid_projection(
            "projection is not initialized schema-v3 metadata",
        ));
    }
    validate_raw_common(
        raw.projection_generation,
        raw.project_revision,
        &raw.source,
        event,
        &raw.updated_at,
    )?;
    if raw.reset != raw.changed_heads.is_empty() {
        return Err(invalid_projection(
            "v3 metadata reset/changed-head shape is invalid",
        ));
    }
    let project_id = CommunityId::from_uuid(raw.project_id);
    validate_changed_heads(project_id, &raw.changed_heads)?;
    let coordinate = crate::project_view::meta_projection_coordinate(project_id);
    require_exact_tags(
        event,
        &meta_tags(
            &coordinate,
            raw.projection_generation,
            raw.project_revision,
            &raw.source,
            raw.membership_snapshot_event_id,
        ),
    )?;
    Ok(V3MetaProjection {
        event_id: event.id,
        project_id,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        entity_counts: raw.entity_counts,
        membership_snapshot_event_id: raw.membership_snapshot_event_id,
        reset: raw.reset,
        changed_heads: raw.changed_heads,
        source: raw.source,
        updated_at: parse_canonical_time(raw.updated_at, "updated_at")?,
    })
}

#[derive(Serialize)]
struct ObjectProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    source: &'a V3ProjectionSource,
    deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    object: Option<&'a ProjectViewObjectV3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tombstone: Option<&'a ProjectViewTombstoneV3>,
    #[serde(skip_serializing_if = "Option::is_none")]
    responsible_role_id: Option<Uuid>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawObjectProjection {
    schema_version: u16,
    projection_type: String,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    source: V3ProjectionSource,
    deleted: bool,
    #[serde(default)]
    object: Option<ProjectViewObjectV3>,
    #[serde(default)]
    tombstone: Option<ProjectViewTombstoneV3>,
    #[serde(default)]
    responsible_role_id: Option<Uuid>,
    updated_at: String,
}

#[derive(Serialize)]
struct EntityProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    entity_revision: u64,
    source: &'a V3ProjectionSource,
    entity_type: RoleContinuityEntity,
    entity: serde_json::Value,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEntityProjection {
    schema_version: u16,
    projection_type: String,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    entity_revision: u64,
    source: V3ProjectionSource,
    entity_type: RoleContinuityEntity,
    entity: serde_json::Value,
    updated_at: String,
}

#[derive(Serialize)]
struct MetaProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    initialized: bool,
    projection_generation: u64,
    project_revision: u64,
    entity_counts: V3EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: &'a [V3ChangedHead],
    source: &'a V3ProjectionSource,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMetaProjection {
    schema_version: u16,
    projection_type: String,
    project_id: Uuid,
    initialized: bool,
    projection_generation: u64,
    project_revision: u64,
    entity_counts: V3EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: Vec<V3ChangedHead>,
    source: V3ProjectionSource,
    updated_at: String,
}

fn entity_value(entity: &V3EntityChange) -> Result<serde_json::Value, SdkError> {
    let value = match entity {
        V3EntityChange::Role(value) => serde_json::to_value(value),
        V3EntityChange::Proposal(value) => serde_json::to_value(value),
        V3EntityChange::Assignment(value) => serde_json::to_value(value),
        V3EntityChange::Commitment(value) => serde_json::to_value(value),
        V3EntityChange::Checkpoint(value) => serde_json::to_value(value),
        V3EntityChange::Handoff(value) => serde_json::to_value(value),
    }
    .map_err(|error| SdkError::InvalidInput(format!("serialize v3 entity: {error}")))?;
    Ok(value)
}

fn parse_entity(
    entity_type: RoleContinuityEntity,
    value: serde_json::Value,
) -> Result<V3EntityChange, SdkError> {
    let parsed = match entity_type {
        RoleContinuityEntity::Role => {
            serde_json::from_value::<RoleDefinitionV3>(value).map(V3EntityChange::Role)
        }
        RoleContinuityEntity::RoleAssignmentProposal => {
            serde_json::from_value::<RoleAssignmentProposal>(value).map(V3EntityChange::Proposal)
        }
        RoleContinuityEntity::RoleAssignment => {
            serde_json::from_value::<RoleAssignment>(value).map(V3EntityChange::Assignment)
        }
        RoleContinuityEntity::WorkCommitment => {
            serde_json::from_value::<WorkCommitment>(value).map(V3EntityChange::Commitment)
        }
        RoleContinuityEntity::RoleCheckpoint => {
            serde_json::from_value::<RoleCheckpoint>(value).map(V3EntityChange::Checkpoint)
        }
        RoleContinuityEntity::RoleHandoff => {
            serde_json::from_value::<RoleHandoff>(value).map(V3EntityChange::Handoff)
        }
    }
    .map_err(|error| invalid_projection(format!("invalid typed v3 entity: {error}")))?;
    if let V3EntityChange::Role(role) = &parsed {
        role.validate()
            .map_err(|error| invalid_projection(error.to_string()))?;
    }
    Ok(parsed)
}

fn validate_context(context: &V3ProjectionContext) -> Result<(), SdkError> {
    require_revision(context.projection_generation, "projection_generation")?;
    require_revision(context.project_revision, "project_revision")?;
    validate_source(&context.source)
}

fn validate_source(source: &V3ProjectionSource) -> Result<(), SdkError> {
    match source {
        V3ProjectionSource::NostrEvent {
            change_id,
            event_id,
        } if change_id != event_id => Err(SdkError::InvalidInput(
            "Nostr source change_id must equal event_id".to_owned(),
        )),
        V3ProjectionSource::Operator { audit_seq, .. }
        | V3ProjectionSource::System { audit_seq, .. }
            if *audit_seq == 0 =>
        {
            Err(SdkError::InvalidInput(
                "audited projection source sequence must be positive".to_owned(),
            ))
        }
        _ => Ok(()),
    }
}

fn validate_raw_common(
    generation: u64,
    project_revision: u64,
    source: &V3ProjectionSource,
    event: &Event,
    updated_at: &str,
) -> Result<(), SdkError> {
    require_revision(generation, "projection_generation")?;
    require_revision(project_revision, "project_revision")?;
    validate_source(source).map_err(|error| invalid_projection(error.to_string()))?;
    let updated_at = parse_canonical_time(updated_at.to_owned(), "updated_at")?;
    require_event_time(event, updated_at)
}

fn validate_changed_heads(
    project_id: CommunityId,
    heads: &[V3ChangedHead],
) -> Result<(), SdkError> {
    let mut coordinates = HashSet::with_capacity(heads.len());
    for head in heads {
        require_revision(head.revision(), "changed_heads.revision")?;
        let expected = match head {
            V3ChangedHead::Entity {
                coordinate,
                entity_type,
                ..
            } => entity_projection_coordinate(
                project_id,
                *entity_type,
                parse_coordinate_id(coordinate, project_id, entity_type.as_str())?,
            ),
            V3ChangedHead::Object {
                coordinate,
                object_type,
                ..
            } => crate::project_view::object_projection_coordinate(
                project_id,
                *object_type,
                parse_coordinate_id(coordinate, project_id, object_type.as_str())?,
            ),
        };
        if head.coordinate() != expected || !coordinates.insert(head.coordinate()) {
            return Err(invalid_projection(
                "v3 metadata changed-head coordinate is invalid or duplicated",
            ));
        }
    }
    Ok(())
}

fn entity_tags(
    coordinate: &str,
    entity_type: RoleContinuityEntity,
    generation: u64,
    entity_revision: u64,
    project_revision: u64,
    source: &V3ProjectionSource,
) -> Vec<Vec<String>> {
    projection_tags(
        coordinate,
        PROJECT_VIEW_V3_ENTITY_TAG,
        entity_type.as_str(),
        generation,
        entity_revision,
        project_revision,
        source,
    )
}

fn object_tags(
    coordinate: &str,
    object_type: ProjectViewObjectType,
    generation: u64,
    object_revision: u64,
    project_revision: u64,
    source: &V3ProjectionSource,
) -> Vec<Vec<String>> {
    projection_tags(
        coordinate,
        PROJECT_VIEW_V3_OBJECT_TAG,
        object_type.as_str(),
        generation,
        object_revision,
        project_revision,
        source,
    )
}

fn projection_tags(
    coordinate: &str,
    subtype: &str,
    entity_type: &str,
    generation: u64,
    revision: u64,
    project_revision: u64,
    source: &V3ProjectionSource,
) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), subtype.to_owned()],
        vec!["type".to_owned(), entity_type.to_owned()],
        vec!["projection_generation".to_owned(), generation.to_string()],
        vec!["revision".to_owned(), revision.to_string()],
        vec!["project_revision".to_owned(), project_revision.to_string()],
        vec!["change".to_owned(), source.change_id().to_hex()],
        vec!["source_type".to_owned(), source.source_type().to_owned()],
    ];
    push_source_event(&mut tags, source);
    tags
}

fn meta_tags(
    coordinate: &str,
    generation: u64,
    project_revision: u64,
    source: &V3ProjectionSource,
    membership: EventId,
) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_META_TAG.to_owned()],
        vec!["projection_generation".to_owned(), generation.to_string()],
        vec!["project_revision".to_owned(), project_revision.to_string()],
        vec!["change".to_owned(), source.change_id().to_hex()],
        vec!["source_type".to_owned(), source.source_type().to_owned()],
        vec!["membership".to_owned(), membership.to_hex()],
    ];
    push_source_event(&mut tags, source);
    tags
}

fn push_source_event(tags: &mut Vec<Vec<String>>, source: &V3ProjectionSource) {
    if let Some(event_id) = source.source_event_id() {
        tags.push(vec![
            "e".to_owned(),
            event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
}

fn parse_coordinate_id(
    coordinate: &str,
    project_id: CommunityId,
    kind: &str,
) -> Result<Uuid, SdkError> {
    let parts = coordinate.split(':').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != "project-view"
        || parts[1] != project_id.as_uuid().to_string()
        || parts[2] != kind
    {
        return Err(invalid_projection("invalid v3 projection coordinate"));
    }
    let id = Uuid::parse_str(parts[3])
        .map_err(|error| invalid_projection(format!("invalid coordinate UUID: {error}")))?;
    if id.is_nil() || id.to_string() != parts[3] {
        return Err(invalid_projection(
            "v3 projection coordinate UUID is not canonical",
        ));
    }
    Ok(id)
}

fn validate_tombstone(
    tombstone: &ProjectViewTombstoneV3,
    project_id: CommunityId,
) -> Result<(), SdkError> {
    if tombstone.id.is_nil()
        || tombstone.object_revision == 0
        || tombstone.object_revision > MAX_SAFE_REVISION
        || tombstone.project_revision == 0
        || tombstone.project_revision > MAX_SAFE_REVISION
        || tombstone.deleted_at < tombstone.created_at
        || tombstone.object_type == ProjectViewObjectType::ProjectProfile
        || tombstone.id == *project_id.as_uuid()
    {
        return Err(invalid_projection("invalid v3 Project object tombstone"));
    }
    Ok(())
}

fn validate_object_identity(
    project_id: CommunityId,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
) -> Result<(), SdkError> {
    if object_id.is_nil()
        || (object_type == ProjectViewObjectType::ProjectProfile
            && object_id != *project_id.as_uuid())
        || (object_type != ProjectViewObjectType::ProjectProfile
            && object_id == *project_id.as_uuid())
    {
        return Err(invalid_projection(
            "v3 Project object identity does not match its Project",
        ));
    }
    Ok(())
}

fn verify_envelope(
    event: &Event,
    expected_relay: &PublicKey,
    expected_kind: u32,
) -> Result<(), SdkError> {
    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay || event.kind.as_u16() as u32 != expected_kind {
        return Err(invalid_projection(
            "projection signer or event kind does not match",
        ));
    }
    Ok(())
}

fn require_exact_tags(event: &Event, expected: &[Vec<String>]) -> Result<(), SdkError> {
    let actual = event.tags.iter().map(Tag::as_slice).collect::<Vec<_>>();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(invalid_projection(
            "projection tags are not the exact canonical sequence",
        ));
    }
    Ok(())
}

fn parse_tags(tags: &[Vec<String>]) -> Result<Vec<Tag>, SdkError> {
    tags.iter()
        .map(|parts| {
            Tag::parse(parts.clone()).map_err(|error| SdkError::InvalidTag(error.to_string()))
        })
        .collect()
}

fn require_revision(value: u64, field: &str) -> Result<(), SdkError> {
    if value == 0 || value > MAX_SAFE_REVISION {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be in 1..={MAX_SAFE_REVISION}"
        )));
    }
    Ok(())
}

fn parse_canonical_time(value: String, field: &str) -> Result<DateTime<Utc>, SdkError> {
    let parsed = DateTime::parse_from_rfc3339(&value)
        .map_err(|error| invalid_projection(format!("invalid {field}: {error}")))?
        .with_timezone(&Utc);
    if parsed.to_rfc3339_opts(SecondsFormat::AutoSi, true) != value {
        return Err(invalid_projection(format!("{field} is not canonical UTC")));
    }
    Ok(parsed)
}

fn require_event_time(event: &Event, value: DateTime<Utc>) -> Result<(), SdkError> {
    let seconds = u64::try_from(value.timestamp())
        .map_err(|_| invalid_projection("projection time precedes the Unix epoch"))?;
    if event.created_at.as_secs() != seconds {
        return Err(invalid_projection(
            "projection event time disagrees with content",
        ));
    }
    Ok(())
}

fn tag<const N: usize>(parts: [&str; N]) -> Result<Tag, SdkError> {
    Tag::parse(parts).map_err(|error| SdkError::InvalidTag(error.to_string()))
}

fn timestamp(value: DateTime<Utc>) -> Result<Timestamp, SdkError> {
    let seconds = u64::try_from(value.timestamp()).map_err(|_| {
        SdkError::InvalidInput("Project View timestamp precedes the Unix epoch".to_owned())
    })?;
    Ok(Timestamp::from(seconds))
}

fn invalid_projection(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}

#[cfg(test)]
mod tests {
    use buzz_core::Keys;
    use buzz_project_view::v2::RoleLevel;
    use buzz_project_view::v3::{ProjectContextReference, ProjectViewObjectDataV3};
    use buzz_project_view::{ProjectRole, ProjectViewRelations};
    use chrono::Duration;

    use super::*;

    #[test]
    fn role_definition_has_one_strict_v3_entity_head() {
        let relay = Keys::generate();
        let actor = Keys::generate().public_key();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp");
        let role = RoleDefinitionV3 {
            role_id: Uuid::new_v4(),
            name: "Maintainer".to_owned(),
            purpose: "Keep releases safe".to_owned(),
            responsibilities: vec!["Review".to_owned()],
            boundaries: vec!["No bypass".to_owned()],
            level: RoleLevel::Admin,
            active: false,
            context_references: Vec::new(),
            object_revision: 2,
            project_revision: 7,
            created_at: now - Duration::seconds(5),
            updated_at: now,
            created_by: actor,
            updated_by: actor,
        };
        let source_id = EventId::all_zeros();
        let context = V3ProjectionContext {
            project_id,
            projection_generation: 3,
            project_revision: 7,
            source: V3ProjectionSource::NostrEvent {
                change_id: source_id,
                event_id: source_id,
            },
            updated_at: now,
        };
        let entity = V3EntityChange::Role(role);
        let event = build_entity_projection(&context, &entity)
            .expect("builder")
            .sign_with_keys(&relay)
            .expect("signed");
        let parsed =
            parse_entity_projection(&event, &relay.public_key(), project_id).expect("strict parse");
        assert_eq!(parsed.entity, entity);

        let active_role = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
            id: entity.entity_id(),
            object_type: ProjectViewObjectType::Role,
            object_revision: 2,
            project_revision: 7,
            created_at: now,
            updated_at: now,
            created_by: actor,
            updated_by: actor,
            data: ProjectViewObjectDataV3::Role(ProjectRole {
                name: "Maintainer".to_owned(),
                purpose: "Keep releases safe".to_owned(),
                responsibilities: Vec::new(),
                boundaries: Vec::new(),
                active: false,
            }),
            relations: ProjectViewRelations::default(),
            context_references: Vec::<ProjectContextReference>::new(),
        }));
        assert!(build_project_object_projection(&context, &active_role, None).is_err());
    }

    #[test]
    fn v2_projection_fails_closed_in_v3_parser() {
        let relay = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp");
        let content = serde_json::json!({
            "schema_version": 2,
            "projection_type": "meta",
            "project_id": project_id.as_uuid(),
            "initialized": true,
            "projection_generation": 1,
            "project_revision": 1,
            "entity_counts": {
                "active_objects": 1,
                "open_proposals": 0,
                "active_assignments": 0,
                "active_commitments": 0,
                "checkpoints": 0,
                "handoffs": 0
            },
            "membership_snapshot_event_id": EventId::all_zeros(),
            "reset": true,
            "changed_heads": [],
            "source": {
                "source_type": "system",
                "change_id": EventId::all_zeros(),
                "audit_seq": 1
            },
            "updated_at": now
        })
        .to_string();
        let event = EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_META as u16), content)
            .tags([])
            .custom_created_at(Timestamp::from(now.timestamp() as u64))
            .sign_with_keys(&relay)
            .expect("signed");
        assert!(parse_meta_projection(&event, &relay.public_key()).is_err());
    }
}
