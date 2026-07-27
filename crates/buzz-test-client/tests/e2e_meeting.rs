//! End-to-end tests for the Meeting V0 lifecycle foundation.
//!
//! These tests require a running relay, Postgres, and Redis, so they remain
//! ignored in the infrastructure-free unit-test gate.

use buzz_core::kind::{
    KIND_MEETING_CREATE, KIND_MEETING_END, KIND_NIP29_EDIT_METADATA, KIND_NIP29_GROUP_MEMBERS,
    KIND_NIP29_GROUP_METADATA, KIND_NIP29_LEAVE_REQUEST, KIND_NIP29_PUT_USER,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use sqlx::{PgPool, Row};
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
        .expect("connect to Meeting V0 E2E database")
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
    .expect("ensure relay community");
    sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("resolve relay community")
}

async fn seed_identity(
    pool: &PgPool,
    community_id: Uuid,
    keys: &Keys,
    relay_role: &str,
    agent_owner: Option<&Keys>,
) {
    let pubkey = keys.public_key().to_bytes();
    let owner_pubkey = agent_owner.map(|owner| owner.public_key().to_bytes());
    sqlx::query(
        "INSERT INTO users \
             (community_id, pubkey, agent_owner_pubkey, channel_add_policy) \
         VALUES ($1, $2, $3, 'anyone') \
         ON CONFLICT (community_id, pubkey) DO UPDATE \
         SET agent_owner_pubkey = EXCLUDED.agent_owner_pubkey",
    )
    .bind(community_id)
    .bind(pubkey.as_slice())
    .bind(owner_pubkey.as_ref().map(<[u8; 32]>::as_slice))
    .execute(pool)
    .await
    .expect("seed Meeting V0 identity");

    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(community_id)
    .bind(keys.public_key().to_hex())
    .bind(relay_role)
    .execute(pool)
    .await
    .expect("seed Meeting V0 relay membership");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize event"))
        .send()
        .await
        .expect("submit event");
    let status = response.status();
    let body = response.text().await.expect("read event response");
    (status, body)
}

fn assert_accepted(status: reqwest::StatusCode, body: &str) {
    let response: Value = serde_json::from_str(body).expect("parse accepted response");
    assert!(
        status.is_success() && response["accepted"].as_bool() == Some(true),
        "expected accepted event, got HTTP {status}: {body}"
    );
}

fn assert_rejected(status: reqwest::StatusCode, body: &str, reason: &str) {
    assert!(
        !status.is_success(),
        "expected rejected event for {reason}, got HTTP {status}: {body}"
    );
    assert!(
        body.contains(reason),
        "expected rejection to contain {reason:?}, got: {body}"
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
        .expect("query meeting events");
    let status = response.status();
    let body = response.text().await.expect("read query response");
    assert!(
        status.is_success(),
        "meeting query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse meeting query response")
}

fn tag_values<'a>(event: &'a Value, name: &str) -> Vec<&'a [Value]> {
    event["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .filter(|tag| tag.first().and_then(Value::as_str) == Some(name))
        .map(Vec::as_slice)
        .collect()
}

#[tokio::test]
#[ignore = "requires a running relay, Postgres, and Redis"]
async fn meeting_stage_one_lifecycle_is_atomic_private_frozen_and_terminal() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let human = Keys::generate();
    let agent = Keys::generate();
    let outsider = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner", None).await;
    seed_identity(&pool, community_id, &human, "member", None).await;
    seed_identity(&pool, community_id, &agent, "member", Some(&host)).await;
    seed_identity(&pool, community_id, &outsider, "member", None).await;

    let meeting_id = Uuid::new_v4();
    let other_participants = [human.public_key().to_hex(), agent.public_key().to_hex()];
    let participant_refs: Vec<&str> = other_participants.iter().map(String::as_str).collect();
    let create = buzz_sdk::build_meeting_create(
        meeting_id,
        "Stage One E2E",
        Some("private lifecycle proof"),
        None,
        &participant_refs,
    )
    .expect("build meeting create")
    .sign_with_keys(&host)
    .expect("sign meeting create");
    let (status, body) = post_event(&host, &create).await;
    assert_accepted(status, &body);

    let metadata_filter = json!([{
        "kinds": [KIND_NIP29_GROUP_METADATA],
        "#d": [meeting_id.to_string()],
        "limit": 10
    }]);
    for participant in [&host, &human, &agent] {
        let metadata = query(participant, metadata_filter.clone()).await;
        assert_eq!(metadata.len(), 1, "participant must discover the meeting");
        assert!(tag_values(&metadata[0], "room_kind")
            .iter()
            .any(|tag| tag.get(1).and_then(Value::as_str) == Some("meeting")));
        assert!(tag_values(&metadata[0], "archived").is_empty());
    }
    assert!(
        query(&outsider, metadata_filter.clone()).await.is_empty(),
        "non-participant must not discover private meeting metadata"
    );

    let roster = query(
        &human,
        json!([{
            "kinds": [KIND_NIP29_GROUP_MEMBERS],
            "#d": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert_eq!(roster.len(), 1);
    let roles: std::collections::HashMap<_, _> = tag_values(&roster[0], "p")
        .into_iter()
        .map(|tag| {
            (
                tag.get(1)
                    .and_then(Value::as_str)
                    .expect("member pubkey")
                    .to_string(),
                tag.get(3)
                    .and_then(Value::as_str)
                    .expect("member role")
                    .to_string(),
            )
        })
        .collect();
    assert_eq!(
        roles.get(&host.public_key().to_hex()),
        Some(&"owner".into())
    );
    assert_eq!(
        roles.get(&human.public_key().to_hex()),
        Some(&"member".into())
    );
    assert_eq!(roles.get(&agent.public_key().to_hex()), Some(&"bot".into()));

    let channel = sqlx::query(
        "SELECT channel_type::text AS channel_type, visibility::text AS visibility, \
                room_kind, archived_at \
         FROM channels WHERE community_id = $1 AND id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read meeting channel");
    assert_eq!(channel.get::<String, _>("channel_type"), "stream");
    assert_eq!(channel.get::<String, _>("visibility"), "private");
    assert_eq!(channel.get::<String, _>("room_kind"), "meeting");
    assert!(channel
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("archived_at")
        .is_none());

    let put_outsider = EventBuilder::new(Kind::Custom(KIND_NIP29_PUT_USER as u16), "")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
            Tag::parse(["p", &outsider.public_key().to_hex()]).expect("p tag"),
            Tag::parse(["role", "member"]).expect("role tag"),
        ])
        .sign_with_keys(&host)
        .expect("sign generic add");
    let (status, body) = post_event(&host, &put_outsider).await;
    assert_rejected(status, &body, "meeting participant roster is frozen");

    let leave = EventBuilder::new(Kind::Custom(KIND_NIP29_LEAVE_REQUEST as u16), "")
        .tags([Tag::parse(["h", &meeting_id.to_string()]).expect("h tag")])
        .sign_with_keys(&human)
        .expect("sign generic leave");
    let (status, body) = post_event(&human, &leave).await;
    assert_rejected(status, &body, "meeting participant roster is frozen");

    let member_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channel_members \
         WHERE community_id = $1 AND channel_id = $2 AND removed_at IS NULL",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("count frozen roster");
    assert_eq!(member_count, 3);

    let end = buzz_sdk::build_meeting_end(meeting_id, &create.id.to_hex())
        .expect("build meeting end")
        .sign_with_keys(&host)
        .expect("sign meeting end");
    let (status, body) = post_event(&host, &end).await;
    assert_accepted(status, &body);

    let ended_metadata = query(&agent, metadata_filter.clone()).await;
    assert_eq!(ended_metadata.len(), 1);
    assert!(
        tag_values(&ended_metadata[0], "archived")
            .iter()
            .any(|tag| tag.get(1).and_then(Value::as_str) == Some("true")),
        "original participants must discover the read-only archive"
    );

    let late_message = EventBuilder::new(Kind::Custom(9), "too late")
        .tags([Tag::parse(["h", &meeting_id.to_string()]).expect("h tag")])
        .sign_with_keys(&host)
        .expect("sign late message");
    let (status, body) = post_event(&host, &late_message).await;
    assert_rejected(status, &body, "channel is archived");

    let unarchive = EventBuilder::new(Kind::Custom(KIND_NIP29_EDIT_METADATA as u16), "")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
            Tag::parse(["archived", "false"]).expect("archived tag"),
        ])
        .sign_with_keys(&host)
        .expect("sign generic unarchive");
    let (status, body) = post_event(&host, &unarchive).await;
    assert_rejected(
        status,
        &body,
        "meeting archive state can only change through kind 42101",
    );

    // A second, distinct valid End command is acknowledged idempotently but
    // rolled back, so only the canonical first End remains in history.
    let distinct_end = buzz_sdk::build_meeting_end(meeting_id, &create.id.to_hex())
        .expect("build duplicate meeting end")
        .custom_created_at(Timestamp::from(Timestamp::now().as_secs() + 1))
        .sign_with_keys(&host)
        .expect("sign duplicate meeting end");
    assert_ne!(distinct_end.id, end.id);
    let (status, body) = post_event(&host, &distinct_end).await;
    assert_accepted(status, &body);
    let response: Value = serde_json::from_str(&body).expect("parse duplicate End response");
    let response_payload: Value = response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str(payload).ok())
        .expect("parse duplicate End payload");
    assert!(
        response_payload["already_ended"].as_bool() == Some(true),
        "duplicate End response must be explicitly idempotent: {body}"
    );

    let lifecycle = query(
        &human,
        json!([{
            "kinds": [KIND_MEETING_CREATE, KIND_MEETING_END],
            "#h": [meeting_id.to_string()],
            "limit": 20
        }]),
    )
    .await;
    assert_eq!(
        lifecycle
            .iter()
            .filter(|event| event["kind"].as_u64() == Some(KIND_MEETING_CREATE as u64))
            .count(),
        1
    );
    assert_eq!(
        lifecycle
            .iter()
            .filter(|event| event["kind"].as_u64() == Some(KIND_MEETING_END as u64))
            .count(),
        1
    );
    assert!(
        query(
            &outsider,
            json!([{
                "kinds": [KIND_MEETING_CREATE, KIND_MEETING_END],
                "#h": [meeting_id.to_string()],
                "limit": 20
            }])
        )
        .await
        .is_empty(),
        "non-participant must not read lifecycle history"
    );

    let invalid_meeting_id = Uuid::new_v4();
    let unknown = Keys::generate();
    let invalid_create = buzz_sdk::build_meeting_create(
        invalid_meeting_id,
        "Must Roll Back",
        None,
        None,
        &[&unknown.public_key().to_hex()],
    )
    .expect("build invalid meeting create")
    .sign_with_keys(&host)
    .expect("sign invalid meeting create");
    let (status, body) = post_event(&host, &invalid_create).await;
    assert_rejected(status, &body, "is not a member of this community");

    let residue: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM events \
             WHERE community_id = $1 AND id = $2), \
            (SELECT count(*) FROM channels \
             WHERE community_id = $1 AND id = $3), \
            (SELECT count(*) FROM channel_members \
             WHERE community_id = $1 AND channel_id = $3), \
            (SELECT count(*) FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $3)",
    )
    .bind(community_id)
    .bind(invalid_create.id.as_bytes().as_slice())
    .bind(invalid_meeting_id)
    .fetch_one(&pool)
    .await
    .expect("inspect failed-create residue");
    assert_eq!(residue, (0, 0, 0, 0));
}
