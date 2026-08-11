use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use buzz_core_pkg::kind::{KIND_NIP43_MEMBERSHIP_LIST, KIND_PROJECT_VIEW_META};
use buzz_core_pkg::CommunityId;
use buzz_project_view_pkg::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3};
use buzz_project_view_pkg::{Goal, ProjectProfile, ProjectViewObjectType, ProjectViewRelations};
use buzz_sdk_pkg::project_view_v3::{
    build_meta_projection, build_project_object_projection, V3EntityCounts, V3ProjectionContext,
    V3ProjectionSource, PROJECT_VIEW_V3_CURRENT_ENTITIES_SCOPE,
};
use chrono::{DateTime, Utc};
use nostr::{EventBuilder, EventId, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use uuid::Uuid;

use super::*;
use crate::app_state::build_app_state;

#[derive(Clone)]
struct SnapshotServerState {
    relay_pubkey: String,
    meta: Event,
    membership: Event,
    objects: Vec<Event>,
    meta_queries: Arc<AtomicUsize>,
    entity_queries: Arc<AtomicUsize>,
}

async fn snapshot_info(AxumState(state): AxumState<SnapshotServerState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": [PROJECT_VIEW_V3_EXTENSION],
        "self": state.relay_pubkey,
    }))
}

async fn snapshot_query(
    AxumState(state): AxumState<SnapshotServerState>,
    Json(filters): Json<Vec<Value>>,
) -> Json<Value> {
    let filter = filters.first().cloned().unwrap_or_else(|| json!({}));
    let kinds = filter
        .get("kinds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if kinds
        .iter()
        .any(|kind| kind.as_u64() == Some(KIND_PROJECT_VIEW_META.into()))
    {
        state.meta_queries.fetch_add(1, Ordering::SeqCst);
        return Json(json!([state.meta]));
    }
    if kinds
        .iter()
        .any(|kind| kind.as_u64() == Some(KIND_NIP43_MEMBERSHIP_LIST.into()))
    {
        return Json(json!([state.membership]));
    }
    if filter
        .get("buzz_project_view")
        .and_then(|value| value.get("scope"))
        .and_then(Value::as_str)
        == Some(PROJECT_VIEW_V3_CURRENT_ENTITIES_SCOPE)
    {
        state.entity_queries.fetch_add(1, Ordering::SeqCst);
        return Json(json!([]));
    }
    Json(serde_json::to_value(state.objects).expect("serialize object projections"))
}

async fn spawn_snapshot_server(state: SnapshotServerState) -> String {
    let app = Router::new()
        .route("/info", get(snapshot_info))
        .route("/query", post(snapshot_query))
        .with_state(state);
    spawn_server(app).await
}

#[derive(Clone)]
struct IdentityServerState {
    relay_pubkey: String,
    extensions: Vec<&'static str>,
    queries: Arc<AtomicUsize>,
    forbidden: bool,
}

async fn identity_info(AxumState(state): AxumState<IdentityServerState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": state.extensions,
        "self": state.relay_pubkey,
    }))
}

async fn identity_query(
    AxumState(state): AxumState<IdentityServerState>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    state.queries.fetch_add(1, Ordering::SeqCst);
    if state.forbidden {
        Err((
            StatusCode::FORBIDDEN,
            Json(json!({"error": "not authorized"})),
        ))
    } else {
        Ok(Json(json!([])))
    }
}

async fn spawn_identity_server(state: IdentityServerState) -> String {
    let app = Router::new()
        .route("/info", get(identity_info))
        .route("/query", post(identity_query))
        .with_state(state);
    spawn_server(app).await
}

async fn spawn_server(app: Router) -> String {
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

fn projection_fixture(meta_signer: Option<&Keys>) -> SnapshotServerState {
    let relay = Keys::generate();
    let actor = Keys::generate().public_key();
    let project_id = CommunityId::from_uuid(Uuid::new_v4());
    let canonical_time =
        DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("fixture timestamp");
    let source_id = EventId::from_byte_array([0x55; 32]);
    let context = V3ProjectionContext {
        project_id,
        projection_generation: 1,
        project_revision: 1,
        source: V3ProjectionSource::System {
            change_id: source_id,
            audit_seq: 1,
        },
        updated_at: canonical_time,
    };
    let profile = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
        id: *project_id.as_uuid(),
        object_type: ProjectViewObjectType::ProjectProfile,
        object_revision: 1,
        project_revision: 1,
        created_at: canonical_time,
        updated_at: canonical_time,
        created_by: actor,
        updated_by: actor,
        data: ProjectViewObjectDataV3::ProjectProfile(ProjectProfile {
            name: "Desktop v3".to_owned(),
            positioning: "Strict latest-version boundary".to_owned(),
            purpose: "Verify the native read model".to_owned(),
            problem: "Legacy schema ambiguity".to_owned(),
            scope: "One Community Project".to_owned(),
            summary: None,
        }),
        relations: ProjectViewRelations::default(),
        context_references: Vec::new(),
    }));
    let profile_event = build_project_object_projection(&context, &profile, None)
        .expect("build v3 profile projection")
        .sign_with_keys(&relay)
        .expect("sign v3 profile projection");
    let goal = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
        id: Uuid::new_v4(),
        object_type: ProjectViewObjectType::Goal,
        object_revision: 1,
        project_revision: 1,
        created_at: canonical_time,
        updated_at: canonical_time,
        created_by: actor,
        updated_by: actor,
        data: ProjectViewObjectDataV3::Goal(Goal {
            title: "Ship v3".to_owned(),
            desired_outcome: "One strict runtime".to_owned(),
            directions: Vec::new(),
            summary: None,
        }),
        relations: ProjectViewRelations::default(),
        context_references: Vec::new(),
    }));
    let goal_event = build_project_object_projection(&context, &goal, None)
        .expect("build v3 goal projection")
        .sign_with_keys(&relay)
        .expect("sign v3 goal projection");
    let membership = EventBuilder::new(Kind::Custom(KIND_NIP43_MEMBERSHIP_LIST as u16), "")
        .tags(vec![
            Tag::parse(["-"]).expect("protection tag"),
            Tag::parse(["member", actor.to_hex().as_str(), "owner"]).expect("owner tag"),
        ])
        .custom_created_at(Timestamp::from(canonical_time.timestamp() as u64))
        .sign_with_keys(&relay)
        .expect("sign membership projection");
    let signer = meta_signer.unwrap_or(&relay);
    let meta = build_meta_projection(
        &context,
        V3EntityCounts {
            active_objects: 2,
            open_proposals: 0,
            active_assignments: 0,
            active_commitments: 0,
            checkpoints: 0,
            handoffs: 0,
        },
        membership.id,
        true,
        &[],
    )
    .expect("build v3 metadata projection")
    .sign_with_keys(signer)
    .expect("sign v3 metadata projection");
    SnapshotServerState {
        relay_pubkey: relay.public_key().to_hex(),
        meta,
        membership,
        objects: vec![profile_event, goal_event],
        meta_queries: Arc::new(AtomicUsize::new(0)),
        entity_queries: Arc::new(AtomicUsize::new(0)),
    }
}

#[tokio::test]
async fn schema_v3_snapshot_is_verified_and_meta_bracketed() {
    let fixture = projection_fixture(None);
    let url = spawn_snapshot_server(fixture.clone()).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    let result = load_project_view(&state).await.expect("load v3 snapshot");
    let ProjectViewLoadResult::Ready {
        schema_version,
        project_revision,
        projection_generation,
        active_object_count,
        objects_v3,
        ..
    } = result
    else {
        panic!("expected ready v3 Project View");
    };
    assert_eq!(schema_version, 3);
    assert_eq!(project_revision, 1);
    assert_eq!(projection_generation, 1);
    assert_eq!(active_object_count, 2);
    assert_eq!(objects_v3.len(), 2);
    assert_eq!(fixture.meta_queries.load(Ordering::SeqCst), 2);
    assert_eq!(fixture.entity_queries.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn v3_snapshot_rejects_a_projection_signed_by_another_key() {
    let wrong_signer = Keys::generate();
    let fixture = projection_fixture(Some(&wrong_signer));
    let url = spawn_snapshot_server(fixture).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    let error = load_project_view(&state)
        .await
        .expect_err("wrong signer must fail closed");
    assert!(error.contains("integrity"));
}

#[tokio::test]
async fn v1_and_v2_only_relays_are_unsupported_without_projection_queries() {
    for extension in ["buzz-project-view-v1", "buzz-project-view-v2"] {
        let queries = Arc::new(AtomicUsize::new(0));
        let state_fixture = IdentityServerState {
            relay_pubkey: Keys::generate().public_key().to_hex(),
            extensions: vec![extension],
            queries: queries.clone(),
            forbidden: false,
        };
        let url = spawn_identity_server(state_fixture).await;
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(url);

        assert!(matches!(
            load_project_view(&state)
                .await
                .expect("load unsupported state"),
            ProjectViewLoadResult::Unsupported
        ));
        assert_eq!(queries.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn semantic_query_availability_is_an_independent_nip11_capability() {
    for (advertised, expected) in [(false, false), (true, true)] {
        let mut extensions = vec![PROJECT_VIEW_V3_EXTENSION];
        if advertised {
            extensions.push(SEMANTIC_QUERY_HTTP_EXTENSION);
        }
        let fixture = IdentityServerState {
            relay_pubkey: Keys::generate().public_key().to_hex(),
            extensions,
            queries: Arc::new(AtomicUsize::new(0)),
            forbidden: false,
        };
        let url = spawn_identity_server(fixture).await;
        let state = build_app_state();
        let identity = read_identity_at(&state, &url)
            .await
            .expect("read NIP-11")
            .expect("v3 identity");
        assert_eq!(identity.semantic_query_http_available, expected);
    }
}

#[tokio::test]
async fn bootstrap_marker_returns_uninitialized_without_projection_queries() {
    let queries = Arc::new(AtomicUsize::new(0));
    let state_fixture = IdentityServerState {
        relay_pubkey: Keys::generate().public_key().to_hex(),
        extensions: vec![PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION],
        queries: queries.clone(),
        forbidden: false,
    };
    let url = spawn_identity_server(state_fixture).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    assert!(matches!(
        load_project_view(&state)
            .await
            .expect("load bootstrap discovery state"),
        ProjectViewLoadResult::Uninitialized { .. }
    ));
    assert_eq!(queries.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_capability_without_meta_fails_integrity_or_preserves_forbidden() {
    for forbidden in [false, true] {
        let state_fixture = IdentityServerState {
            relay_pubkey: Keys::generate().public_key().to_hex(),
            extensions: vec![PROJECT_VIEW_V3_EXTENSION],
            queries: Arc::new(AtomicUsize::new(0)),
            forbidden,
        };
        let url = spawn_identity_server(state_fixture).await;
        let state = build_app_state();
        *state
            .relay_url_override
            .lock()
            .expect("lock Relay override") = Some(url);
        if forbidden {
            assert!(matches!(
                load_project_view(&state)
                    .await
                    .expect("load forbidden v3 state"),
                ProjectViewLoadResult::Forbidden
            ));
        } else {
            let error = load_project_view(&state)
                .await
                .expect_err("ready capability without metadata must fail closed");
            assert!(error.contains("without canonical metadata"));
        }
    }
}

#[tokio::test]
async fn runtime_and_bootstrap_markers_are_mutually_exclusive() {
    let state_fixture = IdentityServerState {
        relay_pubkey: Keys::generate().public_key().to_hex(),
        extensions: vec![
            PROJECT_VIEW_V3_EXTENSION,
            PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION,
        ],
        queries: Arc::new(AtomicUsize::new(0)),
        forbidden: false,
    };
    let url = spawn_identity_server(state_fixture).await;
    let state = build_app_state();
    *state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);

    let error = load_project_view(&state)
        .await
        .expect_err("ambiguous NIP-11 discovery must fail closed");
    assert!(error.contains("both Project View v3 runtime and bootstrap"));
}
