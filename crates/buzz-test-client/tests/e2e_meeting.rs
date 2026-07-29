//! End-to-end tests for versioned Meeting lifecycle foundations.
//!
//! These tests require a running relay, Postgres, and Redis, so they remain
//! ignored in the infrastructure-free unit-test gate.

use buzz_core::kind::{
    KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_SPEECH_INTENT, KIND_MEETING_STATE,
    KIND_NIP29_EDIT_METADATA, KIND_NIP29_GROUP_MEMBERS, KIND_NIP29_GROUP_METADATA,
    KIND_NIP29_LEAVE_REQUEST, KIND_NIP29_PUT_USER,
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

    let outbox_kinds: Vec<i32> = sqlx::query_scalar(
        "SELECT e.kind \
         FROM meeting_event_outbox o \
         JOIN events e ON e.community_id = o.community_id AND e.id = o.event_id \
         WHERE o.community_id = $1 AND o.session_id = $2 \
         ORDER BY o.sequence",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_all(&pool)
    .await
    .expect("read V0 lifecycle outbox order");
    assert_eq!(
        outbox_kinds,
        vec![
            KIND_MEETING_CREATE as i32,
            KIND_MEETING_STATE as i32,
            KIND_MEETING_END as i32,
            KIND_MEETING_STATE as i32,
        ],
        "V0 command and State delivery must share one causal outbox"
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

#[tokio::test]
#[ignore = "requires a running relay with BUZZ_MEETING_V1_CREATE_ENABLED=true, Postgres, and Redis"]
async fn meeting_v1_stage_one_create_query_isolate_and_end() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let owner = Keys::generate();
    let moderator = Keys::generate();
    let agent = Keys::generate();
    let community_admin = Keys::generate();
    let outsider = Keys::generate();
    seed_identity(&pool, community_id, &owner, "owner", None).await;
    seed_identity(&pool, community_id, &moderator, "member", None).await;
    seed_identity(&pool, community_id, &agent, "member", Some(&owner)).await;
    seed_identity(&pool, community_id, &community_admin, "admin", None).await;
    seed_identity(&pool, community_id, &outsider, "member", None).await;

    let meeting_id = Uuid::new_v4();
    let participant_pubkeys = [moderator.public_key().to_hex(), agent.public_key().to_hex()];
    let participant_refs: Vec<&str> = participant_pubkeys.iter().map(String::as_str).collect();
    let create = buzz_sdk::build_meeting_v1_create(buzz_sdk::MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Meeting V1 Stage One",
        description: Some("moderated baton persistence proof"),
        source_channel_id: None,
        author_pubkey: &owner.public_key().to_hex(),
        moderator_pubkey: &moderator.public_key().to_hex(),
        participant_pubkeys: &participant_refs,
    })
    .expect("build Meeting V1 Create")
    .sign_with_keys(&owner)
    .expect("sign Meeting V1 Create");
    let (status, body) = post_event(&owner, &create).await;
    assert_accepted(status, &body);

    let control_filter = json!([{
        "kinds": [KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_STATE],
        "#h": [meeting_id.to_string()],
        "limit": 20
    }]);
    let mut control = Vec::new();
    for _ in 0..30 {
        control = query(&moderator, control_filter.clone()).await;
        if control
            .iter()
            .any(|event| event["kind"].as_u64() == Some(KIND_MEETING_STATE as u64))
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    let create_row = control
        .iter()
        .find(|event| event["kind"].as_u64() == Some(KIND_MEETING_CREATE as u64))
        .expect("V1 Create reaches the shared private log");
    assert_eq!(tag_values(create_row, "v")[0][1], "2");
    assert_eq!(
        tag_values(create_row, "policy")[0][1],
        buzz_sdk::MEETING_V1_POLICY
    );
    assert_eq!(
        tag_values(create_row, "moderator")[0][1],
        moderator.public_key().to_hex()
    );

    let initial_state = control
        .iter()
        .find(|event| {
            event["kind"].as_u64() == Some(KIND_MEETING_STATE as u64)
                && tag_values(event, "state-revision")
                    .first()
                    .and_then(|tag| tag.get(1))
                    .and_then(Value::as_str)
                    == Some("1")
        })
        .expect("initial V1 Relay State reaches the shared private log");
    assert_eq!(tag_values(initial_state, "v")[0][1], "2");
    assert_eq!(
        tag_values(initial_state, "policy")[0][1],
        buzz_sdk::MEETING_V1_POLICY
    );
    assert_eq!(tag_values(initial_state, "phase")[0][1], "moderator_idle");
    assert_eq!(tag_values(initial_state, "floor-revision")[0][1], "1");
    assert_eq!(tag_values(initial_state, "intent-revision")[0][1], "0");
    assert_eq!(tag_values(initial_state, "speech-revision")[0][1], "0");
    assert_eq!(
        tag_values(initial_state, "moderator")[0][1],
        moderator.public_key().to_hex()
    );

    let state_content: Value = serde_json::from_str(
        initial_state["content"]
            .as_str()
            .expect("V1 State content string"),
    )
    .expect("V1 State JSON");
    assert_eq!(state_content["phase"], "moderator_idle");
    assert_eq!(state_content["state_revision"], 1);
    assert_eq!(state_content["floor_revision"], 1);
    assert_eq!(state_content["intent_revision"], 0);
    assert_eq!(state_content["speech_revision"], 0);
    assert_eq!(state_content["control_epoch"], 1);
    assert_eq!(state_content["decision_epoch"], 0);
    assert_eq!(state_content["handoff_depth"], 0);
    assert_eq!(state_content["forced_return_to_moderator"], false);
    assert_eq!(
        state_content["baton_config"]["moderator_decision_ms"],
        180_000
    );
    assert_eq!(
        state_content["baton_config"]["grant_hard_deadline_ms"],
        300_000
    );
    assert_eq!(state_content["baton_config"]["max_handoff_depth"], 5);
    assert_eq!(state_content["baton_config"]["max_open_handoffs"], 32);
    assert_eq!(
        state_content["transition"]["primary_type"],
        "meeting_created"
    );

    let session = sqlx::query(
        "SELECT schema_version, floor_policy_version, moderator_pubkey, status \
         FROM meeting_sessions WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("V1 session projection");
    assert_eq!(session.get::<i32, _>("schema_version"), 2);
    assert_eq!(
        session.get::<String, _>("floor_policy_version"),
        buzz_sdk::MEETING_V1_POLICY
    );
    assert_eq!(
        session.get::<Vec<u8>, _>("moderator_pubkey"),
        moderator.public_key().to_bytes()
    );
    assert_eq!(session.get::<String, _>("status"), "active");

    let participants: Vec<(Vec<u8>, String, String)> = sqlx::query_as(
        "SELECT pubkey, participant_type, channel_role \
         FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2 \
         ORDER BY pubkey",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_all(&pool)
    .await
    .expect("frozen V1 participant projection");
    assert_eq!(participants.len(), 3);
    assert!(participants.iter().any(|(pubkey, participant_type, role)| {
        pubkey == owner.public_key().to_bytes().as_slice()
            && participant_type == "human"
            && role == "owner"
    }));
    assert!(participants.iter().any(|(pubkey, participant_type, role)| {
        pubkey == moderator.public_key().to_bytes().as_slice()
            && participant_type == "human"
            && role == "member"
    }));
    assert!(participants.iter().any(|(pubkey, participant_type, role)| {
        pubkey == agent.public_key().to_bytes().as_slice()
            && participant_type == "agent"
            && role == "bot"
    }));

    let v0_rounds: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM meeting_rounds \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("count V0 rounds for V1 session");
    assert_eq!(v0_rounds, 0, "V1 Create must not initialize the V0 floor");

    let claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 1)
        .expect("build cross-policy V0 Claim")
        .sign_with_keys(&agent)
        .expect("sign cross-policy V0 Claim");
    let (status, body) = post_event(&agent, &claim).await;
    assert!(
        !status.is_success(),
        "V0 Claim must fail closed for V1 session, got HTTP {status}: {body}"
    );

    let v0_shaped_speech = buzz_sdk::build_meeting_speech(
        meeting_id,
        1,
        &"33".repeat(32),
        "must not reach the V0 speech validator",
        &[],
    )
    .expect("build cross-policy V0-shaped speech")
    .sign_with_keys(&agent)
    .expect("sign cross-policy V0-shaped speech");
    let (status, body) = post_event(&agent, &v0_shaped_speech).await;
    assert!(
        !status.is_success() && body.contains("Meeting V1 speech is not available in stage one"),
        "kind 9 must route by persisted V1 policy before V0 validation, got HTTP {status}: {body}"
    );

    let unavailable_v1_command =
        EventBuilder::new(Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16), "")
            .tags([
                Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
                Tag::parse(["v", "2"]).expect("v tag"),
            ])
            .sign_with_keys(&agent)
            .expect("sign stage-one V1 command");
    let (status, body) = post_event(&agent, &unavailable_v1_command).await;
    assert!(
        !status.is_success()
            && body.contains("Meeting V1 baton commands are not available in stage one"),
        "participant must receive the explicit stage-one command boundary, got HTTP {status}: {body}"
    );

    let outsider_probe = EventBuilder::new(Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16), "")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
            Tag::parse(["v", "2"]).expect("v tag"),
        ])
        .sign_with_keys(&outsider)
        .expect("sign outsider V1 command probe");
    let (status, body) = post_event(&outsider, &outsider_probe).await;
    assert!(
        !status.is_success()
            && body.contains("not a participant in this meeting")
            && !body.contains("baton commands are not available"),
        "non-participant must fail before persisted policy is disclosed, got HTTP {status}: {body}"
    );

    assert!(
        query(&outsider, control_filter.clone()).await.is_empty(),
        "outsider must not read V1 lifecycle or Baton State"
    );

    let wrong_end = buzz_sdk::build_meeting_end(meeting_id, &create.id.to_hex())
        .expect("build V0-shaped End")
        .sign_with_keys(&owner)
        .expect("sign V0-shaped End");
    let (status, body) = post_event(&owner, &wrong_end).await;
    assert!(
        !status.is_success(),
        "V0-shaped End must fail closed for V1 session, got HTTP {status}: {body}"
    );

    let outsider_v0_end = buzz_sdk::build_meeting_end(meeting_id, &create.id.to_hex())
        .expect("build outsider V0-shaped End probe")
        .sign_with_keys(&outsider)
        .expect("sign outsider V0-shaped End probe");
    let (status, body) = post_event(&outsider, &outsider_v0_end).await;
    assert!(
        !status.is_success()
            && body.contains("not authorized for this meeting")
            && !body.contains("Meeting V1 End"),
        "outsider V0-shaped End must fail before policy disclosure, got HTTP {status}: {body}"
    );

    let outsider_v1_end = buzz_sdk::build_meeting_v1_end(buzz_sdk::MeetingV1EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
    })
    .expect("build outsider V1 End probe")
    .sign_with_keys(&outsider)
    .expect("sign outsider V1 End probe");
    let (status, body) = post_event(&outsider, &outsider_v1_end).await;
    assert!(
        !status.is_success()
            && body.contains("not authorized for this meeting")
            && !body.contains("Meeting V1 End"),
        "outsider V1-shaped End must fail before policy disclosure, got HTTP {status}: {body}"
    );

    let end = buzz_sdk::build_meeting_v1_end(buzz_sdk::MeetingV1EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
    })
    .expect("build Meeting V1 End")
    .sign_with_keys(&community_admin)
    .expect("sign Meeting V1 End");
    let (status, body) = post_event(&community_admin, &end).await;
    assert_accepted(status, &body);

    let mut terminal_state = None;
    for _ in 0..30 {
        let events = query(&moderator, control_filter.clone()).await;
        terminal_state = events.into_iter().find(|event| {
            event["kind"].as_u64() == Some(KIND_MEETING_STATE as u64)
                && tag_values(event, "phase")
                    .first()
                    .and_then(|tag| tag.get(1))
                    .and_then(Value::as_str)
                    == Some("ended")
        });
        if terminal_state.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let terminal_state = terminal_state.expect("terminal V1 State reaches the shared log");
    assert_eq!(tag_values(&terminal_state, "state-revision")[0][1], "2");
    let terminal_content: Value = serde_json::from_str(
        terminal_state["content"]
            .as_str()
            .expect("terminal V1 State content"),
    )
    .expect("terminal V1 State JSON");
    assert_eq!(terminal_content["phase"], "ended");
    assert_eq!(
        terminal_content["transition"]["primary_type"],
        "meeting_ended"
    );

    let terminal_projection: (String, String, i64) = sqlx::query_as(
        "SELECT ms.status, bs.phase, \
                (SELECT count(*) FROM meeting_rounds mr \
                 WHERE mr.community_id = ms.community_id \
                   AND mr.session_id = ms.session_id) \
         FROM meeting_sessions ms \
         JOIN meeting_baton_state bs \
           ON bs.community_id = ms.community_id AND bs.session_id = ms.session_id \
         WHERE ms.community_id = $1 AND ms.session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("terminal V1 projection");
    assert_eq!(
        terminal_projection,
        ("ended".to_string(), "ended".to_string(), 0)
    );

    let unknown_identity = Keys::generate();
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, 'member')",
    )
    .bind(community_id)
    .bind(unknown_identity.public_key().to_hex())
    .execute(&pool)
    .await
    .expect("seed relay membership without a users identity");
    let rejected_meeting_id = Uuid::new_v4();
    let rejected_create = buzz_sdk::build_meeting_v1_create(buzz_sdk::MeetingV1CreateParams {
        session_id: rejected_meeting_id,
        title: "Identity must be explicit",
        description: None,
        source_channel_id: None,
        author_pubkey: &owner.public_key().to_hex(),
        moderator_pubkey: &owner.public_key().to_hex(),
        participant_pubkeys: &[&unknown_identity.public_key().to_hex()],
    })
    .expect("build missing-identity V1 Create")
    .sign_with_keys(&owner)
    .expect("sign missing-identity V1 Create");
    let (status, body) = post_event(&owner, &rejected_create).await;
    assert!(
        !status.is_success(),
        "missing participant identity must fail closed, got HTTP {status}: {body}"
    );
    let residue: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
            (SELECT count(*) FROM events \
             WHERE community_id = $1 AND id = $2), \
            (SELECT count(*) FROM channels \
             WHERE community_id = $1 AND id = $3), \
            (SELECT count(*) FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $3), \
            (SELECT count(*) FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $3)",
    )
    .bind(community_id)
    .bind(rejected_create.id.as_bytes().as_slice())
    .bind(rejected_meeting_id)
    .fetch_one(&pool)
    .await
    .expect("inspect failed V1 Create residue");
    assert_eq!(residue, (0, 0, 0, 0));
}
