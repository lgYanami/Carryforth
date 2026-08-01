//! Typed Project View mutation bridge for the desktop client.

use buzz_core_pkg::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_project_view_pkg::v2::{ProjectObjectCommand, RoleContinuityChange};
use buzz_project_view_pkg::v3::{
    CreateProjectObjectV3, DeleteProjectObjectV3, NewProjectViewObjectV3, ProjectObjectCommandV3,
    ProjectObjectRequestV3, UpdateProjectObjectV3,
};
use buzz_project_view_pkg::{
    CreateMutation, DeleteMutation, InitializeGoal, InitializeMutation, MutationRequest,
    NewProjectViewObject, ProjectProfile, ProjectViewObjectType, UpdateMutation,
};
use buzz_sdk_pkg::project_view::{
    build_create, build_delete, build_initialize, build_update, object_projection_coordinate,
    parse_meta_projection, parse_object_projection, MetaProjection, ProjectedObject,
};
use buzz_sdk_pkg::project_view_v2::build_project_object_command;
use buzz_sdk_pkg::project_view_v2::{
    parse_entity_projection as parse_v2_entity_projection,
    parse_meta_projection as parse_v2_meta_projection,
    parse_project_object_projection as parse_v2_object_projection, V2MetaProjection,
    V2ProjectedObject, V2ProjectionSource,
};
use buzz_sdk_pkg::project_view_v3::{
    build_project_object_command as build_v3_project_object_command,
    parse_entity_projection as parse_v3_entity_projection,
    parse_meta_projection as parse_v3_meta_projection,
    parse_project_object_projection as parse_v3_object_projection, V3EntityChange,
    V3MetaProjection, V3ProjectedObject, V3ProjectionSource,
};
use nostr::{Event, EventBuilder, Keys};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::relay::{
    query_relay_at_with_keys, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
    SubmitEventResponse,
};

use super::project_view::{read_identity_at, ProjectViewIdentity, ProjectViewSchema};

/// One initial Goal entered by a Human before Project View has an identity.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewInitializationGoalInput {
    /// Human-readable Goal title.
    pub title: String,
    /// Observable outcome that would satisfy the Goal.
    pub desired_outcome: String,
    /// Strategic directions guiding the Goal.
    pub directions: Vec<String>,
}

/// A closed Human intent accepted by the Desktop Project View boundary.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectViewMutationInput {
    /// Atomically establish the Project Profile and one or more Goals.
    Initialize {
        /// Complete Project Profile.
        profile: ProjectProfile,
        /// Initial Goals. Rust supplies their opaque UUID v4 identifiers.
        goals: Vec<ProjectViewInitializationGoalInput>,
    },
    /// Create one typed non-profile object.
    Create {
        /// Project revision on which the Human based this intent.
        expected_project_revision: u64,
        /// Type of object being created.
        object_type: ProjectViewObjectType,
        /// Closed per-type fields. Rust injects the object type and UUID.
        data: Value,
    },
    /// Replace explicitly supplied fields on one active object.
    Update {
        /// Project revision on which the Human based this intent.
        expected_project_revision: u64,
        /// Immutable object type expected by the Human.
        object_type: ProjectViewObjectType,
        /// Stable object identifier.
        object_id: Uuid,
        /// Closed typed patch. Explicit `null` clears an optional relation.
        patch: Value,
    },
    /// Tombstone one unreferenced active object.
    Delete {
        /// Project revision on which the Human based this intent.
        expected_project_revision: u64,
        /// Immutable object type expected by the Human.
        object_type: ProjectViewObjectType,
        /// Stable object identifier.
        object_id: Uuid,
    },
}

/// Result of submitting one revision-checked Human mutation.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectViewMutationResult {
    /// The Relay committed the mutation and its projection was confirmed.
    Applied {
        /// ID of the member-signed mutation event.
        event_id: String,
        /// New canonical project revision.
        project_revision: u64,
        /// Changed object, absent for initialization.
        #[serde(skip_serializing_if = "Option::is_none")]
        object_id: Option<Uuid>,
        /// New object revision, absent for initialization.
        #[serde(skip_serializing_if = "Option::is_none")]
        object_revision: Option<u64>,
        /// Whether the changed object is now a tombstone.
        #[serde(skip_serializing_if = "Option::is_none")]
        deleted: Option<bool>,
    },
    /// The Human's baseline is stale. The mutation was not applied.
    Conflict {
        /// Revision carried by the rejected Human intent.
        expected_project_revision: u64,
        /// Latest verified revision when it could be read.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_project_revision: Option<u64>,
        /// Human-safe Relay diagnostic.
        message: String,
    },
}

#[derive(Debug)]
struct PreparedMutation {
    builder: EventBuilder,
    request: Option<MutationRequest>,
    expected_project_revision: u64,
    target: Option<MutationTarget>,
}

#[derive(Debug, Clone, Copy)]
struct MutationTarget {
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    deleted: bool,
}

#[derive(Debug, Deserialize)]
struct ProjectViewReceipt {
    project_revision: u64,
    object_id: Option<Uuid>,
    object_revision: Option<u64>,
    deleted: Option<bool>,
}

struct MutationContext {
    api_base_url: String,
    identity: ProjectViewIdentity,
    keys: Keys,
}

enum MutationMeta {
    V1(MetaProjection),
    V2(V2MetaProjection),
    V3(V3MetaProjection),
}

impl MutationMeta {
    const fn project_revision(&self) -> u64 {
        match self {
            Self::V1(meta) => meta.project_revision,
            Self::V2(meta) => meta.project_revision,
            Self::V3(meta) => meta.project_revision,
        }
    }

    const fn projection_generation(&self) -> u64 {
        match self {
            Self::V1(meta) => meta.projection_generation,
            Self::V2(meta) => meta.projection_generation,
            Self::V3(meta) => meta.projection_generation,
        }
    }

    const fn project_id(&self) -> buzz_core_pkg::CommunityId {
        match self {
            Self::V1(meta) => meta.project_id,
            Self::V2(meta) => meta.project_id,
            Self::V3(meta) => meta.project_id,
        }
    }

    fn identifies_source(&self, event: &Event) -> bool {
        match self {
            Self::V1(meta) => meta.source_event_id.as_ref() == Some(&event.id),
            Self::V2(meta) => matches!(
                &meta.source,
                V2ProjectionSource::NostrEvent {
                    event_id,
                    change_id,
                } if *event_id == event.id && *change_id == event.id
            ),
            Self::V3(meta) => matches!(
                &meta.source,
                V3ProjectionSource::NostrEvent {
                    event_id,
                    change_id,
                } if *event_id == event.id && *change_id == event.id
            ),
        }
    }
}

struct MutationObjectProjection {
    object_id: Uuid,
    object_type: ProjectViewObjectType,
    object_revision: u64,
    project_revision: u64,
    projection_generation: u64,
    deleted: bool,
}

/// Validate, sign, submit, and confirm one typed Project View mutation.
#[tauri::command]
pub async fn mutate_project_view(
    input: ProjectViewMutationInput,
    state: State<'_, AppState>,
) -> Result<ProjectViewMutationResult, String> {
    execute_mutation(input, &state).await
}

async fn execute_mutation(
    input: ProjectViewMutationInput,
    state: &AppState,
) -> Result<ProjectViewMutationResult, String> {
    // Capture the workspace target and member identity before the first await.
    // A later Community switch must not retarget this already-started intent.
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    let identity = read_identity_at(state, &api_base_url)
        .await?
        .ok_or_else(|| "unsupported: Relay does not advertise Project View".to_owned())?;
    let context = MutationContext {
        api_base_url,
        identity,
        keys,
    };
    let prepared = if context.identity.schema == ProjectViewSchema::V3 {
        prepare_v3_mutation(input)?
    } else {
        prepare_mutation(input)?
    };
    let builder = match context.identity.schema {
        ProjectViewSchema::V1 => prepared.builder,
        ProjectViewSchema::V2 => build_project_object_command(ProjectObjectCommand::new(
            prepared.expected_project_revision,
            None,
            prepared.request.ok_or_else(|| {
                "Project View integrity error: missing legacy mutation request".to_owned()
            })?,
        ))
        .map_err(|error| format!("invalid Project View v2 mutation: {error}"))?,
        ProjectViewSchema::V3 => prepared.builder,
    };
    let event = builder
        .sign_with_keys(&context.keys)
        .map_err(|error| format!("failed to sign Project View mutation: {error}"))?;

    let response =
        match submit_signed_event_at_with_keys(&event, state, &context.api_base_url, &context.keys)
            .await
        {
            Ok(response) => response,
            Err(message) if message.starts_with("relay returned 409") => {
                let current_project_revision = read_meta(state, &context)
                    .await
                    .ok()
                    .flatten()
                    .map(|meta| meta.project_revision());
                return Ok(ProjectViewMutationResult::Conflict {
                    expected_project_revision: prepared.expected_project_revision,
                    current_project_revision,
                    message,
                });
            }
            Err(message) => return Err(message),
        };

    let receipt = parse_receipt(&response, &event)?;
    validate_receipt(&receipt, prepared.target)?;
    confirm_projection(state, &context, &event, &receipt, prepared.target).await?;

    Ok(ProjectViewMutationResult::Applied {
        event_id: event.id.to_hex(),
        project_revision: receipt.project_revision,
        object_id: receipt.object_id,
        object_revision: receipt.object_revision,
        deleted: receipt.deleted,
    })
}

fn prepare_mutation(input: ProjectViewMutationInput) -> Result<PreparedMutation, String> {
    match input {
        ProjectViewMutationInput::Initialize { profile, goals } => {
            let goals: Vec<InitializeGoal> = goals
                .into_iter()
                .map(|goal| InitializeGoal {
                    id: Uuid::new_v4(),
                    title: goal.title,
                    desired_outcome: goal.desired_outcome,
                    directions: goal.directions,
                })
                .collect();
            let request = MutationRequest::Initialize(InitializeMutation {
                profile: profile.clone(),
                goals: goals.clone(),
            });
            Ok(PreparedMutation {
                builder: build_initialize(profile, goals)
                    .map_err(|error| format!("invalid Project View initialization: {error}"))?,
                request: Some(request),
                expected_project_revision: 0,
                target: None,
            })
        }
        ProjectViewMutationInput::Create {
            expected_project_revision,
            object_type,
            data,
        } => {
            if object_type == ProjectViewObjectType::ProjectProfile {
                return Err("the Project Profile can only be created by initialization".to_owned());
            }
            let object_id = Uuid::new_v4();
            let object = create_input(object_type, object_id, data)?;
            let request = MutationRequest::Create(CreateMutation {
                object: object.clone(),
            });
            Ok(PreparedMutation {
                builder: build_create(expected_project_revision, object)
                    .map_err(|error| format!("invalid Project View create: {error}"))?,
                request: Some(request),
                expected_project_revision,
                target: Some(MutationTarget {
                    object_type,
                    object_id,
                    deleted: false,
                }),
            })
        }
        ProjectViewMutationInput::Update {
            expected_project_revision,
            object_type,
            object_id,
            patch,
        } => {
            let update = update_input(object_type, object_id, patch)?;
            let request = MutationRequest::Update(update.clone());
            Ok(PreparedMutation {
                builder: build_update(expected_project_revision, update)
                    .map_err(|error| format!("invalid Project View update: {error}"))?,
                request: Some(request),
                expected_project_revision,
                target: Some(MutationTarget {
                    object_type,
                    object_id,
                    deleted: false,
                }),
            })
        }
        ProjectViewMutationInput::Delete {
            expected_project_revision,
            object_type,
            object_id,
        } => {
            let request = MutationRequest::Delete(DeleteMutation {
                object_type,
                object_id,
            });
            Ok(PreparedMutation {
                builder: build_delete(expected_project_revision, object_type, object_id)
                    .map_err(|error| format!("invalid Project View delete: {error}"))?,
                request: Some(request),
                expected_project_revision,
                target: Some(MutationTarget {
                    object_type,
                    object_id,
                    deleted: true,
                }),
            })
        }
    }
}

fn prepare_v3_mutation(input: ProjectViewMutationInput) -> Result<PreparedMutation, String> {
    let (expected_project_revision, target, request) = match input {
        ProjectViewMutationInput::Initialize { .. } => {
            return Err(
                "unsupported: Project View v3 initialization requires the prepared owner bootstrap command"
                    .to_owned(),
            )
        }
        ProjectViewMutationInput::Create {
            expected_project_revision,
            object_type,
            data,
        } => {
            if object_type == ProjectViewObjectType::ProjectProfile {
                return Err("the Project Profile can only be created by initialization".to_owned());
            }
            let object_id = Uuid::new_v4();
            let object = create_input_v3(object_type, object_id, data)?;
            (
                expected_project_revision,
                MutationTarget {
                    object_type,
                    object_id,
                    deleted: false,
                },
                ProjectObjectRequestV3::Create(CreateProjectObjectV3 { object }),
            )
        }
        ProjectViewMutationInput::Update {
            expected_project_revision,
            object_type,
            object_id,
            patch,
        } => (
            expected_project_revision,
            MutationTarget {
                object_type,
                object_id,
                deleted: false,
            },
            ProjectObjectRequestV3::Update(update_input_v3(object_type, object_id, patch)?),
        ),
        ProjectViewMutationInput::Delete {
            expected_project_revision,
            object_type,
            object_id,
        } => (
            expected_project_revision,
            MutationTarget {
                object_type,
                object_id,
                deleted: true,
            },
            ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
                object_type,
                object_id,
            }),
        ),
    };
    let command = ProjectObjectCommandV3::new(expected_project_revision, None, request);
    let builder = build_v3_project_object_command(command)
        .map_err(|error| format!("invalid Project View v3 mutation: {error}"))?;
    Ok(PreparedMutation {
        builder,
        request: None,
        expected_project_revision,
        target: Some(target),
    })
}

fn create_input(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    data: Value,
) -> Result<NewProjectViewObject, String> {
    let mut object = data
        .as_object()
        .cloned()
        .ok_or_else(|| "Project View create data must be an object".to_owned())?;
    if object.contains_key("id") || object.contains_key("object_type") {
        return Err("Project View create data must omit id and object_type".to_owned());
    }
    object.insert("id".to_owned(), Value::String(object_id.to_string()));
    object.insert(
        "object_type".to_owned(),
        Value::String(object_type.as_str().to_owned()),
    );
    serde_json::from_value(Value::Object(object))
        .map_err(|error| format!("invalid typed Project View create data: {error}"))
}

fn create_input_v3(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    data: Value,
) -> Result<NewProjectViewObjectV3, String> {
    let mut object = data
        .as_object()
        .cloned()
        .ok_or_else(|| "Project View v3 create data must be an object".to_owned())?;
    if object.contains_key("id") || object.contains_key("object_type") {
        return Err("Project View v3 create data must omit id and object_type".to_owned());
    }
    object.insert("id".to_owned(), Value::String(object_id.to_string()));
    object.insert(
        "object_type".to_owned(),
        Value::String(object_type.as_str().to_owned()),
    );
    serde_json::from_value(Value::Object(object))
        .map_err(|error| format!("invalid typed Project View v3 create data: {error}"))
}

fn update_input(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    patch: Value,
) -> Result<UpdateMutation, String> {
    if !patch.is_object() {
        return Err("Project View update patch must be an object".to_owned());
    }
    serde_json::from_value(json!({
        "object_type": object_type.as_str(),
        "object_id": object_id,
        "patch": patch,
    }))
    .map_err(|error| format!("invalid typed Project View update patch: {error}"))
}

fn update_input_v3(
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    patch: Value,
) -> Result<UpdateProjectObjectV3, String> {
    if !patch.is_object() {
        return Err("Project View v3 update patch must be an object".to_owned());
    }
    serde_json::from_value(json!({
        "object_type": object_type.as_str(),
        "object_id": object_id,
        "patch": patch,
    }))
    .map_err(|error| format!("invalid typed Project View v3 update patch: {error}"))
}

fn parse_receipt(
    response: &SubmitEventResponse,
    event: &Event,
) -> Result<ProjectViewReceipt, String> {
    if response.event_id != event.id.to_hex() {
        return Err(
            "Project View integrity error: mutation response event_id differs from the submitted event"
                .to_owned(),
        );
    }
    let payload = response.message.strip_prefix("response:").ok_or_else(|| {
        "Project View integrity error: mutation receipt is missing the canonical `response:` prefix"
            .to_owned()
    })?;
    serde_json::from_str(payload)
        .map_err(|error| format!("Project View integrity error: invalid mutation receipt: {error}"))
}

fn validate_receipt(
    receipt: &ProjectViewReceipt,
    target: Option<MutationTarget>,
) -> Result<(), String> {
    match target {
        None if receipt.object_id.is_none()
            && receipt.object_revision.is_none()
            && receipt.deleted.is_none() =>
        {
            Ok(())
        }
        None => Err(
            "Project View integrity error: initialization receipt contains object fields"
                .to_owned(),
        ),
        Some(target)
            if receipt.object_id == Some(target.object_id)
                && receipt.object_revision.is_some()
                && receipt.deleted == Some(target.deleted) =>
        {
            Ok(())
        }
        Some(_) => Err(
            "Project View integrity error: mutation receipt does not match the requested object"
                .to_owned(),
        ),
    }
}

async fn confirm_projection(
    state: &AppState,
    context: &MutationContext,
    event: &Event,
    receipt: &ProjectViewReceipt,
    target: Option<MutationTarget>,
) -> Result<(), String> {
    let meta = read_meta(state, context).await?.ok_or_else(|| {
        "Project View integrity error: successful mutation has no metadata".to_owned()
    })?;
    if meta.project_revision() < receipt.project_revision {
        return Err(
            "Project View integrity error: metadata is older than the successful mutation receipt"
                .to_owned(),
        );
    }
    if meta.project_revision() == receipt.project_revision && !meta.identifies_source(event) {
        return Err(
            "Project View integrity error: metadata does not identify the submitted mutation"
                .to_owned(),
        );
    }

    let Some(target) = target else {
        return Ok(());
    };
    let projection = read_object(state, context, &meta, target).await?;
    if projection.object_revision < receipt.object_revision.unwrap_or_default() {
        return Err(
            "Project View integrity error: object projection is older than the mutation receipt"
                .to_owned(),
        );
    }
    if projection.project_revision == receipt.project_revision {
        return if projection.deleted == target.deleted {
            Ok(())
        } else {
            Err(
                "Project View integrity error: confirmed projection has the wrong deletion state"
                    .to_owned(),
            )
        };
    }
    if target.deleted && !projection.deleted {
        return Err(
            "Project View integrity error: a confirmed deletion no longer has a tombstone"
                .to_owned(),
        );
    }
    Ok(())
}

async fn read_meta(
    state: &AppState,
    context: &MutationContext,
) -> Result<Option<MutationMeta>, String> {
    let events = query_at(
        state,
        context,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => match context.identity.schema {
            ProjectViewSchema::V1 => parse_meta_projection(event, &context.identity.relay_pubkey)
                .map(MutationMeta::V1)
                .map(Some)
                .map_err(|error| format!("Project View integrity error: {error}")),
            ProjectViewSchema::V2 => {
                parse_v2_meta_projection(event, &context.identity.relay_pubkey)
                    .map(MutationMeta::V2)
                    .map(Some)
                    .map_err(|error| format!("Project View integrity error: {error}"))
            }
            ProjectViewSchema::V3 => {
                parse_v3_meta_projection(event, &context.identity.relay_pubkey)
                    .map(MutationMeta::V3)
                    .map(Some)
                    .map_err(|error| format!("Project View integrity error: {error}"))
            }
        },
        _ => Err(
            "Project View integrity error: metadata query returned multiple current heads"
                .to_owned(),
        ),
    }
}

async fn read_object(
    state: &AppState,
    context: &MutationContext,
    meta: &MutationMeta,
    target: MutationTarget,
) -> Result<MutationObjectProjection, String> {
    let coordinate =
        object_projection_coordinate(meta.project_id(), target.object_type, target.object_id);
    let events = query_at(
        state,
        context,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "#d": [coordinate],
            "limit": 2,
        })],
    )
    .await?;
    let event = match events.as_slice() {
        [event] => event,
        [] => {
            return Err(
                "Project View integrity error: successful mutation has no object projection"
                    .to_owned(),
            )
        }
        _ => {
            return Err(
                "Project View integrity error: object query returned multiple current heads"
                    .to_owned(),
            )
        }
    };
    let projection = match context.identity.schema {
        ProjectViewSchema::V1 => {
            let projection =
                parse_object_projection(event, &context.identity.relay_pubkey, meta.project_id())
                    .map_err(|error| format!("Project View integrity error: {error}"))?;
            MutationObjectProjection {
                object_id: projection.object.id(),
                object_type: projection.object.object_type(),
                object_revision: projection.object.object_revision(),
                project_revision: projection.project_revision,
                projection_generation: projection.projection_generation,
                deleted: matches!(projection.object, ProjectedObject::Tombstone(_)),
            }
        }
        ProjectViewSchema::V2 => parse_v2_confirmed_object(event, context, meta)?,
        ProjectViewSchema::V3 => parse_v3_confirmed_object(event, context, meta)?,
    };
    if projection.object_type != target.object_type || projection.object_id != target.object_id {
        return Err(
            "Project View integrity error: point query returned a different object".to_owned(),
        );
    }
    if projection.projection_generation != meta.projection_generation()
        || projection.project_revision > meta.project_revision()
    {
        return Err(
            "Project View integrity error: object projection does not match current metadata"
                .to_owned(),
        );
    }
    Ok(projection)
}

fn parse_v2_confirmed_object(
    event: &Event,
    context: &MutationContext,
    meta: &MutationMeta,
) -> Result<MutationObjectProjection, String> {
    let projection_type = serde_json::from_str::<Value>(&event.content)
        .ok()
        .and_then(|value| {
            value
                .get("projection_type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            "Project View integrity error: v2 object head has no projection_type".to_owned()
        })?;
    match projection_type.as_str() {
        "entity" => {
            let projection = parse_v2_entity_projection(
                event,
                &context.identity.relay_pubkey,
                meta.project_id(),
            )
            .map_err(|error| format!("Project View integrity error: {error}"))?;
            let RoleContinuityChange::Role(role) = projection.entity else {
                return Err(
                    "Project View integrity error: object coordinate resolved to a non-Role entity"
                        .to_owned(),
                );
            };
            Ok(MutationObjectProjection {
                object_id: role.role_id,
                object_type: ProjectViewObjectType::Role,
                object_revision: role.object_revision,
                project_revision: projection.project_revision,
                projection_generation: projection.projection_generation,
                deleted: false,
            })
        }
        "object" => {
            let projection = parse_v2_object_projection(
                event,
                &context.identity.relay_pubkey,
                meta.project_id(),
            )
            .map_err(|error| format!("Project View integrity error: {error}"))?;
            let (object_id, object_type, object_revision, deleted) = match projection.object {
                V2ProjectedObject::Active(object) => {
                    (object.id, object.object_type, object.object_revision, false)
                }
                V2ProjectedObject::Tombstone(tombstone) => (
                    tombstone.object_id,
                    tombstone.object_type,
                    tombstone.object_revision,
                    true,
                ),
            };
            Ok(MutationObjectProjection {
                object_id,
                object_type,
                object_revision,
                project_revision: projection.project_revision,
                projection_generation: projection.projection_generation,
                deleted,
            })
        }
        _ => Err("Project View integrity error: unsupported v2 object projection type".to_owned()),
    }
}

fn parse_v3_confirmed_object(
    event: &Event,
    context: &MutationContext,
    meta: &MutationMeta,
) -> Result<MutationObjectProjection, String> {
    let projection_type = serde_json::from_str::<Value>(&event.content)
        .ok()
        .and_then(|value| {
            value
                .get("projection_type")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            "Project View integrity error: v3 object head has no projection_type".to_owned()
        })?;
    match projection_type.as_str() {
        "entity" => {
            let projection = parse_v3_entity_projection(
                event,
                &context.identity.relay_pubkey,
                meta.project_id(),
            )
            .map_err(|error| format!("Project View integrity error: {error}"))?;
            let V3EntityChange::Role(role) = projection.entity else {
                return Err(
                    "Project View integrity error: object coordinate resolved to a non-Role v3 entity"
                        .to_owned(),
                );
            };
            Ok(MutationObjectProjection {
                object_id: role.role_id,
                object_type: ProjectViewObjectType::Role,
                object_revision: role.object_revision,
                project_revision: projection.project_revision,
                projection_generation: projection.projection_generation,
                deleted: false,
            })
        }
        "object" => {
            let projection = parse_v3_object_projection(
                event,
                &context.identity.relay_pubkey,
                meta.project_id(),
            )
            .map_err(|error| format!("Project View integrity error: {error}"))?;
            let (object_id, object_type, object_revision, deleted) = match projection.object {
                V3ProjectedObject::Active(object) => {
                    (object.id, object.object_type, object.object_revision, false)
                }
                V3ProjectedObject::Tombstone(tombstone) => (
                    tombstone.id,
                    tombstone.object_type,
                    tombstone.object_revision,
                    true,
                ),
            };
            Ok(MutationObjectProjection {
                object_id,
                object_type,
                object_revision,
                project_revision: projection.project_revision,
                projection_generation: projection.projection_generation,
                deleted,
            })
        }
        _ => Err("Project View integrity error: unsupported v3 object projection type".to_owned()),
    }
}

async fn query_at(
    state: &AppState,
    context: &MutationContext,
    filters: &[Value],
) -> Result<Vec<Event>, String> {
    query_relay_at_with_keys(state, &context.api_base_url, filters, &context.keys, None).await
}

#[cfg(test)]
#[path = "project_view_mutation_tests.rs"]
mod tests;
