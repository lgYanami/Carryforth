//! Verified, read-only Project View bridge for the desktop client.

use std::collections::HashSet;
use std::time::Duration;

use buzz_core_pkg::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_core_pkg::PublicKey;
use buzz_project_view_pkg::{
    ProjectView, ProjectViewEntry, ProjectViewObjectType, ProjectViewState,
};
use buzz_sdk_pkg::project_view::{
    parse_meta_projection, parse_object_projection, MetaProjection, ObjectProjection,
    ProjectedObject,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::State;

use crate::app_state::AppState;
use crate::relay::{
    classify_request_error, parse_json_response, query_relay, relay_api_base_url_with_override,
    relay_error_message,
};

pub(crate) const PROJECT_VIEW_EXTENSION: &str = "buzz-project-view-v1";
const SNAPSHOT_PAGE_SIZE: usize = 500;
const SNAPSHOT_MAX_ATTEMPTS: usize = 3;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProjectViewIdentity {
    pub(crate) relay_pubkey: PublicKey,
}

#[derive(Debug, Deserialize)]
struct Nip11Document {
    #[serde(default)]
    supported_extensions: Vec<String>,
    #[serde(rename = "self")]
    relay_self: Option<String>,
}

struct ProjectSnapshot {
    meta: MetaProjection,
    view: ProjectView,
}

#[derive(Debug)]
enum ProjectViewReadError {
    Forbidden,
    Conflict(String),
    Other(String),
}

type ProjectViewReadResult<T> = Result<T, ProjectViewReadError>;

/// Desktop-facing state of the active Community's Project View.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectViewLoadResult {
    /// The Relay does not advertise Project View support.
    Unsupported,
    /// The current identity may not read this Community's Project View.
    Forbidden,
    /// Project View is supported but has not been initialized.
    Uninitialized {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
    },
    /// A complete, internally consistent and cryptographically verified view.
    Ready {
        /// Canonical Relay signing identity established by NIP-11.
        relay_pubkey: String,
        /// Current optimistic-concurrency revision.
        project_revision: u64,
        /// Current projection generation.
        projection_generation: u64,
        /// Number of active objects declared by the metadata projection.
        active_object_count: u32,
        /// Canonical server time of the projected state.
        updated_at: DateTime<Utc>,
        /// Deterministically assembled Project View hierarchy.
        view: Box<ProjectView>,
    },
}

/// Load the active Community's complete, verified Project View snapshot.
#[tauri::command]
pub async fn get_project_view(state: State<'_, AppState>) -> Result<ProjectViewLoadResult, String> {
    load_project_view(&state).await
}

async fn load_project_view(state: &AppState) -> Result<ProjectViewLoadResult, String> {
    let Some(identity) = read_identity(state).await? else {
        return Ok(ProjectViewLoadResult::Unsupported);
    };

    match fetch_consistent_snapshot(state, identity).await {
        Ok(Some(ProjectSnapshot { meta, view })) => Ok(ProjectViewLoadResult::Ready {
            relay_pubkey: identity.relay_pubkey.to_hex(),
            project_revision: meta.project_revision,
            projection_generation: meta.projection_generation,
            active_object_count: meta.active_object_count,
            updated_at: meta.updated_at,
            view: Box::new(view),
        }),
        Ok(None) => Ok(ProjectViewLoadResult::Uninitialized {
            relay_pubkey: identity.relay_pubkey.to_hex(),
        }),
        Err(ProjectViewReadError::Forbidden) => Ok(ProjectViewLoadResult::Forbidden),
        Err(ProjectViewReadError::Conflict(message))
        | Err(ProjectViewReadError::Other(message)) => Err(message),
    }
}

async fn read_identity(state: &AppState) -> Result<Option<ProjectViewIdentity>, String> {
    read_identity_at(state, &relay_api_base_url_with_override(state)).await
}

pub(crate) async fn read_identity_at(
    state: &AppState,
    api_base_url: &str,
) -> Result<Option<ProjectViewIdentity>, String> {
    crate::relay_admission::wait_for_rate_limit().await;
    let url = format!("{}/info", api_base_url.trim_end_matches('/'));
    let response = state
        .http_client
        .get(url)
        .header(reqwest::header::ACCEPT, "application/nostr+json")
        .send()
        .await
        .map_err(|error| classify_request_error(&error))?;
    if !response.status().is_success() {
        return Err(relay_error_message(response).await);
    }
    let info: Nip11Document = parse_json_response(response).await?;
    if !info
        .supported_extensions
        .iter()
        .any(|extension| extension == PROJECT_VIEW_EXTENSION)
    {
        return Ok(None);
    }

    let relay_self = info.relay_self.ok_or_else(|| {
        integrity_error("NIP-11 advertises Project View without a Relay `self` key")
    })?;
    let relay_pubkey = PublicKey::from_hex(&relay_self)
        .map_err(|error| integrity_error(format!("invalid NIP-11 Relay `self`: {error}")))?;
    if relay_pubkey.to_hex() != relay_self {
        return Err(integrity_error(
            "NIP-11 Relay `self` is not canonical lowercase hex",
        ));
    }
    Ok(Some(ProjectViewIdentity { relay_pubkey }))
}

async fn fetch_consistent_snapshot(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<ProjectSnapshot>> {
    for attempt in 0..SNAPSHOT_MAX_ATTEMPTS {
        match fetch_snapshot_once(state, identity).await {
            Ok(snapshot) => return Ok(snapshot),
            Err(ProjectViewReadError::Conflict(_)) if attempt + 1 < SNAPSHOT_MAX_ATTEMPTS => {
                let backoff_ms = 25_u64 << attempt;
                tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(ProjectViewReadError::Conflict(
        "Project View changed during every bounded snapshot attempt".to_owned(),
    ))
}

async fn fetch_snapshot_once(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<ProjectSnapshot>> {
    let Some(meta) = read_meta(state, identity).await? else {
        return Ok(None);
    };

    let mut after: Option<(String, String)> = None;
    let mut entries = Vec::new();
    let mut object_ids = HashSet::new();

    loop {
        let mut extension = json!({
            "revision": meta.project_revision,
            "projection_generation": meta.projection_generation,
        });
        if let Some((object_type, object_id)) = &after {
            extension["after"] = json!({
                "object_type": object_type,
                "object_id": object_id,
            });
        }
        let filter = json!({
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [identity.relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-active"],
            "limit": SNAPSHOT_PAGE_SIZE,
            "buzz_project_view": extension,
        });
        let page = query_project_view(state, &[filter]).await?;
        if page.len() > SNAPSHOT_PAGE_SIZE {
            return Err(integrity_read_error(
                "snapshot page exceeded the requested page size",
            ));
        }

        for event in &page {
            let projection =
                parse_object_projection(event, &identity.relay_pubkey, meta.project_id)
                    .map_err(|error| integrity_read_error(error.to_string()))?;
            validate_object_against_meta(&projection, &meta)?;
            let object = match projection.object {
                ProjectedObject::Active(object) => *object,
                ProjectedObject::Tombstone(_) => {
                    return Err(integrity_read_error(
                        "active snapshot query returned a tombstone",
                    ));
                }
            };
            let cursor = (
                object.object_type.as_str().to_owned(),
                object.id.to_string(),
            );
            if after.as_ref().is_some_and(|previous| cursor <= *previous) {
                return Err(integrity_read_error(
                    "snapshot page order is not strictly increasing",
                ));
            }
            if !object_ids.insert(object.id) {
                return Err(integrity_read_error(
                    "snapshot contains a duplicate active object id",
                ));
            }
            after = Some(cursor);
            entries.push(ProjectViewEntry::Active(object));
            if entries.len() > meta.active_object_count as usize {
                return Err(integrity_read_error(
                    "snapshot contains more objects than metadata declares",
                ));
            }
        }

        if page.len() < SNAPSHOT_PAGE_SIZE {
            break;
        }
    }

    let final_meta = read_meta(state, identity)
        .await?
        .ok_or_else(|| conflict_error("Project View metadata disappeared"))?;
    if final_meta.projection_generation != meta.projection_generation
        || final_meta.project_revision != meta.project_revision
        || final_meta.event_id != meta.event_id
    {
        return Err(conflict_error(
            "Project View changed while assembling the snapshot",
        ));
    }
    if entries.len() != meta.active_object_count as usize {
        return Err(integrity_read_error(format!(
            "snapshot contains {} active objects but metadata declares {}",
            entries.len(),
            meta.active_object_count
        )));
    }

    let initialized_at = entries.iter().find_map(|entry| match entry {
        ProjectViewEntry::Active(object)
            if object.object_type == ProjectViewObjectType::ProjectProfile =>
        {
            Some(object.created_at)
        }
        _ => None,
    });
    let state = ProjectViewState::from_snapshot(
        meta.project_id,
        meta.project_revision,
        initialized_at,
        Some(meta.updated_at),
        entries,
    )
    .map_err(|error| integrity_read_error(format!("invalid Project View snapshot: {error}")))?;
    let view = ProjectView::assemble(&state)
        .map_err(|error| integrity_read_error(format!("cannot assemble Project View: {error}")))?;
    Ok(Some(ProjectSnapshot { meta, view }))
}

async fn read_meta(
    state: &AppState,
    identity: ProjectViewIdentity,
) -> ProjectViewReadResult<Option<MetaProjection>> {
    let events = query_project_view(
        state,
        &[json!({
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [identity.relay_pubkey.to_hex()],
            "limit": 2,
        })],
    )
    .await?;
    match events.as_slice() {
        [] => Ok(None),
        [event] => parse_meta_projection(event, &identity.relay_pubkey)
            .map(Some)
            .map_err(|error| integrity_read_error(error.to_string())),
        _ => Err(integrity_read_error(
            "metadata query returned multiple current heads",
        )),
    }
}

async fn query_project_view(
    state: &AppState,
    filters: &[serde_json::Value],
) -> ProjectViewReadResult<Vec<Event>> {
    query_relay(state, filters).await.map_err(|message| {
        if message.starts_with("relay returned 403") {
            ProjectViewReadError::Forbidden
        } else if message.starts_with("relay returned 409") {
            conflict_error("Project View changed during snapshot pagination")
        } else {
            ProjectViewReadError::Other(message)
        }
    })
}

fn validate_object_against_meta(
    projection: &ObjectProjection,
    meta: &MetaProjection,
) -> ProjectViewReadResult<()> {
    if projection.project_id != meta.project_id {
        return Err(integrity_read_error(
            "object projection belongs to a different project than metadata",
        ));
    }
    if projection.projection_generation != meta.projection_generation {
        return Err(conflict_error(
            "object projection generation differs from current metadata",
        ));
    }
    if projection.project_revision > meta.project_revision {
        return Err(integrity_read_error(
            "object projection is newer than current metadata",
        ));
    }
    Ok(())
}

fn conflict_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Conflict(message.into())
}

fn integrity_read_error(message: impl Into<String>) -> ProjectViewReadError {
    ProjectViewReadError::Other(integrity_error(message))
}

fn integrity_error(message: impl Into<String>) -> String {
    format!("Project View integrity error: {}", message.into())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use axum::extract::State as AxumState;
    use axum::http::StatusCode;
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use buzz_core_pkg::CommunityId;
    use buzz_project_view_pkg::{
        InitializeGoal, InitializeMutation, Mutation, MutationRequest, ProjectProfile,
        ProjectionPlan,
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
            "supported_extensions": [PROJECT_VIEW_EXTENSION],
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
            "supported_extensions": [PROJECT_VIEW_EXTENSION],
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
        assert_eq!(view.goals.len(), 1);
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
        let uninitialized_url =
            spawn_identity_server(relay_pubkey.clone(), post(empty_query)).await;
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
}
