//! Project Context Stage 3/4 Relay, privacy, operation-gate, and lifecycle E2E.
//!
//! The isolated harness establishes Project View v3, Project Document v1, and
//! an initialized Context Edge catalog. This test drives real signed commands
//! through the Relay and verifies atomic projections, replay/CAS behavior,
//! managed-agent authority, disable semantics, private reads, fan-out, and
//! cross-domain Document/coordinate lifecycle behavior.

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
use buzz_project_document::{
    DocumentCommandRequest, ProjectDocumentCommand, ProjectDocumentReceipt,
};
use buzz_project_view::v3::{
    CreateProjectObjectV3, DeleteProjectObjectV3, DocumentReferenceMode, GoalPatchV3,
    NewProjectViewObjectV3, ProjectContextReference, ProjectObjectCommandV3,
    ProjectObjectRequestV3, UpdateProjectObjectV3,
};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::nip_oa;
use buzz_sdk::project_context::{
    build_project_context_command, parse_project_context_binding, parse_project_context_command,
    parse_project_context_meta, verify_project_context_meta_change,
};
use buzz_sdk::project_document::build_document_command;
use buzz_sdk::project_view_v3::build_project_object_command;
use buzz_test_client::{BuzzTestClient, RelayMessage};
use nostr::{Event, EventBuilder, Filter, Keys, Kind, Tag};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use url::Url;
use uuid::Uuid;

const GOAL_ID: &str = "10000000-0000-4000-8000-00000000c003";
const BACKUP_GOAL_ID: &str = "10000000-0000-4000-8000-00000000c004";

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

fn document_receipt(response: &buzz_ws_client::OkResponse) -> ProjectDocumentReceipt {
    assert!(response.accepted, "{}", response.message);
    serde_json::from_str(
        response
            .message
            .strip_prefix("response:")
            .expect("canonical Project Document receipt prefix"),
    )
    .expect("parse canonical Project Document receipt")
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
    let event = document_event(keys, command);
    let response = client
        .send_event(event)
        .await
        .expect("submit Project Document create");
    assert!(response.accepted, "{}", response.message);
}

fn document_event(keys: &Keys, command: ProjectDocumentCommand) -> Event {
    build_document_command(command)
        .expect("build Project Document command")
        .sign_with_keys(keys)
        .expect("sign Project Document command")
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

async fn current_project_revision(context: &TestContext) -> u64 {
    let revision: i64 = sqlx::query_scalar(
        "SELECT project_revision FROM project_view_state WHERE community_id = $1",
    )
    .bind(context.community_id.as_uuid())
    .fetch_one(&context.pool)
    .await
    .expect("read current Project revision");
    u64::try_from(revision).expect("positive Project revision")
}

async fn submit_project_view_command(
    client: &mut BuzzTestClient,
    keys: &Keys,
    command: ProjectObjectCommandV3,
    label: &str,
) {
    let event = build_project_object_command(command)
        .unwrap_or_else(|error| panic!("build {label}: {error}"))
        .sign_with_keys(keys)
        .unwrap_or_else(|error| panic!("sign {label}: {error}"));
    let response = client
        .send_event(event)
        .await
        .unwrap_or_else(|error| panic!("submit {label}: {error}"));
    assert!(response.accepted, "{label}: {}", response.message);
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
async fn project_context_stage_three_and_four_are_atomic_private_and_lifecycle_safe() {
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

    // Stage 4: the active Context binding participates in the ordinary
    // Project Document deletion precheck and returns the stable domain error.
    let protected_delete = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: document_three,
            },
        ),
    );
    let response = writer
        .send_event(protected_delete)
        .await
        .expect("receive protected Context Document delete rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "conflict:project_document:still_referenced"
    );

    // Tombstoning a Project View coordinate does not rewrite or hide the
    // retained Edge and does not advance the independent Context revision.
    let project_revision: i64 = sqlx::query_scalar(
        "SELECT project_revision FROM project_view_state WHERE community_id = $1",
    )
    .bind(context.community_id.as_uuid())
    .fetch_one(&context.pool)
    .await
    .expect("read Project revision before coordinate tombstone");
    let delete_goal = ProjectObjectCommandV3::new(
        u64::try_from(project_revision).expect("positive Project revision"),
        None,
        ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
            object_type: ProjectViewObjectType::Goal,
            object_id: Uuid::parse_str(GOAL_ID).expect("parse Stage 4 goal UUID"),
        }),
    );
    let delete_goal_event = build_project_object_command(delete_goal)
        .expect("build Project View coordinate tombstone")
        .sign_with_keys(&context.writer)
        .expect("sign Project View coordinate tombstone");
    let response = writer
        .send_event(delete_goal_event)
        .await
        .expect("tombstone Project View coordinate");
    assert!(response.accepted, "{}", response.message);
    let retained_after_goal_delete: (bool, i64, String, i64) = sqlx::query_as(
        "SELECT object.deleted_at IS NOT NULL, state.context_revision, edge.state, \
                (SELECT count(*) FROM project_context_edge_coordinates coordinate \
                 WHERE coordinate.community_id = state.community_id \
                   AND coordinate.coordinate_type = 'project_view_object' \
                   AND coordinate.coordinate_subtype = 'goal' \
                   AND coordinate.coordinate_id = $2) \
         FROM project_view_objects object \
         JOIN project_context_edge_state state ON state.community_id = object.community_id \
         JOIN project_context_edges edge ON edge.community_id = state.community_id \
         WHERE object.community_id = $1 AND object.object_id = $2",
    )
    .bind(context.community_id.as_uuid())
    .bind(Uuid::parse_str(GOAL_ID).expect("parse retained goal UUID"))
    .fetch_one(&context.pool)
    .await
    .expect("read retained Edge after Project View tombstone");
    assert_eq!(
        retained_after_goal_delete,
        (true, 5, "active".to_owned(), 1)
    );

    // The Context Document body remains an ordinary independently versioned
    // Project Document even after one of its Edge coordinates tombstones.
    let update_context_document = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: document_three,
                title: "Context three corrected".to_owned(),
                summary: Some("Updated after coordinate tombstone".to_owned()),
                content_markdown: "# Context three\n\nCorrected semantic explanation.".to_owned(),
            },
        ),
    );
    let response = writer
        .send_event(update_context_document)
        .await
        .expect("update Context Document after coordinate tombstone");
    assert_eq!(document_receipt(&response).document_revision, 2);
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (5, 1, 1, 5)
    );

    let rejected_retained_attach = context_event(
        context.community_id,
        &context.writer,
        5,
        ProjectContextOperation::Attach,
        &coordinates,
        document_one,
    );
    let response = writer
        .send_event(rejected_retained_attach)
        .await
        .expect("receive tombstoned-coordinate attach rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "invalid:project_context:inactive_coordinate"
    );

    let detach_after_goal_delete = context_event(
        context.community_id,
        &context.writer,
        5,
        ProjectContextOperation::Detach,
        &coordinates,
        document_three,
    );
    let response = writer
        .send_event(detach_after_goal_delete.clone())
        .await
        .expect("detach Context Document after coordinate tombstone");
    let receipt_six = receipt(&response);
    assert_eq!(receipt_six.context_revision, 6);
    assert_eq!(receipt_six.edge_document_count, 0);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &detach_after_goal_delete,
        &receipt_six,
        ProjectContextBindingState::Deleted,
        0,
        0,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let released_delete = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: document_three,
            },
        ),
    );
    let response = writer
        .send_event(released_delete)
        .await
        .expect("delete detached Context Document");
    assert_eq!(document_receipt(&response).document_revision, 3);

    // Document coordinates and Context Document bindings are separate roles.
    // A Document can occupy both roles on one Edge and remain a coordinate of
    // an overlapping Edge without implicit propagation between them.
    let shared_document = Uuid::new_v4();
    let coordinate_c = Uuid::new_v4();
    let coordinate_d = Uuid::new_v4();
    let second_context_document = Uuid::new_v4();
    let overlap_candidate = Uuid::new_v4();
    for (document_id, title) in [
        (shared_document, "Shared Context and coordinate"),
        (coordinate_c, "Document coordinate C"),
        (coordinate_d, "Document coordinate D"),
        (second_context_document, "Second overlapping Context"),
        (overlap_candidate, "Rejected overlap candidate"),
    ] {
        create_document(&mut writer, &context.writer, document_id, title).await;
    }
    let resource_id = Uuid::new_v4();
    submit_project_view_command(
        &mut writer,
        &context.writer,
        ProjectObjectCommandV3::new(
            current_project_revision(&context).await,
            None,
            ProjectObjectRequestV3::Create(CreateProjectObjectV3 {
                object: NewProjectViewObjectV3::Resource {
                    id: resource_id,
                    name: "Shared Context guide".to_owned(),
                    resource_kind: "runbook".to_owned(),
                    summary: Some("Stage 4 structural-role fixture".to_owned()),
                    guide_document_id: shared_document,
                    context_references: Vec::new(),
                },
            }),
        ),
        "Resource using the shared Document as its Guide",
    )
    .await;
    submit_project_view_command(
        &mut writer,
        &context.writer,
        ProjectObjectCommandV3::new(
            current_project_revision(&context).await,
            None,
            ProjectObjectRequestV3::Update(UpdateProjectObjectV3::Goal {
                object_id: Uuid::parse_str(BACKUP_GOAL_ID).expect("parse backup Goal UUID"),
                patch: GoalPatchV3 {
                    context_references: Some(vec![ProjectContextReference::Document {
                        document_id: shared_document,
                        mode: DocumentReferenceMode::Live,
                        document_revision: None,
                    }]),
                    ..GoalPatchV3::default()
                },
            }),
        ),
        "Live Context Reference to the shared Document",
    )
    .await;
    let established_independent_roles: (i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM project_view_objects \
             WHERE community_id = $1 AND object_id = $2 \
               AND deleted_at IS NULL AND guide_document_id = $3), \
            (SELECT count(*) FROM project_view_document_context_references \
             WHERE community_id = $1 AND target_document_id = $3 \
               AND reference_mode = 'live')",
    )
    .bind(context.community_id.as_uuid())
    .bind(resource_id)
    .bind(shared_document)
    .fetch_one(&context.pool)
    .await
    .expect("read established Guide and Context Reference roles");
    assert_eq!(established_independent_roles, (1, 1));
    let first_document_coordinates = vec![
        ProjectContextCoordinate::Document {
            document_id: shared_document,
        },
        ProjectContextCoordinate::Document {
            document_id: coordinate_c,
        },
    ];
    let second_document_coordinates = vec![
        ProjectContextCoordinate::Document {
            document_id: shared_document,
        },
        ProjectContextCoordinate::Document {
            document_id: coordinate_d,
        },
    ];
    let attach_shared = context_event(
        context.community_id,
        &context.writer,
        6,
        ProjectContextOperation::Attach,
        &first_document_coordinates,
        shared_document,
    );
    let response = writer
        .send_event(attach_shared.clone())
        .await
        .expect("attach Document in both structural roles");
    let receipt_seven = receipt(&response);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &attach_shared,
        &receipt_seven,
        ProjectContextBindingState::Active,
        1,
        1,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let attach_overlap = context_event(
        context.community_id,
        &context.writer,
        7,
        ProjectContextOperation::Attach,
        &second_document_coordinates,
        second_context_document,
    );
    let response = writer
        .send_event(attach_overlap.clone())
        .await
        .expect("attach overlapping Document-coordinate Edge");
    let receipt_eight = receipt(&response);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &attach_overlap,
        &receipt_eight,
        ProjectContextBindingState::Active,
        2,
        2,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let delete_coordinate_c = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: coordinate_c,
            },
        ),
    );
    let response = writer
        .send_event(delete_coordinate_c)
        .await
        .expect("delete coordinate-only Document");
    assert_eq!(document_receipt(&response).document_revision, 2);
    let update_shared = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: shared_document,
                title: "Shared Context corrected".to_owned(),
                summary: None,
                content_markdown: "# Shared Context\n\nUpdated without changing either Edge."
                    .to_owned(),
            },
        ),
    );
    let response = writer
        .send_event(update_shared)
        .await
        .expect("update shared Context Document");
    assert_eq!(document_receipt(&response).document_revision, 2);
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (8, 2, 2, 8)
    );

    let rejected_first_overlap = context_event(
        context.community_id,
        &context.writer,
        8,
        ProjectContextOperation::Attach,
        &first_document_coordinates,
        overlap_candidate,
    );
    let response = writer
        .send_event(rejected_first_overlap)
        .await
        .expect("receive deleted Document-coordinate rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "invalid:project_context:inactive_coordinate"
    );

    let detach_shared = context_event(
        context.community_id,
        &context.writer,
        8,
        ProjectContextOperation::Detach,
        &first_document_coordinates,
        shared_document,
    );
    let response = writer
        .send_event(detach_shared.clone())
        .await
        .expect("detach shared Context role");
    let receipt_nine = receipt(&response);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &detach_shared,
        &receipt_nine,
        ProjectContextBindingState::Deleted,
        1,
        1,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;

    let independently_protected_delete = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: shared_document,
            },
        ),
    );
    let response = writer
        .send_event(independently_protected_delete)
        .await
        .expect("receive non-Edge deletion protection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "conflict:project_document:still_referenced"
    );
    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (9, 1, 1, 9)
    );

    // Context Edge detach does not mutate the independent Guide or Context
    // Reference. Remove both explicitly, then the same Document may tombstone
    // even though it is still a coordinate of the other retained Edge.
    submit_project_view_command(
        &mut writer,
        &context.writer,
        ProjectObjectCommandV3::new(
            current_project_revision(&context).await,
            None,
            ProjectObjectRequestV3::Update(UpdateProjectObjectV3::Goal {
                object_id: Uuid::parse_str(BACKUP_GOAL_ID).expect("parse backup Goal UUID"),
                patch: GoalPatchV3 {
                    context_references: Some(Vec::new()),
                    ..GoalPatchV3::default()
                },
            }),
        ),
        "remove independent Live Context Reference",
    )
    .await;
    submit_project_view_command(
        &mut writer,
        &context.writer,
        ProjectObjectCommandV3::new(
            current_project_revision(&context).await,
            None,
            ProjectObjectRequestV3::Delete(DeleteProjectObjectV3 {
                object_type: ProjectViewObjectType::Resource,
                object_id: resource_id,
            }),
        ),
        "delete independent Resource Guide owner",
    )
    .await;
    let delete_shared = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: shared_document,
            },
        ),
    );
    let response = writer
        .send_event(delete_shared)
        .await
        .expect("delete Document that remains only an Edge coordinate");
    assert_eq!(document_receipt(&response).document_revision, 3);

    let update_second_context = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: second_context_document,
                title: "Second Context corrected".to_owned(),
                summary: Some("Shared coordinate is now tombstoned".to_owned()),
                content_markdown: "# Second Context\n\nStill editable after tombstone.".to_owned(),
            },
        ),
    );
    let response = writer
        .send_event(update_second_context)
        .await
        .expect("update Context on retained overlapping Edge");
    assert_eq!(document_receipt(&response).document_revision, 2);
    let rejected_second_overlap = context_event(
        context.community_id,
        &context.writer,
        9,
        ProjectContextOperation::Attach,
        &second_document_coordinates,
        overlap_candidate,
    );
    let response = writer
        .send_event(rejected_second_overlap)
        .await
        .expect("receive shared tombstoned-coordinate rejection");
    assert!(!response.accepted);
    assert_eq!(
        response.message,
        "invalid:project_context:inactive_coordinate"
    );

    let detach_second_overlap = context_event(
        context.community_id,
        &context.writer,
        9,
        ProjectContextOperation::Detach,
        &second_document_coordinates,
        second_context_document,
    );
    let response = writer
        .send_event(detach_second_overlap.clone())
        .await
        .expect("detach retained overlapping Edge");
    let receipt_ten = receipt(&response);
    verify_bundle(
        &context,
        &collect_live_bundle(&mut reader, &live_subscription).await,
        &detach_second_overlap,
        &receipt_ten,
        ProjectContextBindingState::Deleted,
        0,
        0,
    );
    let _ = collect_live_bundle(&mut observer, &observer_subscription).await;
    let delete_second_context = document_event(
        &context.writer,
        ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: second_context_document,
            },
        ),
    );
    let response = writer
        .send_event(delete_second_context)
        .await
        .expect("delete released second Context Document");
    assert_eq!(document_receipt(&response).document_revision, 3);

    assert_eq!(
        context_state(&context.pool, context.community_id).await,
        (10, 0, 0, 10)
    );
    let role_independence: (i64, i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM project_context_edges \
             WHERE community_id = $1), \
            (SELECT count(*) FROM project_context_document_bindings \
             WHERE community_id = $1), \
            (SELECT count(*) FROM project_context_edge_coordinates \
             WHERE community_id = $1 AND coordinate_id = $2)",
    )
    .bind(context.community_id.as_uuid())
    .bind(shared_document)
    .fetch_one(&context.pool)
    .await
    .expect("read independent structural roles");
    assert_eq!(role_independence, (3, 5, 2));
    let integrity = db
        .verify_project_context_storage(context.community_id, &context.relay.public_key())
        .await
        .expect("verify lifecycle canonical/projection parity");
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
