//! Typed Project View mutation bridge for the desktop client.

use buzz_core_pkg::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_project_view_pkg::v2::{RoleContinuityEntity, RoleLevel};
use buzz_project_view_pkg::v3::{
    CreateProjectObjectV3, DeleteProjectObjectV3, NewProjectViewObjectV3, ProjectContextReference,
    ProjectObjectCommandV3, ProjectObjectRequestV3, UpdateProjectObjectV3,
};
use buzz_project_view_pkg::ProjectViewObjectType;
use buzz_sdk_pkg::project_view::object_projection_coordinate;
use buzz_sdk_pkg::project_view_v3::{
    build_project_object_command, entity_projection_coordinate, parse_entity_projection,
    parse_meta_projection, parse_project_object_projection, V3EntityChange, V3MetaProjection,
    V3ProjectedObject, V3ProjectionSource, PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_META_TAG,
    PROJECT_VIEW_V3_OBJECT_TAG,
};
use nostr::{Event, EventBuilder, Keys};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;
use uuid::Uuid;

use crate::app_state::AppState;
use crate::relay::{
    query_relay_at_with_keys, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
};

use super::project_view::{read_identity_at, ProjectViewIdentity, ProjectViewSchema};

#[path = "project_view_mutation_receipt.rs"]
mod receipt;

use receipt::{parse_receipt, validate_receipt, ProjectViewReceipt};

/// A closed Human intent accepted by the Desktop Project View boundary.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectViewMutationInput {
    /// Create one typed non-profile object.
    Create {
        /// Project revision on which the Human based this intent.
        expected_project_revision: u64,
        /// Type of object being created.
        object_type: ProjectViewObjectType,
        /// Closed per-type fields. Rust injects the object type and UUID.
        data: Value,
        /// Signed initial governance level. Valid only for Role creation.
        #[serde(default)]
        initial_role_level: Option<RoleLevel>,
        /// Exact active Leader Assignment used by a non-owner Role governor.
        #[serde(default)]
        acting_assignment_id: Option<Uuid>,
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
        /// Exact active Leader Assignment used by a non-owner Role governor.
        #[serde(default)]
        acting_assignment_id: Option<Uuid>,
    },
    /// Tombstone one unreferenced active object.
    Delete {
        /// Project revision on which the Human based this intent.
        expected_project_revision: u64,
        /// Immutable object type expected by the Human.
        object_type: ProjectViewObjectType,
        /// Stable object identifier.
        object_id: Uuid,
        /// Exact active Leader Assignment used by a non-owner Role governor.
        #[serde(default)]
        acting_assignment_id: Option<Uuid>,
    },
    /// Replace the complete canonical Context Reference set on one v3 object.
    Context {
        /// Exact Project revision on which the replacement was based.
        expected_project_revision: u64,
        /// Immutable source object type.
        object_type: ProjectViewObjectType,
        /// Stable source object identifier.
        object_id: Uuid,
        /// Complete canonical replacement set.
        context_references: Vec<ProjectContextReference>,
        /// Exact active Leader Assignment used when the source is a Role.
        #[serde(default)]
        acting_assignment_id: Option<Uuid>,
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
        /// Changed object's stable identifier.
        #[serde(skip_serializing_if = "Option::is_none")]
        object_id: Option<Uuid>,
        /// Changed object's new revision.
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
    expected_project_revision: u64,
    target: MutationTarget,
}

#[derive(Debug, Clone, Copy)]
struct MutationTarget {
    operation: &'static str,
    object_type: ProjectViewObjectType,
    object_id: Uuid,
    deleted: bool,
}

struct MutationContext {
    api_base_url: String,
    identity: ProjectViewIdentity,
    keys: Keys,
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
        .ok_or_else(|| "unsupported: Relay does not advertise Project View v3".to_owned())?;
    if identity.schema != ProjectViewSchema::V3 {
        return Err("unsupported: Project View mutations require schema v3".to_owned());
    }
    identity.require_runtime_ready("Project View mutations")?;
    let context = MutationContext {
        api_base_url,
        identity,
        keys,
    };
    let prepared = prepare_v3_mutation(input)?;
    let event = prepared
        .builder
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
                    .map(|meta| meta.project_revision);
                return Ok(ProjectViewMutationResult::Conflict {
                    expected_project_revision: prepared.expected_project_revision,
                    current_project_revision,
                    message,
                });
            }
            Err(message) => return Err(message),
        };

    let receipt = validate_receipt(parse_receipt(&response, &event)?, prepared.target)?;
    confirm_projection(state, &context, &event, &receipt, prepared.target).await?;

    Ok(ProjectViewMutationResult::Applied {
        event_id: event.id.to_hex(),
        project_revision: receipt.project_revision,
        object_id: receipt.object_id,
        object_revision: receipt.object_revision,
        deleted: receipt.deleted,
    })
}

fn prepare_v3_mutation(input: ProjectViewMutationInput) -> Result<PreparedMutation, String> {
    let (expected_project_revision, target, request, acting_assignment_id, initial_role_level) =
        match input {
            ProjectViewMutationInput::Create {
                expected_project_revision,
                object_type,
                data,
                initial_role_level,
                acting_assignment_id,
            } => {
                if object_type == ProjectViewObjectType::ProjectProfile {
                    return Err(
                        "the Project Profile can only be created by initialization".to_owned()
                    );
                }
                let object_id = Uuid::new_v4();
                let object = create_input_v3(object_type, object_id, data)?;
                let initial_role_level =
                    role_create_level(object_type, initial_role_level, acting_assignment_id)?;
                (
                    expected_project_revision,
                    MutationTarget {
                        operation: "create",
                        object_type,
                        object_id,
                        deleted: false,
                    },
                    ProjectObjectRequestV3::Create(CreateProjectObjectV3 { object }),
                    acting_assignment_id,
                    initial_role_level,
                )
            }
            ProjectViewMutationInput::Update {
                expected_project_revision,
                object_type,
                object_id,
                patch,
                acting_assignment_id,
            } => {
                validate_role_actor_field(object_type, acting_assignment_id)?;
                (
                    expected_project_revision,
                    MutationTarget {
                        operation: "update",
                        object_type,
                        object_id,
                        deleted: false,
                    },
                    ProjectObjectRequestV3::Update(update_input_v3(object_type, object_id, patch)?),
                    acting_assignment_id,
                    None,
                )
            }
            ProjectViewMutationInput::Delete {
                expected_project_revision,
                object_type,
                object_id,
                acting_assignment_id,
            } => {
                validate_role_actor_field(object_type, acting_assignment_id)?;
                (
                    expected_project_revision,
                    MutationTarget {
                        operation: "delete",
                        object_type,
                        object_id,
                        deleted: true,
                    },
                    ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
                        object_type,
                        object_id,
                    }),
                    acting_assignment_id,
                    None,
                )
            }
            ProjectViewMutationInput::Context {
                expected_project_revision,
                object_type,
                object_id,
                context_references,
                acting_assignment_id,
            } => {
                validate_role_actor_field(object_type, acting_assignment_id)?;
                (
                    expected_project_revision,
                    MutationTarget {
                        operation: "update",
                        object_type,
                        object_id,
                        deleted: false,
                    },
                    ProjectObjectRequestV3::Update(update_input_v3(
                        object_type,
                        object_id,
                        json!({ "context_references": context_references }),
                    )?),
                    acting_assignment_id,
                    None,
                )
            }
        };
    let mut command =
        ProjectObjectCommandV3::new(expected_project_revision, acting_assignment_id, request);
    command.initial_role_level = initial_role_level;
    let builder = build_project_object_command(command)
        .map_err(|error| format!("invalid Project View v3 mutation: {error}"))?;
    Ok(PreparedMutation {
        builder,
        expected_project_revision,
        target,
    })
}

fn role_create_level(
    object_type: ProjectViewObjectType,
    initial_role_level: Option<RoleLevel>,
    acting_assignment_id: Option<Uuid>,
) -> Result<Option<RoleLevel>, String> {
    if object_type == ProjectViewObjectType::Role {
        return Ok(Some(initial_role_level.unwrap_or(RoleLevel::Member)));
    }
    if initial_role_level.is_some() || acting_assignment_id.is_some() {
        return Err("Role governance fields are valid only when creating a Role".to_owned());
    }
    Ok(None)
}

fn validate_role_actor_field(
    object_type: ProjectViewObjectType,
    acting_assignment_id: Option<Uuid>,
) -> Result<(), String> {
    if object_type != ProjectViewObjectType::Role && acting_assignment_id.is_some() {
        return Err(
            "acting_assignment_id is valid only when mutating a Role definition".to_owned(),
        );
    }
    Ok(())
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

async fn confirm_projection(
    state: &AppState,
    context: &MutationContext,
    event: &Event,
    receipt: &ProjectViewReceipt,
    target: MutationTarget,
) -> Result<(), String> {
    let meta = read_meta(state, context).await?.ok_or_else(|| {
        "Project View integrity error: successful mutation has no metadata".to_owned()
    })?;
    if meta.project_revision < receipt.project_revision {
        return Err(
            "Project View integrity error: metadata is older than the successful mutation receipt"
                .to_owned(),
        );
    }
    if meta.project_revision == receipt.project_revision
        && !matches!(
            &meta.source,
            V3ProjectionSource::NostrEvent {
                event_id,
                change_id,
            } if *event_id == event.id && *change_id == event.id
        )
    {
        return Err(
            "Project View integrity error: metadata does not identify the submitted mutation"
                .to_owned(),
        );
    }

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
) -> Result<Option<V3MetaProjection>, String> {
    if context.identity.schema != ProjectViewSchema::V3 {
        return Err(
            "Project View integrity error: mutation readback requires schema v3".to_owned(),
        );
    }
    let events = query_at(
        state,
        context,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "#t": [PROJECT_VIEW_V3_META_TAG],
            "limit": 2,
        })],
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_meta_projection(event, &context.identity.relay_pubkey)
            .map(Some)
            .map_err(|error| format!("Project View integrity error: {error}")),
        _ => Err(
            "Project View integrity error: metadata query returned multiple current heads"
                .to_owned(),
        ),
    }
}

async fn read_object(
    state: &AppState,
    context: &MutationContext,
    meta: &V3MetaProjection,
    target: MutationTarget,
) -> Result<MutationObjectProjection, String> {
    let role_entity = target.object_type == ProjectViewObjectType::Role && !target.deleted;
    let coordinate = if role_entity {
        entity_projection_coordinate(
            meta.project_id,
            RoleContinuityEntity::Role,
            target.object_id,
        )
    } else {
        object_projection_coordinate(meta.project_id, target.object_type, target.object_id)
    };
    let projection_tag = if role_entity {
        PROJECT_VIEW_V3_ENTITY_TAG
    } else {
        PROJECT_VIEW_V3_OBJECT_TAG
    };
    let events = query_at(
        state,
        context,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [context.identity.relay_pubkey.to_hex()],
            "#d": [coordinate],
            "#t": [projection_tag],
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
    let projection = parse_confirmed_object(event, context, meta)?;
    if projection.object_type != target.object_type || projection.object_id != target.object_id {
        return Err(
            "Project View integrity error: point query returned a different object".to_owned(),
        );
    }
    if projection.projection_generation != meta.projection_generation
        || projection.project_revision > meta.project_revision
    {
        return Err(
            "Project View integrity error: object projection does not match current metadata"
                .to_owned(),
        );
    }
    Ok(projection)
}

fn parse_confirmed_object(
    event: &Event,
    context: &MutationContext,
    meta: &V3MetaProjection,
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
            let projection =
                parse_entity_projection(event, &context.identity.relay_pubkey, meta.project_id)
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
            let projection = parse_project_object_projection(
                event,
                &context.identity.relay_pubkey,
                meta.project_id,
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
