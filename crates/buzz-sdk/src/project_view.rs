//! Relay projection event builders for Buzz Project View.

use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_project_view::{
    ProjectViewEntry, ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType,
    ProjectViewRelations, ProjectionPlan, MUTATION_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde::Serialize;
use uuid::Uuid;

use crate::SdkError;

const PROJECT_VIEW_TAG: &str = "buzz-project-view";
const PROJECT_VIEW_ACTIVE_TAG: &str = "buzz-project-view-active";
const PROJECT_VIEW_TOMBSTONE_TAG: &str = "buzz-project-view-tombstone";
const PROJECT_VIEW_META_TAG: &str = "buzz-project-view-meta";

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
        InitializeGoal, InitializeMutation, Mutation, MutationRequest, ProjectProfile,
        ProjectViewState,
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
}
