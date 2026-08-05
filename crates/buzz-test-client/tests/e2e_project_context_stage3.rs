//! Project Context Stage 3 Relay, privacy, and operation-gate E2E.
//!
//! The isolated harness establishes Project View v3, Project Document v1, and
//! an initialized Context Edge catalog. This test drives real signed commands
//! through the Relay and verifies atomic projections, replay/CAS behavior,
//! managed-agent authority, disable semantics, private reads, and fan-out.

use std::collections::HashSet;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
};
use buzz_core::tenant::relay_url_authority;
use buzz_core::{CommunityId, RuntimeFence};
use buzz_db::Db;
use buzz_project_context::{
    ProjectContextBindingState, ProjectContextCommand, ProjectContextCoordinate,
    ProjectContextOperation, ProjectContextReceipt, PROJECT_CONTEXT_CAPABILITY,
};
use buzz_project_document::{DocumentCommandRequest, ProjectDocumentCommand};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::nip_oa;
use buzz_sdk::project_context::{
    build_project_context_command, parse_project_context_binding, parse_project_context_command,
    parse_project_context_meta, verify_project_context_meta_change,
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

const GOAL_ID: &str = "10000000-0000-4000-8000-00000000c003";

struct TestContext {
    ws_url: String,
    http_url: String,
    wrong_http_url: String,
    community_id: CommunityId,
    pool: PgPool,
    reader: Keys,
    writer: Keys,
    outsider: Keys,
    agent: Keys,
    relay: Keys,
}

async fn setup() -> TestContext {
    let ws_url = std::env::var("PROJECT_CONTEXT_E2E_RELAY_URL")
        .expect("PROJECT_CONTEXT_E2E_RELAY_URL must be set");
    let parsed = Url::parse(&ws_url).expect("parse Project Context E2E Relay URL");
    let port = parsed
        .port()
        .expect("Project Context E2E Relay URL has a port");
    let http_url = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
        .trim_end_matches('/')
        .to_owned();
    let wrong_http_url = format!("http://127.0.0.1:{port}");
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
    TestContext {
        ws_url,
        http_url,
        wrong_http_url,
        community_id: CommunityId::from_uuid(community_id),
        pool,
        reader: env_keys("PROJECT_CONTEXT_E2E_MEMBER_PRIVATE_KEY"),
        writer: env_keys("PROJECT_CONTEXT_E2E_WRITER_PRIVATE_KEY"),
        outsider: env_keys("PROJECT_CONTEXT_E2E_OUTSIDER_PRIVATE_KEY"),
        agent: env_keys("PROJECT_CONTEXT_E2E_AGENT_PRIVATE_KEY"),
        relay: env_keys("PROJECT_CONTEXT_E2E_RELAY_PRIVATE_KEY"),
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

async fn post_json_at(
    client: &Client,
    base_url: &str,
    keys: &Keys,
    path: &str,
    body: &Value,
) -> (StatusCode, String) {
    let body = serde_json::to_string(body).expect("serialize HTTP request");
    let url = format!("{base_url}{path}");
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
    (status, text)
}

async fn post_json(
    client: &Client,
    context: &TestContext,
    keys: &Keys,
    path: &str,
    body: &Value,
) -> (StatusCode, Value) {
    let (status, text) = post_json_at(client, &context.http_url, keys, path, body).await;
    let value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {path} response ({status}): {error}: {text}"));
    (status, value)
}

fn context_filter() -> Filter {
    Filter::new().kinds([
        Kind::Custom(KIND_PROJECT_CONTEXT_COMMAND as u16),
        Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16),
        Kind::Custom(KIND_PROJECT_CONTEXT_META as u16),
    ])
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
                panic!(
                    "private Project Context event leaked before CLOSED: {}",
                    event.id
                )
            }
            _ => {}
        }
    }
}

async fn collect_live_bundle(client: &mut BuzzTestClient, subscription_id: &str) -> Vec<Event> {
    let mut events = Vec::new();
    while events.len() < 3 {
        match client
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive committed Project Context fan-out")
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

fn context_event(
    project_id: CommunityId,
    keys: &Keys,
    expected_revision: u64,
    operation: ProjectContextOperation,
    coordinates: &[ProjectContextCoordinate],
    document_id: Uuid,
) -> Event {
    let command = ProjectContextCommand::new(
        expected_revision,
        operation,
        coordinates.to_vec(),
        document_id,
    )
    .expect("build canonical Project Context command");
    build_project_context_command(project_id, command)
        .expect("build strict Project Context command event")
        .sign_with_keys(keys)
        .expect("sign Project Context command")
}

fn context_event_with_fence(
    context: &TestContext,
    keys: &Keys,
    coordinates: &[ProjectContextCoordinate],
    document_id: Uuid,
) -> Event {
    let command = ProjectContextCommand::new(
        0,
        ProjectContextOperation::Attach,
        coordinates.to_vec(),
        document_id,
    )
    .expect("build Human attribution fixture")
    .with_runtime_fence(
        Uuid::new_v4(),
        RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 1,
        },
    );
    build_project_context_command(context.community_id, command)
        .expect("build attributed Context command")
        .sign_with_keys(keys)
        .expect("sign attributed Context command")
}

fn receipt(response: &buzz_ws_client::OkResponse) -> ProjectContextReceipt {
    assert!(response.accepted, "{}", response.message);
    serde_json::from_str(
        response
            .message
            .strip_prefix("response:")
            .expect("canonical Project Context receipt prefix"),
    )
    .expect("parse canonical Project Context receipt")
}

fn verify_bundle(
    context: &TestContext,
    events: &[Event],
    command: &Event,
    expected_receipt: &ProjectContextReceipt,
    expected_state: ProjectContextBindingState,
    expected_edge_count: u64,
    expected_bound_count: u64,
) {
    let kinds: HashSet<u32> = events
        .iter()
        .map(|event| u32::from(event.kind.as_u16()))
        .collect();
    assert_eq!(
        kinds,
        HashSet::from([
            KIND_PROJECT_CONTEXT_COMMAND,
            KIND_PROJECT_CONTEXT_EDGE_BINDING,
            KIND_PROJECT_CONTEXT_META,
        ])
    );
    let observed_command = events
        .iter()
        .find(|event| u32::from(event.kind.as_u16()) == KIND_PROJECT_CONTEXT_COMMAND)
        .expect("fan-out contains Context command");
    assert_eq!(observed_command, command);
    let parsed_command = parse_project_context_command(observed_command, context.community_id)
        .expect("verify fan-out Context command");
    assert_eq!(
        parsed_command.context_document_id(),
        expected_receipt.context_document_id
    );

    let binding_event = events
        .iter()
        .find(|event| u32::from(event.kind.as_u16()) == KIND_PROJECT_CONTEXT_EDGE_BINDING)
        .expect("fan-out contains Context binding");
    let meta_event = events
        .iter()
        .find(|event| u32::from(event.kind.as_u16()) == KIND_PROJECT_CONTEXT_META)
        .expect("fan-out contains Context metadata");
    let binding = parse_project_context_binding(
        binding_event,
        &context.relay.public_key(),
        context.community_id,
    )
    .expect("verify Relay-signed Context binding");
    let meta = parse_project_context_meta(
        meta_event,
        &context.relay.public_key(),
        context.community_id,
    )
    .expect("verify Relay-signed Context metadata");
    verify_project_context_meta_change(&meta, &binding)
        .expect("metadata binds the exact changed binding event");
    assert_eq!(binding.projection.source_event_id, command.id);
    assert_eq!(
        binding.projection.context_revision,
        expected_receipt.context_revision
    );
    assert_eq!(binding.projection.edge_key, expected_receipt.edge_key);
    assert_eq!(binding.projection.state, expected_state);
    assert_eq!(
        meta.projection.context_revision,
        expected_receipt.context_revision
    );
    assert_eq!(meta.projection.active_edge_count, expected_edge_count);
    assert_eq!(meta.projection.bound_document_count, expected_bound_count);
}

async fn create_document(client: &mut BuzzTestClient, keys: &Keys, document_id: Uuid, title: &str) {
    let command = ProjectDocumentCommand::new(
        0,
        DocumentCommandRequest::Create {
            document_id,
            title: title.to_owned(),
            summary: Some("Project Context Stage 3 fixture".to_owned()),
            content_markdown: format!("# {title}\n\nCross-coordinate context."),
        },
    );
    let event = build_document_command(command)
        .expect("build Project Document create")
        .sign_with_keys(keys)
        .expect("sign Project Document create");
    let response = client
        .send_event(event)
        .await
        .expect("submit Project Document create");
    assert!(response.accepted, "{}", response.message);
}

async fn context_state(pool: &PgPool, community_id: CommunityId) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT state.context_revision, state.active_edge_count, state.bound_document_count, \
                (SELECT count(*) FROM project_context_edge_changes change \
                 WHERE change.community_id = state.community_id) \
         FROM project_context_edge_state state WHERE state.community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_one(pool)
    .await
    .expect("read canonical Project Context state")
}

fn assert_no_context_events(body: &Value) {
    let events = body.as_array().expect("HTTP query event array");
    assert!(events.iter().all(|event| {
        event["kind"].as_u64().is_none_or(|kind| {
            ![
                u64::from(KIND_PROJECT_CONTEXT_COMMAND),
                u64::from(KIND_PROJECT_CONTEXT_EDGE_BINDING),
                u64::from(KIND_PROJECT_CONTEXT_META),
            ]
            .contains(&kind)
        })
    }));
}

#[tokio::test]
#[ignore = "requires isolated Project View v3, Project Document, Relay, PostgreSQL, and Redis"]
async fn project_context_stage_three_is_atomic_private_and_operation_aware() {
    let context = setup().await;
    let http = Client::new();
    let db = Db::from_pool(context.pool.clone());

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
            .any(|value| value == PROJECT_CONTEXT_CAPABILITY)));
    assert_eq!(info["self"], context.relay.public_key().to_hex());

    let mut reader = BuzzTestClient::connect(&context.ws_url, &context.reader)
        .await
        .expect("connect Context reader");
    let mut writer = BuzzTestClient::connect(&context.ws_url, &context.writer)
        .await
        .expect("connect Context writer");
    let mut observer = BuzzTestClient::connect(&context.ws_url, &context.writer)
        .await
        .expect("connect independent Context observer");
    let mut outsider = BuzzTestClient::connect(&context.ws_url, &context.outsider)
        .await
        .expect("connect Context outsider");
    let auth_json = nip_oa::compute_auth_tag(
        &context.reader,
        &context.agent.public_key(),
        &format!("kind={KIND_PROJECT_CONTEXT_COMMAND}"),
    )
    .expect("build managed Agent owner attestation");
    let auth_tag = nip_oa::parse_auth_tag(&auth_json).expect("parse managed Agent auth tag");
    let mut agent = BuzzTestClient::connect_unauthenticated(&context.ws_url)
        .await
        .expect("connect managed Agent");
    agent
        .authenticate_with_nip_oa(&context.agent, &auth_tag)
        .await
        .expect("authenticate managed Agent through current owner");

    let live_subscription = format!("pce-live-{}", Uuid::new_v4());
    reader
        .subscribe(&live_subscription, vec![context_filter()])
        .await
        .expect("subscribe to private Context stream");
    let initial = reader
        .collect_until_eose(&live_subscription, Duration::from_secs(5))
        .await
        .expect("collect initial Context snapshot");
    assert_eq!(initial.len(), 1, "bootstrap exposes only reset metadata");
    let reset = parse_project_context_meta(
        &initial[0],
        &context.relay.public_key(),
        context.community_id,
    )
    .expect("verify reset metadata");
    assert!(reset.projection.reset);
    assert_eq!(reset.projection.context_revision, 0);

    let observer_subscription = format!("pce-observer-{}", Uuid::new_v4());
    observer
        .subscribe(&observer_subscription, vec![context_filter()])
        .await
        .expect("subscribe independent Context observer");
    let observer_initial = observer
        .collect_until_eose(&observer_subscription, Duration::from_secs(5))
        .await
        .expect("collect observer Context snapshot");
    assert_eq!(observer_initial.len(), 1);

    let outsider_subscription = format!("pce-outsider-{}", Uuid::new_v4());
    outsider
        .subscribe(&outsider_subscription, vec![context_filter()])
        .await
        .expect("send outsider Context subscription");
    expect_closed(&mut outsider, &outsider_subscription, "membership_required").await;

    let document_one = Uuid::new_v4();
    let document_two = Uuid::new_v4();
    let document_three = Uuid::new_v4();
    create_document(&mut writer, &context.writer, document_one, "Context one").await;
    create_document(&mut writer, &context.writer, document_two, "Context two").await;
    create_document(
        &mut writer,
        &context.writer,
        document_three,
        "Context three",
    )
    .await;
    let coordinates = vec![
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::ProjectProfile,
            object_id: *context.community_id.as_uuid(),
        },
        ProjectContextCoordinate::ProjectViewObject {
            object_type: ProjectViewObjectType::Goal,
            object_id: Uuid::parse_str(GOAL_ID).expect("parse Stage 3 goal UUID"),
        },
    ];

    let human_fence =
        context_event_with_fence(&context, &context.writer, &coordinates, document_one);
    let response = writer
        .send_event(human_fence)
        .await
        .expect("receive Human attribution rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "conflict:project_context:acting_assignment"
    );

    let stale_agent_fence =
        context_event_with_fence(&context, &context.agent, &coordinates, document_one);
    let response = agent
        .send_event(stale_agent_fence)
        .await
        .expect("receive stale managed attribution rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "conflict:project_context:acting_assignment"
    );

    let outsider_attach = context_event(
        context.community_id,
        &context.outsider,
        0,
        ProjectContextOperation::Attach,
        &coordinates,
        document_one,
    );
    let response = outsider
        .send_event(outsider_attach)
        .await
        .expect("receive outsider command rejection");
    assert!(!response.accepted);
    assert!(response.message.contains("membership_required"));
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (0, 0, 0, 0)
    );

    let attach_one = context_event(
        context.community_id,
        &context.agent,
        0,
        ProjectContextOperation::Attach,
        &coordinates,
        document_one,
    );
    let response = agent
        .send_event(attach_one.clone())
        .await
        .expect("submit managed Agent attach without attribution");
    let receipt_one = receipt(&response);
    assert_eq!(receipt_one.actor, context.agent.public_key());
    assert_eq!(receipt_one.acting_assignment_id, None);
    assert_eq!(receipt_one.context_revision, 1);
    assert_eq!(receipt_one.edge_document_count, 1);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &attach_one,
        &receipt_one,
        ProjectContextBindingState::Active,
        1,
        1,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let before_replay = context_state(&context.pool, context.community_id).await;
    let replay = agent
        .send_event(attach_one.clone())
        .await
        .expect("replay managed Agent attach");
    assert_eq!(receipt(&replay), receipt_one);
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        before_replay
    );
    assert!(
        reader.recv_event(Duration::from_millis(500)).await.is_err(),
        "replayed Context command was fanned out again"
    );

    sqlx::query(
        "INSERT INTO community_bans \
            (community_id, pubkey, muted_until, mute_reason, actor_pubkey) \
         VALUES ($1, $2, clock_timestamp() + interval '1 hour', 'Stage 3 timeout', $3)",
    )
    .bind(context.community_id.as_uuid())
    .bind(context.writer.public_key().as_bytes())
    .bind(context.relay.public_key().as_bytes())
    .execute(&context.pool)
    .await
    .expect("time out Context writer");
    let timed_out_attach = context_event(
        context.community_id,
        &context.writer,
        1,
        ProjectContextOperation::Attach,
        &coordinates,
        document_two,
    );
    let response = writer
        .send_event(timed_out_attach)
        .await
        .expect("receive timed-out command rejection");
    assert!(!response.accepted);
    assert!(
        response.message.contains("restricted")
            || response.message.contains("temporarily")
            || response.message.contains("not_authorized"),
        "unexpected timeout rejection: {}",
        response.message
    );
    sqlx::query("DELETE FROM community_bans WHERE community_id = $1 AND pubkey = $2")
        .bind(context.community_id.as_uuid())
        .bind(context.writer.public_key().as_bytes())
        .execute(&context.pool)
        .await
        .expect("clear Context writer timeout");

    let stale_attach = context_event(
        context.community_id,
        &context.writer,
        0,
        ProjectContextOperation::Attach,
        &coordinates,
        document_two,
    );
    let response = writer
        .send_event(stale_attach)
        .await
        .expect("receive stale Context conflict");
    assert!(!response.accepted);
    assert_eq!(response.message, "conflict:project_context:revision");

    let attach_two = context_event(
        context.community_id,
        &context.writer,
        1,
        ProjectContextOperation::Attach,
        &coordinates,
        document_two,
    );
    let response = writer
        .send_event(attach_two.clone())
        .await
        .expect("append second Context Document");
    let receipt_two = receipt(&response);
    assert_eq!(receipt_two.context_revision, 2);
    assert_eq!(receipt_two.edge_key, receipt_one.edge_key);
    assert_eq!(receipt_two.edge_document_count, 2);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &attach_two,
        &receipt_two,
        ProjectContextBindingState::Active,
        1,
        2,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let all_context = json!([{
        "kinds": [
            KIND_PROJECT_CONTEXT_COMMAND,
            KIND_PROJECT_CONTEXT_EDGE_BINDING,
            KIND_PROJECT_CONTEXT_META
        ],
        "limit": 100
    }]);
    let (status, body) = post_json(&http, &context, &context.reader, "/query", &all_context).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.as_array().is_some_and(|events| events.len() >= 5));
    let (status, count) = post_json(&http, &context, &context.reader, "/count", &all_context).await;
    assert_eq!(status, StatusCode::OK, "{count}");
    assert!(count["count"].as_u64().is_some_and(|value| value >= 5));
    let (status, body) =
        post_json(&http, &context, &context.outsider, "/query", &all_context).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");

    let known_ids = json!([{
        "ids": [attach_one.id.to_hex(), attach_two.id.to_hex()],
        "limit": 10
    }]);
    let (status, body) = post_json(&http, &context, &context.outsider, "/query", &known_ids).await;
    assert!(
        status != StatusCode::OK || body.as_array().is_some_and(Vec::is_empty),
        "IDs-only query leaked private Context events: {body}"
    );
    let mixed = json!([{
        "kinds": [1, KIND_PROJECT_CONTEXT_EDGE_BINDING],
        "limit": 100
    }]);
    let (status, body) = post_json(&http, &context, &context.outsider, "/query", &mixed).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_no_context_events(&body);
    let (wrong_status, wrong_body) = post_json_at(
        &http,
        &context.wrong_http_url,
        &context.reader,
        "/query",
        &all_context,
    )
    .await;
    assert!(
        !wrong_status.is_success(),
        "wrong host unexpectedly returned {wrong_body}"
    );

    db.set_project_context_edge_enabled_checked(context.community_id, false, None)
        .await
        .expect("disable Context attach gate");
    let disabled_info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch disabled NIP-11")
        .json()
        .await
        .expect("parse disabled NIP-11");
    assert!(disabled_info["supported_extensions"]
        .as_array()
        .is_none_or(|extensions| extensions
            .iter()
            .all(|value| value != PROJECT_CONTEXT_CAPABILITY)));
    let (status, body) = post_json(&http, &context, &context.reader, "/query", &all_context).await;
    assert_eq!(status, StatusCode::OK, "disabled read failed: {body}");
    assert!(body.as_array().is_some_and(|events| !events.is_empty()));
    let replay = agent
        .send_event(attach_one)
        .await
        .expect("receive disabled attach replay rejection");
    assert!(!replay.accepted);
    assert_eq!(replay.message, "unavailable:project_context:disabled");

    sqlx::query(
        "INSERT INTO community_bans (community_id, pubkey, banned, actor_pubkey) \
         VALUES ($1, $2, true, $3)",
    )
    .bind(context.community_id.as_uuid())
    .bind(context.writer.public_key().as_bytes())
    .bind(context.relay.public_key().as_bytes())
    .execute(&context.pool)
    .await
    .expect("ban active Context observer");
    let (status, _) = post_json(&http, &context, &context.writer, "/query", &all_context).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let detach_one = context_event(
        context.community_id,
        &context.agent,
        2,
        ProjectContextOperation::Detach,
        &coordinates,
        document_one,
    );
    let response = agent
        .send_event(detach_one.clone())
        .await
        .expect("detach non-final Context Document while disabled");
    let receipt_three = receipt(&response);
    assert_eq!(receipt_three.context_revision, 3);
    assert_eq!(receipt_three.edge_state, ProjectContextBindingState::Active);
    assert_eq!(receipt_three.edge_document_count, 1);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &detach_one,
        &receipt_three,
        ProjectContextBindingState::Deleted,
        1,
        1,
    );
    assert!(
        observer
            .recv_event(Duration::from_millis(500))
            .await
            .is_err(),
        "banned observer received private Context fan-out"
    );
    sqlx::query("DELETE FROM community_bans WHERE community_id = $1 AND pubkey = $2")
        .bind(context.community_id.as_uuid())
        .bind(context.writer.public_key().as_bytes())
        .execute(&context.pool)
        .await
        .expect("restore Context observer");

    let detach_two = context_event(
        context.community_id,
        &context.writer,
        3,
        ProjectContextOperation::Detach,
        &coordinates,
        document_two,
    );
    let response = writer
        .send_event(detach_two.clone())
        .await
        .expect("detach final Context Document while disabled");
    let receipt_four = receipt(&response);
    assert_eq!(receipt_four.context_revision, 4);
    assert_eq!(receipt_four.edge_state, ProjectContextBindingState::Deleted);
    assert_eq!(receipt_four.edge_document_count, 0);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &detach_two,
        &receipt_four,
        ProjectContextBindingState::Deleted,
        0,
        0,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let disabled_attach = context_event(
        context.community_id,
        &context.writer,
        4,
        ProjectContextOperation::Attach,
        &coordinates,
        document_three,
    );
    let response = writer
        .send_event(disabled_attach)
        .await
        .expect("receive disabled attach rejection");
    assert!(!response.accepted);
    assert_eq!(response.message, "unavailable:project_context:disabled");
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (4, 0, 0, 4)
    );

    db.set_project_context_edge_enabled_checked(
        context.community_id,
        true,
        Some(&context.relay.public_key()),
    )
    .await
    .expect("re-enable verified Context attach gate");
    let enabled_info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch re-enabled NIP-11")
        .json()
        .await
        .expect("parse re-enabled NIP-11");
    assert!(enabled_info["supported_extensions"]
        .as_array()
        .is_some_and(|extensions| extensions
            .iter()
            .any(|value| value == PROJECT_CONTEXT_CAPABILITY)));

    let recreate = context_event(
        context.community_id,
        &context.writer,
        4,
        ProjectContextOperation::Attach,
        &coordinates,
        document_three,
    );
    let response = writer
        .send_event(recreate.clone())
        .await
        .expect("recreate exact all-active edge");
    let receipt_five = receipt(&response);
    assert_eq!(receipt_five.context_revision, 5);
    assert_eq!(receipt_five.edge_key, receipt_one.edge_key);
    assert_eq!(receipt_five.edge_state, ProjectContextBindingState::Active);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &recreate,
        &receipt_five,
        ProjectContextBindingState::Active,
        1,
        1,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (5, 1, 1, 5)
    );
    let integrity = db
        .verify_project_context_storage(context.community_id, &context.relay.public_key())
        .await
        .expect("verify final canonical/projection parity");
    assert_eq!(integrity.orphan_projection_count, 0);
    assert_eq!(integrity.pointer_mismatch_count, 0);

    reader
        .disconnect()
        .await
        .expect("disconnect Context reader");
    writer
        .disconnect()
        .await
        .expect("disconnect Context writer");
    observer
        .disconnect()
        .await
        .expect("disconnect Context observer");
    outsider
        .disconnect()
        .await
        .expect("disconnect Context outsider");
    agent.disconnect().await.expect("disconnect managed Agent");
    context.pool.close().await;
}
