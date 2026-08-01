use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use buzz_core_pkg::CommunityId;
use buzz_project_view_pkg::{
    InitializeGoal, InitializeMutation, Mutation, MutationRequest, ProjectProfile, ProjectionPlan,
};
use buzz_sdk_pkg::project_view::{
    build_meta_projection, build_object_projection, changed_head_for,
};
use nostr::Keys;
use serde_json::Value;
use tokio::net::TcpListener;
use uuid::Uuid;

use super::*;
use crate::app_state::build_app_state;

#[derive(Clone)]
struct SnapshotServerState {
    relay_pubkey: String,
    meta: Event,
    objects: Vec<Event>,
    meta_queries: Arc<AtomicUsize>,
    snapshot_queries: Arc<AtomicUsize>,
}

async fn snapshot_info(AxumState(state): AxumState<SnapshotServerState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": [PROJECT_VIEW_V1_EXTENSION],
        "self": state.relay_pubkey,
    }))
}

async fn snapshot_query(
    AxumState(state): AxumState<SnapshotServerState>,
    Json(filters): Json<Vec<Value>>,
) -> Json<Value> {
    let filter = filters.first().cloned().unwrap_or_else(|| json!({}));
    if filter.get("buzz_project_view").is_some() {
        state.snapshot_queries.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::to_value(state.objects).expect("serialize object projections"))
    } else {
        state.meta_queries.fetch_add(1, Ordering::SeqCst);
        Json(serde_json::to_value([state.meta]).expect("serialize metadata projection"))
    }
}

async fn spawn_snapshot_server(state: SnapshotServerState) -> String {
    let app = Router::new()
        .route("/info", get(snapshot_info))
        .route("/query", post(snapshot_query))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Project View test server");
    let address = listener.local_addr().expect("read test server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve Project View fixture");
    });
    format!("http://{address}")
}

#[derive(Clone)]
struct IdentityServerState {
    relay_pubkey: String,
}

async fn identity_info(AxumState(state): AxumState<IdentityServerState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": [PROJECT_VIEW_V1_EXTENSION],
        "self": state.relay_pubkey,
    }))
}

async fn unsupported_info() -> Json<Value> {
    Json(json!({
        "supported_extensions": [],
    }))
}

async fn empty_query() -> Json<Value> {
    Json(json!([]))
}

async fn forbidden_query() -> (StatusCode, Json<Value>) {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error": "not authorized"})),
    )
}

async fn spawn_identity_server(
    relay_pubkey: String,
    query_handler: axum::routing::MethodRouter<IdentityServerState>,
) -> String {
    let app = Router::new()
        .route("/info", get(identity_info))
        .route("/query", query_handler)
        .with_state(IdentityServerState { relay_pubkey });
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Project View state test server");
    let address = listener.local_addr().expect("read test server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve Project View state fixture");
    });
    format!("http://{address}")
}

async fn spawn_unsupported_server() -> String {
    let app = Router::new().route("/info", get(unsupported_info));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind unsupported Project View test server");
    let address = listener.local_addr().expect("read test server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve unsupported Project View fixture");
    });
    format!("http://{address}")
}

fn projection_fixture() -> SnapshotServerState {
    let project_id = CommunityId::from_uuid(Uuid::new_v4());
    let mutation = Mutation::new(
        0,
        MutationRequest::Initialize(InitializeMutation {
            profile: ProjectProfile {
                name: "Desktop integration".to_owned(),
                positioning: "Verified snapshots".to_owned(),
                purpose: "Exercise the native client boundary".to_owned(),
                problem: "Untrusted projection input".to_owned(),
                scope: "Project View".to_owned(),
            },
            goals: vec![InitializeGoal {
                id: Uuid::new_v4(),
                title: "Ship".to_owned(),
                desired_outcome: "One consistent desktop read model".to_owned(),
                directions: Vec::new(),
            }],
        }),
    );
    let (state, outcome) = ProjectViewState::new(project_id)
        .reduce(
            &mutation,
            Keys::generate().public_key(),
            DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("fixture timestamp"),
        )
        .expect("initialize fixture");
    let plan = ProjectionPlan::for_mutation(&state, &outcome, [0x44; 32], 1)
        .expect("build projection plan");
    let relay = Keys::generate();
    let mut paired = plan
        .entries()
        .iter()
        .map(|entry| {
            let event = build_object_projection(&plan, entry)
                .expect("build object projection")
                .sign_with_keys(&relay)
                .expect("sign object projection");
            let head = changed_head_for(&plan, entry, &event).expect("build changed head");
            (
                entry.object_type().as_str().to_owned(),
                entry.id(),
                event,
                head,
            )
        })
        .collect::<Vec<_>>();
    paired.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.to_string().cmp(&right.1.to_string()))
    });
    let heads = paired
        .iter()
        .map(|(_, _, _, head)| head.clone())
        .collect::<Vec<_>>();
    let objects = paired.into_iter().map(|(_, _, event, _)| event).collect();
    let meta = build_meta_projection(&plan, &heads)
        .expect("build metadata projection")
        .sign_with_keys(&relay)
        .expect("sign metadata projection");
    SnapshotServerState {
        relay_pubkey: relay.public_key().to_hex(),
        meta,
        objects,
        meta_queries: Arc::new(AtomicUsize::new(0)),
        snapshot_queries: Arc::new(AtomicUsize::new(0)),
    }
}

#[tokio::test]
async fn desktop_snapshot_verifies_and_assembles_read_model() {
    let fixture = projection_fixture();
    let counters = fixture.clone();
    let url = spawn_snapshot_server(fixture).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    let result = load_project_view(&state)
        .await
        .expect("load verified Project View");
    let ProjectViewLoadResult::Ready {
        project_revision,
        active_object_count,
        view,
        ..
    } = result
    else {
        panic!("expected initialized Project View");
    };

    assert_eq!(project_revision, 1);
    assert_eq!(active_object_count, 2);
    assert_eq!(view.expect("legacy View payload").goals.len(), 1);
    assert_eq!(
        counters.meta_queries.load(Ordering::SeqCst),
        2,
        "snapshot must bracket pagination with metadata reads"
    );
    assert_eq!(counters.snapshot_queries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn desktop_snapshot_rejects_a_projection_from_an_unadvertised_signer() {
    let mut fixture = projection_fixture();
    fixture.relay_pubkey = Keys::generate().public_key().to_hex();
    let url = spawn_snapshot_server(fixture).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    let error = load_project_view(&state)
        .await
        .expect_err("wrongly signed Project View must fail closed");
    assert!(error.starts_with("Project View integrity error:"));
}

#[tokio::test]
async fn desktop_reports_capability_initialization_and_permission_states() {
    let unsupported_url = spawn_unsupported_server().await;
    let unsupported_state = build_app_state();
    *unsupported_state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(unsupported_url);
    assert!(matches!(
        load_project_view(&unsupported_state).await,
        Ok(ProjectViewLoadResult::Unsupported)
    ));

    let relay_pubkey = Keys::generate().public_key().to_hex();
    let uninitialized_url = spawn_identity_server(relay_pubkey.clone(), post(empty_query)).await;
    let uninitialized_state = build_app_state();
    *uninitialized_state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(uninitialized_url);
    assert!(matches!(
        load_project_view(&uninitialized_state).await,
        Ok(ProjectViewLoadResult::Uninitialized { .. })
    ));

    let forbidden_url = spawn_identity_server(relay_pubkey, post(forbidden_query)).await;
    let forbidden_state = build_app_state();
    *forbidden_state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(forbidden_url);
    assert!(matches!(
        load_project_view(&forbidden_state).await,
        Ok(ProjectViewLoadResult::Forbidden)
    ));
}
