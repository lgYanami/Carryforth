//! End-to-end Project View protocol coverage.
//!
//! The test needs a running Relay with PostgreSQL, Redis, migrations through
//! 0025, and an explicit `BUZZ_RELAY_PRIVATE_KEY`. It creates an isolated
//! `*.localhost` Community by default so initialization is repeatable.

use std::process::Command;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{
    KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::tenant::relay_url_authority;
use buzz_project_view::{
    CreateMutation, InitializeGoal, InitializeMutation, Mutation, MutationRequest,
    NewProjectViewObject, ProjectProfile,
};
use buzz_test_client::{BuzzTestClient, RelayMessage, TestClientError};
use nostr::{Event, EventBuilder, Filter, Keys, Kind, PublicKey, Tag, ToBech32};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use url::Url;
use uuid::Uuid;

struct TestContext {
    ws_url: String,
    http_url: String,
    host: String,
    community_id: Uuid,
    pool: PgPool,
}

fn base_relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_owned())
}

async fn setup() -> TestContext {
    let ws_url = if let Ok(explicit) = std::env::var("PROJECT_VIEW_E2E_RELAY_URL") {
        explicit
    } else {
        let mut url = Url::parse(&base_relay_url()).expect("parse RELAY_URL");
        let isolated_host = format!("project-view-{}.localhost", Uuid::new_v4().simple());
        url.set_host(Some(&isolated_host))
            .expect("set isolated Project View host");
        url.to_string().trim_end_matches('/').to_owned()
    };
    let http_url = ws_url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    let host = relay_url_authority(&ws_url);
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_owned());
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect(&database_url)
        .await
        .expect("connect Project View E2E database");
    let community_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO communities (id, host) \
         VALUES ($1, $2) \
         ON CONFLICT (lower(host)) DO NOTHING",
    )
    .bind(community_id)
    .bind(&host)
    .execute(&pool)
    .await
    .expect("resolve admin-enabled Project View E2E Community");
    let community_id: Uuid =
        sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
            .bind(&host)
            .fetch_one(&pool)
            .await
            .expect("resolve Project View E2E Community");
    TestContext {
        ws_url,
        http_url,
        host,
        community_id,
        pool,
    }
}

async fn seed_member(context: &TestContext, keys: &Keys) {
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, 'member') \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = 'member'",
    )
    .bind(context.community_id)
    .bind(keys.public_key().to_hex())
    .execute(&context.pool)
    .await
    .expect("seed Project View E2E member");
}

fn initialize_mutation() -> Mutation {
    Mutation::new(
        0,
        MutationRequest::Initialize(InitializeMutation {
            profile: ProjectProfile {
                name: "Protocol E2E".to_owned(),
                positioning: "One canonical project view".to_owned(),
                purpose: "Prove native Buzz protocol parity".to_owned(),
                problem: "Project context is fragmented".to_owned(),
                scope: "Relay Slice 3".to_owned(),
            },
            goals: vec![InitializeGoal {
                id: Uuid::new_v4(),
                title: "Complete Relay integration".to_owned(),
                desired_outcome: "WS and HTTP observe one current state".to_owned(),
                directions: vec!["Keep security gates shared".to_owned()],
            }],
        }),
    )
}

fn create_goal_mutation(expected_revision: u64, title: &str) -> Mutation {
    Mutation::new(
        expected_revision,
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Goal {
                id: Uuid::new_v4(),
                title: title.to_owned(),
                desired_outcome: format!("{title} is complete"),
                directions: Vec::new(),
            },
        }),
    )
}

fn create_role_mutation(expected_revision: u64, role_id: Uuid) -> Mutation {
    Mutation::new(
        expected_revision,
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Role {
                id: role_id,
                name: "Module maintainer".to_owned(),
                purpose: "Own one bounded implementation area".to_owned(),
                responsibilities: vec!["Keep the module healthy".to_owned()],
                boundaries: vec!["Does not govern Leader roles".to_owned()],
                active: true,
            },
        }),
    )
}

fn mutation_event(keys: &Keys, mutation: &Mutation) -> Event {
    EventBuilder::new(
        Kind::Custom(KIND_PROJECT_VIEW_MUTATION as u16),
        serde_json::to_string(mutation).expect("serialize Project View mutation"),
    )
    .tags([
        Tag::parse(["-"]).expect("protected tag"),
        Tag::parse(["t", "buzz-project-view-mutation"]).expect("mutation tag"),
    ])
    .sign_with_keys(keys)
    .expect("sign Project View mutation")
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
    let json = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("parse {path} response ({status}): {error}: {text}"));
    (status, json)
}

async fn submit_http(
    client: &Client,
    context: &TestContext,
    keys: &Keys,
    event: &Event,
) -> (StatusCode, Value) {
    post_json(
        client,
        context,
        keys,
        "/events",
        &serde_json::to_value(event).expect("serialize event"),
    )
    .await
}

fn event_project_revision(event: &Event) -> Option<u64> {
    serde_json::from_str::<Value>(&event.content)
        .ok()
        .and_then(|content| content.get("project_revision")?.as_u64())
}

async fn collect_live_revision(
    client: &mut BuzzTestClient,
    sub_id: &str,
    revision: u64,
    expected: usize,
) -> Vec<Event> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut events = Vec::new();
    while events.len() < expected {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::ZERO);
        assert!(
            !remaining.is_zero(),
            "timed out collecting revision {revision}"
        );
        match client
            .recv_event(remaining)
            .await
            .expect("receive Project View live event")
        {
            RelayMessage::Event {
                subscription_id,
                event,
            } if subscription_id == sub_id && event_project_revision(&event) == Some(revision) => {
                events.push(*event);
            }
            _ => {}
        }
    }
    events
}

fn verify_projection_events(events: &[Event], relay_pubkey: PublicKey) {
    for event in events {
        assert!(
            matches!(
                event.kind.as_u16() as u32,
                KIND_PROJECT_VIEW_OBJECT | KIND_PROJECT_VIEW_META
            ),
            "unexpected Project View live kind {}",
            event.kind.as_u16()
        );
        assert_eq!(event.pubkey, relay_pubkey);
        event.verify().expect("valid Relay projection signature");
    }
}

fn projection_has_actor(events: &[Event], actor: PublicKey) -> bool {
    events.iter().any(|event| {
        serde_json::from_str::<Value>(&event.content)
            .ok()
            .and_then(|content| content["object"]["updated_by"].as_str().map(str::to_owned))
            .is_some_and(|updated_by| updated_by == actor.to_hex())
    })
}

async fn current_meta(
    client: &Client,
    context: &TestContext,
    keys: &Keys,
    relay_pubkey: PublicKey,
) -> Value {
    let (status, result) = post_json(
        client,
        context,
        keys,
        "/query",
        &json!([{
            "kinds": [KIND_PROJECT_VIEW_META],
            "authors": [relay_pubkey.to_hex()],
            "limit": 2
        }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "meta query failed: {result}");
    let events = result.as_array().expect("meta result array");
    assert_eq!(events.len(), 1, "expected one current metadata head");
    serde_json::from_str(
        events[0]["content"]
            .as_str()
            .expect("metadata content string"),
    )
    .expect("parse metadata content")
}

fn create_goal_with_real_cli(
    context: &TestContext,
    writer: &Keys,
    expected_project_revision: u64,
) -> Value {
    let buzz = std::env::var("PROJECT_VIEW_E2E_BUZZ_BIN")
        .expect("PROJECT_VIEW_E2E_BUZZ_BIN must point at the real buzz CLI");
    let input_path = std::env::temp_dir().join(format!(
        "buzz-project-view-cli-{}.json",
        Uuid::new_v4().simple()
    ));
    std::fs::write(
        &input_path,
        serde_json::to_vec(&json!({
            "title": "Created by the real buzz CLI",
            "desired_outcome": "The packaged agent surface exercises typed Project View writes",
            "directions": ["Do not hand-write kind or tags"]
        }))
        .expect("serialize CLI fixture"),
    )
    .expect("write CLI fixture");

    let expected_revision = expected_project_revision.to_string();
    let output = Command::new(buzz)
        .args([
            "--format",
            "compact",
            "project-view",
            "create",
            "goal",
            "--expected-project-revision",
            &expected_revision,
            "--data",
        ])
        .arg(&input_path)
        .env("BUZZ_RELAY_URL", &context.http_url)
        .env(
            "BUZZ_PRIVATE_KEY",
            writer
                .secret_key()
                .to_bech32()
                .expect("encode E2E writer nsec"),
        )
        .env_remove("BUZZ_AUTH_TAG")
        .output()
        .expect("run real buzz CLI");
    let _ = std::fs::remove_file(&input_path);
    assert!(
        output.status.success(),
        "real buzz CLI failed (status={}): stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse real buzz CLI JSON output")
}

fn run_real_buzz(context: &TestContext, keys: &Keys, args: &[&str]) -> Value {
    let buzz = std::env::var("PROJECT_VIEW_E2E_BUZZ_BIN")
        .expect("PROJECT_VIEW_E2E_BUZZ_BIN must point at the real buzz CLI");
    let output = Command::new(buzz)
        .args(["--format", "compact"])
        .args(args)
        .env("BUZZ_RELAY_URL", &context.http_url)
        .env(
            "BUZZ_PRIVATE_KEY",
            keys.secret_key()
                .to_bech32()
                .expect("encode E2E command signer nsec"),
        )
        .env_remove("BUZZ_AUTH_TAG")
        .output()
        .expect("run real buzz CLI");
    assert!(
        output.status.success(),
        "real buzz CLI failed (status={}): stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("parse real buzz CLI JSON output")
}

fn run_real_buzz_rejected(context: &TestContext, keys: &Keys, args: &[&str]) -> String {
    let buzz = std::env::var("PROJECT_VIEW_E2E_BUZZ_BIN")
        .expect("PROJECT_VIEW_E2E_BUZZ_BIN must point at the real buzz CLI");
    let output = Command::new(buzz)
        .args(["--format", "compact"])
        .args(args)
        .env("BUZZ_RELAY_URL", &context.http_url)
        .env(
            "BUZZ_PRIVATE_KEY",
            keys.secret_key()
                .to_bech32()
                .expect("encode E2E command signer nsec"),
        )
        .env_remove("BUZZ_AUTH_TAG")
        .output()
        .expect("run rejected real buzz CLI command");
    assert!(
        !output.status.success(),
        "expected real buzz CLI command to fail: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn run_real_admin(args: &[&str]) -> String {
    let admin = std::env::var("PROJECT_VIEW_E2E_ADMIN_BIN")
        .expect("PROJECT_VIEW_E2E_ADMIN_BIN must point at the real buzz-admin CLI");
    let relay_private_key = std::env::var("PROJECT_VIEW_E2E_RELAY_PRIVATE_KEY")
        .expect("PROJECT_VIEW_E2E_RELAY_PRIVATE_KEY must match the running Relay");
    let output = Command::new(admin)
        .args(args)
        .env(
            "DATABASE_URL",
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
        )
        .env("BUZZ_RELAY_PRIVATE_KEY", relay_private_key)
        .output()
        .expect("run real buzz-admin");
    assert!(
        output.status.success(),
        "real buzz-admin failed (status={}): stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
#[ignore = "requires running Relay, Postgres, Redis, and stable relay signer"]
async fn project_view_ws_http_read_write_pagination_and_live_revocation() {
    let context = setup().await;
    let writer = Keys::generate();
    let agent = Keys::generate();
    let successor = Keys::generate();
    let reader = Keys::generate();
    seed_member(&context, &writer).await;
    seed_member(&context, &agent).await;
    seed_member(&context, &successor).await;
    seed_member(&context, &reader).await;
    let http = Client::new();

    let info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch NIP-11")
        .json()
        .await
        .expect("parse NIP-11");
    let relay_pubkey = PublicKey::parse(
        info["self"]
            .as_str()
            .expect("stable Relay must advertise NIP-11 self"),
    )
    .expect("parse Relay self");
    assert!(
        info["supported_extensions"]
            .as_array()
            .is_some_and(|extensions| {
                extensions
                    .iter()
                    .any(|value| value == "buzz-project-view-v1")
            }),
        "Project View capability was not advertised for {}: {info}",
        context.host
    );

    let sub_id = format!("project-view-{}", Uuid::new_v4());
    let mut subscriber = BuzzTestClient::connect(&context.ws_url, &reader)
        .await
        .expect("connect Project View subscriber");
    subscriber
        .subscribe(
            &sub_id,
            vec![Filter::new()
                .kinds([
                    Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16),
                    Kind::Custom(KIND_PROJECT_VIEW_META as u16),
                ])
                .authors([relay_pubkey])],
        )
        .await
        .expect("open Project View subscription");
    let historical = subscriber
        .collect_until_eose(&sub_id, Duration::from_secs(5))
        .await
        .expect("collect empty Project View history");
    assert!(historical.is_empty());

    let mut ws_writer = BuzzTestClient::connect(&context.ws_url, &writer)
        .await
        .expect("connect Project View writer");
    let initialized = ws_writer
        .send_event(mutation_event(&writer, &initialize_mutation()))
        .await
        .expect("submit WS initialization");
    assert!(
        initialized.accepted,
        "WS initialization rejected: {}",
        initialized.message
    );
    let revision_one = collect_live_revision(&mut subscriber, &sub_id, 1, 3).await;
    verify_projection_events(&revision_one, relay_pubkey);

    let create_revision_two = mutation_event(
        &agent,
        &create_goal_mutation(1, "Exercise HTTP mutation path"),
    );
    let (status, response) = submit_http(&http, &context, &agent, &create_revision_two).await;
    assert_eq!(status, StatusCode::OK, "HTTP mutation failed: {response}");
    assert_eq!(response["accepted"], true);
    let revision_two = collect_live_revision(&mut subscriber, &sub_id, 2, 2).await;
    verify_projection_events(&revision_two, relay_pubkey);
    assert!(
        projection_has_actor(&revision_two, agent.public_key()),
        "revision two did not preserve the Agent mutation source"
    );

    let meta = current_meta(&http, &context, &writer, relay_pubkey).await;
    assert_eq!(meta["project_revision"], 2);
    assert_eq!(meta["projection_generation"], 1);
    assert_eq!(meta["active_object_count"], 3);

    let first_page_filter = json!([{
        "kinds": [KIND_PROJECT_VIEW_OBJECT],
        "authors": [relay_pubkey.to_hex()],
        "#t": ["buzz-project-view-active"],
        "limit": 2,
        "buzz_project_view": {
            "revision": 2,
            "projection_generation": 1
        }
    }]);
    let (status, first_page) =
        post_json(&http, &context, &writer, "/query", &first_page_filter).await;
    assert_eq!(status, StatusCode::OK, "first snapshot page: {first_page}");
    let first_page = first_page.as_array().expect("first page array");
    assert_eq!(first_page.len(), 2);
    let last_content: Value = serde_json::from_str(
        first_page
            .last()
            .and_then(|event| event["content"].as_str())
            .expect("last page event content"),
    )
    .expect("parse last page event");
    let cursor_type = last_content["object"]["object_type"]
        .as_str()
        .expect("cursor object type");
    let cursor_id = last_content["object"]["id"]
        .as_str()
        .expect("cursor object id");
    let (status, second_page) = post_json(
        &http,
        &context,
        &writer,
        "/query",
        &json!([{
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [relay_pubkey.to_hex()],
            "#t": ["buzz-project-view-active"],
            "limit": 2,
            "buzz_project_view": {
                "revision": 2,
                "projection_generation": 1,
                "after": {
                    "object_type": cursor_type,
                    "object_id": cursor_id
                }
            }
        }]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "second snapshot page: {second_page}"
    );
    assert_eq!(second_page.as_array().map(Vec::len), Some(1));

    let (status, count) = post_json(
        &http,
        &context,
        &writer,
        "/count",
        &json!([{"kinds": [KIND_PROJECT_VIEW_OBJECT]}]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Project View count failed: {count}");
    assert_eq!(count["count"], 3);

    let cli_result = create_goal_with_real_cli(&context, &writer, 2);
    assert_eq!(cli_result["accepted"], true);
    assert!(cli_result["object_id"].as_str().is_some());
    let revision_three = collect_live_revision(&mut subscriber, &sub_id, 3, 2).await;
    verify_projection_events(&revision_three, relay_pubkey);
    assert!(
        projection_has_actor(&revision_three, writer.public_key()),
        "revision three did not return authorship to the Human CLI writer"
    );
    let meta = current_meta(&http, &context, &writer, relay_pubkey).await;
    assert_eq!(meta["project_revision"], 3);
    assert_eq!(meta["active_object_count"], 4);

    sqlx::query("DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(context.community_id)
        .bind(reader.public_key().to_hex())
        .execute(&context.pool)
        .await
        .expect("revoke Project View reader");
    let revision_four_event =
        mutation_event(&agent, &create_goal_mutation(3, "Verify live revocation"));
    let (status, response) = submit_http(&http, &context, &agent, &revision_four_event).await;
    assert_eq!(status, StatusCode::OK, "revision four failed: {response}");
    assert!(matches!(
        subscriber.recv_event(Duration::from_millis(750)).await,
        Err(TestClientError::Timeout)
    ));

    let mixed_sub = format!("project-view-mixed-{}", Uuid::new_v4());
    subscriber
        .subscribe(
            &mixed_sub,
            vec![Filter::new().kinds([
                Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16),
                Kind::TextNote,
            ])],
        )
        .await
        .expect("open mixed subscription after revocation");
    let mixed_history = subscriber
        .collect_until_eose(&mixed_sub, Duration::from_secs(5))
        .await
        .expect("collect mixed history");
    assert!(
        mixed_history
            .iter()
            .all(|event| event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_OBJECT),
        "revoked reader received Project View history through mixed filter"
    );

    let exclusive_sub = format!("project-view-exclusive-{}", Uuid::new_v4());
    subscriber
        .subscribe(
            &exclusive_sub,
            vec![Filter::new().kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))],
        )
        .await
        .expect("send exclusive subscription");
    loop {
        match subscriber
            .recv_event(Duration::from_secs(5))
            .await
            .expect("receive exclusive Project View rejection")
        {
            RelayMessage::Closed {
                subscription_id,
                message,
            } if subscription_id == exclusive_sub => {
                assert!(message.starts_with("restricted:"));
                break;
            }
            _ => {}
        }
    }

    let (status, stale_page) =
        post_json(&http, &context, &writer, "/query", &first_page_filter).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "revision-pinned stale page must conflict: {stale_page}"
    );
    let stale_mutation = mutation_event(&writer, &create_goal_mutation(2, "Stale write"));
    let (status, conflict) = submit_http(&http, &context, &writer, &stale_mutation).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "stale mutation must conflict: {conflict}"
    );

    let row = sqlx::query(
        "SELECT project_revision, projection_generation \
         FROM project_view_state WHERE community_id = $1",
    )
    .bind(context.community_id)
    .fetch_one(&context.pool)
    .await
    .expect("read final Project View state");
    assert_eq!(row.get::<i64, _>("project_revision"), 4);
    assert_eq!(row.get::<i64, _>("projection_generation"), 1);

    let role_id = Uuid::new_v4();
    let role_event = mutation_event(&writer, &create_role_mutation(4, role_id));
    let (status, response) = submit_http(&http, &context, &writer, &role_event).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "v1 Role creation failed: {response}"
    );
    assert_eq!(response["accepted"], true);

    run_real_admin(&["project-view", "disable", "--community", &context.host]);
    let relay_owner = Keys::parse(
        &std::env::var("PROJECT_VIEW_E2E_RELAY_PRIVATE_KEY")
            .expect("PROJECT_VIEW_E2E_RELAY_PRIVATE_KEY must be set"),
    )
    .expect("parse Relay owner key");
    let relay_pubkey_hex = relay_pubkey.to_hex();
    let cutover = run_real_admin(&[
        "project-view",
        "cutover-v2",
        "--community",
        &context.host,
        "--idempotency-key",
        "project-view-role-continuity-e2e",
        "--expected-pubkey",
        &relay_pubkey_hex,
    ]);
    let cutover: Value = serde_json::from_str(&cutover).expect("parse v2 cutover result");
    assert_eq!(cutover["project_revision"], 6);
    assert_eq!(cutover["projection_generation"], 2);
    run_real_admin(&["project-view", "enable", "--community", &context.host]);

    let info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch v2 NIP-11")
        .json()
        .await
        .expect("parse v2 NIP-11");
    assert!(
        info["supported_extensions"]
            .as_array()
            .is_some_and(|extensions| {
                extensions
                    .iter()
                    .any(|value| value == "buzz-project-view-v2")
            }),
        "Project View v2 capability was not advertised: {info}"
    );

    let role_id_text = role_id.to_string();
    let agent_pubkey = agent.public_key().to_hex();
    let offered = run_real_buzz(
        &context,
        &relay_owner,
        &[
            "roles",
            "offer",
            "--role",
            &role_id_text,
            "--member",
            &agent_pubkey,
            "--expected-project-revision",
            "6",
        ],
    );
    assert_eq!(offered["accepted"], true);
    let proposals = run_real_buzz(
        &context,
        &agent,
        &["roles", "proposals", "--status", "open"],
    );
    let proposal_id = proposals["proposals"]
        .as_array()
        .and_then(|proposals| {
            proposals
                .iter()
                .find(|proposal| proposal["role_id"] == role_id_text)
        })
        .and_then(|proposal| proposal["proposal_id"].as_str())
        .expect("find open Role offer")
        .to_owned();
    let accepted = run_real_buzz(
        &context,
        &agent,
        &[
            "roles",
            "proposal",
            "accept",
            &proposal_id,
            "--expected-project-revision",
            "7",
        ],
    );
    assert_eq!(accepted["accepted"], true);

    let current = run_real_buzz(&context, &agent, &["roles", "current"]);
    assert_eq!(current["project_revision"], 8);
    assert_eq!(current["assigned"], true);
    assert_eq!(current["role"]["role_id"], role_id_text);
    let first_assignment_id = current["assignment"]["assignment_id"]
        .as_str()
        .expect("first active Assignment ID")
        .to_owned();
    let rejection = run_real_buzz_rejected(
        &context,
        &agent,
        &[
            "roles",
            "assignment",
            "end",
            &first_assignment_id,
            "--expected-project-revision",
            "8",
        ],
    );
    assert!(
        rejection.contains("restricted:project_view:self_end"),
        "unexpected self-end rejection: {rejection}"
    );

    let successor_pubkey = successor.public_key().to_hex();
    let offered = run_real_buzz(
        &context,
        &relay_owner,
        &[
            "roles",
            "offer",
            "--role",
            &role_id_text,
            "--member",
            &successor_pubkey,
            "--expected-project-revision",
            "8",
        ],
    );
    assert_eq!(offered["accepted"], true);
    let proposals = run_real_buzz(
        &context,
        &successor,
        &["roles", "proposals", "--status", "open"],
    );
    let replacement_proposal_id = proposals["proposals"]
        .as_array()
        .and_then(|proposals| {
            proposals
                .iter()
                .find(|proposal| proposal["role_id"] == role_id_text)
        })
        .and_then(|proposal| proposal["proposal_id"].as_str())
        .expect("find replacement Role offer")
        .to_owned();
    let accepted = run_real_buzz(
        &context,
        &successor,
        &[
            "roles",
            "proposal",
            "accept",
            &replacement_proposal_id,
            "--expected-project-revision",
            "9",
        ],
    );
    assert_eq!(accepted["accepted"], true);

    let role = run_real_buzz(&context, &successor, &["roles", "get", &role_id_text]);
    assert_eq!(role["project_revision"], 10);
    assert_eq!(
        role["current_assignment"]["member_pubkey"],
        successor_pubkey
    );
    assert_eq!(role["assignment_history"].as_array().map(Vec::len), Some(2));
    assert_eq!(role["handoffs"].as_array().map(Vec::len), Some(1));

    ws_writer.disconnect().await.expect("disconnect writer");
    subscriber
        .disconnect()
        .await
        .expect("disconnect subscriber");
}
