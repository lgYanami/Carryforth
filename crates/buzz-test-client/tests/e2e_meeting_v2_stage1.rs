//! End-to-end proof for the Meeting V2 stage-one vertical slice.
//!
//! Requires a disposable Relay database and a Relay started with
//! `BUZZ_MEETING_V2_CREATE_ENABLED=true`.

use buzz_core::kind::{
    KIND_MEETING_BOARD, KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_FLOOR_CLAIM,
    KIND_MEETING_FLOOR_SIGNAL, KIND_MEETING_SPEECH_INTENT, KIND_STREAM_MESSAGE,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

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

async fn test_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
    PgPool::connect(&database_url)
        .await
        .expect("connect to Meeting V2 E2E database")
}

async fn ensure_community(pool: &PgPool) -> Uuid {
    let host = relay_http_url()
        .split_once("://")
        .map_or_else(relay_http_url, |(_, authority)| authority.to_string());
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO communities (id, host) VALUES ($1, $2) \
         ON CONFLICT (lower(host)) DO NOTHING",
    )
    .bind(id)
    .bind(&host)
    .execute(pool)
    .await
    .expect("ensure Relay community");
    sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("resolve Relay community")
}

async fn seed_identity(pool: &PgPool, community_id: Uuid, keys: &Keys, role: &str) {
    let pubkey = keys.public_key().to_bytes();
    sqlx::query(
        "INSERT INTO users (community_id, pubkey, channel_add_policy) \
         VALUES ($1, $2, 'anyone') \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET deactivated_at = NULL",
    )
    .bind(community_id)
    .bind(pubkey.as_slice())
    .execute(pool)
    .await
    .expect("seed Meeting V2 identity");
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
    .expect("seed Meeting V2 Relay membership");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize Meeting V2 event"))
        .send()
        .await
        .expect("submit Meeting V2 event");
    let status = response.status();
    let body = response.text().await.expect("read Meeting V2 response");
    (status, body)
}

fn assert_accepted(status: reqwest::StatusCode, body: &str) -> Value {
    let response: Value = serde_json::from_str(body).expect("parse accepted response");
    assert!(
        status.is_success() && response["accepted"].as_bool() == Some(true),
        "expected accepted event, got HTTP {status}: {body}"
    );
    response
}

fn assert_rejected(status: reqwest::StatusCode, body: &str, reason: &str) {
    assert!(
        !status.is_success(),
        "expected rejection containing {reason:?}, got HTTP {status}: {body}"
    );
    assert!(
        body.contains(reason),
        "expected rejection containing {reason:?}, got: {body}"
    );
}

async fn query(keys: &Keys, filters: Value) -> Vec<Value> {
    let response = reqwest::Client::new()
        .post(format!("{}/query", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(filters.to_string())
        .send()
        .await
        .expect("query Meeting V2 events");
    let status = response.status();
    let body = response.text().await.expect("read Meeting V2 query");
    assert!(
        status.is_success(),
        "Meeting V2 query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse Meeting V2 query")
}

fn tag_value<'a>(event: &'a Value, name: &str) -> Option<&'a str> {
    event["tags"]
        .as_array()?
        .iter()
        .filter_map(Value::as_array)
        .find(|tag| tag.first().and_then(Value::as_str) == Some(name))?
        .get(1)?
        .as_str()
}

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_stage_one_create_read_permissions_and_mutations_fail_closed() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    let outsider = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;
    seed_identity(&pool, community_id, &outsider, "member").await;

    let smuggled_id = Uuid::new_v4();
    let smuggled = EventBuilder::new(
        Kind::Custom(KIND_MEETING_CREATE as u16),
        r##"{"format":"markdown","body":"# Smuggled"}"##,
    )
    .tags([
        Tag::parse(["h", &smuggled_id.to_string()]).expect("smuggled h"),
        Tag::parse(["name", "Smuggled moderator"]).expect("smuggled name"),
        Tag::parse(["v", "3"]).expect("smuggled v"),
        Tag::parse(["policy", buzz_sdk::MEETING_V2_POLICY]).expect("smuggled policy"),
        Tag::parse(["moderator", &participant.public_key().to_hex()]).expect("smuggled moderator"),
        Tag::parse(["p", &participant.public_key().to_hex()]).expect("smuggled participant"),
    ])
    .sign_with_keys(&host)
    .expect("sign smuggled V2 Create");
    let (status, body) = post_event(&host, &smuggled).await;
    assert_rejected(status, &body, "moderator");

    let meeting_id = Uuid::new_v4();
    let participant_hex = participant.public_key().to_hex();
    let host_hex = host.public_key().to_hex();
    let board_body =
        "# Discussion goal\nChoose a release boundary.\n\n## Agenda\n- Evidence\n- Decision";
    let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
        session_id: meeting_id,
        title: "Meeting V2 stage one",
        description: Some("current-board vertical slice"),
        source_channel_id: None,
        author_pubkey: &host_hex,
        participant_pubkeys: &[participant_hex.as_str()],
        initial_board: board_body,
    })
    .expect("build Meeting V2 Create")
    .sign_with_keys(&host)
    .expect("sign Meeting V2 Create");
    assert!(tag_value(
        &serde_json::to_value(&create).expect("Create JSON"),
        "moderator"
    )
    .is_none());
    let (status, body) = post_event(&host, &create).await;
    let response = assert_accepted(status, &body);
    let response_payload: Value = response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str(payload).ok())
        .expect("parse Meeting V2 Create response payload");
    assert_eq!(response_payload["schema_version"], 3);
    assert_eq!(
        response_payload["floor_policy_version"],
        buzz_sdk::MEETING_V2_POLICY
    );
    assert_eq!(response_payload["moderator"], host_hex);
    let board_event_id = response_payload["board_event_id"]
        .as_str()
        .expect("Create response board event ID");

    let board_filter = json!([{
        "kinds": [KIND_MEETING_BOARD],
        "#h": [meeting_id.to_string()],
        "limit": 10
    }]);
    let host_board = query(&host, board_filter.clone()).await;
    let participant_board = query(&participant, board_filter.clone()).await;
    assert_eq!(host_board.len(), 1);
    assert_eq!(participant_board, host_board);
    assert!(query(&outsider, board_filter).await.is_empty());
    let board_event: Event =
        serde_json::from_value(host_board[0].clone()).expect("decode Relay-signed board");
    board_event.verify().expect("verify Relay-signed board");
    assert_eq!(board_event.id.to_hex(), board_event_id);
    assert_eq!(tag_value(&host_board[0], "v"), Some("3"));
    assert_eq!(
        tag_value(&host_board[0], "policy"),
        Some(buzz_sdk::MEETING_V2_POLICY)
    );
    assert_eq!(
        tag_value(&host_board[0], "moderator"),
        Some(host_hex.as_str())
    );
    let board = buzz_sdk::parse_meeting_v2_board_content(&board_event.content)
        .expect("parse queried current board");
    assert_eq!(board.body, board_body);

    let forged_board = EventBuilder::new(
        Kind::Custom(KIND_MEETING_BOARD as u16),
        board_event.content.clone(),
    )
    .tags(board_event.tags.clone())
    .sign_with_keys(&host)
    .expect("sign forged current board");
    let (status, body) = post_event(&host, &forged_board).await;
    assert_rejected(status, &body, "relay-only kind");

    let floor_claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 1)
        .expect("build V0 floor Claim against V2")
        .sign_with_keys(&participant)
        .expect("sign V0 floor Claim against V2");
    let (status, body) = post_event(&participant, &floor_claim).await;
    assert_rejected(status, &body, "only available for Meeting V0");

    let floor_signal = buzz_sdk::build_meeting_floor_ready(meeting_id, 1, "stage-one-proof")
        .expect("build V0 floor Signal against V2")
        .sign_with_keys(&participant)
        .expect("sign V0 floor Signal against V2");
    let (status, body) = post_event(&participant, &floor_signal).await;
    assert_rejected(status, &body, "only available for Meeting V0");

    let v1_intent =
        buzz_sdk::build_meeting_v1_intent_submit(buzz_sdk::MeetingV1IntentSubmitParams {
            session_id: meeting_id,
            basis_speech_revision: 0,
            addressed_to: None,
            summary: "V1 command must not enter a V2 session",
        })
        .expect("build V1 Intent against V2")
        .sign_with_keys(&participant)
        .expect("sign V1 Intent against V2");
    let (status, body) = post_event(&participant, &v1_intent).await;
    assert_rejected(status, &body, "non-V1 session");

    let speech = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), "too early")
        .tags([Tag::parse(["h", &meeting_id.to_string()]).expect("speech h")])
        .sign_with_keys(&participant)
        .expect("sign speech against V2");
    let (status, body) = post_event(&participant, &speech).await;
    assert_rejected(status, &body, "V2 speech is unavailable during stage one");

    let end = EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), "")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("End h"),
            Tag::parse(["v", "3"]).expect("End v"),
            Tag::parse(["policy", buzz_sdk::MEETING_V2_POLICY]).expect("End policy"),
            Tag::parse(["e", &create.id.to_hex()]).expect("End Create reference"),
            Tag::parse(["reason", "manual"]).expect("End reason"),
        ])
        .sign_with_keys(&host)
        .expect("sign End against V2");
    let (status, body) = post_event(&host, &end).await;
    assert_rejected(status, &body, "V2 End is unavailable during stage one");

    let projection: (i32, String, Vec<u8>, Vec<u8>, String, i64) = sqlx::query_as(
        "SELECT s.schema_version, s.floor_policy_version, s.host_pubkey, \
                s.moderator_pubkey, b.runtime_phase, b.control_epoch \
         FROM meeting_sessions s \
         JOIN meeting_v2_bootstrap_state b \
           ON b.community_id = s.community_id AND b.session_id = s.session_id \
         WHERE s.community_id = $1 AND s.session_id = $2 AND s.status = 'active'",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read active Meeting V2 projection");
    assert_eq!(projection.0, 3);
    assert_eq!(projection.1, buzz_sdk::MEETING_V2_POLICY);
    assert_eq!(projection.2, host.public_key().to_bytes());
    assert_eq!(projection.3, host.public_key().to_bytes());
    assert_eq!(
        (projection.4.as_str(), projection.5),
        ("bootstrap_locked", 1)
    );
    let persisted_counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
             count(*) FILTER (WHERE e.kind = $3), \
             count(*) FILTER (WHERE e.kind = $4), \
             count(*) FILTER (WHERE e.kind = $5), \
             count(*) FILTER (WHERE e.kind = $6), \
             count(*) FILTER (WHERE e.kind = $7) \
         FROM events e \
         WHERE e.community_id = $1 AND e.channel_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(KIND_MEETING_BOARD as i32)
    .bind(KIND_MEETING_END as i32)
    .bind(KIND_STREAM_MESSAGE as i32)
    .bind(KIND_MEETING_FLOOR_CLAIM as i32)
    .bind(KIND_MEETING_FLOOR_SIGNAL as i32)
    .fetch_one(&pool)
    .await
    .expect("count accepted and rejected Meeting V2 events");
    assert_eq!(persisted_counts, (1, 0, 0, 0, 0));
    let v1_intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE community_id = $1 AND channel_id = $2 AND kind = $3",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(KIND_MEETING_SPEECH_INTENT as i32)
    .fetch_one(&pool)
    .await
    .expect("count rejected V1 intents");
    assert_eq!(v1_intent_count, 0);
    let outbox_counts: (i64, i64) = sqlx::query_as(
        "SELECT \
             count(*) FILTER (WHERE event_id = $3), \
             count(*) FILTER (WHERE event_id = $4) \
         FROM meeting_event_outbox \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(create.id.as_bytes().as_slice())
    .bind(board_event.id.as_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .expect("count Meeting V2 outbox rows");
    assert_eq!(outbox_counts, (1, 0));
}
