//! Stage 1 Project Document flag-off security E2E.
//!
//! This deliberately does not exercise a public Document handler: Stage 1 has
//! none. It proves that a normal member cannot submit either protocol side,
//! that NIP-11 does not advertise the capability, and that even a projection
//! row inserted behind the Relay cannot escape through exclusive, mixed,
//! kindless, by-ID, COUNT, HTTP, or WebSocket reads.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::tenant::relay_url_authority;
use buzz_core::CommunityId;
use buzz_project_document::{DocumentCommandRequest, ProjectDocumentCommand};
use buzz_sdk::project_document::build_document_command;
use buzz_test_client::{BuzzTestClient, RelayMessage};
use nostr::{Event, EventBuilder, Filter, Keys, Kind, Tag};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

struct TestContext {
    ws_url: String,
    http_url: String,
    community_id: Uuid,
    pool: PgPool,
    member: Keys,
    relay: Keys,
}

async fn setup() -> TestContext {
    let ws_url = std::env::var("PROJECT_DOCUMENT_E2E_RELAY_URL")
        .expect("PROJECT_DOCUMENT_E2E_RELAY_URL must be set");
    Url::parse(&ws_url).expect("parse Project Document E2E Relay URL");
    let http_url = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
        .trim_end_matches('/')
        .to_owned();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for the E2E");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("connect Project Document E2E database");
    let host = relay_url_authority(&ws_url);
    let community_id: Uuid =
        sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("resolve host-bound Project Document E2E Community");
    let member = Keys::parse(
        &std::env::var("PROJECT_DOCUMENT_E2E_MEMBER_PRIVATE_KEY")
            .expect("PROJECT_DOCUMENT_E2E_MEMBER_PRIVATE_KEY must be set"),
    )
    .expect("parse member key");
    let relay = Keys::parse(
        &std::env::var("PROJECT_DOCUMENT_E2E_RELAY_PRIVATE_KEY")
            .expect("PROJECT_DOCUMENT_E2E_RELAY_PRIVATE_KEY must be set"),
    )
    .expect("parse Relay key");
    TestContext {
        ws_url,
        http_url,
        community_id,
        pool,
        member,
        relay,
    }
}

fn nip98_header(keys: &Keys, url: &str, body: &str) -> String {
    let payload = hex::encode(Sha256::digest(body.as_bytes()));
    let nonce = Uuid::new_v4().to_string();
    let event = EventBuilder::new(Kind::Custom(27_235), "")
        .tags([
            Tag::parse(["u", url]).expect("u tag"),
            Tag::parse(["method", "POST"]).expect("method tag"),
            Tag::parse(["payload", payload.as_str()]).expect("payload tag"),
            Tag::parse(["nonce", nonce.as_str()]).expect("nonce tag"),
        ])
        .sign_with_keys(keys)
        .expect("sign NIP-98 request");
    format!(
        "Nostr {}",
        BASE64.encode(serde_json::to_string(&event).expect("serialize NIP-98 request"))
    )
}

async fn post_json(
    client: &Client,
    context: &TestContext,
    path: &str,
    body: &Value,
) -> (StatusCode, Value) {
    let body = serde_json::to_string(body).expect("serialize HTTP request");
    let url = format!("{}{path}", context.http_url);
    let response = client
        .post(&url)
        .header("Authorization", nip98_header(&context.member, &url, &body))
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap_or_else(|error| panic!("POST {path}: {error}"));
    let status = response.status();
    let text = response.text().await.expect("read HTTP response");
    let value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {path} response ({status}): {error}: {text}"));
    (status, value)
}

async fn expect_closed(client: &mut BuzzTestClient, subscription_id: &str, reason: &str) {
    loop {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive subscription rejection")
        {
            RelayMessage::Closed {
                subscription_id: actual,
                message,
            } if actual == subscription_id => {
                assert!(
                    message.contains(reason),
                    "unexpected CLOSED reason: {message}"
                );
                return;
            }
            RelayMessage::Event { event, .. } => {
                panic!("private Document event leaked before CLOSED: {}", event.id)
            }
            _ => {}
        }
    }
}

async fn expect_count(client: &mut BuzzTestClient, subscription_id: &str, expected: u64) {
    loop {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive COUNT response")
        {
            RelayMessage::Count {
                subscription_id: actual,
                count,
            } if actual == subscription_id => {
                assert_eq!(count, expected);
                return;
            }
            RelayMessage::Event { event, .. } => {
                panic!("private Document event leaked while counting: {}", event.id)
            }
            _ => {}
        }
    }
}

async fn assert_kindless_query_terminates_without_event(
    client: &mut BuzzTestClient,
    subscription_id: &str,
) {
    loop {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive kindless query terminal message")
        {
            RelayMessage::Eose {
                subscription_id: actual,
            }
            | RelayMessage::Closed {
                subscription_id: actual,
                ..
            } if actual == subscription_id => return,
            RelayMessage::Event {
                subscription_id: actual,
                event,
            } if actual == subscription_id => {
                panic!("kindless by-ID query leaked Document event {}", event.id)
            }
            _ => {}
        }
    }
}

fn create_command_event(keys: &Keys) -> Event {
    let command = ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id: Uuid::new_v4(),
            title: "Stage 1 remains private".to_owned(),
            summary: None,
            content_markdown: "# Not publicly routable".to_owned(),
        },
    );
    build_document_command(command)
        .expect("build strict Document command")
        .sign_with_keys(keys)
        .expect("sign Document command")
}

#[tokio::test]
#[ignore = "requires isolated Relay, PostgreSQL, Redis, and stable Relay signer"]
async fn project_document_stage_one_is_unadvertised_unwritable_and_unreadable() {
    let context = setup().await;
    let http = Client::new();

    let info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch NIP-11")
        .json()
        .await
        .expect("parse NIP-11");
    assert!(
        info["supported_extensions"]
            .as_array()
            .is_none_or(|extensions| extensions
                .iter()
                .all(|value| value != "buzz-project-document-v1")),
        "Stage 1 must not advertise Project Document: {info}"
    );

    let enabled: bool =
        sqlx::query_scalar("SELECT project_document_enabled FROM communities WHERE id = $1")
            .bind(context.community_id)
            .fetch_one(&context.pool)
            .await
            .expect("read Document feature flag");
    assert!(!enabled, "Stage 1 Community flag must stay false");
    let canonical_rows: i64 = sqlx::query_scalar(
        "SELECT \
           (SELECT count(*) FROM project_document_state WHERE community_id = $1) + \
           (SELECT count(*) FROM project_documents WHERE community_id = $1) + \
           (SELECT count(*) FROM project_document_revisions WHERE community_id = $1) + \
           (SELECT count(*) FROM project_document_changes WHERE community_id = $1)",
    )
    .bind(context.community_id)
    .fetch_one(&context.pool)
    .await
    .expect("count canonical Document rows");
    assert_eq!(canonical_rows, 0, "real Community must not be bootstrapped");

    let mut ws = BuzzTestClient::connect(&context.ws_url, &context.member)
        .await
        .expect("connect ordinary member");
    let command_event = create_command_event(&context.member);
    let command_response = ws
        .send_event(command_event.clone())
        .await
        .expect("receive command rejection");
    assert!(!command_response.accepted);
    assert_eq!(
        command_response.message,
        "unavailable:project_document:disabled"
    );

    let (status, body) = post_json(
        &http,
        &context,
        "/events",
        &serde_json::to_value(&command_event).expect("serialize command event"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_document:disabled"));

    let forged_projection = EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
        "malformed client projection",
    )
    .sign_with_keys(&context.member)
    .expect("sign forged projection");
    let projection_response = ws
        .send_event(forged_projection)
        .await
        .expect("receive relay-only rejection");
    assert!(!projection_response.accepted);
    assert!(projection_response.message.contains("relay-only"));

    let exclusive_req = format!("pd-exclusive-{}", Uuid::new_v4());
    ws.subscribe(
        &exclusive_req,
        vec![Filter::new().kind(Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16))],
    )
    .await
    .expect("send exclusive Document REQ");
    expect_closed(
        &mut ws,
        &exclusive_req,
        "unavailable:project_document:disabled",
    )
    .await;

    let exclusive_count = format!("pd-count-exclusive-{}", Uuid::new_v4());
    ws.send_raw(&json!([
        "COUNT",
        exclusive_count,
        {"kinds": [KIND_PROJECT_DOCUMENT_REVISION]}
    ]))
    .await
    .expect("send exclusive Document COUNT");
    expect_closed(
        &mut ws,
        &exclusive_count,
        "unavailable:project_document:disabled",
    )
    .await;

    let document_events_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE community_id = $1 \
         AND kind IN ($2, $3, $4, $5)",
    )
    .bind(context.community_id)
    .bind(KIND_PROJECT_DOCUMENT_COMMAND as i32)
    .bind(KIND_PROJECT_DOCUMENT_HEAD as i32)
    .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
    .bind(KIND_PROJECT_DOCUMENT_META as i32)
    .fetch_one(&context.pool)
    .await
    .expect("count stored Document events");
    assert_eq!(
        document_events_before, 0,
        "rejected submissions must not persist"
    );

    // Simulate a test fixture or operator mistake behind the Relay. The row is
    // Relay-signed but intentionally not canonical; every public read path must
    // still deny it while the capability is off.
    let injected = EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
        "mistakenly inserted private projection",
    )
    .tags([Tag::parse(["d", &Uuid::new_v4().to_string()]).expect("d tag")])
    .sign_with_keys(&context.relay)
    .expect("sign injected projection");
    let (_, inserted) = buzz_db::event::insert_event(
        &context.pool,
        CommunityId::from_uuid(context.community_id),
        &injected,
        None,
    )
    .await
    .expect("insert behind-Relay projection fixture");
    assert!(inserted);

    let mixed_filter = Filter::new().id(injected.id).kinds([
        Kind::TextNote,
        Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
    ]);
    let mixed_req = format!("pd-mixed-{}", Uuid::new_v4());
    ws.subscribe(&mixed_req, vec![mixed_filter.clone()])
        .await
        .expect("send mixed by-ID REQ");
    let mixed_events = ws
        .collect_until_eose(&mixed_req, Duration::from_secs(5))
        .await
        .expect("collect mixed by-ID query");
    assert!(
        mixed_events.is_empty(),
        "mixed REQ leaked injected projection"
    );

    let kindless_req = format!("pd-kindless-{}", Uuid::new_v4());
    ws.subscribe(&kindless_req, vec![Filter::new().id(injected.id)])
        .await
        .expect("send kindless by-ID REQ");
    assert_kindless_query_terminates_without_event(&mut ws, &kindless_req).await;

    let mixed_count = format!("pd-count-mixed-{}", Uuid::new_v4());
    ws.send_raw(&json!(["COUNT", mixed_count, mixed_filter]))
        .await
        .expect("send mixed COUNT");
    expect_count(&mut ws, &mixed_count, 0).await;

    let exclusive_query = json!([{"kinds": [KIND_PROJECT_DOCUMENT_HEAD]}]);
    let (status, body) = post_json(&http, &context, "/query", &exclusive_query).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_document:disabled"));

    let mixed_query = json!([{
        "ids": [injected.id.to_hex()],
        "kinds": [1, KIND_PROJECT_DOCUMENT_HEAD]
    }]);
    let (status, body) = post_json(&http, &context, "/query", &mixed_query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(0));

    let kindless_query = json!([{"ids": [injected.id.to_hex()]}]);
    let (status, body) = post_json(&http, &context, "/query", &kindless_query).await;
    assert!(
        status != StatusCode::OK || body.as_array().is_some_and(Vec::is_empty),
        "kindless HTTP query leaked injected projection: {body}"
    );

    let (status, body) = post_json(&http, &context, "/count", &exclusive_query).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_document:disabled"));
    let (status, body) = post_json(&http, &context, "/count", &mixed_query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 0);

    ws.disconnect().await.expect("disconnect ordinary member");
    context.pool.close().await;
}
