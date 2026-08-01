//! Relay projection event builders for Buzz Project View.

use std::collections::HashSet;

use buzz_core::kind::{
    KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_view::{
    validate_projected_object, CreateMutation, DeleteMutation, InitializeGoal, InitializeMutation,
    Mutation, MutationRequest, NewProjectViewObject, ProjectProfile, ProjectViewEntry,
    ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations,
    ProjectionPlan, UpdateMutation, MAX_SAFE_REVISION, MUTATION_SCHEMA_VERSION,
};
use chrono::{DateTime, SecondsFormat, Utc};
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::{Uuid, Variant};

use crate::SdkError;

const PROJECT_VIEW_TAG: &str = "buzz-project-view";
const PROJECT_VIEW_ACTIVE_TAG: &str = "buzz-project-view-active";
const PROJECT_VIEW_TOMBSTONE_TAG: &str = "buzz-project-view-tombstone";
const PROJECT_VIEW_META_TAG: &str = "buzz-project-view-meta";
const PROJECT_VIEW_MUTATION_TAG: &str = "buzz-project-view-mutation";

/// Build an initialization command for a previously uninitialized Project
/// View.
pub fn build_initialize(
    profile: ProjectProfile,
    goals: Vec<InitializeGoal>,
) -> Result<EventBuilder, SdkError> {
    build_mutation(Mutation::new(
        0,
        MutationRequest::Initialize(InitializeMutation { profile, goals }),
    ))
}

/// Build a revision-checked command that creates one typed object.
pub fn build_create(
    expected_project_revision: u64,
    object: NewProjectViewObject,
) -> Result<EventBuilder, SdkError> {
    build_mutation(Mutation::new(
        expected_project_revision,
        MutationRequest::Create(CreateMutation { object }),
    ))
}

/// Build a revision-checked command that applies one typed patch.
pub fn build_update(
    expected_project_revision: u64,
    update: UpdateMutation,
) -> Result<EventBuilder, SdkError> {
    build_mutation(Mutation::new(
        expected_project_revision,
        MutationRequest::Update(update),
    ))
}

/// Build a revision-checked command that tombstones one active object.
pub fn build_delete(
    expected_project_revision: u64,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
) -> Result<EventBuilder, SdkError> {
    build_mutation(Mutation::new(
        expected_project_revision,
        MutationRequest::Delete(DeleteMutation {
            object_type,
            object_id,
        }),
    ))
}

fn build_mutation(mutation: Mutation) -> Result<EventBuilder, SdkError> {
    mutation
        .validate_for_submission()
        .map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    let content = serde_json::to_string(&mutation)
        .map_err(|error| SdkError::InvalidInput(format!("serialize mutation: {error}")))?;
    Mutation::from_json(&content).map_err(|error| SdkError::InvalidInput(error.to_string()))?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_MUTATION as u16), content)
            .tags([tag(["-"])?, tag(["t", PROJECT_VIEW_MUTATION_TAG])?]),
    )
}

/// One signed object projection referenced by a metadata projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectViewChangedHead {
    /// Canonical Project View object coordinate.
    pub coordinate: String,
    /// Hex Nostr event id of the new projection head.
    pub event_id: String,
    /// Object revision carried by the head.
    pub object_revision: u64,
    /// Whether the head is a tombstone.
    pub deleted: bool,
}

/// A verified object head referenced by a metadata projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedChangedHead {
    /// Canonical Project View object coordinate.
    pub coordinate: String,
    /// Signed projection event identifier.
    pub event_id: EventId,
    /// Object revision carried by that head.
    pub object_revision: u64,
    /// Whether the referenced head is a tombstone.
    pub deleted: bool,
}

/// Verified metadata for one complete Project View generation and revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetaProjection {
    /// Signed metadata event identifier.
    pub event_id: EventId,
    /// Server-resolved project/community identity encoded by the Relay.
    pub project_id: CommunityId,
    /// Projection generation, incremented by operator reprojection.
    pub projection_generation: u64,
    /// Current optimistic-concurrency revision.
    pub project_revision: u64,
    /// Number of active object heads in the snapshot.
    pub active_object_count: u32,
    /// Whether readers must discard the previous projection generation.
    pub reset: bool,
    /// Object heads changed by an ordinary mutation.
    pub changed_heads: Vec<VerifiedChangedHead>,
    /// Source mutation event for an incremental projection.
    pub source_event_id: Option<EventId>,
    /// Canonical server time of the projected state.
    pub updated_at: DateTime<Utc>,
}

/// Minimal deleted-object state exposed by an object projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectedTombstone {
    /// Stable object identifier.
    pub object_id: Uuid,
    /// Immutable object type.
    pub object_type: ProjectViewObjectType,
    /// Object revision assigned to the deletion.
    pub object_revision: u64,
    /// Project revision assigned to the deletion.
    pub project_revision: u64,
    /// Canonical deletion time.
    pub deleted_at: DateTime<Utc>,
}

/// Active or deleted payload carried by a verified object projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedObject {
    /// Complete active object state.
    Active(Box<ProjectViewObject>),
    /// Minimal tombstone state with no business body.
    Tombstone(ProjectedTombstone),
}

impl ProjectedObject {
    /// Return the stable object identifier.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Active(object) => object.id,
            Self::Tombstone(tombstone) => tombstone.object_id,
        }
    }

    /// Return the immutable object type.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::Active(object) => object.object_type,
            Self::Tombstone(tombstone) => tombstone.object_type,
        }
    }

    /// Return the current object revision.
    #[must_use]
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.object_revision,
            Self::Tombstone(tombstone) => tombstone.object_revision,
        }
    }
}

/// One verified Relay-authored object projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectProjection {
    /// Signed projection event identifier.
    pub event_id: EventId,
    /// Project/community identity established by metadata.
    pub project_id: CommunityId,
    /// Projection generation of this head.
    pub projection_generation: u64,
    /// Project revision at which this head was emitted.
    pub project_revision: u64,
    /// Source mutation event for an incremental projection.
    pub source_event_id: Option<EventId>,
    /// Active object or tombstone carried by the event.
    pub object: ProjectedObject,
}

/// A verified Project View projection of either supported kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedProjection {
    /// Object projection (kind 40903).
    Object(Box<ObjectProjection>),
    /// Metadata projection (kind 40904).
    Meta(MetaProjection),
}

/// Parse and verify one Relay-authored metadata projection.
///
/// Signature, signer, kind, exact protocol tags, canonical scalar encodings,
/// content/tag agreement, and reset/source semantics are all checked.
pub fn parse_meta_projection(
    event: &Event,
    expected_relay: &PublicKey,
) -> Result<MetaProjection, SdkError> {
    verify_event_envelope(event, expected_relay, KIND_PROJECT_VIEW_META)?;
    let raw: RawMetaProjection = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid metadata content: {error}")))?;
    if raw.schema_version != MUTATION_SCHEMA_VERSION {
        return Err(invalid_projection(format!(
            "unsupported projection schema version {}",
            raw.schema_version
        )));
    }
    if raw.projection_type != "meta" {
        return Err(invalid_projection(
            "metadata projection_type must be \"meta\"",
        ));
    }
    if !raw.initialized {
        return Err(invalid_projection(
            "metadata projection must describe initialized state",
        ));
    }

    let project_uuid = parse_canonical_uuid(&raw.project_id, "project_id")?;
    let project_id = CommunityId::from_uuid(project_uuid);
    require_positive_revision(raw.projection_generation, "projection_generation")?;
    require_positive_revision(raw.project_revision, "project_revision")?;
    let source_event_id = raw
        .source_event_id
        .as_deref()
        .map(|value| parse_canonical_event_id(value, "source_event_id"))
        .transpose()?;
    let updated_at = parse_canonical_timestamp(&raw.updated_at, "updated_at")?;
    let created_at = u64::try_from(updated_at.timestamp())
        .map_err(|_| invalid_projection("updated_at precedes the Unix epoch"))?;
    if event.created_at.as_secs() != created_at {
        return Err(invalid_projection(
            "metadata event timestamp does not match updated_at",
        ));
    }

    if raw.reset {
        if source_event_id.is_some() || !raw.changed_heads.is_empty() {
            return Err(invalid_projection(
                "reset metadata must omit source_event_id and changed_heads",
            ));
        }
    } else if source_event_id.is_none() || raw.changed_heads.is_empty() {
        return Err(invalid_projection(
            "incremental metadata requires source_event_id and changed_heads",
        ));
    }

    let mut coordinates = HashSet::with_capacity(raw.changed_heads.len());
    let mut event_ids = HashSet::with_capacity(raw.changed_heads.len());
    let mut changed_heads = Vec::with_capacity(raw.changed_heads.len());
    for raw_head in raw.changed_heads {
        validate_changed_head_coordinate(&raw_head.coordinate, project_id)?;
        if !coordinates.insert(raw_head.coordinate.clone()) {
            return Err(invalid_projection(
                "metadata contains a duplicate changed-head coordinate",
            ));
        }
        let event_id = parse_canonical_event_id(&raw_head.event_id, "changed_heads.event_id")?;
        if !event_ids.insert(event_id) {
            return Err(invalid_projection(
                "metadata contains a duplicate changed-head event id",
            ));
        }
        require_positive_revision(raw_head.object_revision, "changed_heads.object_revision")?;
        changed_heads.push(VerifiedChangedHead {
            coordinate: raw_head.coordinate,
            event_id,
            object_revision: raw_head.object_revision,
            deleted: raw_head.deleted,
        });
    }

    let coordinate = meta_projection_coordinate(project_id);
    let generation = canonical_decimal(raw.projection_generation);
    let revision = canonical_decimal(raw.project_revision);
    let source_hex = source_event_id.as_ref().map(EventId::to_hex);
    let mut expected_tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), PROJECT_VIEW_META_TAG.to_owned()],
        vec!["projection_generation".to_owned(), generation],
        vec!["project_revision".to_owned(), revision],
    ];
    if let Some(source_hex) = source_hex {
        expected_tags.push(vec![
            "e".to_owned(),
            source_hex,
            String::new(),
            "source".to_owned(),
        ]);
    }
    require_exact_tags(event, &expected_tags)?;

    Ok(MetaProjection {
        event_id: event.id,
        project_id,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        active_object_count: raw.active_object_count,
        reset: raw.reset,
        changed_heads,
        source_event_id,
        updated_at,
    })
}

/// Parse and verify one Relay-authored object projection for an expected
/// project.
pub fn parse_object_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: CommunityId,
) -> Result<ObjectProjection, SdkError> {
    verify_event_envelope(event, expected_relay, KIND_PROJECT_VIEW_OBJECT)?;
    let value: Value = serde_json::from_str(&event.content)
        .map_err(|error| invalid_projection(format!("invalid object content: {error}")))?;
    let raw: RawObjectProjection = serde_json::from_value(value.clone())
        .map_err(|error| invalid_projection(format!("invalid object content: {error}")))?;
    if raw.schema_version != MUTATION_SCHEMA_VERSION {
        return Err(invalid_projection(format!(
            "unsupported projection schema version {}",
            raw.schema_version
        )));
    }
    if raw.projection_type != "object" {
        return Err(invalid_projection(
            "object projection_type must be \"object\"",
        ));
    }

    let project_uuid = parse_canonical_uuid(&raw.project_id, "project_id")?;
    let project_id = CommunityId::from_uuid(project_uuid);
    if project_id != expected_project {
        return Err(invalid_projection(
            "object projection belongs to a different project",
        ));
    }
    require_positive_revision(raw.projection_generation, "projection_generation")?;
    require_positive_revision(raw.project_revision, "project_revision")?;
    require_positive_revision(raw.object_revision, "object_revision")?;
    let source_event_id = raw
        .source_event_id
        .as_deref()
        .map(|source| parse_canonical_event_id(source, "source_event_id"))
        .transpose()?;

    let object = if raw.deleted {
        for forbidden in ["object", "data", "relations", "locator"] {
            if value.get(forbidden).is_some() {
                return Err(invalid_projection(format!(
                    "tombstone must not contain {forbidden}"
                )));
            }
        }
        let object_id = parse_canonical_uuid(
            raw.object_id
                .as_deref()
                .ok_or_else(|| invalid_projection("tombstone object_id is required"))?,
            "object_id",
        )?;
        let object_type = raw
            .object_type
            .ok_or_else(|| invalid_projection("tombstone object_type is required"))?;
        validate_project_object_identity(project_id, object_type, object_id)?;
        let deleted_at = parse_canonical_timestamp(
            raw.deleted_at
                .as_deref()
                .ok_or_else(|| invalid_projection("tombstone deleted_at is required"))?,
            "deleted_at",
        )?;
        ProjectedObject::Tombstone(ProjectedTombstone {
            object_id,
            object_type,
            object_revision: raw.object_revision,
            project_revision: raw.project_revision,
            deleted_at,
        })
    } else {
        for forbidden in ["object_id", "object_type", "deleted_at"] {
            if value.get(forbidden).is_some() {
                return Err(invalid_projection(format!(
                    "active projection must not contain {forbidden}"
                )));
            }
        }
        let projected = raw
            .object
            .ok_or_else(|| invalid_projection("active projection object is required"))?;
        let id = parse_canonical_uuid(&projected.id, "object.id")?;
        validate_project_object_identity(project_id, projected.object_type, id)?;
        let created_at = parse_canonical_timestamp(&projected.created_at, "object.created_at")?;
        let updated_at = parse_canonical_timestamp(&projected.updated_at, "object.updated_at")?;
        let created_by = parse_canonical_pubkey(&projected.created_by, "object.created_by")?;
        let updated_by = parse_canonical_pubkey(&projected.updated_by, "object.updated_by")?;
        let data: ProjectViewObjectData = serde_json::from_value(serde_json::json!({
            "object_type": projected.object_type.as_str(),
            "data": projected.data,
        }))
        .map_err(|error| invalid_projection(format!("invalid typed object data: {error}")))?;
        let object = ProjectViewObject {
            id,
            object_type: projected.object_type,
            object_revision: raw.object_revision,
            project_revision: raw.project_revision,
            created_at,
            updated_at,
            created_by,
            updated_by,
            data,
            relations: projected.relations,
        };
        validate_projected_object(&object)
            .map_err(|error| invalid_projection(format!("invalid active object: {error}")))?;
        ProjectedObject::Active(Box::new(object))
    };

    let coordinate = object_projection_coordinate(project_id, object.object_type(), object.id());
    let generation = canonical_decimal(raw.projection_generation);
    let object_revision = canonical_decimal(raw.object_revision);
    let project_revision = canonical_decimal(raw.project_revision);
    let state_tag = if matches!(object, ProjectedObject::Active(_)) {
        PROJECT_VIEW_ACTIVE_TAG
    } else {
        PROJECT_VIEW_TOMBSTONE_TAG
    };
    let source_hex = source_event_id.as_ref().map(EventId::to_hex);
    let mut expected_tags = vec![
        vec!["-".to_owned()],
        vec!["d".to_owned(), coordinate],
        vec!["t".to_owned(), PROJECT_VIEW_TAG.to_owned()],
        vec!["t".to_owned(), state_tag.to_owned()],
        vec!["type".to_owned(), object.object_type().as_str().to_owned()],
        vec!["projection_generation".to_owned(), generation],
        vec!["revision".to_owned(), object_revision],
        vec!["project_revision".to_owned(), project_revision],
    ];
    if let Some(source_hex) = source_hex {
        expected_tags.push(vec![
            "e".to_owned(),
            source_hex,
            String::new(),
            "source".to_owned(),
        ]);
    }
    require_exact_tags(event, &expected_tags)?;

    Ok(ObjectProjection {
        event_id: event.id,
        project_id,
        projection_generation: raw.projection_generation,
        project_revision: raw.project_revision,
        source_event_id,
        object,
    })
}

/// Verify either Project View projection kind.
///
/// Metadata establishes the project identity itself. Object projections
/// require `expected_project` so a valid event from another Community cannot
/// be mixed into a snapshot.
pub fn verify_projection(
    event: &Event,
    expected_relay: &PublicKey,
    expected_project: Option<CommunityId>,
) -> Result<VerifiedProjection, SdkError> {
    match event.kind.as_u16() as u32 {
        KIND_PROJECT_VIEW_META => {
            let meta = parse_meta_projection(event, expected_relay)?;
            if expected_project.is_some_and(|project| project != meta.project_id) {
                return Err(invalid_projection(
                    "metadata projection belongs to a different project",
                ));
            }
            Ok(VerifiedProjection::Meta(meta))
        }
        KIND_PROJECT_VIEW_OBJECT => {
            let project_id = expected_project.ok_or_else(|| {
                invalid_projection("expected project is required for object projection")
            })?;
            parse_object_projection(event, expected_relay, project_id)
                .map(Box::new)
                .map(VerifiedProjection::Object)
        }
        _ => Err(invalid_projection(
            "unsupported Project View projection kind",
        )),
    }
}

/// Derive the immutable projection coordinate for one object identity.
#[must_use]
pub fn object_projection_coordinate(
    project_id: buzz_core::CommunityId,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
) -> String {
    format!(
        "project-view:{}:{}:{object_id}",
        project_id.as_uuid(),
        object_type.as_str()
    )
}

/// Derive the singleton metadata coordinate for one Project View.
#[must_use]
pub fn meta_projection_coordinate(project_id: buzz_core::CommunityId) -> String {
    format!("project-view:{}:meta", project_id.as_uuid())
}

/// Build an unsigned relay-authored object projection event.
///
/// The caller signs this builder with the relay identity. Keeping signing out
/// of this function lets the Relay and `buzz-admin reproject` reuse identical
/// wire construction without moving private keys into the SDK.
pub fn build_object_projection(
    plan: &ProjectionPlan,
    entry: &ProjectViewEntry,
) -> Result<EventBuilder, SdkError> {
    if !plan.entries().contains(entry) {
        return Err(SdkError::InvalidInput(
            "object projection entry is not part of the projection plan".to_owned(),
        ));
    }

    let coordinate =
        object_projection_coordinate(plan.project_id(), entry.object_type(), entry.id());
    let generation = canonical_decimal(plan.projection_generation());
    let object_revision = canonical_decimal(entry.object_revision());
    let project_revision = canonical_decimal(plan.project_revision());
    let source_event_id = plan.source_event_id().map(hex::encode);
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_VIEW_TAG])?,
        tag([
            "t",
            if matches!(entry, ProjectViewEntry::Active(_)) {
                PROJECT_VIEW_ACTIVE_TAG
            } else {
                PROJECT_VIEW_TOMBSTONE_TAG
            },
        ])?,
        tag(["type", entry.object_type().as_str()])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["revision", object_revision.as_str()])?,
        tag(["project_revision", project_revision.as_str()])?,
    ];
    if let Some(source_event_id) = source_event_id.as_deref() {
        tags.push(tag(["e", source_event_id, "", "source"])?);
    }

    let content = match entry {
        ProjectViewEntry::Active(object) => {
            serde_json::to_string(&ActiveObjectProjectionContent::from_plan(plan, object)?)
        }
        ProjectViewEntry::Tombstone(tombstone) => {
            serde_json::to_string(&TombstoneProjectionContent {
                schema_version: MUTATION_SCHEMA_VERSION,
                projection_type: "object",
                project_id: *plan.project_id().as_uuid(),
                projection_generation: plan.projection_generation(),
                project_revision: plan.project_revision(),
                object_revision: tombstone.object_revision,
                source_event_id,
                deleted: true,
                object_id: tombstone.id,
                object_type: tombstone.object_type,
                deleted_at: tombstone.deleted_at,
            })
        }
    }
    .map_err(|error| SdkError::InvalidInput(format!("serialize projection: {error}")))?;

    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16), content)
            .tags(tags)
            .custom_created_at(timestamp(plan.updated_at())?),
    )
}

/// Build the metadata projection after every object projection has been signed.
pub fn build_meta_projection(
    plan: &ProjectionPlan,
    changed_heads: &[ProjectViewChangedHead],
) -> Result<EventBuilder, SdkError> {
    if plan.reset() && !changed_heads.is_empty() {
        return Err(SdkError::InvalidInput(
            "reset metadata must not carry incremental changed heads".to_owned(),
        ));
    }
    if !plan.reset() && changed_heads.len() != plan.entries().len() {
        return Err(SdkError::InvalidInput(
            "mutation metadata must reference every changed object projection".to_owned(),
        ));
    }

    let coordinate = meta_projection_coordinate(plan.project_id());
    let generation = canonical_decimal(plan.projection_generation());
    let project_revision = canonical_decimal(plan.project_revision());
    let source_event_id = plan.source_event_id().map(hex::encode);
    let mut tags = vec![
        tag(["-"])?,
        tag(["d", coordinate.as_str()])?,
        tag(["t", PROJECT_VIEW_TAG])?,
        tag(["t", PROJECT_VIEW_META_TAG])?,
        tag(["projection_generation", generation.as_str()])?,
        tag(["project_revision", project_revision.as_str()])?,
    ];
    if let Some(source_event_id) = source_event_id.as_deref() {
        tags.push(tag(["e", source_event_id, "", "source"])?);
    }

    let content = serde_json::to_string(&MetaProjectionContent {
        schema_version: MUTATION_SCHEMA_VERSION,
        projection_type: "meta",
        project_id: *plan.project_id().as_uuid(),
        initialized: true,
        projection_generation: plan.projection_generation(),
        project_revision: plan.project_revision(),
        active_object_count: plan.active_object_count(),
        reset: plan.reset(),
        changed_heads,
        source_event_id,
        updated_at: plan.updated_at(),
    })
    .map_err(|error| SdkError::InvalidInput(format!("serialize projection: {error}")))?;

    Ok(
        EventBuilder::new(Kind::Custom(KIND_PROJECT_VIEW_META as u16), content)
            .tags(tags)
            .custom_created_at(timestamp(plan.updated_at())?),
    )
}

/// Bind a signed event back to the changed-head record used by metadata.
pub fn changed_head_for(
    plan: &ProjectionPlan,
    entry: &ProjectViewEntry,
    event: &Event,
) -> Result<ProjectViewChangedHead, SdkError> {
    if event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_OBJECT {
        return Err(SdkError::InvalidInput(
            "changed head is not a Project View object projection".to_owned(),
        ));
    }
    Ok(ProjectViewChangedHead {
        coordinate: object_projection_coordinate(
            plan.project_id(),
            entry.object_type(),
            entry.id(),
        ),
        event_id: event.id.to_hex(),
        object_revision: entry.object_revision(),
        deleted: matches!(entry, ProjectViewEntry::Tombstone(_)),
    })
}

#[derive(Deserialize)]
struct RawMetaProjection {
    schema_version: u16,
    projection_type: String,
    project_id: String,
    initialized: bool,
    projection_generation: u64,
    project_revision: u64,
    active_object_count: u32,
    reset: bool,
    changed_heads: Vec<RawChangedHead>,
    source_event_id: Option<String>,
    updated_at: String,
}

#[derive(Deserialize)]
struct RawChangedHead {
    coordinate: String,
    event_id: String,
    object_revision: u64,
    deleted: bool,
}

#[derive(Deserialize)]
struct RawObjectProjection {
    schema_version: u16,
    projection_type: String,
    project_id: String,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    source_event_id: Option<String>,
    deleted: bool,
    object: Option<RawActiveProjectedObject>,
    object_id: Option<String>,
    object_type: Option<ProjectViewObjectType>,
    deleted_at: Option<String>,
}

#[derive(Deserialize)]
struct RawActiveProjectedObject {
    id: String,
    object_type: ProjectViewObjectType,
    created_at: String,
    updated_at: String,
    created_by: String,
    updated_by: String,
    data: Value,
    relations: ProjectViewRelations,
}

fn verify_event_envelope(
    event: &Event,
    expected_relay: &PublicKey,
    expected_kind: u32,
) -> Result<(), SdkError> {
    event
        .verify()
        .map_err(|error| invalid_projection(format!("invalid event signature: {error}")))?;
    if event.pubkey != *expected_relay {
        return Err(invalid_projection(
            "projection signer does not match NIP-11 relay identity",
        ));
    }
    if event.kind.as_u16() as u32 != expected_kind {
        return Err(invalid_projection(format!(
            "projection kind must be {expected_kind}"
        )));
    }
    Ok(())
}

fn require_exact_tags(event: &Event, expected: &[Vec<String>]) -> Result<(), SdkError> {
    let actual: Vec<&[String]> = event.tags.iter().map(Tag::as_slice).collect();
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .any(|(actual, expected)| *actual != expected.as_slice())
    {
        return Err(invalid_projection(
            "projection tags are not the exact canonical tag sequence",
        ));
    }
    Ok(())
}

fn require_positive_revision(value: u64, field: &'static str) -> Result<(), SdkError> {
    if value == 0 || value > MAX_SAFE_REVISION {
        return Err(invalid_projection(format!(
            "{field} must be within 1..={MAX_SAFE_REVISION}"
        )));
    }
    Ok(())
}

fn parse_canonical_uuid(value: &str, field: &'static str) -> Result<Uuid, SdkError> {
    let parsed = Uuid::parse_str(value)
        .map_err(|error| invalid_projection(format!("invalid {field}: {error}")))?;
    if parsed.to_string() != value {
        return Err(invalid_projection(format!(
            "{field} is not a canonical lowercase UUID"
        )));
    }
    Ok(parsed)
}

fn parse_canonical_event_id(value: &str, field: &'static str) -> Result<EventId, SdkError> {
    let parsed = EventId::from_hex(value)
        .map_err(|error| invalid_projection(format!("invalid {field}: {error}")))?;
    if parsed.to_hex() != value {
        return Err(invalid_projection(format!(
            "{field} is not canonical lowercase hex"
        )));
    }
    Ok(parsed)
}

fn parse_canonical_pubkey(value: &str, field: &'static str) -> Result<PublicKey, SdkError> {
    let parsed = PublicKey::from_hex(value)
        .map_err(|error| invalid_projection(format!("invalid {field}: {error}")))?;
    if parsed.to_hex() != value {
        return Err(invalid_projection(format!(
            "{field} is not canonical lowercase hex"
        )));
    }
    Ok(parsed)
}

fn parse_canonical_timestamp(value: &str, field: &'static str) -> Result<DateTime<Utc>, SdkError> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| invalid_projection(format!("invalid {field}: {error}")))?
        .with_timezone(&Utc);
    if parsed.to_rfc3339_opts(SecondsFormat::AutoSi, true) != value {
        return Err(invalid_projection(format!(
            "{field} is not a canonical UTC timestamp"
        )));
    }
    Ok(parsed)
}

fn validate_project_object_identity(
    project_id: CommunityId,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
) -> Result<(), SdkError> {
    let project_uuid = *project_id.as_uuid();
    if object_type == ProjectViewObjectType::ProjectProfile {
        if object_id != project_uuid {
            return Err(invalid_projection(
                "project profile id does not match project id",
            ));
        }
    } else if object_id == project_uuid
        || object_id.get_version_num() != 4
        || object_id.get_variant() != Variant::RFC4122
    {
        return Err(invalid_projection(
            "non-profile object id must be an unreserved UUID v4",
        ));
    }
    Ok(())
}

fn validate_changed_head_coordinate(
    coordinate: &str,
    expected_project: CommunityId,
) -> Result<(), SdkError> {
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() != 4 || parts[0] != "project-view" {
        return Err(invalid_projection(
            "changed-head coordinate has invalid shape",
        ));
    }
    let project_uuid = parse_canonical_uuid(parts[1], "changed_heads.coordinate.project_id")?;
    if project_uuid != *expected_project.as_uuid() {
        return Err(invalid_projection(
            "changed-head coordinate belongs to a different project",
        ));
    }
    let object_type: ProjectViewObjectType =
        serde_json::from_value(Value::String(parts[2].to_owned())).map_err(|error| {
            invalid_projection(format!("invalid changed-head object type: {error}"))
        })?;
    let object_id = parse_canonical_uuid(parts[3], "changed_heads.coordinate.object_id")?;
    validate_project_object_identity(expected_project, object_type, object_id)?;
    if object_projection_coordinate(expected_project, object_type, object_id) != coordinate {
        return Err(invalid_projection(
            "changed-head coordinate is not canonical",
        ));
    }
    Ok(())
}

fn invalid_projection(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}

#[derive(Serialize)]
struct ActiveObjectProjectionContent {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_event_id: Option<String>,
    deleted: bool,
    object: ActiveProjectedObject,
}

impl ActiveObjectProjectionContent {
    fn from_plan(plan: &ProjectionPlan, object: &ProjectViewObject) -> Result<Self, SdkError> {
        Ok(Self {
            schema_version: MUTATION_SCHEMA_VERSION,
            projection_type: "object",
            project_id: *plan.project_id().as_uuid(),
            projection_generation: plan.projection_generation(),
            project_revision: plan.project_revision(),
            object_revision: object.object_revision,
            source_event_id: plan.source_event_id().map(hex::encode),
            deleted: false,
            object: ActiveProjectedObject {
                id: object.id,
                object_type: object.object_type,
                created_at: object.created_at,
                updated_at: object.updated_at,
                created_by: object.created_by.to_hex(),
                updated_by: object.updated_by.to_hex(),
                data: object_data_value(&object.data)?,
                relations: object.relations,
            },
        })
    }
}

#[derive(Serialize)]
struct ActiveProjectedObject {
    id: Uuid,
    object_type: ProjectViewObjectType,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    created_by: String,
    updated_by: String,
    data: serde_json::Value,
    relations: ProjectViewRelations,
}

#[derive(Serialize)]
struct TombstoneProjectionContent {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    projection_generation: u64,
    project_revision: u64,
    object_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_event_id: Option<String>,
    deleted: bool,
    object_id: Uuid,
    object_type: ProjectViewObjectType,
    deleted_at: DateTime<Utc>,
}

#[derive(Serialize)]
struct MetaProjectionContent<'a> {
    schema_version: u16,
    projection_type: &'static str,
    project_id: Uuid,
    initialized: bool,
    projection_generation: u64,
    project_revision: u64,
    active_object_count: u32,
    reset: bool,
    changed_heads: &'a [ProjectViewChangedHead],
    #[serde(skip_serializing_if = "Option::is_none")]
    source_event_id: Option<String>,
    updated_at: DateTime<Utc>,
}

fn object_data_value(data: &ProjectViewObjectData) -> Result<serde_json::Value, SdkError> {
    let mut envelope = serde_json::to_value(data)
        .map_err(|error| SdkError::InvalidInput(format!("serialize object data: {error}")))?;
    envelope
        .as_object_mut()
        .and_then(|object| object.remove("data"))
        .ok_or_else(|| {
            SdkError::InvalidInput("Project View object data has no serialized body".to_owned())
        })
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

fn canonical_decimal(value: u64) -> String {
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    use buzz_core::CommunityId;
    use buzz_project_view::{
        CreateMutation, GoalPatch, InitializeGoal, InitializeMutation, Mutation, MutationRequest,
        NewProjectViewObject, Patch, ProjectProfile, ProjectViewState, UpdateMutation,
    };
    use nostr::Keys;

    fn initialized_state() -> (ProjectViewState, buzz_project_view::MutationOutcome) {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let mutation = Mutation::new(
            0,
            MutationRequest::Initialize(InitializeMutation {
                profile: ProjectProfile {
                    name: "SDK test".to_owned(),
                    positioning: "Canonical current state".to_owned(),
                    purpose: "Verify projection wire format".to_owned(),
                    problem: "Wire drift".to_owned(),
                    scope: "Unit test".to_owned(),
                },
                goals: vec![InitializeGoal {
                    id: Uuid::new_v4(),
                    title: "Ship".to_owned(),
                    desired_outcome: "Projection is verifiable".to_owned(),
                    directions: Vec::new(),
                }],
            }),
        );
        ProjectViewState::new(community_id)
            .reduce(
                &mutation,
                Keys::generate().public_key(),
                DateTime::from_timestamp(1_800_000_000, 0).expect("valid test timestamp"),
            )
            .expect("initialize Project View")
    }

    fn has_tag(event: &Event, expected: &[&str]) -> bool {
        event.tags.iter().any(|tag| {
            tag.as_slice()
                .iter()
                .map(String::as_str)
                .eq(expected.iter().copied())
        })
    }

    #[test]
    fn mutation_projection_wire_binds_source_heads_and_relay_signer() {
        let (state, outcome) = initialized_state();
        let source_event_id = [0x11; 32];
        let plan = ProjectionPlan::for_mutation(&state, &outcome, source_event_id, 1)
            .expect("build mutation projection plan");
        let relay = Keys::generate();
        let mut heads = Vec::new();

        for entry in plan.entries() {
            let event = build_object_projection(&plan, entry)
                .expect("build object projection")
                .sign_with_keys(&relay)
                .expect("sign object projection");
            assert_eq!(event.kind.as_u16() as u32, KIND_PROJECT_VIEW_OBJECT);
            assert_eq!(event.pubkey, relay.public_key());
            assert_eq!(
                event.tags.iter().next().map(Tag::as_slice),
                Some(&["-".to_owned()][..])
            );
            assert!(has_tag(&event, &["t", PROJECT_VIEW_TAG]));
            assert!(has_tag(
                &event,
                &["e", &hex::encode(source_event_id), "", "source"]
            ));
            event.verify().expect("valid object projection signature");
            let parsed = parse_object_projection(&event, &relay.public_key(), state.project_id())
                .expect("parse object projection");
            assert_eq!(parsed.object.id(), entry.id());
            assert_eq!(parsed.object.object_revision(), entry.object_revision());
            heads.push(changed_head_for(&plan, entry, &event).expect("bind changed head"));
        }

        let meta = build_meta_projection(&plan, &heads)
            .expect("build metadata projection")
            .sign_with_keys(&relay)
            .expect("sign metadata projection");
        assert_eq!(meta.kind.as_u16() as u32, KIND_PROJECT_VIEW_META);
        assert!(has_tag(&meta, &["t", PROJECT_VIEW_META_TAG]));
        let content: serde_json::Value =
            serde_json::from_str(&meta.content).expect("parse metadata projection");
        assert_eq!(content["project_revision"], 1);
        assert_eq!(content["projection_generation"], 1);
        assert_eq!(content["active_object_count"], 2);
        assert_eq!(content["reset"], false);
        assert_eq!(content["changed_heads"].as_array().map(Vec::len), Some(2));
        assert_eq!(content["source_event_id"], hex::encode(source_event_id));
        let parsed =
            parse_meta_projection(&meta, &relay.public_key()).expect("parse metadata projection");
        assert_eq!(parsed.project_id, state.project_id());
        assert_eq!(parsed.project_revision, 1);
        assert_eq!(parsed.changed_heads.len(), 2);
    }

    #[test]
    fn reprojection_wire_is_reset_only_and_has_no_command_source() {
        let (state, _) = initialized_state();
        let plan = ProjectionPlan::for_reprojection(&state, 2).expect("build reprojection plan");
        assert!(plan.reset());
        assert_eq!(plan.source_event_id(), None);
        assert_eq!(plan.entries().len(), state.entries().len());

        let relay = Keys::generate();
        let object = build_object_projection(&plan, &plan.entries()[0])
            .expect("build reset object")
            .sign_with_keys(&relay)
            .expect("sign reset object");
        assert!(!object
            .tags
            .iter()
            .any(|tag| tag.as_slice().first().is_some_and(|value| value == "e")));
        let object_content: serde_json::Value =
            serde_json::from_str(&object.content).expect("parse reset object");
        assert!(object_content.get("source_event_id").is_none());

        let meta = build_meta_projection(&plan, &[])
            .expect("build reset metadata")
            .sign_with_keys(&relay)
            .expect("sign reset metadata");
        let meta_content: serde_json::Value =
            serde_json::from_str(&meta.content).expect("parse reset metadata");
        assert_eq!(meta_content["reset"], true);
        assert_eq!(
            meta_content["changed_heads"].as_array().map(Vec::len),
            Some(0)
        );
        assert!(meta_content.get("source_event_id").is_none());
        assert!(build_meta_projection(
            &plan,
            &[ProjectViewChangedHead {
                coordinate: "unexpected".to_owned(),
                event_id: object.id.to_hex(),
                object_revision: 1,
                deleted: false,
            }]
        )
        .is_err());
    }

    #[test]
    fn mutation_builders_emit_exact_closed_command_wire() {
        let keys = Keys::generate();
        let goal_id = Uuid::new_v4();
        let initialize = build_initialize(
            ProjectProfile {
                name: "SDK command".to_owned(),
                positioning: "Typed".to_owned(),
                purpose: "Avoid hand-written events".to_owned(),
                problem: "Protocol drift".to_owned(),
                scope: "Project View".to_owned(),
            },
            vec![InitializeGoal {
                id: goal_id,
                title: "Ship".to_owned(),
                desired_outcome: "CLI works".to_owned(),
                directions: Vec::new(),
            }],
        )
        .expect("build initialize")
        .sign_with_keys(&keys)
        .expect("sign initialize");
        assert_eq!(initialize.kind.as_u16() as u32, KIND_PROJECT_VIEW_MUTATION);
        assert_eq!(
            initialize
                .tags
                .iter()
                .map(Tag::as_slice)
                .collect::<Vec<_>>(),
            vec![
                &["-".to_owned()][..],
                &["t".to_owned(), PROJECT_VIEW_MUTATION_TAG.to_owned()][..]
            ]
        );
        let mutation = Mutation::from_json(&initialize.content).expect("parse mutation");
        assert_eq!(mutation.expected_project_revision, 0);

        let update = build_update(
            4,
            UpdateMutation::Goal {
                object_id: goal_id,
                patch: GoalPatch {
                    title: Patch::Set("Updated".to_owned()),
                    ..GoalPatch::default()
                },
            },
        )
        .expect("build update")
        .sign_with_keys(&keys)
        .expect("sign update");
        let mutation = Mutation::from_json(&update.content).expect("parse update");
        assert_eq!(mutation.expected_project_revision, 4);
    }

    #[test]
    fn mutation_builders_reject_invalid_local_fields() {
        let result = build_create(
            1,
            NewProjectViewObject::Goal {
                id: Uuid::new_v4(),
                title: " \t".to_owned(),
                desired_outcome: "Outcome".to_owned(),
                directions: Vec::new(),
            },
        );
        assert!(matches!(result, Err(SdkError::InvalidInput(_))));

        let result = build_update(
            1,
            UpdateMutation::Goal {
                object_id: Uuid::new_v4(),
                patch: GoalPatch::default(),
            },
        );
        assert!(matches!(result, Err(SdkError::InvalidInput(_))));
    }

    #[test]
    fn projection_parser_rejects_wrong_relay_and_accepts_unknown_optional_content() {
        let (state, outcome) = initialized_state();
        let plan =
            ProjectionPlan::for_mutation(&state, &outcome, [0x22; 32], 1).expect("projection plan");
        let relay = Keys::generate();
        let other = Keys::generate();
        let event = build_object_projection(&plan, &plan.entries()[0])
            .expect("build projection")
            .sign_with_keys(&relay)
            .expect("sign projection");
        assert!(parse_object_projection(&event, &other.public_key(), state.project_id()).is_err());

        let mut content: Value = serde_json::from_str(&event.content).expect("projection content");
        content["future_optional"] = serde_json::json!({"ignored": true});
        let event_with_optional = EventBuilder::new(event.kind, content.to_string())
            .tags(event.tags.iter().cloned())
            .custom_created_at(event.created_at)
            .sign_with_keys(&relay)
            .expect("sign projection with optional field");
        parse_object_projection(
            &event_with_optional,
            &relay.public_key(),
            state.project_id(),
        )
        .expect("unknown optional projection field is forward compatible");
    }

    #[test]
    fn tombstone_projection_never_accepts_a_business_body() {
        let (mut state, _) = initialized_state();
        let actor = Keys::generate().public_key();
        let role_id = Uuid::new_v4();
        let create = Mutation::new(
            state.project_revision(),
            MutationRequest::Create(CreateMutation {
                object: NewProjectViewObject::Role {
                    id: role_id,
                    name: "Maintainer".to_owned(),
                    purpose: "Maintain".to_owned(),
                    responsibilities: Vec::new(),
                    boundaries: Vec::new(),
                    active: true,
                },
            }),
        );
        state
            .apply(
                &create,
                actor,
                DateTime::from_timestamp(1_800_000_001, 0).expect("timestamp"),
            )
            .expect("create role");
        let delete = Mutation::new(
            state.project_revision(),
            MutationRequest::Delete(DeleteMutation {
                object_type: ProjectViewObjectType::Role,
                object_id: role_id,
            }),
        );
        let outcome = state
            .apply(
                &delete,
                actor,
                DateTime::from_timestamp(1_800_000_002, 0).expect("timestamp"),
            )
            .expect("delete role");
        let plan = ProjectionPlan::for_mutation(&state, &outcome, [0x33; 32], 1)
            .expect("delete projection plan");
        let relay = Keys::generate();
        let event = build_object_projection(&plan, &plan.entries()[0])
            .expect("build tombstone")
            .sign_with_keys(&relay)
            .expect("sign tombstone");
        assert!(matches!(
            parse_object_projection(&event, &relay.public_key(), state.project_id())
                .expect("parse tombstone")
                .object,
            ProjectedObject::Tombstone(_)
        ));

        let mut content: Value = serde_json::from_str(&event.content).expect("tombstone content");
        content["data"] = serde_json::json!({"secret": "must not survive deletion"});
        let invalid = EventBuilder::new(event.kind, content.to_string())
            .tags(event.tags.iter().cloned())
            .custom_created_at(event.created_at)
            .sign_with_keys(&relay)
            .expect("sign invalid tombstone");
        assert!(
            parse_object_projection(&invalid, &relay.public_key(), state.project_id()).is_err()
        );
    }
}
