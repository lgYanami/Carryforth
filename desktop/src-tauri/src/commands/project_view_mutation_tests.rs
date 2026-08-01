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
        "supported_extensions": [super::super::project_view::PROJECT_VIEW_V1_EXTENSION],
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
fn v3_resource_mutation_is_guide_only_and_keeps_unknown_kinds() {
    let guide_document_id = Uuid::new_v4();
    let prepared = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Resource,
        data: json!({
            "name": "Release console",
            "resource_kind": "internal-release-console-v7",
            "summary": "Coordinates releases",
            "guide_document_id": guide_document_id,
        }),
    })
    .expect("prepare v3 Resource");
    let event = prepared
        .builder
        .sign_with_keys(&Keys::generate())
        .expect("sign v3 Resource");
    let command = buzz_project_view_pkg::v3::ProjectObjectCommandV3::from_json(&event.content)
        .expect("parse closed v3 command");
    let buzz_project_view_pkg::v3::ProjectObjectRequestV3::Create(create) = command.request else {
        panic!("expected v3 create");
    };
    assert!(matches!(
        create.object,
        buzz_project_view_pkg::v3::NewProjectViewObjectV3::Resource {
            resource_kind,
            guide_document_id: actual_guide,
            ..
        } if resource_kind == "internal-release-console-v7" && actual_guide == guide_document_id
    ));
    assert!(!event.content.contains("locator"));
}

#[test]
fn v3_resource_rejects_the_legacy_locator_shape() {
    let error = prepare_v3_mutation(ProjectViewMutationInput::Create {
        expected_project_revision: 8,
        object_type: ProjectViewObjectType::Resource,
        data: json!({
            "name": "Legacy",
            "resource_type": "repository",
            "locator": {"locator_type": "url", "value": "https://example.test"},
            "description": "must not cross the v3 boundary",
        }),
    })
    .expect_err("legacy Resource must fail closed");
    assert!(error.contains("invalid typed Project View v3 create data"));
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
