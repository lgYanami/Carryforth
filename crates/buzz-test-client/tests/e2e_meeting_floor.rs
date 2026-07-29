//! End-to-end proof for the Meeting V0 speech-floor protocol.
//!
//! Requires a running relay, Postgres, and Redis. Expiry coverage advances the
//! persisted test lease after observing the Grant, so the test remains fast
//! with the five-minute production lease.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use buzz_core::kind::{
    KIND_MEETING_FLOOR_CLAIM, KIND_MEETING_FLOOR_SIGNAL, KIND_MEETING_ROUND_STATE,
    KIND_NIP29_GROUP_METADATA, KIND_STREAM_MESSAGE_V2,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
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
        .expect("connect to Meeting V0 floor E2E database")
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

async fn expire_test_grant(pool: &PgPool, community_id: Uuid, meeting_id: Uuid, round: u64) {
    let round = i64::try_from(round).expect("test round fits i64");
    let result = sqlx::query(
        "UPDATE meeting_rounds \
         SET lease_expires_at = clock_timestamp() - INTERVAL '1 second' \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND phase = 'granted'",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(round)
    .execute(pool)
    .await
    .expect("advance granted test lease to expiry");
    assert_eq!(
        result.rows_affected(),
        1,
        "exactly one granted test round must be advanced"
    );
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
    .expect("seed Meeting V0 floor identity");
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
    .expect("seed Meeting V0 floor relay membership");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize event"))
        .send()
        .await
        .expect("submit Meeting V0 floor event");
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
        .expect("query Meeting V0 floor events");
    let status = response.status();
    let body = response.text().await.expect("read query response");
    assert!(
        status.is_success(),
        "meeting floor query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse meeting floor query")
}

fn tag_value<'a>(event: &'a Value, name: &str) -> Option<&'a str> {
    event["tags"]
        .as_array()?
        .iter()
        .filter_map(Value::as_array)
        .find(|tag| tag.first().and_then(Value::as_str) == Some(name))
        .and_then(|tag| tag.get(1))
        .and_then(Value::as_str)
}

fn floor_revision(event: &Value) -> u64 {
    tag_value(event, "floor-revision")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

async fn floor_events(keys: &Keys, meeting_id: Uuid) -> Vec<Value> {
    query(
        keys,
        json!([{
            "kinds": [
                KIND_MEETING_FLOOR_CLAIM,
                KIND_MEETING_ROUND_STATE,
                KIND_MEETING_FLOOR_SIGNAL
            ],
            "#h": [meeting_id.to_string()],
            "limit": 500
        }]),
    )
    .await
}

async fn wait_for_state(
    keys: &Keys,
    meeting_id: Uuid,
    round: u64,
    phase: &str,
    timeout: Duration,
) -> Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(state) = floor_events(keys, meeting_id)
            .await
            .into_iter()
            .filter(|event| event["kind"].as_u64() == Some(KIND_MEETING_ROUND_STATE as u64))
            .filter(|event| {
                tag_value(event, "meeting-round").and_then(|value| value.parse::<u64>().ok())
                    == Some(round)
                    && tag_value(event, "phase") == Some(phase)
            })
            .max_by_key(floor_revision)
        {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for meeting {meeting_id} round {round} phase {phase}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn content_json(event: &Value) -> Value {
    event["content"]
        .as_str()
        .and_then(|content| serde_json::from_str(content).ok())
        .unwrap_or_else(|| json!({}))
}

#[test]
fn shared_floor_v1_fixture_is_versioned_and_complete() {
    let fixture: Value = serde_json::from_str(include_str!("../fixtures/meeting_v0_floor_v1.json"))
        .expect("parse shared Meeting V0 floor fixture");
    assert_eq!(fixture["fixture_version"].as_u64(), Some(1));
    assert_eq!(fixture["protocol"]["meeting_version"].as_str(), Some("1"));
    assert_eq!(
        fixture["protocol"]["floor_policy"].as_str(),
        Some("uniform-v0")
    );
    assert_eq!(
        fixture["protocol"]["kinds"],
        json!({
            "speech": 9,
            "claim": 42102,
            "round_state": 42103,
            "floor_signal": 42104
        })
    );

    let scenarios = fixture["scenarios"].as_array().expect("fixture scenarios");
    let names: BTreeSet<_> = scenarios
        .iter()
        .filter_map(|scenario| scenario["name"].as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["expired", "lost", "normal", "reconnect"])
    );
    for scenario in scenarios {
        let delivery = scenario["delivery"].as_array().expect("scenario delivery");
        assert!(!delivery.is_empty(), "scenario must contain control events");
        for event in delivery {
            assert_eq!(
                event["event_id"].as_str().map(str::len),
                Some(64),
                "fixture event IDs remain Nostr-shaped"
            );
            assert_eq!(event["policy"].as_str(), Some("uniform-v0"));
            assert!(
                matches!(
                    event["phase"].as_str(),
                    Some("open" | "claiming" | "granted" | "closed")
                ),
                "fixture phase must be part of the V0 state machine"
            );
        }
    }
}

#[tokio::test]
#[ignore = "requires a running relay, Postgres, and Redis"]
async fn agent_ready_pass_settles_early_and_yield_immediately_opens_the_next_round() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let agent_a = Keys::generate();
    let agent_b = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner", None).await;
    seed_identity(&pool, community_id, &agent_a, "member", Some(&host)).await;
    seed_identity(&pool, community_id, &agent_b, "member", Some(&host)).await;

    let meeting_id = Uuid::new_v4();
    let agent_pubkeys = [agent_a.public_key().to_hex(), agent_b.public_key().to_hex()];
    let participant_refs: Vec<&str> = agent_pubkeys.iter().map(String::as_str).collect();
    let create = buzz_sdk::build_meeting_create(
        meeting_id,
        "Stage Three Agent Floor E2E",
        Some("Ready, Pass, early settlement, and Yield"),
        None,
        &participant_refs,
    )
    .expect("build meeting create")
    .sign_with_keys(&host)
    .expect("sign meeting create");
    let (status, body) = post_event(&host, &create).await;
    assert_accepted(status, &body);
    wait_for_state(&host, meeting_id, 1, "open", Duration::from_secs(2)).await;

    let basis_a = "activation:agent-a";
    let basis_b = "activation:agent-b";
    let ready_a = buzz_sdk::build_meeting_floor_ready(meeting_id, 1, basis_a)
        .expect("build Agent A Ready")
        .sign_with_keys(&agent_a)
        .expect("sign Agent A Ready");
    let ready_b = buzz_sdk::build_meeting_floor_ready(meeting_id, 1, basis_b)
        .expect("build Agent B Ready")
        .sign_with_keys(&agent_b)
        .expect("sign Agent B Ready");
    for (keys, event) in [(&agent_a, &ready_a), (&agent_b, &ready_b)] {
        let (status, body) = post_event(keys, event).await;
        assert_accepted(status, &body);
    }
    let (status, body) = post_event(&agent_a, &ready_a).await;
    assert_accepted(status, &body);

    let human_ready = buzz_sdk::build_meeting_floor_ready(meeting_id, 1, "activation:human")
        .expect("build Human Ready")
        .sign_with_keys(&host)
        .expect("sign Human Ready");
    let (status, body) = post_event(&host, &human_ready).await;
    assert_rejected(status, &body, "only an Agent participant");

    let claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 1)
        .expect("build Agent A Claim")
        .sign_with_keys(&agent_a)
        .expect("sign Agent A Claim");
    let (status, body) = post_event(&agent_a, &claim).await;
    assert_accepted(status, &body);
    let claiming = wait_for_state(&host, meeting_id, 1, "claiming", Duration::from_secs(2)).await;
    let claiming_content = content_json(&claiming);
    assert_eq!(
        claiming_content["decision_cohort"].as_array().map(Vec::len),
        Some(2)
    );
    let settle_not_before = claiming_content["settle_not_before_ms"]
        .as_i64()
        .expect("settle boundary");
    let claim_deadline = claiming_content["claim_deadline_ms"]
        .as_i64()
        .expect("Claim deadline");
    assert!(
        claim_deadline > settle_not_before,
        "early-settlement boundary must precede the configured deadline"
    );

    let pass = buzz_sdk::build_meeting_floor_pass(meeting_id, 1, basis_b)
        .expect("build Agent B Pass")
        .sign_with_keys(&agent_b)
        .expect("sign Agent B Pass");
    let (status, body) = post_event(&agent_b, &pass).await;
    assert_accepted(status, &body);
    let grant = wait_for_state(&host, meeting_id, 1, "granted", Duration::from_secs(8)).await;
    assert_eq!(
        tag_value(&grant, "holder"),
        Some(agent_a.public_key().to_hex().as_str())
    );
    let grant_id = grant["id"].as_str().expect("Grant event ID").to_string();

    let yield_event = buzz_sdk::build_meeting_floor_yield(meeting_id, 1, &grant_id)
        .expect("build Yield")
        .sign_with_keys(&agent_a)
        .expect("sign Yield");
    let (status, body) = post_event(&agent_a, &yield_event).await;
    assert_accepted(status, &body);
    let (status, body) = post_event(&agent_a, &yield_event).await;
    assert_accepted(status, &body);
    wait_for_state(&host, meeting_id, 2, "open", Duration::from_secs(2)).await;

    let round_one_closed =
        wait_for_state(&host, meeting_id, 1, "closed", Duration::from_secs(2)).await;
    assert_eq!(
        content_json(&round_one_closed)["outcome"].as_str(),
        Some("yielded")
    );
    let speeches = query(
        &host,
        json!([{
            "kinds": [9],
            "#h": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert!(
        speeches.is_empty(),
        "PASS and Yield must not create a candidate or public speech"
    );
}

#[tokio::test]
#[ignore = "requires a running relay, Postgres, and Redis"]
async fn meeting_floor_is_unique_grant_bound_recoverable_and_shared() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let human = Keys::generate();
    let agent_a = Keys::generate();
    let agent_b = Keys::generate();
    let outsider = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner", None).await;
    seed_identity(&pool, community_id, &human, "member", None).await;
    seed_identity(&pool, community_id, &agent_a, "member", Some(&host)).await;
    seed_identity(&pool, community_id, &agent_b, "member", Some(&host)).await;
    seed_identity(&pool, community_id, &outsider, "member", None).await;
    let participants = [&host, &human, &agent_a, &agent_b];
    let by_pubkey: HashMap<String, &Keys> = participants
        .iter()
        .map(|keys| (keys.public_key().to_hex(), *keys))
        .collect();

    let meeting_id = Uuid::new_v4();
    let other_pubkeys = [
        human.public_key().to_hex(),
        agent_a.public_key().to_hex(),
        agent_b.public_key().to_hex(),
    ];
    let other_refs: Vec<&str> = other_pubkeys.iter().map(String::as_str).collect();
    let create = buzz_sdk::build_meeting_create(
        meeting_id,
        "Stage Two E2E",
        Some("floor protocol proof"),
        None,
        &other_refs,
    )
    .expect("build meeting create")
    .sign_with_keys(&host)
    .expect("sign meeting create");
    let (status, body) = post_event(&host, &create).await;
    assert_accepted(status, &body);
    wait_for_state(&host, meeting_id, 1, "open", Duration::from_secs(2)).await;

    let claims: Vec<Event> = participants
        .iter()
        .map(|keys| {
            buzz_sdk::build_meeting_floor_claim(meeting_id, 1)
                .expect("build Claim")
                .sign_with_keys(keys)
                .expect("sign Claim")
        })
        .collect();
    let (claim_0, claim_1, claim_2, claim_3) = tokio::join!(
        post_event(&host, &claims[0]),
        post_event(&human, &claims[1]),
        post_event(&agent_a, &claims[2]),
        post_event(&agent_b, &claims[3]),
    );
    for (status, body) in [claim_0, claim_1, claim_2, claim_3] {
        assert_accepted(status, &body);
    }
    let (status, body) = post_event(&host, &claims[0]).await;
    assert_accepted(status, &body);

    let conflicting_claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 1)
        .expect("build conflicting Claim")
        .custom_created_at(Timestamp::from(Timestamp::now().as_secs() + 1))
        .sign_with_keys(&host)
        .expect("sign conflicting Claim");
    assert_ne!(conflicting_claim.id, claims[0].id);
    let (status, body) = post_event(&host, &conflicting_claim).await;
    assert_rejected(status, &body, "canonical Claim");

    let grant = wait_for_state(&host, meeting_id, 1, "granted", Duration::from_secs(5)).await;
    let holder_pubkey = tag_value(&grant, "holder")
        .expect("granted state holder")
        .to_string();
    let winner = by_pubkey
        .get(&holder_pubkey)
        .copied()
        .expect("winner is a participant");
    let grant_id = grant["id"].as_str().expect("Grant event ID").to_string();
    let grant_content = content_json(&grant);
    let claim_ids: BTreeSet<_> = grant_content["claim_event_ids"]
        .as_array()
        .expect("Grant canonical Claims")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(claim_ids.len(), 4);

    let fake_round_state = EventBuilder::new(Kind::Custom(KIND_MEETING_ROUND_STATE as u16), "{}")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", "1"]).expect("round tag"),
            Tag::parse(["floor-revision", "999"]).expect("revision tag"),
            Tag::parse(["phase", "granted"]).expect("phase tag"),
            Tag::parse(["policy", "uniform-v0"]).expect("policy tag"),
        ])
        .sign_with_keys(&human)
        .expect("sign fake Round State");
    let (status, body) = post_event(&human, &fake_round_state).await;
    assert_rejected(status, &body, "relay-only");

    let non_winner = participants
        .iter()
        .copied()
        .find(|keys| keys.public_key().to_hex() != holder_pubkey)
        .expect("non-winner");
    let unauthorized =
        buzz_sdk::build_meeting_speech(meeting_id, 1, &grant_id, "I did not win this round.", &[])
            .expect("build unauthorized speech")
            .sign_with_keys(non_winner)
            .expect("sign unauthorized speech");
    let (status, body) = post_event(non_winner, &unauthorized).await;
    assert_rejected(status, &body, "current floor holder");

    let threaded = EventBuilder::new(Kind::Custom(9), "thread tags are forbidden")
        .tags([
            Tag::parse(["h", &meeting_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", "1"]).expect("round tag"),
            Tag::parse(["meeting-grant", &grant_id]).expect("Grant tag"),
            Tag::parse(["e", &create.id.to_hex(), "", "reply"]).expect("reply tag"),
        ])
        .sign_with_keys(winner)
        .expect("sign threaded speech");
    let (status, body) = post_event(winner, &threaded).await;
    assert_rejected(status, &body, "tag e is not allowed");

    let v2 = EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE_V2 as u16), "not canonical")
        .tags([Tag::parse(["h", &meeting_id.to_string()]).expect("h tag")])
        .sign_with_keys(winner)
        .expect("sign kind 40002");
    let (status, body) = post_event(winner, &v2).await;
    assert_rejected(status, &body, "not part of the canonical Meeting log");

    let mention = non_winner.public_key().to_hex();
    let speech = buzz_sdk::build_meeting_speech(
        meeting_id,
        1,
        &grant_id,
        "The shared proposal.",
        &[mention.as_str()],
    )
    .expect("build winner speech")
    .sign_with_keys(winner)
    .expect("sign winner speech");
    let (status, body) = post_event(winner, &speech).await;
    assert_accepted(status, &body);
    let (status, body) = post_event(winner, &speech).await;
    assert_accepted(status, &body);
    let second_speech =
        buzz_sdk::build_meeting_speech(meeting_id, 1, &grant_id, "A forbidden second speech.", &[])
            .expect("build second speech")
            .custom_created_at(Timestamp::from(Timestamp::now().as_secs() + 1))
            .sign_with_keys(winner)
            .expect("sign second speech");
    let (status, body) = post_event(winner, &second_speech).await;
    assert_rejected(status, &body, "Grant already consumed");
    wait_for_state(&human, meeting_id, 2, "open", Duration::from_secs(2)).await;

    let speech_filter = json!([{
        "kinds": [9],
        "#h": [meeting_id.to_string()],
        "limit": 100
    }]);
    for participant in participants {
        let history = query(participant, speech_filter.clone()).await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0]["id"].as_str(), Some(speech.id.to_hex().as_str()));
        assert_eq!(history[0]["pubkey"].as_str(), Some(holder_pubkey.as_str()));
        assert_eq!(tag_value(&history[0], "p"), Some(mention.as_str()));
    }
    assert!(query(&outsider, speech_filter.clone()).await.is_empty());

    let round_two_claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 2)
        .expect("build Round 2 Claim")
        .sign_with_keys(&agent_b)
        .expect("sign Round 2 Claim");
    let (status, body) = post_event(&agent_b, &round_two_claim).await;
    assert_accepted(status, &body);
    wait_for_state(&host, meeting_id, 2, "granted", Duration::from_secs(5)).await;
    expire_test_grant(&pool, community_id, meeting_id, 2).await;
    let expired = wait_for_state(&host, meeting_id, 2, "closed", Duration::from_secs(5)).await;
    assert_eq!(content_json(&expired)["outcome"].as_str(), Some("expired"));
    wait_for_state(&host, meeting_id, 3, "open", Duration::from_secs(2)).await;

    let end = buzz_sdk::build_meeting_end(meeting_id, &create.id.to_hex())
        .expect("build meeting End")
        .sign_with_keys(&host)
        .expect("sign meeting End");
    let (status, body) = post_event(&host, &end).await;
    assert_accepted(status, &body);
    let ended = wait_for_state(&human, meeting_id, 3, "closed", Duration::from_secs(2)).await;
    assert_eq!(content_json(&ended)["outcome"].as_str(), Some("ended"));

    let late_claim = buzz_sdk::build_meeting_floor_claim(meeting_id, 3)
        .expect("build late Claim")
        .sign_with_keys(&human)
        .expect("sign late Claim");
    let (status, body) = post_event(&human, &late_claim).await;
    assert_rejected(status, &body, "meeting has ended");

    for participant in participants {
        let controls = floor_events(participant, meeting_id).await;
        let ids: BTreeSet<_> = controls
            .iter()
            .filter_map(|event| event["id"].as_str())
            .collect();
        assert!(!ids.is_empty());
        let mut revisions: Vec<u64> = controls
            .iter()
            .filter(|event| event["kind"].as_u64() == Some(KIND_MEETING_ROUND_STATE as u64))
            .map(floor_revision)
            .collect();
        revisions.sort_unstable();
        assert_eq!(
            revisions.iter().copied().collect::<BTreeSet<_>>().len(),
            revisions.len(),
            "each floor revision must name exactly one Round State"
        );
        assert_eq!(
            revisions,
            (1..=u64::try_from(revisions.len()).expect("revision count fits u64"))
                .collect::<Vec<_>>(),
            "floor revisions must be monotonic and gap-free"
        );
    }
    assert!(
        floor_events(&outsider, meeting_id).await.is_empty(),
        "non-participant must not read the floor control log"
    );

    let metadata = query(
        &human,
        json!([{
            "kinds": [KIND_NIP29_GROUP_METADATA],
            "#d": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert_eq!(metadata.len(), 1);
}
