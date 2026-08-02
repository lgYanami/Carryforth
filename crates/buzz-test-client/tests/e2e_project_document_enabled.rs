//! Stage 2 Project Document enabled-path and private fan-out E2E.
//!
//! The isolated harness bootstraps and enables one Project View v2 Community.
//! This test proves signed command/projection fan-out, closed HTTP pagination,
//! current membership enforcement at query and final dispatch, and Relay-only
//! projection writes.

use std::collections::HashSet;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::tenant::relay_url_authority;
use buzz_project_document::{
    DocumentCommandRequest, ProjectDocumentCommand, ProjectDocumentReceipt,
};
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
    reader: Keys,
    writer: Keys,
    outsider: Keys,
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
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for E2E");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("connect Project Document E2E database");
    let host = relay_url_authority(&ws_url);
    let community_id =
        sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("resolve host-bound Community");
    TestContext {
        ws_url,
        http_url,
        community_id,
        pool,
        reader: env_keys("PROJECT_DOCUMENT_E2E_MEMBER_PRIVATE_KEY"),
        writer: env_keys("PROJECT_DOCUMENT_E2E_WRITER_PRIVATE_KEY"),
        outsider: env_keys("PROJECT_DOCUMENT_E2E_OUTSIDER_PRIVATE_KEY"),
        relay: env_keys("PROJECT_DOCUMENT_E2E_RELAY_PRIVATE_KEY"),
    }
}

fn env_keys(name: &str) -> Keys {
    Keys::parse(&std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set")))
        .unwrap_or_else(|error| panic!("parse {name}: {error}"))
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
    keys: &Keys,
    path: &str,
    body: &Value,
) -> (StatusCode, Value) {
    let body = serde_json::to_string(body).expect("serialize HTTP request");
    let url = format!("{}{path}", context.http_url);
    let response = client
        .post(&url)
        .header("Authorization", nip98_header(keys, &url, &body))
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

fn command_event(keys: &Keys, command: ProjectDocumentCommand) -> Event {
    build_document_command(command)
        .expect("build strict Document command")
        .sign_with_keys(keys)
        .expect("sign Document command")
}

async fn collect_live_bundle(client: &mut BuzzTestClient, subscription_id: &str) -> Vec<Event> {
    let mut events = Vec::new();
    while events.len() < 4 {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive committed Document fan-out")
        {
            RelayMessage::Event {
                subscription_id: actual,
                event,
            } if actual == subscription_id => events.push(*event),
            _ => {}
        }
    }
    events
}

#[tokio::test]
#[ignore = "requires isolated enabled Relay, PostgreSQL, Redis, and stable Relay signer"]
async fn project_document_stage_two_is_verified_private_and_revocation_safe() {
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
    assert!(info["supported_extensions"]
        .as_array()
        .is_some_and(|extensions| extensions
            .iter()
            .any(|value| value == "buzz-project-document-v1")));
    assert_eq!(info["self"], context.relay.public_key().to_hex());

    let mut reader = BuzzTestClient::connect(&context.ws_url, &context.reader)
        .await
        .expect("connect reader");
    let mut writer = BuzzTestClient::connect(&context.ws_url, &context.writer)
        .await
        .expect("connect writer");
    let mut outsider = BuzzTestClient::connect(&context.ws_url, &context.outsider)
        .await
        .expect("connect outsider");

    let live_subscription = format!("pd-live-{}", Uuid::new_v4());
    reader
        .subscribe(
            &live_subscription,
            vec![Filter::new().kinds([
                Kind::Custom(KIND_PROJECT_DOCUMENT_COMMAND as u16),
                Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
                Kind::Custom(KIND_PROJECT_DOCUMENT_REVISION as u16),
                Kind::Custom(KIND_PROJECT_DOCUMENT_META as u16),
            ])],
        )
        .await
        .expect("subscribe to private Document stream");
    let initial = reader
        .collect_until_eose(&live_subscription, Duration::from_secs(5))
        .await
        .expect("collect initial private snapshot");
    assert_eq!(initial.len(), 1, "empty bootstrap exposes only metadata");
    assert_eq!(
        u32::from(initial[0].kind.as_u16()),
        KIND_PROJECT_DOCUMENT_META
    );

    let outsider_subscription = format!("pd-outsider-{}", Uuid::new_v4());
    outsider
        .subscribe(
            &outsider_subscription,
            vec![Filter::new().kind(Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16))],
        )
        .await
        .expect("send outsider subscription");
    expect_closed(&mut outsider, &outsider_subscription, "membership_required").await;

    let document_id = Uuid::new_v4();
    let create = command_event(
        &context.writer,
        ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id,
                title: "Stage 2 canary".to_owned(),
                summary: Some("metadata remains body-free".to_owned()),
                content_markdown: "# Canary\n\nprivate body marker".to_owned(),
            },
        ),
    );
    let response = writer
        .send_event(create.clone())
        .await
        .expect("submit create command");
    assert!(response.accepted, "{}", response.message);
    let receipt: ProjectDocumentReceipt = serde_json::from_str(
        response
            .message
            .strip_prefix("response:")
            .expect("canonical receipt prefix"),
    )
    .expect("parse canonical receipt");
    assert_eq!(receipt.change_id, create.id);
    assert_eq!(receipt.document_revision, 1);

    let fanout = collect_live_bundle(&mut reader, &live_subscription).await;
    let kinds: HashSet<u32> = fanout
        .iter()
        .map(|event| u32::from(event.kind.as_u16()))
        .collect();
    assert_eq!(
        kinds,
        HashSet::from([
            KIND_PROJECT_DOCUMENT_COMMAND,
            KIND_PROJECT_DOCUMENT_HEAD,
            KIND_PROJECT_DOCUMENT_REVISION,
            KIND_PROJECT_DOCUMENT_META,
        ])
    );
    assert!(fanout.iter().all(|event| {
        u32::from(event.kind.as_u16()) == KIND_PROJECT_DOCUMENT_COMMAND
            || event.pubkey == context.relay.public_key()
    }));

    let active_page = json!([{
        "kinds": [KIND_PROJECT_DOCUMENT_HEAD],
        "authors": [context.relay.public_key().to_hex()],
        "#t": ["buzz-project-document-head"],
        "limit": 100,
        "buzz_project_document": {
            "scope": "active_heads",
            "projection_generation": 1,
            "catalog_revision": 1,
        }
    }]);
    let (status, body) = post_json(&http, &context, &context.reader, "/query", &active_page).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(1));
    assert!(!body.to_string().contains("private body marker"));
    let (status, body) =
        post_json(&http, &context, &context.outsider, "/query", &active_page).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    sqlx::query("DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(context.community_id)
        .bind(context.reader.public_key().to_hex())
        .execute(&context.pool)
        .await
        .expect("revoke reader membership");
    let update = command_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id,
                title: "Stage 2 canary updated".to_owned(),
                summary: None,
                content_markdown: "# Canary\n\nrevision two".to_owned(),
            },
        ),
    );
    let response = writer
        .send_event(update)
        .await
        .expect("submit update command");
    assert!(response.accepted, "{}", response.message);
    assert!(
        reader.recv_event(Duration::from_millis(750)).await.is_err(),
        "revoked reader received a post-revocation Document event"
    );

    sqlx::query("INSERT INTO relay_members (community_id, pubkey, role) VALUES ($1, $2, 'member')")
        .bind(context.community_id)
        .bind(context.reader.public_key().to_hex())
        .execute(&context.pool)
        .await
        .expect("restore reader membership");

    let history = json!([{
        "kinds": [KIND_PROJECT_DOCUMENT_REVISION],
        "authors": [context.relay.public_key().to_hex()],
        "#t": ["buzz-project-document-revision"],
        "limit": 20,
        "buzz_project_document": {
            "scope": "history",
            "projection_generation": 1,
            "document_id": document_id,
            "max_document_revision": 2,
        }
    }]);
    let (status, body) = post_json(&http, &context, &context.reader, "/query", &history).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(2));

    let forged = EventBuilder::new(
        Kind::Custom(KIND_PROJECT_DOCUMENT_HEAD as u16),
        "forged projection",
    )
    .sign_with_keys(&context.writer)
    .expect("sign forged projection");
    let response = writer
        .send_event(forged)
        .await
        .expect("receive Relay-only rejection");
    assert!(!response.accepted);
    assert!(response.message.contains("relay-only"));

    reader.disconnect().await.expect("disconnect reader");
    writer.disconnect().await.expect("disconnect writer");
    outsider.disconnect().await.expect("disconnect outsider");
    context.pool.close().await;
}
