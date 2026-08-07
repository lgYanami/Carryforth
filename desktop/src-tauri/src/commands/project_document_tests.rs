use std::sync::Arc;

use axum::extract::State as AxumState;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::*;
use crate::app_state::build_app_state;
use crate::relay::{RelayHttpError, RelayHttpErrorCategory};

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
        "supported_extensions": ["buzz-project-view-v3", "buzz-project-document-v1"],
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
    let address = listener.local_addr().expect("read identity server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve delayed identity fixture");
    });
    format!("http://{address}")
}

#[derive(Clone)]
struct VerifiedReadState {
    relay_pubkey: String,
    meta: Event,
    head: Event,
    revision: Event,
}

async fn verified_read_identity(AxumState(state): AxumState<VerifiedReadState>) -> Json<Value> {
    Json(json!({
        "supported_extensions": ["buzz-project-document-v1"],
        "self": state.relay_pubkey,
    }))
}

async fn verified_read_query(
    AxumState(state): AxumState<VerifiedReadState>,
    Json(filters): Json<Vec<Value>>,
) -> Json<Value> {
    let kind = filters
        .first()
        .and_then(|filter| filter.get("kinds"))
        .and_then(Value::as_array)
        .and_then(|kinds| kinds.first())
        .and_then(Value::as_u64);
    let event = match kind {
        Some(value) if value == u64::from(KIND_PROJECT_DOCUMENT_META) => state.meta,
        Some(value) if value == u64::from(KIND_PROJECT_DOCUMENT_HEAD) => state.head,
        Some(value) if value == u64::from(KIND_PROJECT_DOCUMENT_REVISION) => state.revision,
        _ => return Json(json!([])),
    };
    Json(serde_json::to_value([event]).expect("serialize projection fixture"))
}

async fn spawn_verified_read_server(state: VerifiedReadState) -> String {
    let app = Router::new()
        .route("/info", get(verified_read_identity))
        .route("/query", post(verified_read_query))
        .with_state(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind verified read server");
    let address = listener
        .local_addr()
        .expect("read projection server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve projection fixture");
    });
    format!("http://{address}")
}

fn projection_fixture(path: &str) -> Event {
    let content = match path {
        "meta" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/meta-incremental.json"
        ),
        "head" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/head-active.json"
        ),
        "revision" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/revision-active.json"
        ),
        "tombstone" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/events/revision-tombstone.json"
        ),
        "wrong_signer" => include_str!(
            "../../../../docs/nips/fixtures/project-document-v1/invalid/wrong-signer.json"
        ),
        _ => panic!("unknown projection fixture"),
    };
    serde_json::from_str(content).expect("parse signed projection fixture")
}

async fn read_context_with_revision(revision: Event) -> (Arc<AppState>, DocumentContext) {
    let meta = projection_fixture("meta");
    let relay_pubkey = meta.pubkey.to_hex();
    let url = spawn_verified_read_server(VerifiedReadState {
        relay_pubkey,
        meta,
        head: projection_fixture("head"),
        revision,
    })
    .await;
    let state = Arc::new(build_app_state());
    *state
        .relay_url_override
        .lock()
        .expect("lock projection Relay") = Some(url);
    let context = capture_context("community-a".to_owned(), &state)
        .await
        .expect("capture projection context");
    (state, context)
}

fn mutation_input(mutation: Value) -> Value {
    json!({
        "communityKey": "community-a",
        "projectId": "00000000-0000-4000-8000-000000000001",
        "relayPubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        "projectionGeneration": 3,
        "mutation": mutation,
    })
}

#[test]
fn desktop_mutation_payload_deserializes_camel_case_variant_fields() {
    let create_id = Uuid::new_v4();
    let create: MutateProjectDocumentInput = serde_json::from_value(mutation_input(json!({
        "type": "create",
        "documentId": create_id,
        "title": "Decision",
        "summary": "Accepted decision",
        "contentMarkdown": "# Decision\n",
    })))
    .expect("deserialize frontend create payload");
    assert!(matches!(
        create.mutation,
        ProjectDocumentMutation::Create {
            document_id: Some(document_id),
            title,
            summary: Some(summary),
            content_markdown,
        } if document_id == create_id
            && title == "Decision"
            && summary == "Accepted decision"
            && content_markdown == "# Decision\n"
    ));

    let document_id = Uuid::new_v4();
    let update: MutateProjectDocumentInput = serde_json::from_value(mutation_input(json!({
        "type": "update",
        "documentId": document_id,
        "expectedDocumentRevision": 7,
        "title": "Runbook",
        "summary": null,
        "contentMarkdown": "# Recover\n",
    })))
    .expect("deserialize frontend update payload");
    assert!(matches!(
        update.mutation,
        ProjectDocumentMutation::Update {
            document_id: parsed_id,
            expected_document_revision: 7,
            title,
            summary: None,
            content_markdown,
        } if parsed_id == document_id
            && title == "Runbook"
            && content_markdown == "# Recover\n"
    ));

    let delete: MutateProjectDocumentInput = serde_json::from_value(mutation_input(json!({
        "type": "delete",
        "documentId": document_id,
        "expectedDocumentRevision": 8,
    })))
    .expect("deserialize frontend delete payload");
    assert!(matches!(
        delete.mutation,
        ProjectDocumentMutation::Delete {
            document_id: parsed_id,
            expected_document_revision: 8,
        } if parsed_id == document_id
    ));
}

#[test]
fn desktop_mutations_are_closed_full_snapshots() {
    let document_id = Uuid::new_v4();
    let command = mutation_command(ProjectDocumentMutation::Update {
        document_id,
        expected_document_revision: 7,
        title: "Runbook".to_owned(),
        summary: Some("Recovery steps".to_owned()),
        content_markdown: "# Recover\n".to_owned(),
    });
    assert_eq!(command.expected_document_revision, 7);
    assert_eq!(command.document_id(), document_id);
    assert!(matches!(
        command.request,
        DocumentCommandRequest::Update {
            title,
            summary: Some(summary),
            content_markdown,
            ..
        } if title == "Runbook"
            && summary == "Recovery steps"
            && content_markdown == "# Recover\n"
    ));
}

#[test]
fn create_allocates_an_opaque_non_nil_document_id() {
    let command = mutation_command(ProjectDocumentMutation::Create {
        document_id: None,
        title: "Decision".to_owned(),
        summary: None,
        content_markdown: "Accepted".to_owned(),
    });
    assert_eq!(command.expected_document_revision, 0);
    assert!(!command.document_id().is_nil());
    assert!(command.validate_for_submission().is_ok());
}

#[test]
fn typed_http_errors_do_not_retain_relay_bodies() {
    let mapped = ProjectDocumentCommandError::from_http(
        RelayHttpError {
            status: Some(503),
            category: RelayHttpErrorCategory::Unavailable,
            message: "secret body that must not cross Tauri".to_owned(),
            retry_after_seconds: Some(4),
            request_may_have_reached_relay: false,
        },
        false,
    );
    let encoded = serde_json::to_string(&mapped).expect("serialize command error");
    assert_eq!(mapped.code, "unavailable");
    assert_eq!(mapped.retry_after_seconds, Some(4));
    assert!(!encoded.contains("secret body"));
}

#[test]
fn delivery_unknown_exposes_only_the_command_coordinate() {
    let event_id = EventId::from_byte_array([0x42; 32]);
    let error = ProjectDocumentCommandError::delivery_unknown(event_id);
    let encoded = serde_json::to_value(error).expect("serialize delivery error");
    assert_eq!(encoded["code"], "delivery_unknown");
    assert_eq!(encoded["eventId"], event_id.to_hex());
}

#[tokio::test]
async fn context_capture_cannot_be_retargeted_by_a_community_switch() {
    let relay = Keys::generate();
    let original_signer = Keys::generate();
    let replacement_signer = Keys::generate();
    let request_started = Arc::new(Notify::new());
    let release_response = Arc::new(Notify::new());
    let old_url = spawn_delayed_identity_server(DelayedIdentityState {
        relay_pubkey: relay.public_key().to_hex(),
        request_started: Arc::clone(&request_started),
        release_response: Arc::clone(&release_response),
    })
    .await;
    let state = Arc::new(build_app_state());
    *state.keys.lock().expect("lock original signer") = original_signer.clone();
    *state
        .relay_url_override
        .lock()
        .expect("lock original Relay") = Some(old_url.clone());

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
        .expect("join context capture")
        .expect("capture original context");
    assert_eq!(context.community_key, "community-a");
    assert_eq!(context.api_base_url, old_url);
    assert_eq!(context.keys.public_key(), original_signer.public_key());
    assert_eq!(context.relay_pubkey, relay.public_key());
}

#[tokio::test]
async fn native_pinned_read_rejects_an_unadvertised_projection_signer() {
    let (state, context) = read_context_with_revision(projection_fixture("wrong_signer")).await;
    let meta = read_meta(&state, &context)
        .await
        .expect("read signed metadata");
    let document_id =
        Uuid::parse_str("9c23f672-a397-42d1-b933-104ba2674f26").expect("Document UUID");

    let error = read_document(&state, &context, &meta, document_id, Some(8))
        .await
        .expect_err("wrong Relay signer must fail closed");
    assert_eq!(error.code, "internal");
    assert!(!error.message.contains("content_markdown"));
}

#[tokio::test]
async fn native_current_read_rejects_a_revision_other_than_the_head_pointer() {
    let (state, context) = read_context_with_revision(projection_fixture("tombstone")).await;
    let meta = read_meta(&state, &context)
        .await
        .expect("read signed metadata");
    let document_id =
        Uuid::parse_str("9c23f672-a397-42d1-b933-104ba2674f26").expect("Document UUID");

    let error = read_document(&state, &context, &meta, document_id, None)
        .await
        .expect_err("revision not named by the head must fail closed");
    assert_eq!(error.code, "internal");
    assert!(error.message.contains("head pointer"));
}
