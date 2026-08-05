//! Project Context Stage 1 fail-closed security E2E.
//!
//! The test inserts protocol rows behind the Relay, then proves that an
//! unready implementation cannot expose them through explicit, mixed,
//! kindless, IDs-only, wildcard, COUNT, or HTTP read paths.

use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
};
use buzz_core::tenant::relay_url_authority;
use buzz_core::CommunityId;
use buzz_project_context::{
    ProjectContextCommand, ProjectContextCoordinate, ProjectContextOperation,
    PROJECT_CONTEXT_CAPABILITY,
};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::project_context::build_project_context_command;
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
    // Stage 1 reuses the isolated Project Document Relay harness because it
    // already supplies a scratch event store, one member, and the Relay key.
    let ws_url = std::env::var("PROJECT_DOCUMENT_E2E_RELAY_URL")
        .expect("PROJECT_DOCUMENT_E2E_RELAY_URL must be set");
    Url::parse(&ws_url).expect("parse Project Context E2E Relay URL");
    let http_url = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
        .trim_end_matches('/')
        .to_owned();
    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for E2E");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("connect Project Context E2E database");
    let host = relay_url_authority(&ws_url);
    let community_id: Uuid =
        sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("resolve host-bound Project Context E2E Community");
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

async fn expect_closed(client: &mut BuzzTestClient, subscription_id: &str) {
    loop {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive Project Context subscription rejection")
        {
            RelayMessage::Closed {
                subscription_id: actual,
                message,
            } if actual == subscription_id => {
                assert_eq!(message, "unavailable:project_context:not_ready");
                return;
            }
            RelayMessage::Event { event, .. } => {
                panic!("Project Context event leaked before CLOSED: {}", event.id)
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
            .expect("receive Project Context COUNT response")
        {
            RelayMessage::Count {
                subscription_id: actual,
                count,
            } if actual == subscription_id => {
                assert_eq!(count, expected);
                return;
            }
            RelayMessage::Event { event, .. } => {
                panic!("Project Context event leaked while counting: {}", event.id)
            }
            _ => {}
        }
    }
}

async fn collect_until_terminal(client: &mut BuzzTestClient, subscription_id: &str) -> Vec<Event> {
    let mut events = Vec::new();
    loop {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive Project Context query terminal message")
        {
            RelayMessage::Eose {
                subscription_id: actual,
            }
            | RelayMessage::Closed {
                subscription_id: actual,
                ..
            } if actual == subscription_id => return events,
            RelayMessage::Event {
                subscription_id: actual,
                event,
            } if actual == subscription_id => events.push(*event),
            _ => {}
        }
    }
}

fn command_event(context: &TestContext) -> Event {
    let command = ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        vec![
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Requirement,
                object_id: Uuid::new_v4(),
            },
            ProjectContextCoordinate::ProjectViewObject {
                object_type: ProjectViewObjectType::Resource,
                object_id: Uuid::new_v4(),
            },
        ],
        Uuid::new_v4(),
    )
    .expect("build canonical Project Context command");
    build_project_context_command(CommunityId::from_uuid(context.community_id), command)
        .expect("build Project Context event")
        .sign_with_keys(&context.member)
        .expect("sign Project Context command")
}

fn relay_projection(context: &TestContext, kind: u32, label: &str) -> Event {
    let coordinate = format!("project-context-stage1:{label}:{}", Uuid::new_v4());
    EventBuilder::new(
        Kind::Custom(kind as u16),
        format!("behind-Relay Project Context {label} fixture"),
    )
    .tags([
        Tag::parse(["-"]).expect("private tag"),
        Tag::parse(["d", coordinate.as_str()]).expect("d tag"),
    ])
    .sign_with_keys(&context.relay)
    .expect("sign injected Relay projection")
}

fn ids_filter(events: &[Event]) -> Filter {
    events
        .iter()
        .fold(Filter::new(), |filter, event| filter.id(event.id))
}

fn assert_no_injected_events(events: &[Event], injected: &[Event], surface: &str) {
    assert!(
        events
            .iter()
            .all(|event| injected.iter().all(|candidate| candidate.id != event.id)),
        "{surface} leaked a Project Context event"
    );
}

#[tokio::test]
#[ignore = "requires isolated Relay, PostgreSQL, Redis, and stable Relay signer"]
async fn project_context_stage_one_is_unadvertised_unwritable_and_unreadable() {
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
                .all(|value| value != PROJECT_CONTEXT_CAPABILITY)),
        "Stage 1 must not advertise Project Context: {info}"
    );

    let mut ws = BuzzTestClient::connect(&context.ws_url, &context.member)
        .await
        .expect("connect ordinary member");
    let command = command_event(&context);
    let command_response = ws
        .send_event(command.clone())
        .await
        .expect("receive command rejection");
    assert!(!command_response.accepted);
    assert_eq!(
        command_response.message,
        "unavailable:project_context:not_ready"
    );

    let (status, body) = post_json(
        &http,
        &context,
        "/events",
        &serde_json::to_value(&command).expect("serialize command event"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_context:not_ready"));

    let forged_projection = EventBuilder::new(
        Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
        "client-forged Project Context projection",
    )
    .sign_with_keys(&context.member)
    .expect("sign forged projection");
    let projection_response = ws
        .send_event(forged_projection)
        .await
        .expect("receive relay-only rejection");
    assert!(!projection_response.accepted);
    assert!(projection_response.message.contains("relay-only"));

    let exclusive_req = format!("pce-exclusive-{}", Uuid::new_v4());
    ws.subscribe(
        &exclusive_req,
        vec![Filter::new().kinds([
            Kind::Custom(KIND_PROJECT_CONTEXT_COMMAND as u16),
            Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
            Kind::Custom(KIND_PROJECT_CONTEXT_META as u16),
        ])],
    )
    .await
    .expect("send exclusive Project Context REQ");
    expect_closed(&mut ws, &exclusive_req).await;

    let exclusive_count = format!("pce-count-exclusive-{}", Uuid::new_v4());
    ws.send_raw(&json!([
        "COUNT",
        exclusive_count,
        {
            "kinds": [
                KIND_PROJECT_CONTEXT_COMMAND,
                KIND_PROJECT_CONTEXT_EDGE_BINDING,
                KIND_PROJECT_CONTEXT_META
            ]
        }
    ]))
    .await
    .expect("send exclusive Project Context COUNT");
    expect_closed(&mut ws, &exclusive_count).await;

    // Simulate fixture injection or an operator mistake behind the Relay. The
    // read boundary must remain closed independently of content validity.
    let injected = vec![
        command,
        relay_projection(&context, KIND_PROJECT_CONTEXT_EDGE_BINDING, "binding"),
        relay_projection(&context, KIND_PROJECT_CONTEXT_META, "meta"),
    ];
    for event in &injected {
        let (_, inserted) = buzz_db::event::insert_event(
            &context.pool,
            CommunityId::from_uuid(context.community_id),
            event,
            None,
        )
        .await
        .expect("insert behind-Relay Project Context fixture");
        assert!(inserted);
    }

    let mixed_filter = ids_filter(&injected).kinds([
        Kind::TextNote,
        Kind::Custom(KIND_PROJECT_CONTEXT_COMMAND as u16),
        Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
        Kind::Custom(KIND_PROJECT_CONTEXT_META as u16),
    ]);
    let mixed_req = format!("pce-mixed-{}", Uuid::new_v4());
    ws.subscribe(&mixed_req, vec![mixed_filter.clone()])
        .await
        .expect("send mixed by-ID REQ");
    let mixed_events = collect_until_terminal(&mut ws, &mixed_req).await;
    assert_no_injected_events(&mixed_events, &injected, "mixed WS REQ");

    let kindless_req = format!("pce-kindless-{}", Uuid::new_v4());
    ws.subscribe(&kindless_req, vec![ids_filter(&injected)])
        .await
        .expect("send kindless by-ID REQ");
    let kindless_events = collect_until_terminal(&mut ws, &kindless_req).await;
    assert_no_injected_events(&kindless_events, &injected, "kindless WS REQ");

    let wildcard_req = format!("pce-wildcard-{}", Uuid::new_v4());
    ws.subscribe(
        &wildcard_req,
        vec![Filter::new().kinds([
            Kind::TextNote,
            Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
        ])],
    )
    .await
    .expect("send mixed wildcard REQ");
    let wildcard_events = collect_until_terminal(&mut ws, &wildcard_req).await;
    assert_no_injected_events(&wildcard_events, &injected, "wildcard WS REQ");

    let mixed_count = format!("pce-count-mixed-{}", Uuid::new_v4());
    ws.send_raw(&json!(["COUNT", mixed_count, mixed_filter]))
        .await
        .expect("send mixed Project Context COUNT");
    expect_count(&mut ws, &mixed_count, 0).await;

    let injected_ids: Vec<String> = injected.iter().map(|event| event.id.to_hex()).collect();
    let exclusive_query = json!([{
        "kinds": [
            KIND_PROJECT_CONTEXT_COMMAND,
            KIND_PROJECT_CONTEXT_EDGE_BINDING,
            KIND_PROJECT_CONTEXT_META
        ]
    }]);
    let (status, body) = post_json(&http, &context, "/query", &exclusive_query).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_context:not_ready"));

    let mixed_query = json!([{
        "ids": injected_ids,
        "kinds": [
            1,
            KIND_PROJECT_CONTEXT_COMMAND,
            KIND_PROJECT_CONTEXT_EDGE_BINDING,
            KIND_PROJECT_CONTEXT_META
        ]
    }]);
    let (status, body) = post_json(&http, &context, "/query", &mixed_query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.as_array().map(Vec::len), Some(0));

    let kindless_query = json!([{
        "ids": injected.iter().map(|event| event.id.to_hex()).collect::<Vec<_>>()
    }]);
    let (status, body) = post_json(&http, &context, "/query", &kindless_query).await;
    assert!(
        status != StatusCode::OK || body.as_array().is_some_and(Vec::is_empty),
        "kindless HTTP query leaked Project Context: {body}"
    );

    let wildcard_query = json!([{
        "kinds": [1, KIND_PROJECT_CONTEXT_EDGE_BINDING]
    }]);
    let (status, body) = post_json(&http, &context, "/query", &wildcard_query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let events = body.as_array().expect("wildcard HTTP event array");
    assert!(
        events.iter().all(|event| {
            let id = event["id"].as_str();
            injected
                .iter()
                .all(|candidate| id != Some(candidate.id.to_hex().as_str()))
        }),
        "wildcard HTTP query leaked Project Context: {body}"
    );

    let (status, body) = post_json(&http, &context, "/count", &exclusive_query).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body
        .to_string()
        .contains("unavailable:project_context:not_ready"));
    let (status, body) = post_json(&http, &context, "/count", &mixed_query).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["count"], 0);

    let deleted =
        sqlx::query("DELETE FROM events WHERE community_id = $1 AND kind IN ($2, $3, $4)")
            .bind(context.community_id)
            .bind(KIND_PROJECT_CONTEXT_COMMAND as i32)
            .bind(KIND_PROJECT_CONTEXT_EDGE_BINDING as i32)
            .bind(KIND_PROJECT_CONTEXT_META as i32)
            .execute(&context.pool)
            .await
            .expect("remove isolated Project Context fixtures");
    assert_eq!(deleted.rows_affected(), 3);

    ws.disconnect().await.expect("disconnect ordinary member");
    context.pool.close().await;
}
