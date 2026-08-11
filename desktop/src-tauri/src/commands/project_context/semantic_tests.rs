use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use axum::routing::{get, post};
use axum::{Json, Router};
use buzz_project_view_pkg::ProjectViewObjectType;
use nostr::{EventBuilder, Keys, Kind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

fn coordinate(seed: u128) -> ProjectContextCoordinateDto {
    ProjectContextCoordinateDto::ProjectViewObject {
        object_type: ProjectViewObjectType::Issue,
        object_id: Uuid::from_u128(seed | (4 << 76) | (2 << 62)),
    }
}

async fn spawn_capability_server(
    relay_pubkey: String,
) -> (String, Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let request_count = Arc::new(AtomicUsize::new(0));
    let query_count = Arc::new(AtomicUsize::new(0));
    let info_relay = relay_pubkey;
    let info_count = request_count.clone();
    let info = move || {
        let relay_pubkey = info_relay.clone();
        let info_count = info_count.clone();
        async move {
            info_count.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::json!({
                "supported_extensions": [
                    crate::commands::project_view::PROJECT_VIEW_V3_EXTENSION
                ],
                "self": relay_pubkey,
            }))
        }
    };
    let request_counted = request_count.clone();
    let counted = query_count.clone();
    let query = move || {
        let request_counted = request_counted.clone();
        let counted = counted.clone();
        async move {
            request_counted.fetch_add(1, Ordering::SeqCst);
            counted.fetch_add(1, Ordering::SeqCst);
            Json(serde_json::json!([]))
        }
    };
    let app = Router::new()
        .route("/info", get(info))
        .route("/query", post(query));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind capability server");
    let address = listener.local_addr().expect("capability server address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("serve capability fixture");
    });
    (format!("ws://{address}"), request_count, query_count)
}

async fn response_from_loopback(body: &[u8], chunked: bool) -> reqwest::Response {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind response fixture");
    let address = listener.local_addr().expect("response fixture address");
    let body = body.to_vec();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept response request");
        let mut request = [0_u8; 2048];
        let _ = socket.read(&mut request).await;
        if chunked {
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                .await
                .expect("write chunked headers");
            socket
                .write_all(format!("{:x}\r\n", body.len()).as_bytes())
                .await
                .expect("write chunk size");
            socket.write_all(&body).await.expect("write chunk body");
            socket
                .write_all(b"\r\n0\r\n\r\n")
                .await
                .expect("finish chunks");
        } else {
            socket
                .write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
                )
                .await
                .expect("write fixed headers");
            socket.write_all(&body).await.expect("write fixed body");
        }
    });
    reqwest::Client::new()
        .get(format!("http://{address}"))
        .send()
        .await
        .expect("fetch response fixture")
}

#[test]
fn input_is_closed_and_problem_debug_is_not_available() {
    let input: SemanticProjectContextQueryInput = serde_json::from_value(serde_json::json!({
        "communityKey": "community-a",
        "appliedWorkspaceToken": Uuid::new_v4().to_string(),
        "problem": "  why does this recur?  ",
        "initialCoordinates": [coordinate(1)],
        "contextCoordinates": []
    }))
    .expect("closed semantic input");
    let validated = validate_input(input).expect("valid input");
    assert_eq!(validated.problem, "why does this recur?");

    assert!(
        serde_json::from_value::<SemanticProjectContextQueryInput>(serde_json::json!({
            "communityKey": "community-a",
            "appliedWorkspaceToken": Uuid::new_v4().to_string(),
            "problem": "why",
            "initialCoordinates": [],
            "contextCoordinates": [],
            "projectId": Uuid::new_v4()
        }))
        .is_err()
    );
}

#[test]
fn strict_event_array_rejects_zero_two_and_unknown_outer_fields() {
    assert!(parse_single_exact_event(b"[]").is_err());
    let keys = Keys::generate();
    let event = EventBuilder::new(Kind::TextNote, "fixture")
        .sign_with_keys(&keys)
        .expect("event");
    let two = serde_json::to_vec(&[&event, &event]).expect("two events");
    assert!(parse_single_exact_event(&two).is_err());

    let mut value = serde_json::to_value(&event).expect("event value");
    value
        .as_object_mut()
        .expect("event object")
        .insert("unknown".to_owned(), Value::Bool(true));
    let unknown = serde_json::to_vec(&[value]).expect("unknown event field");
    assert!(parse_single_exact_event(&unknown).is_err());
}

#[test]
fn error_mapping_is_closed_and_rate_hint_is_capped() {
    let hint = parse_retry_hint(r#"{"error":"retry in 999999s"}"#)
        .map(|seconds| seconds.min(crate::relay_admission::MAX_HINT_SECONDS));
    let error = SemanticProjectContextQueryError::busy(hint);
    assert_eq!(error.code, "busy");
    assert_eq!(
        error.retry_after_seconds,
        Some(crate::relay_admission::MAX_HINT_SECONDS)
    );
    assert!(error.retryable);

    let unavailable = map_http_status(StatusCode::SERVICE_UNAVAILABLE, b"untrusted body");
    assert_eq!(unavailable.code, "unavailable");
    assert_eq!(unavailable.status, Some(503));
    assert!(!unavailable.message.contains("untrusted body"));
}

#[tokio::test]
async fn capability_off_stops_before_any_query_post() {
    let relay = Keys::generate();
    let caller = Keys::generate();
    let (relay_url, request_count, query_count) =
        spawn_capability_server(relay.public_key().to_hex()).await;
    let state = crate::app_state::build_app_state();
    let applied = state
        .apply_workspace_transition("community-a".to_owned(), relay_url, Some(caller))
        .expect("apply capability fixture");
    let input = SemanticProjectContextQueryInput {
        community_key: "community-a".to_owned(),
        applied_workspace_token: applied.applied_workspace_token,
        problem: "this must not leave Desktop".to_owned(),
        initial_coordinates: Vec::new(),
        context_coordinates: Vec::new(),
    };
    let error = match query_project_context_semantic_inner(input, &state).await {
        Err(error) => error,
        Ok(_) => panic!("capability-off query must fail closed"),
    };
    assert_eq!(error.code, "unsupported");
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(query_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn stale_workspace_token_stops_before_any_http_request() {
    let relay = Keys::generate();
    let caller = Keys::generate();
    let (relay_url, request_count, _) = spawn_capability_server(relay.public_key().to_hex()).await;
    let state = crate::app_state::build_app_state();
    let applied_a = state
        .apply_workspace_transition("community-a".to_owned(), relay_url.clone(), Some(caller))
        .expect("apply workspace A");
    state
        .apply_workspace_transition("community-b".to_owned(), relay_url, Some(Keys::generate()))
        .expect("publish workspace B");

    let input = SemanticProjectContextQueryInput {
        community_key: "community-a".to_owned(),
        applied_workspace_token: applied_a.applied_workspace_token,
        problem: "this must not leave Desktop".to_owned(),
        initial_coordinates: Vec::new(),
        context_coordinates: Vec::new(),
    };
    let error = match query_project_context_semantic_inner(input, &state).await {
        Err(error) => error,
        Ok(_) => panic!("stale workspace token must fail closed"),
    };
    assert_eq!(error.code, "conflict");
    assert_eq!(request_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn bounded_reader_enforces_content_length_and_chunk_stream_caps() {
    let exact = response_from_loopback(b"1234", false).await;
    let exact_body = match read_bounded_response(exact, 4).await {
        Ok(body) => body,
        Err(_) => panic!("body at cap must be accepted"),
    };
    assert_eq!(exact_body, b"1234");

    let oversized_length = response_from_loopback(b"12345", false).await;
    assert!(matches!(
        read_bounded_response(oversized_length, 4).await,
        Err(BoundedResponseError::TooLarge)
    ));

    let oversized_chunk = response_from_loopback(b"12345", true).await;
    assert!(matches!(
        read_bounded_response(oversized_chunk, 4).await,
        Err(BoundedResponseError::TooLarge)
    ));
}
