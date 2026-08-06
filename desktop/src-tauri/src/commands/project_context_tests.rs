use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use buzz_core_pkg::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_project_view_pkg::v3::{
    ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3, ProjectViewTombstoneV3,
};
use buzz_project_view_pkg::{
    Priority, ProjectViewObjectType, ProjectViewRelations, Requirement, RequirementStatus,
};
use nostr::Keys;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::project_view_hydration::project_view_coordinate_detail;
use super::*;
use crate::app_state::build_app_state;
use crate::relay::{RelayHttpError, RelayHttpErrorCategory};

const PROJECT_ID: &str = "3f2b2e8f-3f1d-4e91-91ac-5e5f1f0a2d77";
const REQUIREMENT_ID: &str = "0fd3a16e-4da4-48c1-aa6a-63b3661091d0";
const RESOURCE_ID: &str = "e0a286dd-4391-4a45-b843-62b2c57b014a";
const DOCUMENT_ID: &str = "9c23f672-a397-42d1-b933-104ba2674f26";

fn uuid(value: &str) -> Uuid {
    Uuid::parse_str(value).expect("fixture UUID")
}

fn requirement_coordinate() -> ProjectContextCoordinate {
    ProjectContextCoordinate::ProjectViewObject {
        object_type: ProjectViewObjectType::Requirement,
        object_id: uuid(REQUIREMENT_ID),
    }
}

fn resource_coordinate() -> ProjectContextCoordinate {
    ProjectContextCoordinate::ProjectViewObject {
        object_type: ProjectViewObjectType::Resource,
        object_id: uuid(RESOURCE_ID),
    }
}

fn document_coordinate() -> ProjectContextCoordinate {
    ProjectContextCoordinate::Document {
        document_id: uuid(DOCUMENT_ID),
    }
}

fn projection_fixture(path: &str) -> Event {
    let content = match path {
        "context_meta" => include_str!(
            "../../../../docs/nips/fixtures/project-context-edge-v1/events/meta-incremental.json"
        ),
        "context_meta_reproject" => include_str!(
            "../../../../docs/nips/fixtures/project-context-edge-v1/events/meta-reset-reproject.json"
        ),
        "binding" => include_str!(
            "../../../../docs/nips/fixtures/project-context-edge-v1/events/binding-active.json"
        ),
        "document_meta" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/meta-incremental.json"
        ),
        "document_head" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/head-active.json"
        ),
        _ => panic!("unknown fixture"),
    };
    serde_json::from_str(content).expect("parse signed fixture")
}

#[derive(Debug, Clone, Copy)]
enum BindingPagesMode {
    Normal,
    Empty,
    RepeatFirstPage,
}

#[derive(Clone)]
struct QueryServerState {
    relay_pubkey: String,
    supported_extensions: Vec<String>,
    context_meta_sequence: Vec<Event>,
    binding: Event,
    document_meta: Event,
    document_head: Event,
    context_meta_queries: Arc<AtomicUsize>,
    recorded_filters: Arc<std::sync::Mutex<Vec<Value>>>,
    binding_pages_mode: BindingPagesMode,
}

async fn query_info(AxumState(state): AxumState<QueryServerState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": state.supported_extensions,
        "self": state.relay_pubkey,
    }))
}

async fn query_fixture(
    AxumState(state): AxumState<QueryServerState>,
    Json(filters): Json<Vec<Value>>,
) -> Response {
    let filter = filters.first().cloned().unwrap_or_else(|| json!({}));
    state
        .recorded_filters
        .lock()
        .expect("record query filter")
        .push(filter.clone());
    let kind = filter
        .get("kinds")
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.first())
        .and_then(Value::as_u64);
    match kind {
        Some(value) if value == u64::from(KIND_PROJECT_CONTEXT_META) => {
            if state.context_meta_sequence.is_empty() {
                return Json(json!([])).into_response();
            }
            let query = state.context_meta_queries.fetch_add(1, Ordering::SeqCst);
            let event =
                state.context_meta_sequence[query % state.context_meta_sequence.len()].clone();
            Json(json!([event])).into_response()
        }
        Some(value) if value == u64::from(KIND_PROJECT_CONTEXT_EDGE_BINDING) => {
            let page = filter.get("page").and_then(Value::as_u64).unwrap_or(1);
            let include = match state.binding_pages_mode {
                BindingPagesMode::Normal => page == 1,
                BindingPagesMode::Empty => false,
                BindingPagesMode::RepeatFirstPage => true,
            };
            Json(if include {
                json!([state.binding])
            } else {
                json!([])
            })
            .into_response()
        }
        Some(value) if value == u64::from(KIND_PROJECT_DOCUMENT_META) => {
            Json(json!([state.document_meta])).into_response()
        }
        Some(value) if value == u64::from(KIND_PROJECT_DOCUMENT_HEAD) => {
            Json(json!([state.document_head])).into_response()
        }
        Some(value)
            if value == u64::from(KIND_PROJECT_VIEW_META)
                || value == u64::from(KIND_PROJECT_VIEW_OBJECT) =>
        {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "Project View hydration unavailable"})),
            )
                .into_response()
        }
        _ => Json(json!([])).into_response(),
    }
}

async fn spawn_query_server(state: QueryServerState) -> String {
    let app = Router::new()
        .route("/info", get(query_info))
        .route("/query", post(query_fixture))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind Project Context test server");
    let address = listener.local_addr().expect("read test server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve Project Context fixture");
    });
    format!("http://{address}")
}

fn query_server_state(
    supported_extensions: &[&str],
    context_meta_sequence: Vec<Event>,
    binding_pages_mode: BindingPagesMode,
) -> QueryServerState {
    let relay_pubkey = context_meta_sequence
        .first()
        .cloned()
        .unwrap_or_else(|| projection_fixture("context_meta"))
        .pubkey
        .to_hex();
    QueryServerState {
        relay_pubkey,
        supported_extensions: supported_extensions
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        context_meta_sequence,
        binding: projection_fixture("binding"),
        document_meta: projection_fixture("document_meta"),
        document_head: projection_fixture("document_head"),
        context_meta_queries: Arc::new(AtomicUsize::new(0)),
        recorded_filters: Arc::new(std::sync::Mutex::new(Vec::new())),
        binding_pages_mode,
    }
}

async fn captured_context(state: QueryServerState) -> (Arc<AppState>, ProjectContextReadContext) {
    let url = spawn_query_server(state).await;
    let app_state = Arc::new(build_app_state());
    *app_state
        .relay_url_override
        .lock()
        .expect("lock Relay override") = Some(url);
    let context = capture_context("community-a".to_owned(), &app_state)
        .await
        .expect("capture Context read boundary");
    (app_state, context)
}

#[test]
fn desktop_query_payload_is_closed_and_uses_camel_case_fields() {
    let parsed: QueryProjectContextInput = serde_json::from_value(json!({
        "communityKey": "community-a",
        "query": {
            "type": "exact",
            "coordinates": [
                {
                    "type": "project_view_object",
                    "objectType": "resource",
                    "objectId": RESOURCE_ID,
                },
                {
                    "type": "project_view_object",
                    "objectType": "requirement",
                    "objectId": REQUIREMENT_ID,
                }
            ]
        }
    }))
    .expect("deserialize closed Desktop query");
    let canonical = canonicalize_query(parsed.query).expect("canonicalize exact query");
    assert_eq!(
        canonical.coordinates(),
        &[requirement_coordinate(), resource_coordinate()]
    );

    let extra = serde_json::from_value::<QueryProjectContextInput>(json!({
        "communityKey": "community-a",
        "query": {
            "type": "incident",
            "coordinate": {
                "type": "document",
                "documentId": DOCUMENT_ID,
                "rawEvent": "forbidden",
            }
        }
    }));
    assert!(extra.is_err(), "unknown coordinate fields must be rejected");
}

#[test]
fn query_canonicalization_enforces_cardinality_and_duplicates() {
    let exact = canonicalize_query(ProjectContextQueryDto::Exact {
        coordinates: vec![
            coordinate_dto(&resource_coordinate()),
            coordinate_dto(&requirement_coordinate()),
        ],
    })
    .expect("canonical exact");
    assert_eq!(
        exact.coordinates(),
        &[requirement_coordinate(), resource_coordinate()]
    );

    let too_small = canonicalize_query(ProjectContextQueryDto::Exact {
        coordinates: vec![coordinate_dto(&requirement_coordinate())],
    })
    .expect_err("exact requires a complete Edge set");
    assert_eq!(too_small.code, "invalid_input");

    let duplicate = canonicalize_query(ProjectContextQueryDto::ContainsAll {
        coordinates: vec![
            coordinate_dto(&document_coordinate()),
            coordinate_dto(&document_coordinate()),
        ],
    })
    .expect_err("contains-all must reject duplicate coordinates");
    assert_eq!(duplicate.code, "invalid_input");

    assert!(matches!(
        canonicalize_query(ProjectContextQueryDto::ContainsAll {
            coordinates: Vec::new(),
        }),
        Ok(CanonicalContextQuery::ContainsAll(coordinates)) if coordinates.is_empty()
    ));
}

#[test]
fn query_filters_use_exact_edge_or_canonical_coordinate_indexes() {
    let relay = Keys::generate().public_key();
    let project_id = uuid(PROJECT_ID);
    let coordinates = vec![requirement_coordinate(), resource_coordinate()];
    let exact = CanonicalContextQuery::Exact(coordinates.clone());
    let exact_filter = binding_filter(&relay, project_id, &exact).expect("exact filter");
    let edge_key = EdgeKey::derive(project_id, &coordinates).expect("derive Edge key");
    assert_eq!(
        exact_filter["#g"],
        json!([project_context_edge_coordinate(
            CommunityId::from_uuid(project_id),
            edge_key,
        )])
    );
    assert!(exact_filter.get("#c").is_none());

    let incident = CanonicalContextQuery::Incident(requirement_coordinate());
    let incident_filter = binding_filter(&relay, project_id, &incident).expect("incident filter");
    assert_eq!(
        incident_filter["#c"],
        json!([requirement_coordinate().tag_value(project_id)])
    );

    let contains =
        CanonicalContextQuery::ContainsAll(vec![requirement_coordinate(), resource_coordinate()]);
    let contains_filter = binding_filter(&relay, project_id, &contains).expect("contains filter");
    assert_eq!(
        contains_filter["#c"],
        json!([requirement_coordinate().tag_value(project_id)])
    );

    let all = binding_filter(
        &relay,
        project_id,
        &CanonicalContextQuery::ContainsAll(Vec::new()),
    )
    .expect("all filter");
    assert!(all.get("#c").is_none());
    assert!(all.get("#g").is_none());
}

#[test]
fn post_aggregation_query_semantics_do_not_merge_exact_sets() {
    let project_id = uuid(PROJECT_ID);
    let a = requirement_coordinate();
    let b = resource_coordinate();
    let c = document_coordinate();
    let ab = ProjectContextEdge::from_snapshot(
        project_id,
        vec![a.clone(), b.clone()],
        vec![Uuid::new_v4()],
    )
    .expect("AB Edge");
    let abc = ProjectContextEdge::from_snapshot(
        project_id,
        vec![a.clone(), b.clone(), c.clone()],
        vec![Uuid::new_v4()],
    )
    .expect("ABC Edge");

    let mut exact_collision = vec![ab.clone(), abc.clone()];
    let error = apply_query_semantics(
        &CanonicalContextQuery::Exact(vec![a.clone(), b.clone()]),
        project_id,
        &mut exact_collision,
    )
    .expect_err("exact cannot accept a superset beside AB");
    assert_eq!(error.code, "verification_failed");

    let mut contains = vec![ab, abc];
    apply_query_semantics(
        &CanonicalContextQuery::ContainsAll(vec![a, b, c]),
        project_id,
        &mut contains,
    )
    .expect("contains-all filter");
    assert_eq!(contains.len(), 1);
    assert_eq!(contains[0].coordinates().len(), 3);
}

#[test]
fn detail_mapping_distinguishes_active_tombstone_and_unavailable() {
    let actor = Keys::generate().public_key();
    let now = DateTime::<Utc>::from_timestamp(1_800_000_000, 0).expect("timestamp");
    let coordinate = requirement_coordinate();
    let active = ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
        id: uuid(REQUIREMENT_ID),
        object_type: ProjectViewObjectType::Requirement,
        object_revision: 3,
        project_revision: 9,
        created_at: now,
        updated_at: now,
        created_by: actor,
        updated_by: actor,
        data: ProjectViewObjectDataV3::Requirement(Requirement {
            title: "Desktop Context graph".to_owned(),
            description: "Read-only graph".to_owned(),
            status: RequirementStatus::Ready,
            priority: Priority::High,
        }),
        relations: ProjectViewRelations::default(),
        context_references: Vec::new(),
    }));
    let active_detail = project_view_coordinate_detail(&coordinate, &active);
    assert_eq!(active_detail.state, ProjectContextDetailState::Active);
    assert_eq!(
        active_detail.title.as_deref(),
        Some("Desktop Context graph")
    );
    assert_eq!(active_detail.object_revision, Some(3));

    let tombstone = ProjectViewEntryV3::Tombstone(ProjectViewTombstoneV3 {
        id: uuid(REQUIREMENT_ID),
        object_type: ProjectViewObjectType::Requirement,
        object_revision: 4,
        project_revision: 10,
        created_at: now,
        deleted_at: now,
        created_by: actor,
        deleted_by: actor,
    });
    let tombstone_detail = project_view_coordinate_detail(&coordinate, &tombstone);
    assert_eq!(
        tombstone_detail.state,
        ProjectContextDetailState::Tombstoned
    );
    assert_eq!(tombstone_detail.object_revision, Some(4));

    assert_eq!(
        unavailable_coordinate(&coordinate).state,
        ProjectContextDetailState::Unavailable
    );

    let tombstoned_document = DocumentMetadata::Tombstoned {
        document_revision: 9,
        deleted_at: now,
        deleted_by: actor,
    };
    assert_eq!(
        document_coordinate_detail(&document_coordinate(), Some(&tombstoned_document)).state,
        ProjectContextDetailState::Tombstoned
    );
    let context_documents = BTreeSet::from([uuid(DOCUMENT_ID)]);
    let documents = BTreeMap::from([(uuid(DOCUMENT_ID), tombstoned_document)]);
    let error = validate_context_documents_active(&context_documents, &documents)
        .expect_err("a tombstoned Context Document is a verified contradiction");
    assert_eq!(error.code, "verification_failed");
}

#[test]
fn typed_http_errors_do_not_retain_relay_bodies() {
    let mapped = ProjectContextCommandError::from_http(RelayHttpError {
        status: Some(503),
        category: RelayHttpErrorCategory::Unavailable,
        message: "secret Relay response body".to_owned(),
        retry_after_seconds: Some(7),
        request_may_have_reached_relay: false,
    });
    let encoded = serde_json::to_string(&mapped).expect("serialize Context error");
    assert_eq!(mapped.code, "unavailable");
    assert_eq!(mapped.retry_after_seconds, Some(7));
    assert!(!encoded.contains("secret Relay response body"));

    assert_eq!(
        ProjectContextCommandError::from_identity_error("relay returned 503").code,
        "unavailable"
    );
    assert_eq!(
        ProjectContextCommandError::from_identity_error("relay returned 403").code,
        "restricted"
    );
}

#[tokio::test]
async fn verified_query_reads_when_edge_capability_is_off_and_keeps_bodies_out() {
    let server = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Normal,
    );
    let evidence = server.clone();
    let (state, context) = captured_context(server).await;
    assert!(context.identity.project_context_reference_supported);
    assert!(!context.identity.project_context_edge_supported);

    let query = CanonicalContextQuery::ContainsAll(Vec::new());
    let snapshot = read_edge_snapshot(&state, &context, &query)
        .await
        .expect("read stable Context Edge snapshot");
    let result = build_result(&state, &context, &query, snapshot)
        .await
        .expect("hydrate body-free result");

    assert_eq!(result.community_key, "community-a");
    assert_eq!(result.project_id, uuid(PROJECT_ID));
    assert!(!result.context.capability_enabled);
    assert_eq!(result.edges.len(), 1);
    assert_eq!(result.edges[0].coordinate_keys.len(), 2);
    assert_eq!(result.edges[0].context_document_ids, [uuid(DOCUMENT_ID)]);
    assert_eq!(
        result.project_view_observation.state,
        ProjectContextSourceState::Unavailable
    );
    assert!(result
        .coordinate_details
        .iter()
        .all(|detail| detail.state == ProjectContextDetailState::Unavailable));
    assert_eq!(
        result.document_observation.state,
        ProjectContextSourceState::Observed
    );
    assert_eq!(result.document_details.len(), 1);
    assert_eq!(
        result.document_details[0].state,
        ProjectContextDetailState::Active
    );

    let encoded = serde_json::to_string(&result).expect("serialize query result");
    assert!(!encoded.contains("contentMarkdown"));
    assert!(!encoded.contains("content_markdown"));
    assert!(!encoded.contains("fetchCommand"));
    assert!(!encoded.contains("rawEvent"));

    let filters = evidence
        .recorded_filters
        .lock()
        .expect("read query evidence");
    let binding_pages = filters
        .iter()
        .filter(|filter| filter["kinds"] == json!([KIND_PROJECT_CONTEXT_EDGE_BINDING]))
        .map(|filter| filter["page"].as_u64().expect("binding page"))
        .collect::<Vec<_>>();
    assert_eq!(binding_pages, [1, 2]);
    assert_eq!(evidence.context_meta_queries.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn context_capabilities_are_independent_in_both_directions() {
    let edge_only = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Normal,
    );
    let (_state, context) = captured_context(edge_only).await;
    assert!(!context.identity.project_context_reference_supported);
    assert!(context.identity.project_context_edge_supported);

    let both = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Normal,
    );
    let (_state, context) = captured_context(both).await;
    assert!(context.identity.project_context_reference_supported);
    assert!(context.identity.project_context_edge_supported);

    let neither = query_server_state(
        &["buzz-project-view-v3", "buzz-project-document-v1"],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Normal,
    );
    let (_state, context) = captured_context(neither).await;
    assert!(!context.identity.project_context_reference_supported);
    assert!(!context.identity.project_context_edge_supported);
}

#[tokio::test]
async fn capability_off_without_verified_context_meta_is_unavailable() {
    let server = query_server_state(
        &["buzz-project-view-v3", "buzz-project-document-v1"],
        Vec::new(),
        BindingPagesMode::Empty,
    );
    let (state, context) = captured_context(server).await;
    let error = read_edge_snapshot(
        &state,
        &context,
        &CanonicalContextQuery::ContainsAll(Vec::new()),
    )
    .await
    .expect_err("capability advertising cannot invent an empty catalog");
    assert_eq!(error.code, "unavailable");
}

#[tokio::test]
async fn exact_query_can_return_a_verified_zero_match_without_inventing_an_edge() {
    let server = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Empty,
    );
    let (state, context) = captured_context(server).await;
    let query = CanonicalContextQuery::Exact(vec![requirement_coordinate(), document_coordinate()]);
    let snapshot = read_edge_snapshot(&state, &context, &query)
        .await
        .expect("read an exact zero-match result");
    assert!(snapshot.edges.is_empty());

    let result = build_result(&state, &context, &query, snapshot)
        .await
        .expect("retain query anchors without creating topology");
    assert!(result.edges.is_empty());
    assert_eq!(result.coordinate_details.len(), 2);
}

#[tokio::test]
async fn exact_incident_and_contains_all_return_the_same_verified_edge_membership() {
    let server = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::Normal,
    );
    let (state, context) = captured_context(server).await;
    let coordinates = vec![requirement_coordinate(), resource_coordinate()];
    let queries = [
        CanonicalContextQuery::Exact(coordinates.clone()),
        CanonicalContextQuery::Incident(requirement_coordinate()),
        CanonicalContextQuery::ContainsAll(vec![requirement_coordinate()]),
    ];
    let expected_key = EdgeKey::derive(uuid(PROJECT_ID), &coordinates).expect("derive Edge key");

    for query in queries {
        let snapshot = read_edge_snapshot(&state, &context, &query)
            .await
            .expect("read query mode");
        assert_eq!(snapshot.edges.len(), 1);
        assert_eq!(snapshot.edges[0].key(), expected_key);
        assert_eq!(snapshot.edges[0].coordinates(), coordinates);
        assert_eq!(
            snapshot.edges[0].context_document_ids(),
            &[uuid(DOCUMENT_ID)]
        );
    }
}

#[tokio::test]
async fn unstable_context_metadata_retries_the_complete_snapshot_then_conflicts() {
    let server = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![
            projection_fixture("context_meta"),
            projection_fixture("context_meta_reproject"),
        ],
        BindingPagesMode::Normal,
    );
    let evidence = server.clone();
    let (state, context) = captured_context(server).await;
    let error = read_edge_snapshot(
        &state,
        &context,
        &CanonicalContextQuery::ContainsAll(Vec::new()),
    )
    .await
    .expect_err("an unstable observation must not return a mixed graph");
    assert_eq!(error.code, "snapshot_conflict");
    assert_eq!(evidence.context_meta_queries.load(Ordering::SeqCst), 6);
}

#[tokio::test]
async fn stable_pagination_that_makes_no_progress_fails_verification() {
    let server = query_server_state(
        &[
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1",
        ],
        vec![projection_fixture("context_meta")],
        BindingPagesMode::RepeatFirstPage,
    );
    let (state, context) = captured_context(server).await;
    let error = read_edge_snapshot(
        &state,
        &context,
        &CanonicalContextQuery::ContainsAll(Vec::new()),
    )
    .await
    .expect_err("non-progressing pagination must fail closed");
    assert_eq!(error.code, "verification_failed");
}

#[derive(Clone)]
struct DelayedIdentityState {
    relay_pubkey: String,
    request_started: Arc<Notify>,
    release_response: Arc<Notify>,
}

async fn delayed_identity(AxumState(state): AxumState<DelayedIdentityState>) -> Json<Value> {
    state.request_started.notify_one();
    state.release_response.notified().await;
    Json(json!({
        "supported_extensions": [
            "buzz-project-view-v3",
            "buzz-project-document-v1",
            "buzz-project-context-edge-v1"
        ],
        "self": state.relay_pubkey,
    }))
}

async fn spawn_delayed_identity_server(state: DelayedIdentityState) -> String {
    let app = Router::new()
        .route("/info", get(delayed_identity))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind delayed identity server");
    let address = listener.local_addr().expect("read identity address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve delayed identity fixture");
    });
    format!("http://{address}")
}

#[tokio::test]
async fn context_capture_cannot_be_retargeted_by_a_community_switch() {
    let relay = Keys::generate();
    let original_signer = Keys::generate();
    let replacement_signer = Keys::generate();
    let request_started = Arc::new(Notify::new());
    let release_response = Arc::new(Notify::new());
    let original_url = spawn_delayed_identity_server(DelayedIdentityState {
        relay_pubkey: relay.public_key().to_hex(),
        request_started: Arc::clone(&request_started),
        release_response: Arc::clone(&release_response),
    })
    .await;
    let state = Arc::new(build_app_state());
    *state.keys.lock().expect("lock signer") = original_signer.clone();
    *state.relay_url_override.lock().expect("lock Relay") = Some(original_url.clone());

    let pending_state = Arc::clone(&state);
    let pending =
        tokio::spawn(
            async move { capture_context("community-a".to_owned(), &pending_state).await },
        );
    request_started.notified().await;
    *state.keys.lock().expect("replace signer") = replacement_signer;
    *state.relay_url_override.lock().expect("replace Relay") =
        Some("http://127.0.0.1:1".to_owned());
    release_response.notify_one();

    let context = pending
        .await
        .expect("join capture")
        .expect("capture original Context boundary");
    assert_eq!(context.community_key, "community-a");
    assert_eq!(context.api_base_url, original_url);
    assert_eq!(context.keys.public_key(), original_signer.public_key());
    assert_eq!(context.identity.relay_pubkey, relay.public_key());
}
