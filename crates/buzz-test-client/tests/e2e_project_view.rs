//! End-to-end Project View protocol coverage.
//!
//! The test needs a running Relay with PostgreSQL, Redis, the current schema,
//! and an explicit `BUZZ_RELAY_PRIVATE_KEY`. It creates an isolated schema-v3
//! Community by default so greenfield initialization is repeatable. Legacy
//! schema migration is intentionally covered only by explicitly named
//! migration/recovery fixtures.

use std::process::Command;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use buzz_core::kind::{KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_OBJECT};
use buzz_core::tenant::{relay_url_authority, CommunityId};
use buzz_project_view::v3::{
    CreateProjectObjectV3, NewProjectViewObjectV3, ProjectObjectCommandV3, ProjectObjectRequestV3,
};
use buzz_sdk::project_view_v3::{
    build_project_object_command, parse_entity_projection, parse_meta_projection,
    parse_project_object_projection, PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION,
    PROJECT_VIEW_V3_ENTITY_TAG, PROJECT_VIEW_V3_EXTENSION, PROJECT_VIEW_V3_META_TAG,
    PROJECT_VIEW_V3_OBJECT_TAG,
};
use buzz_test_client::{BuzzTestClient, RelayMessage, TestClientError};
use nostr::{
    Alphabet, Event, EventBuilder, Filter, Keys, Kind, PublicKey, SingleLetterTag, Tag, ToBech32,
};
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
    assert_eq!(
        std::env::var("PROJECT_VIEW_E2E_SCRATCH_DATABASE").as_deref(),
        Ok("1"),
        "refusing Project View E2E without an explicit scratch-database sentinel"
    );
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
        "INSERT INTO communities (id, host, project_view_schema_version) \
         VALUES ($1, $2, 3) \
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
    let schema_version: i16 =
        sqlx::query_scalar("SELECT project_view_schema_version FROM communities WHERE id = $1")
            .bind(community_id)
            .fetch_one(&pool)
            .await
            .expect("read Project View E2E schema version");
    assert_eq!(
        schema_version, 3,
        "Project View E2E must start on schema v3"
    );
    TestContext {
        ws_url,
        http_url,
        host,
        community_id,
        pool,
    }
}

async fn seed_member_with_role(context: &TestContext, keys: &Keys, role: &str) {
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(context.community_id)
    .bind(keys.public_key().to_hex())
    .bind(role)
    .execute(&context.pool)
    .await
    .expect("seed Project View E2E member");
}

async fn seed_member(context: &TestContext, keys: &Keys) {
    seed_member_with_role(context, keys, "member").await;
}

fn create_goal_command(expected_revision: u64, title: &str) -> ProjectObjectCommandV3 {
    ProjectObjectCommandV3::new(
        expected_revision,
        None,
        ProjectObjectRequestV3::Create(CreateProjectObjectV3 {
            object: NewProjectViewObjectV3::Goal {
                id: Uuid::new_v4(),
                title: title.to_owned(),
                desired_outcome: format!("{title} is complete"),
                directions: Vec::new(),
                context_references: Vec::new(),
            },
        }),
    )
}

fn mutation_event(keys: &Keys, command: ProjectObjectCommandV3) -> Event {
    build_project_object_command(command)
        .expect("build Project View v3 command")
        .sign_with_keys(keys)
        .expect("sign Project View v3 command")
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

fn verify_projection_events(events: &[Event], relay_pubkey: PublicKey, community_id: Uuid) {
    let project_id = CommunityId::from_uuid(community_id);
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
        let projection_type = serde_json::from_str::<Value>(&event.content)
            .expect("parse Project View v3 projection envelope")["projection_type"]
            .as_str()
            .expect("v3 projection type")
            .to_owned();
        match projection_type.as_str() {
            "object" => {
                parse_project_object_projection(event, &relay_pubkey, project_id)
                    .expect("strict schema-v3 object projection");
            }
            "entity" => {
                parse_entity_projection(event, &relay_pubkey, project_id)
                    .expect("strict schema-v3 entity projection");
            }
            "meta" => {
                let projection = parse_meta_projection(event, &relay_pubkey)
                    .expect("strict schema-v3 metadata projection");
                assert_eq!(projection.project_id, project_id);
            }
            other => panic!("unexpected Project View v3 projection type {other}"),
        }
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
            "#t": [PROJECT_VIEW_V3_META_TAG],
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

fn create_role_with_real_cli(
    context: &TestContext,
    owner: &Keys,
    expected_project_revision: u64,
) -> Value {
    let input_path = std::env::temp_dir().join(format!(
        "buzz-project-view-v3-role-{}.json",
        Uuid::new_v4().simple()
    ));
    std::fs::write(
        &input_path,
        serde_json::to_vec(&json!({
            "name": "Module maintainer",
            "purpose": "Own one bounded implementation area",
            "responsibilities": ["Keep the module healthy"],
            "boundaries": ["Does not govern Leader roles"],
            "active": true,
            "context_references": []
        }))
        .expect("serialize schema-v3 Role fixture"),
    )
    .expect("write schema-v3 Role fixture");
    let path = input_path.to_string_lossy().into_owned();
    let expected_revision = expected_project_revision.to_string();
    let created = run_real_buzz(
        context,
        owner,
        &[
            "project-view",
            "create",
            "role",
            "--role-level",
            "member",
            "--expected-project-revision",
            &expected_revision,
            "--data",
            &path,
        ],
    );
    let _ = std::fs::remove_file(&input_path);
    created
}

fn initialize_v3_with_real_cli(context: &TestContext, owner: &Keys) -> Value {
    let owner_pubkey = owner.public_key().to_hex();
    let prepared = run_real_admin(&[
        "project-view",
        "prepare-v3",
        "--community",
        &context.host,
        "--idempotency-key",
        &format!("project-view-greenfield-e2e-{}", Uuid::new_v4()),
        "--operator-pubkey",
        &owner_pubkey,
    ]);
    let preparation_operation_id = serde_json::from_str::<Value>(&prepared)
        .expect("parse prepare-v3 receipt")["operation_id"]
        .as_str()
        .expect("prepare-v3 operation ID")
        .to_owned();
    let role_id = Uuid::new_v4();
    let input_path = std::env::temp_dir().join(format!(
        "buzz-project-view-v3-init-{}.json",
        Uuid::new_v4().simple()
    ));
    std::fs::write(
        &input_path,
        serde_json::to_vec(&json!({
            "schema_version": 3,
            "expected_project_revision": 0,
            "request": {
                "type": "initialize",
                "preparation_operation_id": preparation_operation_id,
                "profile": {
                    "name": "Protocol E2E",
                    "positioning": "One canonical schema-v3 Project View",
                    "purpose": "Prove native Buzz protocol parity",
                    "problem": "Project context is fragmented",
                    "scope": "Current Relay and first-party clients"
                },
                "goals": [{
                    "id": Uuid::new_v4(),
                    "title": "Complete Relay integration",
                    "desired_outcome": "WS and HTTP observe one current state",
                    "directions": ["Keep security gates shared"]
                }],
                "initial_roles": [{
                    "role_id": role_id,
                    "name": "Community owner",
                    "purpose": "Own initial Project governance",
                    "responsibilities": ["Administer the Project"],
                    "boundaries": ["Human governance only"],
                    "level": "admin",
                    "active": true,
                    "context_references": []
                }],
                "initial_governance_assignments": [{
                    "member_pubkey": owner_pubkey,
                    "role_id": role_id,
                    "proposal_id": Uuid::new_v4(),
                    "assignment_id": Uuid::new_v4()
                }]
            }
        }))
        .expect("serialize schema-v3 initialization fixture"),
    )
    .expect("write schema-v3 initialization fixture");
    let path = input_path.to_string_lossy().into_owned();
    let initialized = run_real_buzz(
        context,
        owner,
        &["project-view", "init-v3", "--command", &path],
    );
    let _ = std::fs::remove_file(&input_path);
    assert_eq!(initialized["accepted"], true);
    initialized
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
async fn project_view_v3_greenfield_ws_http_cli_history_and_live_revocation() {
    let context = setup().await;
    let writer = Keys::parse(
        &std::env::var("PROJECT_VIEW_E2E_OWNER_PRIVATE_KEY")
            .expect("PROJECT_VIEW_E2E_OWNER_PRIVATE_KEY must match RELAY_OWNER_PUBKEY"),
    )
    .expect("parse Project View E2E owner key");
    let agent = Keys::generate();
    let successor = Keys::generate();
    let reader = Keys::generate();
    seed_member_with_role(&context, &writer, "owner").await;
    seed_member(&context, &agent).await;
    seed_member(&context, &successor).await;
    seed_member(&context, &reader).await;
    let http = Client::new();

    let pre_initialize_info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch pre-initialize NIP-11")
        .json()
        .await
        .expect("parse pre-initialize NIP-11");
    assert!(
        pre_initialize_info["supported_extensions"]
            .as_array()
            .is_some_and(|extensions| {
                extensions
                    .iter()
                    .any(|value| value == PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION)
                    && extensions.iter().all(|value| {
                        value.as_str().is_none_or(|extension| {
                            !extension.starts_with("buzz-project-view-")
                                || extension == PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION
                        })
                    })
            }),
        "an uninitialized schema-v3 Community must advertise only bootstrap discovery: {pre_initialize_info}"
    );

    let initialized = initialize_v3_with_real_cli(&context, &writer);
    assert_eq!(initialized["accepted"], true);
    let initialized_state = sqlx::query(
        "SELECT state.project_revision, state.projection_generation, community.project_view_enabled \
         FROM project_view_state state \
         JOIN communities community ON community.id = state.community_id \
         WHERE state.community_id = $1",
    )
    .bind(context.community_id)
    .fetch_one(&context.pool)
    .await
    .expect("read initialized schema-v3 state");
    assert_eq!(initialized_state.get::<i64, _>("project_revision"), 1);
    assert_eq!(initialized_state.get::<i64, _>("projection_generation"), 1);
    assert!(!initialized_state.get::<bool, _>("project_view_enabled"));
    let initialized_info: Value = http
        .get(format!("{}/info", context.http_url))
        .send()
        .await
        .expect("fetch initialized-disabled NIP-11")
        .json()
        .await
        .expect("parse initialized-disabled NIP-11");
    assert!(
        initialized_info["supported_extensions"]
            .as_array()
            .is_none_or(|extensions| {
                extensions.iter().all(|value| {
                    value
                        .as_str()
                        .is_none_or(|extension| !extension.starts_with("buzz-project-view-"))
                })
            }),
        "initialized-disabled Project View must advertise neither bootstrap nor runtime: {initialized_info}"
    );
    run_real_admin(&["project-view", "enable", "--community", &context.host]);

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
                    .any(|value| value == PROJECT_VIEW_V3_EXTENSION)
                    && extensions.iter().all(|value| {
                        value.as_str().is_none_or(|extension| {
                            !extension.starts_with("buzz-project-view-")
                                || extension == PROJECT_VIEW_V3_EXTENSION
                        })
                    })
                    && extensions
                        .iter()
                        .all(|value| value != PROJECT_VIEW_V3_BOOTSTRAP_EXTENSION)
            }),
        "only the current Project View capability must be advertised for {}: {info}",
        context.host
    );
    let initial_view = run_real_buzz(&context, &writer, &["project-view", "get"]);
    assert_eq!(initial_view["project_view_schema_version"], 3);
    assert_eq!(initial_view["project_revision"], 1);
    assert_eq!(initial_view["projection_generation"], 1);
    assert_eq!(initial_view["objects"].as_array().map(Vec::len), Some(3));

    let sub_id = format!("project-view-{}", Uuid::new_v4());
    let mut subscriber = BuzzTestClient::connect(&context.ws_url, &reader)
        .await
        .expect("connect Project View subscriber");
    subscriber
        .subscribe(
            &sub_id,
            vec![
                Filter::new()
                    .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                    .author(relay_pubkey)
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::T),
                        PROJECT_VIEW_V3_OBJECT_TAG,
                    ),
                Filter::new()
                    .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                    .author(relay_pubkey)
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::T),
                        PROJECT_VIEW_V3_ENTITY_TAG,
                    ),
                Filter::new()
                    .kind(Kind::Custom(KIND_PROJECT_VIEW_META as u16))
                    .author(relay_pubkey)
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::T),
                        PROJECT_VIEW_V3_META_TAG,
                    ),
            ],
        )
        .await
        .expect("open Project View subscription");
    let historical = subscriber
        .collect_until_eose(&sub_id, Duration::from_secs(5))
        .await
        .expect("collect initialized Project View v3 heads");
    assert_eq!(
        historical.len(),
        6,
        "profile, goal, role, proposal, assignment, and metadata heads"
    );
    verify_projection_events(&historical, relay_pubkey, context.community_id);

    let create_revision_two = mutation_event(
        &agent,
        create_goal_command(1, "Exercise HTTP mutation path"),
    );
    let (status, response) = submit_http(&http, &context, &agent, &create_revision_two).await;
    assert_eq!(status, StatusCode::OK, "HTTP mutation failed: {response}");
    assert_eq!(response["accepted"], true);
    let revision_two = collect_live_revision(&mut subscriber, &sub_id, 2, 2).await;
    verify_projection_events(&revision_two, relay_pubkey, context.community_id);
    assert!(
        projection_has_actor(&revision_two, agent.public_key()),
        "revision two did not preserve the Agent mutation source"
    );

    let meta = current_meta(&http, &context, &writer, relay_pubkey).await;
    assert_eq!(meta["project_revision"], 2);
    assert_eq!(meta["projection_generation"], 1);
    assert_eq!(meta["schema_version"], 3);
    assert_eq!(meta["entity_counts"]["active_objects"], 4);

    let object_heads_filter = json!([{
        "kinds": [KIND_PROJECT_VIEW_OBJECT],
        "authors": [relay_pubkey.to_hex()],
        "#t": [PROJECT_VIEW_V3_OBJECT_TAG],
        "limit": 20
    }]);
    let (status, object_heads) =
        post_json(&http, &context, &writer, "/query", &object_heads_filter).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "schema-v3 object-head query: {object_heads}"
    );
    assert_eq!(object_heads.as_array().map(Vec::len), Some(3));

    let (status, entity_heads) = post_json(
        &http,
        &context,
        &writer,
        "/query",
        &json!([{
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [relay_pubkey.to_hex()],
            "#t": [PROJECT_VIEW_V3_ENTITY_TAG],
            "limit": 20
        }]),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "schema-v3 entity-head query: {entity_heads}"
    );
    assert_eq!(entity_heads.as_array().map(Vec::len), Some(3));

    let legacy_projection_tag = format!("buzz-project-view-v{}-object", 2);
    let (legacy_status, legacy_heads) = post_json(
        &http,
        &context,
        &writer,
        "/query",
        &json!([{
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [relay_pubkey.to_hex()],
            "#t": [legacy_projection_tag],
            "limit": 20
        }]),
    )
    .await;
    assert!(
        legacy_status != StatusCode::OK || legacy_heads.as_array().is_some_and(Vec::is_empty),
        "legacy Project View projection tags must be rejected or filtered: {legacy_heads}"
    );

    let (status, count) = post_json(
        &http,
        &context,
        &writer,
        "/count",
        &json!([{
            "kinds": [KIND_PROJECT_VIEW_OBJECT],
            "authors": [relay_pubkey.to_hex()],
            "#t": [PROJECT_VIEW_V3_OBJECT_TAG]
        }]),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "Project View count failed: {count}");
    assert_eq!(count["count"], 3);

    let cli_result = create_goal_with_real_cli(&context, &writer, 2);
    assert_eq!(cli_result["accepted"], true);
    assert!(cli_result["object_id"].as_str().is_some());
    let revision_three = collect_live_revision(&mut subscriber, &sub_id, 3, 2).await;
    verify_projection_events(&revision_three, relay_pubkey, context.community_id);
    assert!(
        projection_has_actor(&revision_three, writer.public_key()),
        "revision three did not return authorship to the Human CLI writer"
    );
    let meta = current_meta(&http, &context, &writer, relay_pubkey).await;
    assert_eq!(meta["project_revision"], 3);
    assert_eq!(meta["entity_counts"]["active_objects"], 5);

    sqlx::query("DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(context.community_id)
        .bind(reader.public_key().to_hex())
        .execute(&context.pool)
        .await
        .expect("revoke Project View reader");
    let revision_four_event =
        mutation_event(&agent, create_goal_command(3, "Verify live revocation"));
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
            vec![
                Filter::new()
                    .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                    .author(relay_pubkey)
                    .custom_tag(
                        SingleLetterTag::lowercase(Alphabet::T),
                        PROJECT_VIEW_V3_OBJECT_TAG,
                    ),
                Filter::new().kind(Kind::TextNote),
            ],
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
            vec![Filter::new()
                .kind(Kind::Custom(KIND_PROJECT_VIEW_OBJECT as u16))
                .author(relay_pubkey)
                .custom_tag(
                    SingleLetterTag::lowercase(Alphabet::T),
                    PROJECT_VIEW_V3_OBJECT_TAG,
                )],
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

    let stale_mutation = mutation_event(&writer, create_goal_command(2, "Stale write"));
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

    let role_created = create_role_with_real_cli(&context, &writer, 4);
    assert_eq!(role_created["accepted"], true);
    let role_id_text = role_created["object_id"]
        .as_str()
        .expect("schema-v3 Role ID")
        .to_owned();
    let agent_pubkey = agent.public_key().to_hex();
    let offered = run_real_buzz(
        &context,
        &writer,
        &[
            "roles",
            "offer",
            "--role",
            &role_id_text,
            "--member",
            &agent_pubkey,
            "--expected-project-revision",
            "5",
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
            "6",
        ],
    );
    assert_eq!(accepted["accepted"], true);

    let current = run_real_buzz(&context, &agent, &["roles", "current"]);
    assert_eq!(current["project_view_schema_version"], 3);
    assert_eq!(current["project_revision"], 7);
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
            "7",
        ],
    );
    assert!(
        rejection.contains("restricted:project_view:self_end"),
        "unexpected self-end rejection: {rejection}"
    );

    let successor_pubkey = successor.public_key().to_hex();
    let offered = run_real_buzz(
        &context,
        &writer,
        &[
            "roles",
            "offer",
            "--role",
            &role_id_text,
            "--member",
            &successor_pubkey,
            "--expected-project-revision",
            "7",
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
            "8",
        ],
    );
    assert_eq!(accepted["accepted"], true);

    let role = run_real_buzz(&context, &successor, &["roles", "get", &role_id_text]);
    assert_eq!(role["project_view_schema_version"], 3);
    assert_eq!(role["project_revision"], 9);
    assert_eq!(
        role["current_assignment"]["member_pubkey"],
        successor_pubkey
    );
    assert_eq!(role["assignment_history"].as_array().map(Vec::len), Some(2));
    assert_eq!(role["handoffs"].as_array().map(Vec::len), Some(1));

    subscriber
        .disconnect()
        .await
        .expect("disconnect subscriber");
}
