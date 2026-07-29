//! Two-phase Meeting V1 rollout/restart acceptance test.
//!
//! Run `create_rollout_fixture_before_gate_closes` with V1 creation enabled,
//! restart the same Relay/database with creation disabled, then run
//! `existing_v1_survives_closed_gate_and_v0_still_works` with the same fixture
//! path. CI owns the process restart between phases.

use std::path::PathBuf;
use std::time::Duration;

use buzz_core::kind::KIND_MEETING_STATE;
use buzz_sdk::{MeetingV1CreateParams, MeetingV1EndParams, MeetingV1HumanFloorRequestParams};
use nostr::{Event, Keys};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

const OWNER_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000011";
const HUMAN_SECRET: &str = "0000000000000000000000000000000000000000000000000000000000000012";

#[derive(Debug, Serialize, Deserialize)]
struct RolloutFixture {
    meeting_id: Uuid,
    create_event: Event,
}

fn relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_string())
}

fn relay_http_url() -> String {
    relay_url()
        .replace("wss://", "https://")
        .replace("ws://", "http://")
        .trim_end_matches('/')
        .to_string()
}

fn fixture_path() -> PathBuf {
    std::env::var_os("BUZZ_MEETING_ROLLOUT_FIXTURE")
        .map(PathBuf::from)
        .expect("BUZZ_MEETING_ROLLOUT_FIXTURE must identify the orchestrator-owned fixture")
}

fn owner_keys() -> Keys {
    Keys::parse(OWNER_SECRET).expect("valid deterministic rollout owner key")
}

fn human_keys() -> Keys {
    Keys::parse(HUMAN_SECRET).expect("valid deterministic rollout human key")
}

async fn test_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
    PgPool::connect(&database_url)
        .await
        .expect("connect to Meeting rollout E2E database")
}

async fn ensure_community(pool: &PgPool) -> Uuid {
    let host = relay_http_url()
        .split_once("://")
        .map_or_else(relay_http_url, |(_, authority)| authority.to_string());
    let proposed_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO communities (id, host) VALUES ($1, $2) \
         ON CONFLICT (lower(host)) DO NOTHING",
    )
    .bind(proposed_id)
    .bind(&host)
    .execute(pool)
    .await
    .expect("ensure rollout Relay community");
    sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("resolve rollout Relay community")
}

async fn seed_human(pool: &PgPool, community_id: Uuid, keys: &Keys, role: &str) {
    let pubkey = keys.public_key().to_bytes();
    sqlx::query(
        "INSERT INTO users (community_id, pubkey, channel_add_policy) \
         VALUES ($1, $2, 'anyone') \
         ON CONFLICT (community_id, pubkey) DO UPDATE \
         SET agent_owner_pubkey = NULL",
    )
    .bind(community_id)
    .bind(pubkey.as_slice())
    .execute(pool)
    .await
    .expect("seed rollout identity");
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(community_id)
    .bind(keys.public_key().to_hex())
    .bind(role)
    .execute(pool)
    .await
    .expect("seed rollout Relay membership");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize rollout event"))
        .send()
        .await
        .expect("submit rollout event");
    let status = response.status();
    let body = response.text().await.expect("read rollout response");
    (status, body)
}

fn assert_accepted(status: reqwest::StatusCode, body: &str) {
    let response: Value = serde_json::from_str(body).expect("parse accepted rollout response");
    assert!(
        status.is_success() && response["accepted"].as_bool() == Some(true),
        "expected accepted rollout event, got HTTP {status}: {body}"
    );
}

async fn wait_for_phase(keys: &Keys, meeting_id: Uuid, phase: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let response = reqwest::Client::new()
            .post(format!("{}/query", relay_http_url()))
            .header("X-Pubkey", keys.public_key().to_hex())
            .header("Content-Type", "application/json")
            .body(
                json!([{
                    "kinds": [KIND_MEETING_STATE],
                    "#h": [meeting_id.to_string()],
                    "limit": 50
                }])
                .to_string(),
            )
            .send()
            .await
            .expect("query rollout Meeting State");
        let status = response.status();
        let body = response.text().await.expect("read rollout State query");
        assert!(
            status.is_success(),
            "rollout State query failed with HTTP {status}: {body}"
        );
        let events: Vec<Value> =
            serde_json::from_str(&body).expect("parse rollout State query response");
        let observed = events.iter().any(|event| {
            event["content"]
                .as_str()
                .and_then(|content| serde_json::from_str::<Value>(content).ok())
                .is_some_and(|content| content["phase"] == phase)
        });
        if observed {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Meeting {meeting_id} did not reach phase {phase}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "phase 1 requires a Relay with BUZZ_MEETING_V1_CREATE_ENABLED=true"]
async fn create_rollout_fixture_before_gate_closes() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let owner = owner_keys();
    let human = human_keys();
    seed_human(&pool, community_id, &owner, "owner").await;
    seed_human(&pool, community_id, &human, "member").await;

    let meeting_id = Uuid::new_v4();
    let owner_pubkey = owner.public_key().to_hex();
    let human_pubkey = human.public_key().to_hex();
    let create_event = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Meeting V1 rollout restart",
        description: Some("persists across the create-gate restart"),
        source_channel_id: None,
        author_pubkey: &owner_pubkey,
        moderator_pubkey: &owner_pubkey,
        participant_pubkeys: &[&human_pubkey],
    })
    .expect("build rollout Meeting V1 Create")
    .sign_with_keys(&owner)
    .expect("sign rollout Meeting V1 Create");
    let (status, body) = post_event(&owner, &create_event).await;
    assert_accepted(status, &body);
    wait_for_phase(&owner, meeting_id, "moderator_idle").await;

    let fixture = RolloutFixture {
        meeting_id,
        create_event,
    };
    std::fs::write(
        fixture_path(),
        serde_json::to_vec(&fixture).expect("serialize rollout fixture"),
    )
    .expect("persist rollout fixture for phase 2");
}

#[tokio::test]
#[ignore = "phase 2 requires the phase-1 database and a restarted Relay with V1 creation disabled"]
async fn existing_v1_survives_closed_gate_and_v0_still_works() {
    let fixture: RolloutFixture = serde_json::from_slice(
        &std::fs::read(fixture_path()).expect("read phase-1 rollout fixture"),
    )
    .expect("parse phase-1 rollout fixture");
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let owner = owner_keys();
    let human = human_keys();
    seed_human(&pool, community_id, &owner, "owner").await;
    seed_human(&pool, community_id, &human, "member").await;

    // Exact Create replay remains idempotent even after operators close the
    // gate, because duplicate detection precedes the new-create check.
    let (status, body) = post_event(&owner, &fixture.create_event).await;
    assert_accepted(status, &body);
    assert!(
        body.contains("duplicate"),
        "exact Create replay must preserve duplicate success: {body}"
    );

    let floor_request =
        buzz_sdk::build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: fixture.meeting_id,
        })
        .expect("build post-restart Human Floor Request")
        .sign_with_keys(&human)
        .expect("sign post-restart Human Floor Request");
    let (status, body) = post_event(&human, &floor_request).await;
    assert_accepted(status, &body);
    wait_for_phase(&owner, fixture.meeting_id, "offered").await;

    let rejected_id = Uuid::new_v4();
    let owner_pubkey = owner.public_key().to_hex();
    let human_pubkey = human.public_key().to_hex();
    let rejected_create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: rejected_id,
        title: "must not expand rollout",
        description: None,
        source_channel_id: None,
        author_pubkey: &owner_pubkey,
        moderator_pubkey: &owner_pubkey,
        participant_pubkeys: &[&human_pubkey],
    })
    .expect("build gated Meeting V1 Create")
    .sign_with_keys(&owner)
    .expect("sign gated Meeting V1 Create");
    let (status, body) = post_event(&owner, &rejected_create).await;
    assert!(
        !status.is_success() && body.contains("Meeting V1 creation is disabled"),
        "new V1 must be rejected after closing the gate, got HTTP {status}: {body}"
    );

    let v0_id = Uuid::new_v4();
    let v0_create =
        buzz_sdk::build_meeting_create(v0_id, "V0 remains available", None, None, &[&human_pubkey])
            .expect("build V0 Create while V1 gate is closed")
            .sign_with_keys(&owner)
            .expect("sign V0 Create while V1 gate is closed");
    let (status, body) = post_event(&owner, &v0_create).await;
    assert_accepted(status, &body);

    let end = buzz_sdk::build_meeting_v1_end(MeetingV1EndParams {
        session_id: fixture.meeting_id,
        create_event_id: &fixture.create_event.id.to_hex(),
    })
    .expect("build post-restart Meeting V1 End")
    .sign_with_keys(&owner)
    .expect("sign post-restart Meeting V1 End");
    let (status, body) = post_event(&owner, &end).await;
    assert_accepted(status, &body);
    wait_for_phase(&owner, fixture.meeting_id, "ended").await;
}
