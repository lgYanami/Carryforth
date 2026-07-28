//! Typed Project View mutation bridge for the desktop client.

use buzz_core_pkg::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_project_view_pkg::{
    InitializeGoal, NewProjectViewObject, ProjectProfile, ProjectViewObjectType, UpdateMutation,
};
use buzz_sdk_pkg::project_view::{
    build_create, build_delete, build_initialize, build_update, object_projection_coordinate,
    parse_meta_projection, parse_object_projection, MetaProjection, ProjectedObject,
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

use super::project_view::{read_identity_at, ProjectViewIdentity};

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
    let prepared = prepare_mutation(input)?;
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
            let goals = goals
                .into_iter()
                .map(|goal| InitializeGoal {
                    id: Uuid::new_v4(),
                    title: goal.title,
                    desired_outcome: goal.desired_outcome,
                    directions: goal.directions,
                })
                .collect();
            Ok(PreparedMutation {
                builder: build_initialize(profile, goals)
                    .map_err(|error| format!("invalid Project View initialization: {error}"))?,
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
            Ok(PreparedMutation {
                builder: build_create(expected_project_revision, object)
                    .map_err(|error| format!("invalid Project View create: {error}"))?,
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
            Ok(PreparedMutation {
                builder: build_update(expected_project_revision, update)
                    .map_err(|error| format!("invalid Project View update: {error}"))?,
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
        } => Ok(PreparedMutation {
            builder: build_delete(expected_project_revision, object_type, object_id)
                .map_err(|error| format!("invalid Project View delete: {error}"))?,
            expected_project_revision,
            target: Some(MutationTarget {
                object_type,
                object_id,
                deleted: true,
            }),
        }),
    }
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
    if meta.project_revision < receipt.project_revision {
        return Err(
            "Project View integrity error: metadata is older than the successful mutation receipt"
                .to_owned(),
        );
    }
    if meta.project_revision == receipt.project_revision
        && meta.source_event_id.as_ref() != Some(&event.id)
    {
        return Err(
            "Project View integrity error: metadata does not identify the submitted mutation"
                .to_owned(),
        );
    }

    let Some(target) = target else {
        return Ok(());
    };
    let projection = read_object(state, context, &meta, target).await?;
    if projection.object.object_revision() < receipt.object_revision.unwrap_or_default() {
        return Err(
            "Project View integrity error: object projection is older than the mutation receipt"
                .to_owned(),
        );
    }
    if projection.project_revision == receipt.project_revision {
        return match (&projection.object, target.deleted) {
            (ProjectedObject::Tombstone(_), true) | (ProjectedObject::Active(_), false) => Ok(()),
            _ => Err(
                "Project View integrity error: confirmed projection has the wrong deletion state"
                    .to_owned(),
            ),
        };
    }
    if target.deleted && !matches!(projection.object, ProjectedObject::Tombstone(_)) {
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
) -> Result<Option<MetaProjection>, String> {
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
    meta: &MetaProjection,
    target: MutationTarget,
) -> Result<buzz_sdk_pkg::project_view::ObjectProjection, String> {
    let coordinate =
        object_projection_coordinate(meta.project_id, target.object_type, target.object_id);
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
    let projection =
        parse_object_projection(event, &context.identity.relay_pubkey, meta.project_id)
            .map_err(|error| format!("Project View integrity error: {error}"))?;
    if projection.object.object_type() != target.object_type
        || projection.object.id() != target.object_id
    {
        return Err(
            "Project View integrity error: point query returned a different object".to_owned(),
        );
    }
    if projection.projection_generation != meta.projection_generation
        || projection.project_id != meta.project_id
        || projection.project_revision > meta.project_revision
    {
        return Err(
            "Project View integrity error: object projection does not match current metadata"
                .to_owned(),
        );
    }
    Ok(projection)
}

async fn query_at(
    state: &AppState,
    context: &MutationContext,
    filters: &[Value],
) -> Result<Vec<Event>, String> {
    query_relay_at_with_keys(state, &context.api_base_url, filters, &context.keys, None).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::State as AxumState;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use buzz_core_pkg::CommunityId;
    use buzz_project_view_pkg::{
        InitializeMutation, Mutation, MutationRequest, ProjectViewEntry, ProjectViewState,
        ProjectionPlan,
    };
    use buzz_sdk_pkg::project_view::{
        build_meta_projection, build_object_projection, changed_head_for,
    };
    use chrono::{DateTime, Utc};
    use tokio::net::TcpListener;

    use super::*;
    use crate::app_state::build_app_state;

    #[derive(Clone, Copy)]
    enum MutationServerMode {
        Applied,
        Conflict,
    }

    #[derive(Clone)]
    struct MutationServerState {
        relay: Keys,
        mode: MutationServerMode,
        canonical: Arc<Mutex<ProjectViewState>>,
        meta: Arc<Mutex<Option<Event>>>,
        objects: Arc<Mutex<Vec<Event>>>,
        submissions: Arc<AtomicUsize>,
    }

    async fn mutation_info(AxumState(state): AxumState<MutationServerState>) -> Json<Value> {
        Json(json!({
            "supported_extensions": [super::super::project_view::PROJECT_VIEW_EXTENSION],
            "self": state.relay.public_key().to_hex(),
        }))
    }

    async fn mutation_submit(
        AxumState(state): AxumState<MutationServerState>,
        Json(event): Json<Event>,
    ) -> (StatusCode, Json<Value>) {
        state.submissions.fetch_add(1, Ordering::SeqCst);
        if matches!(state.mode, MutationServerMode::Conflict) {
            return (
                StatusCode::CONFLICT,
                Json(json!({"error": "conflict:project_view:revision_conflict"})),
            );
        }

        let mutation = Mutation::from_json(&event.content).expect("parse submitted mutation");
        let current = state
            .canonical
            .lock()
            .expect("lock canonical Project View fixture")
            .clone();
        let (project_state, outcome) = current
            .reduce(
                &mutation,
                event.pubkey,
                DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("fixture timestamp"),
            )
            .expect("reduce submitted mutation");
        let plan = ProjectionPlan::for_mutation(&project_state, &outcome, event.id.to_bytes(), 1)
            .expect("build mutation projection plan");
        let paired = plan
            .entries()
            .iter()
            .map(|entry| {
                let projection = build_object_projection(&plan, entry)
                    .expect("build object projection")
                    .sign_with_keys(&state.relay)
                    .expect("sign object projection");
                let head = changed_head_for(&plan, entry, &projection).expect("bind changed head");
                (projection, head)
            })
            .collect::<Vec<_>>();
        let heads = paired
            .iter()
            .map(|(_, head)| head.clone())
            .collect::<Vec<_>>();
        let objects = paired
            .into_iter()
            .map(|(projection, _)| projection)
            .collect::<Vec<_>>();
        let meta = build_meta_projection(&plan, &heads)
            .expect("build metadata projection")
            .sign_with_keys(&state.relay)
            .expect("sign metadata projection");
        *state
            .canonical
            .lock()
            .expect("lock canonical Project View fixture") = project_state;
        *state.meta.lock().expect("lock metadata fixture") = Some(meta);
        *state.objects.lock().expect("lock object fixtures") = objects;

        let mut receipt = serde_json::Map::new();
        receipt.insert(
            "project_revision".to_owned(),
            Value::from(outcome.project_revision),
        );
        if let [entry] = outcome.changed_entries.as_slice() {
            receipt.insert(
                "object_id".to_owned(),
                Value::String(entry.id().to_string()),
            );
            receipt.insert(
                "object_revision".to_owned(),
                Value::from(entry.object_revision()),
            );
            receipt.insert(
                "deleted".to_owned(),
                Value::Bool(matches!(entry, ProjectViewEntry::Tombstone(_))),
            );
        }

        (
            StatusCode::OK,
            Json(json!({
                "event_id": event.id.to_hex(),
                "accepted": true,
                "message": format!("response:{}", Value::Object(receipt)),
            })),
        )
    }

    async fn mutation_query(
        AxumState(state): AxumState<MutationServerState>,
        Json(filters): Json<Vec<Value>>,
    ) -> Json<Value> {
        let object_query = filters
            .first()
            .and_then(|filter| filter.get("kinds"))
            .and_then(Value::as_array)
            .is_some_and(|kinds| {
                kinds
                    .iter()
                    .any(|kind| kind.as_u64() == Some(KIND_PROJECT_VIEW_OBJECT as u64))
            });
        let events = if object_query {
            state.objects.lock().expect("lock object fixtures").clone()
        } else {
            state
                .meta
                .lock()
                .expect("lock metadata fixture")
                .clone()
                .into_iter()
                .collect::<Vec<_>>()
        };
        Json(serde_json::to_value(events).expect("serialize metadata fixture"))
    }

    async fn spawn_mutation_server_with_state(
        mode: MutationServerMode,
        canonical: ProjectViewState,
    ) -> (String, MutationServerState) {
        let state = MutationServerState {
            relay: Keys::generate(),
            mode,
            canonical: Arc::new(Mutex::new(canonical)),
            meta: Arc::new(Mutex::new(None)),
            objects: Arc::new(Mutex::new(Vec::new())),
            submissions: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route("/info", get(mutation_info))
            .route("/events", post(mutation_submit))
            .route("/query", post(mutation_query))
            .with_state(state.clone());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind Project View mutation server");
        let address = listener
            .local_addr()
            .expect("read Project View mutation server address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("serve Project View mutation fixture");
        });
        (format!("http://{address}"), state)
    }

    async fn spawn_mutation_server(mode: MutationServerMode) -> (String, MutationServerState) {
        spawn_mutation_server_with_state(
            mode,
            ProjectViewState::new(CommunityId::from_uuid(Uuid::new_v4())),
        )
        .await
    }

    fn profile() -> ProjectProfile {
        ProjectProfile {
            name: "Lora".to_owned(),
            positioning: "Shared context".to_owned(),
            purpose: "Coordinate Humans and Agents".to_owned(),
            problem: "Fragmented project context".to_owned(),
            scope: "Project View".to_owned(),
        }
    }

    fn initialized_project_state(project_id: CommunityId) -> ProjectViewState {
        let mutation = Mutation::new(
            0,
            MutationRequest::Initialize(InitializeMutation {
                profile: profile(),
                goals: vec![InitializeGoal {
                    id: Uuid::new_v4(),
                    title: "Foundation".to_owned(),
                    desired_outcome: "An initialized View".to_owned(),
                    directions: Vec::new(),
                }],
            }),
        );
        ProjectViewState::new(project_id)
            .reduce(
                &mutation,
                Keys::generate().public_key(),
                DateTime::<Utc>::from_timestamp(1_799_999_000, 0).expect("fixture timestamp"),
            )
            .expect("initialize canonical fixture")
            .0
    }

    #[test]
    fn initialization_generates_opaque_goal_ids_and_uses_revision_zero() {
        let prepared = prepare_mutation(ProjectViewMutationInput::Initialize {
            profile: profile(),
            goals: vec![ProjectViewInitializationGoalInput {
                title: "Ship".to_owned(),
                desired_outcome: "A usable View".to_owned(),
                directions: vec!["Keep one truth".to_owned()],
            }],
        })
        .expect("prepare initialization");
        let event = prepared
            .builder
            .sign_with_keys(&Keys::generate())
            .expect("sign");
        let mutation = Mutation::from_json(&event.content).expect("parse mutation");
        assert_eq!(mutation.expected_project_revision, 0);
        let MutationRequest::Initialize(initialize) = mutation.request else {
            panic!("expected initialization");
        };
        assert_eq!(initialize.profile, profile());
        assert_eq!(initialize.goals.len(), 1);
        assert_eq!(initialize.goals[0].id.get_version_num(), 4);
    }

    #[test]
    fn create_and_update_are_parsed_by_closed_domain_types() {
        let create = prepare_mutation(ProjectViewMutationInput::Create {
            expected_project_revision: 4,
            object_type: ProjectViewObjectType::Plan,
            data: json!({
                "title": "Client",
                "description": "Human interface",
                "status": "active",
                "under_goal_id": null,
            }),
        })
        .expect("prepare create");
        assert_eq!(create.expected_project_revision, 4);
        assert_eq!(
            create.target.expect("target").object_type,
            ProjectViewObjectType::Plan
        );

        let object_id = Uuid::new_v4();
        let update = prepare_mutation(ProjectViewMutationInput::Update {
            expected_project_revision: 5,
            object_type: ProjectViewObjectType::Issue,
            object_id,
            patch: json!({
                "status": "resolved",
                "about": null,
            }),
        })
        .expect("prepare update");
        assert_eq!(update.target.expect("target").object_id, object_id);
    }

    #[test]
    fn create_rejects_unknown_fields_before_signing() {
        let error = prepare_mutation(ProjectViewMutationInput::Create {
            expected_project_revision: 1,
            object_type: ProjectViewObjectType::Goal,
            data: json!({
                "title": "Ship",
                "desired_outcome": "Done",
                "directions": [],
                "raw_json_escape_hatch": true,
            }),
        })
        .expect_err("unknown field must fail");
        assert!(error.contains("unknown field"));
    }

    #[test]
    fn receipt_requires_the_canonical_response_prefix() {
        let event = build_initialize(
            profile(),
            vec![InitializeGoal {
                id: Uuid::new_v4(),
                title: "Ship".to_owned(),
                desired_outcome: "A usable View".to_owned(),
                directions: Vec::new(),
            }],
        )
        .expect("build initialization")
        .sign_with_keys(&Keys::generate())
        .expect("sign initialization");
        let response = SubmitEventResponse {
            event_id: event.id.to_hex(),
            accepted: true,
            message: json!({"project_revision": 1}).to_string(),
        };

        let error = parse_receipt(&response, &event).expect_err("raw JSON must fail closed");
        assert!(error.contains("canonical `response:` prefix"));
    }

    #[tokio::test]
    async fn desktop_initialization_submits_once_and_confirms_signed_metadata() {
        let (url, fixture) = spawn_mutation_server(MutationServerMode::Applied).await;
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(url);

        let result = execute_mutation(
            ProjectViewMutationInput::Initialize {
                profile: profile(),
                goals: vec![ProjectViewInitializationGoalInput {
                    title: "Ship".to_owned(),
                    desired_outcome: "A usable View".to_owned(),
                    directions: vec!["Keep one truth".to_owned()],
                }],
            },
            &state,
        )
        .await
        .expect("initialize Project View");

        assert!(matches!(
            result,
            ProjectViewMutationResult::Applied {
                project_revision: 1,
                object_id: None,
                object_revision: None,
                deleted: None,
                ..
            }
        ));
        assert_eq!(fixture.submissions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn desktop_create_confirms_the_signed_object_projection() {
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let canonical = initialized_project_state(project_id);
        let (url, fixture) =
            spawn_mutation_server_with_state(MutationServerMode::Applied, canonical).await;
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(url);

        let result = execute_mutation(
            ProjectViewMutationInput::Create {
                expected_project_revision: 1,
                object_type: ProjectViewObjectType::Plan,
                data: json!({
                    "title": "Client",
                    "description": "Human interface",
                    "status": "active",
                    "under_goal_id": null,
                }),
            },
            &state,
        )
        .await
        .expect("create Project View object");

        assert!(matches!(
            result,
            ProjectViewMutationResult::Applied {
                project_revision: 2,
                object_id: Some(_),
                object_revision: Some(1),
                deleted: Some(false),
                ..
            }
        ));
        assert_eq!(fixture.submissions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn revision_conflict_is_typed_and_never_retried() {
        let (url, fixture) = spawn_mutation_server(MutationServerMode::Conflict).await;
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(url);

        let result = execute_mutation(
            ProjectViewMutationInput::Create {
                expected_project_revision: 7,
                object_type: ProjectViewObjectType::Plan,
                data: json!({
                    "title": "Client",
                    "description": "Human interface",
                    "status": "active",
                    "under_goal_id": null,
                }),
            },
            &state,
        )
        .await
        .expect("return typed conflict");

        assert!(matches!(
            result,
            ProjectViewMutationResult::Conflict {
                expected_project_revision: 7,
                current_project_revision: None,
                ..
            }
        ));
        assert_eq!(
            fixture.submissions.load(Ordering::SeqCst),
            1,
            "a stale Human intent must never be retried automatically"
        );
    }
}
