//! Project View v2 Role continuity command and Relay projection wire format.

use std::collections::{BTreeSet, HashSet};

use buzz_core::kind::{
    KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION,
    KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::v2::{
    CommunityMemberRole, RoleAssignment, RoleAssignmentProposal, RoleCommand, RoleContinuityChange,
    RoleContinuityEntity, RoleDefinition, RoleHandoff, SchemaVersion,
};
use buzz_project_view::{
    validate_projected_object, ProjectViewEntry, ProjectViewObject, ProjectViewObjectType,
    ProjectViewTombstone, MAX_SAFE_REVISION,
};
use chrono::{DateTime, SecondsFormat, Utc};
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::SdkError;

const PROJECT_VIEW_TAG: &str = "buzz-project-view";
const PROJECT_VIEW_V2_ENTITY_TAG: &str = "buzz-project-view-v2-entity";
const PROJECT_VIEW_V2_OBJECT_TAG: &str = "buzz-project-view-v2-object";
const PROJECT_VIEW_META_TAG: &str = "buzz-project-view-meta";
const PROJECT_VIEW_MUTATION_TAG: &str = "buzz-project-view-mutation";

/// Build a protected kind `44300` Role command.
pub fn build_role_command(command: RoleCommand) -> Result<EventBuilder, SdkError> {
    command
        .validate_for_submission()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = serde_json::to_string(&command)
        .map_err(|error| SdkError::InvalidInput(format!("serialize Role command: {error}")))?;
    RoleCommand::from_json(&content).map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_MUTATION as u16), content)
            .tags([tag(["-"])?, tag(["t", PROJECT_VIEW_MUTATION_TAG])?]),
    )
}

/// Typed source carried by every v2 entity and metadata head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum V2ProjectionSource {
    /// Verified member-signed Nostr command.
    NostrEvent {
        /// Stable change ID; equal to the source event ID.
        change_id: EventId,
        /// Verified source command.
        event_id: EventId,
    },
    /// Operator action tied to the Community audit chain.
    Operator {
        /// Domain-separated stable change ID.
        change_id: EventId,
        /// Referenced Community audit sequence.
        audit_seq: u64,
    },
    /// Trusted system action tied to the Community audit chain.
    System {
        /// Domain-separated stable change ID.
        change_id: EventId,
        /// Referenced Community audit sequence.
        audit_seq: u64,
    },
}

impl V2ProjectionSource {
    /// Stable source spelling.
    #[must_use]
    pub const fn source_type(&self) -> &'static str {
        match self {
            Self::NostrEvent { .. } => "nostr_event",
            Self::Operator { .. } => "operator",
            Self::System { .. } => "system",
        }
    }

    /// Stable change identifier.
    #[must_use]
    pub const fn change_id(&self) -> EventId {
        match self {
            Self::NostrEvent { change_id, .. }
            | Self::Operator { change_id, .. }
            | Self::System { change_id, .. } => *change_id,
        }
    }

    /// Source event when this change came from Nostr.
    #[must_use]
    pub const fn source_event_id(&self) -> Option<EventId> {
        match self {
            Self::NostrEvent { event_id, .. } => Some(*event_id),
            Self::Operator { .. } | Self::System { .. } => None,
        }
    }
}

/// Shared immutable inputs for all heads emitted by one accepted v2 change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2ProjectionContext {
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Current projection signer generation.
    pub projection_generation: u64,
    /// New project revision.
    pub project_revision: u64,
    /// Typed accepted change source.
    pub source: V2ProjectionSource,
    /// Relay canonical time.
    pub updated_at: DateTime<Utc>,
}

/// Materialized entity counts carried by v2 metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V2EntityCounts {
    /// Active Project View objects.
    pub active_objects: u32,
    /// Durably open Proposals.
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

/// One signed v2 entity head referenced by metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V2ChangedHead {
    /// Stable entity coordinate.
    pub coordinate: String,
    /// Signed head event ID.
    pub event_id: EventId,
    /// Entity discriminator.
    pub entity_type: RoleContinuityEntity,
    /// Per-entity revision.
    pub entity_revision: u64,
}

/// Verified v2 continuity entity projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2EntityProjection {
    /// Signed projection event ID.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Signer generation.
    pub projection_generation: u64,
    /// Project revision.
    pub project_revision: u64,
    /// Per-entity revision.
    pub entity_revision: u64,
    /// Typed change source.
    pub source: V2ProjectionSource,
    /// Complete current entity head.
    pub entity: RoleContinuityChange,
    /// Relay canonical time at which this head was emitted.
    pub updated_at: DateTime<Utc>,
}

/// Minimal deleted Project View object carried by a v2 reset head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct V2ObjectTombstone {
    /// Stable object identifier.
    pub object_id: Uuid,
    /// Immutable object type.
    pub object_type: ProjectViewObjectType,
    /// Object revision assigned to deletion.
    pub object_revision: u64,
    /// Project revision assigned to deletion.
    pub project_revision: u64,
    /// Original creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical deletion time.
    pub deleted_at: DateTime<Utc>,
    /// Original creator.
    pub created_by: PublicKey,
    /// Verified deleting actor.
    pub deleted_by: PublicKey,
}

impl From<&ProjectViewTombstone> for V2ObjectTombstone {
    fn from(value: &ProjectViewTombstone) -> Self {
        Self {
            object_id: value.id,
            object_type: value.object_type,
            object_revision: value.object_revision,
            project_revision: value.project_revision,
            created_at: value.created_at,
            deleted_at: value.deleted_at,
            created_by: value.created_by,
            deleted_by: value.deleted_by,
        }
    }
}

/// Active or deleted non-Role Project View object in a v2 head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum V2ProjectedObject {
    /// Complete active object.
    Active(Box<ProjectViewObject>),
    /// Immutable tombstone.
    Tombstone(V2ObjectTombstone),
}

impl V2ProjectedObject {
    /// Stable object identifier.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Active(object) => object.id,
            Self::Tombstone(tombstone) => tombstone.object_id,
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

    /// Current object revision.
    #[must_use]
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.object_revision,
            Self::Tombstone(tombstone) => tombstone.object_revision,
        }
    }
}

/// Verified v2 head for one ordinary Project View object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2ProjectObjectProjection {
    /// Signed event identifier.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Projection signer generation.
    pub projection_generation: u64,
    /// Project revision at which the reset head was emitted.
    pub project_revision: u64,
    /// Typed accepted change source.
    pub source: V2ProjectionSource,
    /// Complete object or tombstone.
    pub object: V2ProjectedObject,
    /// Relay canonical emission time.
    pub updated_at: DateTime<Utc>,
}

/// One verified member row from the exact NIP-43 snapshot referenced by v2
/// metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2MembershipMember {
    /// Stable member identity.
    pub pubkey: PublicKey,
    /// Canonical Community permission level.
    pub role: CommunityMemberRole,
}

/// Verified Relay-authored NIP-43 snapshot bound by a v2 metadata head.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2MembershipProjection {
    /// Signed snapshot event identifier.
    pub event_id: EventId,
    /// Members in canonical public-key order.
    pub members: Vec<V2MembershipMember>,
    /// Whole-second event time.
    pub created_at: Timestamp,
}

/// Verified v2 metadata projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V2MetaProjection {
    /// Signed projection event ID.
    pub event_id: EventId,
    /// Community/Project identity.
    pub project_id: CommunityId,
    /// Signer generation.
    pub projection_generation: u64,
    /// Project revision.
    pub project_revision: u64,
    /// Materialized entity counts.
    pub entity_counts: V2EntityCounts,
    /// Exact NIP-43 snapshot corresponding to canonical membership.
    pub membership_snapshot_event_id: EventId,
    /// Whether clients must discard an older generation.
    pub reset: bool,
    /// Heads changed by this incremental mutation.
    pub changed_heads: Vec<V2ChangedHead>,
    /// Typed accepted change source.
    pub source: V2ProjectionSource,
    /// Relay canonical time.
    pub updated_at: DateTime<Utc>,
}

/// Derive the stable v2 continuity entity coordinate.
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

/// Build one unsigned v2 head for an ordinary Project View object.
///
/// Active Roles use [`build_entity_projection`] because their v2 body contains
/// the governance `level`; this builder handles the other active object types
/// and every historical tombstone during generation reset.
pub fn build_project_object_projection(
    context: &V2ProjectionContext,
    entry: &ProjectViewEntry,
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    if entry.id().is_nil() {
        return Err(SdkError::InvalidInput(
            "v2 Project View object ID cannot be nil".to_owned(),
        ));
    }
    if matches!(entry, ProjectViewEntry::Active(object) if object.object_type == ProjectViewObjectType::Role)
    {
        return Err(SdkError::InvalidInput(
            "active v2 Roles must use the Role entity projection".to_owned(),
        ));
    }
    if let ProjectViewEntry::Active(object) = entry {
        validate_projected_object(object)
            .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
        validate_v2_object_identity(context.project_id, object.object_type, object.id)?;
    }

    let coordinate = crate::project_view::object_projection_coordinate(
        context.project_id,
        entry.object_type(),
        entry.id(),
    );
    let generation = context.projection_generation.to_string();
    let object_revision = entry.object_revision().to_string();
    let project_revision = context.project_revision.to_string();
    let change_id = context.source.change_id().to_hex();
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_VIEW_TAG])?,
        tag(["t", PROJECT_VIEW_V2_OBJECT_TAG])?,
        tag(["type", entry.object_type().as_str()])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["revision", object_revision.as_str()])?,
        tag(["project_revision", project_revision.as_str()])?,
        tag(["change", change_id.as_str()])?,
        tag(["source_type", context.source.source_type()])?,
    ];
    if let Some(event_id) = context.source.source_event_id() {
        let event_id = event_id.to_hex();
        tags.push(tag(["e", event_id.as_str(), "", "source"])?);
    }
    let (deleted, object, tombstone) = match entry {
        ProjectViewEntry::Active(object) => (false, Some(object), None),
        ProjectViewEntry::Tombstone(tombstone) => {
            (true, None, Some(V2ObjectTombstone::from(tombstone)))
        }
    };
    let content = serde_json::to_string(&ProjectObjectProjectionContent {
        schema_version: SchemaVersion::V2.as_u16(),
        projection_type: "object",
        project_id: *context.project_id.as_uuid(),
        projection_generation: context.projection_generation,
        project_revision: context.project_revision,
        object_revision: entry.object_revision(),
        source: &context.source,
        deleted,
        object,
        tombstone,
        updated_at: context.updated_at,
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize v2 Project object: {error}")))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16), content)
            .tags(tags)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Strictly parse and verify one ordinary Project View v2 object head.
pub fn parse_project_object_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<V2ProjectObjectProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_OBJECT)?;
    let raw: RawProjectObjectProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v2 object content: {error}")))?;
    if raw.schema_version != SchemaVersion::V2.as_u16() || raw.projection_type != "object" {
        return Err(invalid_projection(
            "projection is not a schema-v2 Project object head",
        ));
    }
    if raw.project_id != *expected_project.as_uuid() {
        return Err(invalid_projection(
            "v2 object belongs to a different Project",
        ));
    }
    require_revision(raw.projection_generation, "projection_generation")?;
    require_revision(raw.project_revision, "project_revision")?;
    require_revision(raw.object_revision, "object_revision")?;
    validate_source(&raw.source)?;
    let updated_at = canonical_time(raw.updated_at, "updated_at")?;
    require_event_time(event, updated_at)?;

    let object = match (raw.deleted, raw.object, raw.tombstone) {
        (false, Some(object), None) => {
            if object.object_type == ProjectViewObjectType::Role {
                return Err(invalid_projection(
                    "active v2 Role must use a Role entity head",
                ));
            }
            validate_projected_object(&object)
                .map_err(|error| invalid_projection(error.to_string()))?;
            validate_v2_object_identity(expected_project, object.object_type, object.id)?;
            V2ProjectedObject::Active(Box::new(object))
        }
        (true, None, Some(tombstone)) => {
            validate_v2_tombstone(&tombstone, expected_project)?;
            V2ProjectedObject::Tombstone(tombstone)
        }
        _ => {
            return Err(invalid_projection(
                "v2 object active/tombstone shape is invalid",
            ));
        }
    };
    if object.object_revision() != raw.object_revision {
        return Err(invalid_projection(
            "v2 object revision disagrees with its payload",
        ));
    }
    let coordinate = crate::project_view::object_projection_coordinate(
        expected_project,
        object.object_type(),
        object.id(),
    );
    let expected_tags = project_object_tags(
        &coordinate,
        object.object_type(),
        raw.projection_generation,
        raw.object_revision,
        raw.project_revision,
        &raw.source,
    );
    require_exact_tags(event, &expected_tags)?;
    Ok(V2ProjectObjectProjection {
        event_id: event.id,
        project_id: expected_project,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        source: raw.source,
        object,
        updated_at,
    })
}

/// Strictly parse and verify the NIP-43 membership snapshot referenced by v2
/// metadata.
pub fn parse_membership_projection(
    event: &Event,
    expected_relay: &PublicKey,
) -> Result<V2MembershipProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_NIP43_MEMBERSHIP_LIST)?;
    if !event.content.is_empty() {
        return Err(invalid_projection(
            "v2 membership snapshot content must be empty",
        ));
    }
    let tags = event.tags.iter().map(Tag::as_slice).collect::<Vec<_>>();
    if tags.first().copied() != Some(["-".to_owned()].as_slice()) {
        return Err(invalid_projection(
            "v2 membership snapshot must begin with one protection tag",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut members = Vec::with_capacity(tags.len().saturating_sub(1));
    for tag in tags.iter().skip(1) {
        if tag.len() != 3 || tag.first().map(String::as_str) != Some("member") {
            return Err(invalid_projection(
                "v2 membership snapshot contains a non-canonical tag",
            ));
        }
        let pubkey_text = tag
            .get(1)
            .ok_or_else(|| invalid_projection("v2 membership snapshot member pubkey is missing"))?;
        let pubkey = PublicKey::from_hex(pubkey_text).map_err(|error| {
            invalid_projection(format!("invalid membership public key: {error}"))
        })?;
        if pubkey.to_hex() != *pubkey_text || !seen.insert(pubkey) {
            return Err(invalid_projection(
                "v2 membership snapshot pubkeys must be unique canonical lowercase hex",
            ));
        }
        let role = match tag.get(2).map(String::as_str) {
            Some("owner") => CommunityMemberRole::Owner,
            Some("admin") => CommunityMemberRole::Admin,
            Some("member") => CommunityMemberRole::Member,
            _ => {
                return Err(invalid_projection(
                    "v2 membership snapshot contains an invalid Community role",
                ));
            }
        };
        members.push(V2MembershipMember { pubkey, role });
    }
    if members
        .windows(2)
        .any(|window| window[0].pubkey >= window[1].pubkey)
    {
        return Err(invalid_projection(
            "v2 membership snapshot is not in canonical public-key order",
        ));
    }
    if members
        .iter()
        .filter(|member| member.role == CommunityMemberRole::Owner)
        .count()
        != 1
    {
        return Err(invalid_projection(
            "v2 membership snapshot must contain exactly one owner",
        ));
    }
    Ok(V2MembershipProjection {
        event_id: event.id,
        members,
        created_at: event.created_at,
    })
}

/// Build one unsigned Relay-authored v2 continuity entity head.
pub fn build_entity_projection(
    context: &V2ProjectionContext,
    entity: &RoleContinuityChange,
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    let entity_id = entity.entity_id();
    if entity_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "v2 projection entity ID cannot be nil".to_owned(),
        ));
    }
    let coordinate =
        entity_projection_coordinate(context.project_id, entity.entity_type(), entity_id);
    let generation = context.projection_generation.to_string();
    let entity_revision = entity.entity_revision().to_string();
    let project_revision = context.project_revision.to_string();
    let change_id = context.source.change_id().to_hex();
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_VIEW_TAG])?,
        tag(["t", PROJECT_VIEW_V2_ENTITY_TAG])?,
        tag(["type", entity.entity_type().as_str()])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["revision", entity_revision.as_str()])?,
        tag(["project_revision", project_revision.as_str()])?,
        tag(["change", change_id.as_str()])?,
        tag(["source_type", context.source.source_type()])?,
    ];
    if let Some(event_id) = context.source.source_event_id() {
        let event_id = event_id.to_hex();
        tags.push(tag(["e", event_id.as_str(), "", "source"])?);
    }
    let (entity_type, entity_value) = entity_parts(entity)?;
    let content = serde_json::to_string(&EntityProjectionContent {
        schema_version: SchemaVersion::V2.as_u16(),
        projection_type: "entity",
        project_id: *context.project_id.as_uuid(),
        projection_generation: context.projection_generation,
        project_revision: context.project_revision,
        entity_revision: entity.entity_revision(),
        source: &context.source,
        entity_type,
        entity: entity_value,
        updated_at: context.updated_at,
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize v2 entity: {error}")))?;

    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16), content)
            .tags(tags)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Bind a signed entity event into a v2 metadata changed-head entry.
pub fn changed_head_for(
    context: &V2ProjectionContext,
    entity: &RoleContinuityChange,
    event: &Event,
) -> Result<V2ChangedHead, SdkError> {
    if event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_OBJECT {
        return Err(SdkError::InvalidInput(
            "v2 changed head has the wrong event kind".to_owned(),
        ));
    }
    Ok(V2ChangedHead {
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

/// Build unsigned v2 metadata after every changed entity has been signed.
pub fn build_meta_projection(
    context: &V2ProjectionContext,
    counts: V2EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: &[V2ChangedHead],
) -> Result<EventBuilder, SdkError> {
    validate_context(context)?;
    if reset && !changed_heads.is_empty() {
        return Err(SdkError::InvalidInput(
            "reset v2 metadata cannot carry changed heads".to_owned(),
        ));
    }
    if !reset && changed_heads.is_empty() {
        return Err(SdkError::InvalidInput(
            "incremental v2 metadata requires changed heads".to_owned(),
        ));
    }
    let mut coordinates = HashSet::with_capacity(changed_heads.len());
    if changed_heads
        .iter()
        .any(|head| !coordinates.insert(head.coordinate.as_str()))
    {
        return Err(SdkError::InvalidInput(
            "v2 metadata contains duplicate changed heads".to_owned(),
        ));
    }
    let coordinate = crate::project_view::meta_projection_coordinate(context.project_id);
    let generation = context.projection_generation.to_string();
    let project_revision = context.project_revision.to_string();
    let change_id = context.source.change_id().to_hex();
    let membership_id = membership_snapshot_event_id.to_hex();
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_VIEW_TAG])?,
        tag(["t", PROJECT_VIEW_META_TAG])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["project_revision", project_revision.as_str()])?,
        tag(["change", change_id.as_str()])?,
        tag(["source_type", context.source.source_type()])?,
        tag(["membership", membership_id.as_str()])?,
    ];
    if let Some(event_id) = context.source.source_event_id() {
        let event_id = event_id.to_hex();
        tags.push(tag(["e", event_id.as_str(), "", "source"])?);
    }
    let content = serde_json::to_string(&MetaProjectionContent {
        schema_version: SchemaVersion::V2.as_u16(),
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
    .map_err(|error| SdkError::InvalidInput(format!("serialize v2 metadata: {error}")))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_META as u16), content)
            .tags(tags)
            .custom_created_at(timestamp(context.updated_at)?),
    )
}

/// Strictly parse and verify one v2 continuity entity projection.
pub fn parse_entity_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<V2EntityProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_OBJECT)?;
    let raw: RawEntityProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v2 entity content: {error}")))?;
    if raw.schema_version != SchemaVersion::V2.as_u16() || raw.projection_type != "entity" {
        return Err(invalid_projection(
            "projection is not a schema-v2 entity head",
        ));
    }
    if raw.project_id != *expected_project.as_uuid() {
        return Err(invalid_projection(
            "v2 entity belongs to a different Project",
        ));
    }
    require_revision(raw.projection_generation, "projection_generation")?;
    require_revision(raw.project_revision, "project_revision")?;
    require_revision(raw.entity_revision, "entity_revision")?;
    validate_source(&raw.source)?;
    let updated_at = canonical_time(raw.updated_at, "updated_at")?;
    require_event_time(event, updated_at)?;
    let entity = parse_entity(raw.entity_type, raw.entity)?;
    if entity.entity_revision() != raw.entity_revision {
        return Err(invalid_projection(
            "v2 entity revision disagrees with its payload",
        ));
    }
    let coordinate =
        entity_projection_coordinate(expected_project, entity.entity_type(), entity.entity_id());
    let expected_tags = projection_tags(
        &coordinate,
        entity.entity_type(),
        raw.projection_generation,
        raw.entity_revision,
        raw.project_revision,
        &raw.source,
    );
    require_exact_tags(event, &expected_tags)?;
    Ok(V2EntityProjection {
        event_id: event.id,
        project_id: expected_project,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        entity_revision: raw.entity_revision,
        source: raw.source,
        entity,
        updated_at,
    })
}

/// Strictly parse and verify one v2 metadata projection.
pub fn parse_meta_projection(
    event: &Event,
    expected_relay: &PublicKey,
) -> Result<V2MetaProjection, SdkError> {
    verify_envelope(event, expected_relay, KIND_PROJECT_VIEW_META)?;
    let raw: RawMetaProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid v2 metadata content: {error}")))?;
    if raw.schema_version != SchemaVersion::V2.as_u16()
        || raw.projection_type != "meta"
        || !raw.initialized
    {
        return Err(invalid_projection(
            "projection is not initialized schema-v2 metadata",
        ));
    }
    require_revision(raw.projection_generation, "projection_generation")?;
    require_revision(raw.project_revision, "project_revision")?;
    validate_source(&raw.source)?;
    let updated_at = canonical_time(raw.updated_at, "updated_at")?;
    require_event_time(event, updated_at)?;
    if raw.reset != raw.changed_heads.is_empty() {
        return Err(invalid_projection(
            "v2 metadata reset/changed-head shape is invalid",
        ));
    }
    let project_id = CommunityId::from_uuid(raw.project_id);
    let mut coordinates = HashSet::with_capacity(raw.changed_heads.len());
    for head in &raw.changed_heads {
        require_revision(head.entity_revision, "changed_heads.entity_revision")?;
        let expected = entity_projection_coordinate(
            project_id,
            head.entity_type,
            parse_coordinate_id(&head.coordinate, project_id, head.entity_type)?,
        );
        if head.coordinate != expected || !coordinates.insert(head.coordinate.as_str()) {
            return Err(invalid_projection(
                "v2 metadata changed-head coordinate is invalid or duplicated",
            ));
        }
    }
    let coordinate = crate::project_view::meta_projection_coordinate(project_id);
    let expected_tags = meta_tags(
        &coordinate,
        raw.projection_generation,
        raw.project_revision,
        &raw.source,
        raw.membership_snapshot_event_id,
    );
    require_exact_tags(event, &expected_tags)?;
    Ok(V2MetaProjection {
        event_id: event.id,
        project_id,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        entity_counts: raw.entity_counts,
        membership_snapshot_event_id: raw.membership_snapshot_event_id,
        reset: raw.reset,
        changed_heads: raw.changed_heads,
        source: raw.source,
        updated_at,
    })
}

#[derive(Serialize)]
struct EntityProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    entity_revision: u64,
    source: &'a V2ProjectionSource,
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
    source: V2ProjectionSource,
    entity_type: RoleContinuityEntity,
    entity: serde_json::Value,
    updated_at: String,
}

#[derive(Serialize)]
struct ProjectObjectProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    source: &'a V2ProjectionSource,
    deleted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    object: Option<&'a ProjectViewObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tombstone: Option<V2ObjectTombstone>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProjectObjectProjection {
    schema_version: u16,
    projection_type: String,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    source: V2ProjectionSource,
    deleted: bool,
    #[serde(default)]
    object: Option<ProjectViewObject>,
    #[serde(default)]
    tombstone: Option<V2ObjectTombstone>,
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
    entity_counts: V2EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: &'a [V2ChangedHead],
    source: &'a V2ProjectionSource,
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
    entity_counts: V2EntityCounts,
    membership_snapshot_event_id: EventId,
    reset: bool,
    changed_heads: Vec<V2ChangedHead>,
    source: V2ProjectionSource,
    updated_at: String,
}

fn entity_parts(
    entity: &RoleContinuityChange,
) -> Result<(RoleContinuityEntity, serde_json::Value), SdkError> {
    let value = match entity {
        RoleContinuityChange::Role(value) => serde_json::to_value(value),
        RoleContinuityChange::Proposal(value) => serde_json::to_value(value),
        RoleContinuityChange::Assignment(value) => serde_json::to_value(value),
        RoleContinuityChange::Handoff(value) => serde_json::to_value(value),
    }
    .map_err(|error| SdkError::InvalidInput(format!("serialize v2 entity: {error}")))?;
    Ok((entity.entity_type(), value))
}

fn parse_entity(
    entity_type: RoleContinuityEntity,
    value: serde_json::Value,
) -> Result<RoleContinuityChange, SdkError> {
    match entity_type {
        RoleContinuityEntity::Role => {
            serde_json::from_value::<RoleDefinition>(value).map(RoleContinuityChange::Role)
        }
        RoleContinuityEntity::RoleAssignmentProposal => {
            serde_json::from_value::<RoleAssignmentProposal>(value)
                .map(RoleContinuityChange::Proposal)
        }
        RoleContinuityEntity::RoleAssignment => {
            serde_json::from_value::<RoleAssignment>(value).map(RoleContinuityChange::Assignment)
        }
        RoleContinuityEntity::RoleHandoff => {
            serde_json::from_value::<RoleHandoff>(value).map(RoleContinuityChange::Handoff)
        }
    }
    .map_err(|error| invalid_projection(format!("invalid typed v2 entity: {error}")))
}

fn validate_context(context: &V2ProjectionContext) -> Result<(), SdkError> {
    require_revision(context.projection_generation, "projection_generation")?;
    require_revision(context.project_revision, "project_revision")?;
    validate_source(&context.source)
}

fn validate_source(source: &V2ProjectionSource) -> Result<(), SdkError> {
    match source {
        V2ProjectionSource::NostrEvent {
            change_id,
            event_id,
        } if change_id != event_id => Err(SdkError::InvalidInput(
            "Nostr source change_id must equal event_id".to_owned(),
        )),
        V2ProjectionSource::Operator { audit_seq, .. }
        | V2ProjectionSource::System { audit_seq, .. }
            if *audit_seq == 0 =>
        {
            Err(SdkError::InvalidInput(
                "audited projection source sequence must be positive".to_owned(),
            ))
        }
        _ => Ok(()),
    }
}

fn projection_tags(
    coordinate: &str,
    entity_type: RoleContinuityEntity,
    generation: u64,
    entity_revision: u64,
    project_revision: u64,
    source: &V2ProjectionSource,
) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_V2_ENTITY_TAG.to_owned()],
        vec!["type".to_owned(), entity_type.as_str().to_owned()],
        vec!["projection_generation".to_owned(), generation.to_string()],
        vec!["revision".to_owned(), entity_revision.to_string()],
        vec!["project_revision".to_owned(), project_revision.to_string()],
        vec!["change".to_owned(), source.change_id().to_hex()],
        vec!["source_type".to_owned(), source.source_type().to_owned()],
    ];
    if let Some(event_id) = source.source_event_id() {
        tags.push(vec![
            "e".to_owned(),
            event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
    tags
}

fn project_object_tags(
    coordinate: &str,
    object_type: ProjectViewObjectType,
    generation: u64,
    object_revision: u64,
    project_revision: u64,
    source: &V2ProjectionSource,
) -> Vec<Vec<String>> {
    let mut tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_V2_OBJECT_TAG.to_owned()],
        vec!["type".to_owned(), object_type.as_str().to_owned()],
        vec!["projection_generation".to_owned(), generation.to_string()],
        vec!["revision".to_owned(), object_revision.to_string()],
        vec!["project_revision".to_owned(), project_revision.to_string()],
        vec!["change".to_owned(), source.change_id().to_hex()],
        vec!["source_type".to_owned(), source.source_type().to_owned()],
    ];
    if let Some(event_id) = source.source_event_id() {
        tags.push(vec![
            "e".to_owned(),
            event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
    tags
}

fn meta_tags(
    coordinate: &str,
    generation: u64,
    project_revision: u64,
    source: &V2ProjectionSource,
    membership_snapshot_event_id: EventId,
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
        vec![
            "membership".to_owned(),
            membership_snapshot_event_id.to_hex(),
        ],
    ];
    if let Some(event_id) = source.source_event_id() {
        tags.push(vec![
            "e".to_owned(),
            event_id.to_hex(),
            String::new(),
            "source".to_owned(),
        ]);
    }
    tags
}

fn parse_coordinate_id(
    coordinate: &str,
    project_id: CommunityId,
    entity_type: RoleContinuityEntity,
) -> Result<Uuid, SdkError> {
    let parts = coordinate.split(':').collect::<Vec<_>>();
    if parts.len() != 4
        || parts[0] != "project-view"
        || parts[1] != project_id.as_uuid().to_string()
        || parts[2] != entity_type.as_str()
    {
        return Err(invalid_projection("invalid v2 entity coordinate"));
    }
    let id = Uuid::parse_str(parts[3])
        .map_err(|error| invalid_projection(format!("invalid entity coordinate UUID: {error}")))?;
    if id.is_nil() || id.to_string() != parts[3] {
        return Err(invalid_projection(
            "v2 entity coordinate UUID is not canonical",
        ));
    }
    Ok(id)
}

fn validate_v2_tombstone(
    tombstone: &V2ObjectTombstone,
    project_id: CommunityId,
) -> Result<(), SdkError> {
    if tombstone.object_id.is_nil()
        || tombstone.object_revision == 0
        || tombstone.object_revision > MAX_SAFE_REVISION
        || tombstone.project_revision == 0
        || tombstone.project_revision > MAX_SAFE_REVISION
        || tombstone.deleted_at < tombstone.created_at
        || (tombstone.object_type == ProjectViewObjectType::ProjectProfile
            && tombstone.object_id != *project_id.as_uuid())
        || (tombstone.object_type != ProjectViewObjectType::ProjectProfile
            && tombstone.object_id == *project_id.as_uuid())
    {
        return Err(invalid_projection("invalid v2 Project object tombstone"));
    }
    Ok(())
}

fn validate_v2_object_identity(
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
            "v2 Project object identity does not match its Project",
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

fn require_revision(value: u64, field: &str) -> Result<(), SdkError> {
    if value == 0 || value > MAX_SAFE_REVISION {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be in 1..={MAX_SAFE_REVISION}"
        )));
    }
    Ok(())
}

fn canonical_time(value: String, field: &str) -> Result<DateTime<Utc>, SdkError> {
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
    use super::*;
    use buzz_core::Keys;
    use buzz_project_view::v2::{
        ProposalStatus, ProposalType, RoleAssignmentProposal, RoleContinuityChange,
    };
    use chrono::Duration;

    #[test]
    fn role_command_and_entity_projection_round_trip() {
        let member = Keys::generate();
        let relay = Keys::generate();
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let now = DateTime::from_timestamp(1_800_000_000, 0).expect("timestamp");
        let proposal = RoleAssignmentProposal {
            proposal_id: Uuid::new_v4(),
            role_id: Uuid::new_v4(),
            candidate_pubkey: member.public_key(),
            proposal_type: ProposalType::Request,
            candidate_accepted_at: Some(now),
            authorized_by: None,
            authorized_at: None,
            expected_target_assignment_id: None,
            expected_candidate_assignment_id: None,
            expires_at: now + Duration::days(3),
            status: ProposalStatus::Open,
            reason: None,
            created_by: member.public_key(),
            created_at: now,
            resolved_at: None,
            entity_revision: 1,
            project_revision: 9,
        };
        let entity = RoleContinuityChange::Proposal(proposal);
        let source_id = EventId::all_zeros();
        let context = V2ProjectionContext {
            project_id,
            projection_generation: 2,
            project_revision: 9,
            source: V2ProjectionSource::NostrEvent {
                change_id: source_id,
                event_id: source_id,
            },
            updated_at: now,
        };
        let event = build_entity_projection(&context, &entity)
            .expect("builder")
            .sign_with_keys(&relay)
            .expect("signed projection");
        let parsed =
            parse_entity_projection(&event, &relay.public_key(), project_id).expect("verified");
        assert_eq!(parsed.entity, entity);
    }

    #[test]
    fn membership_projection_requires_one_owner_and_canonical_member_order() {
        let relay = Keys::generate();
        let mut members = [
            (Keys::generate().public_key(), CommunityMemberRole::Member),
            (Keys::generate().public_key(), CommunityMemberRole::Owner),
            (Keys::generate().public_key(), CommunityMemberRole::Admin),
        ];
        members.sort_by_key(|(pubkey, _)| *pubkey);
        let tags = std::iter::once(Tag::parse(["-"]).expect("protection tag"))
            .chain(members.iter().map(|(pubkey, role)| {
                Tag::parse(["member", pubkey.to_hex().as_str(), role.as_str()]).expect("member tag")
            }))
            .collect::<Vec<_>>();
        let event = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
            .tags(tags)
            .custom_created_at(Timestamp::from(1_800_000_000_u64))
            .sign_with_keys(&relay)
            .expect("sign membership snapshot");
        let parsed =
            parse_membership_projection(&event, &relay.public_key()).expect("verified membership");
        assert_eq!(parsed.members.len(), 3);
        assert_eq!(
            parsed
                .members
                .iter()
                .filter(|member| member.role == CommunityMemberRole::Owner)
                .count(),
            1
        );

        members.swap(0, 1);
        let unordered_tags = std::iter::once(Tag::parse(["-"]).expect("protection tag"))
            .chain(members.iter().map(|(pubkey, role)| {
                Tag::parse(["member", pubkey.to_hex().as_str(), role.as_str()]).expect("member tag")
            }))
            .collect::<Vec<_>>();
        let unordered = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
            .tags(unordered_tags)
            .custom_created_at(Timestamp::from(1_800_000_000_u64))
            .sign_with_keys(&relay)
            .expect("sign unordered membership snapshot");
        assert!(parse_membership_projection(&unordered, &relay.public_key()).is_err());
    }
}
