//! End-to-end proof for the Meeting V1 moderated-baton protocol.
//!
//! Requires a running Relay with Meeting V1 enabled, Postgres, and Redis. The
//! timeout branch advances its own persisted deadline, so it does not depend on
//! short production timing configuration.

use std::time::Duration;

use buzz_core::kind::{
    KIND_MEETING_END, KIND_MEETING_OFFER_RESPONSE, KIND_MEETING_STATE, KIND_STREAM_MESSAGE,
};
use buzz_sdk::{
    MeetingV1CreateParams, MeetingV1DirectedHandoff, MeetingV1EndParams,
    MeetingV1GrantProgressParams, MeetingV1HandoffType, MeetingV1HumanFloorRequestParams,
    MeetingV1IntentSubmitParams, MeetingV1ModeratorSelectParams, MeetingV1OfferAckParams,
    MeetingV1ProgressStage, MeetingV1Selection, MeetingV1SpeechParams,
};
use buzz_test_client::{BuzzTestClient, RelayMessage, TestClientError};
use nostr::{Alphabet, Event, EventBuilder, Filter, Keys, Kind, SingleLetterTag, Tag, Timestamp};
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
        .expect("connect to Meeting V1 baton E2E database")
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
    .expect("seed Meeting V1 identity");

    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, pubkey) DO UPDATE \
         SET role = EXCLUDED.role",
    )
    .bind(community_id)
    .bind(keys.public_key().to_hex())
    .bind(relay_role)
    .execute(pool)
    .await
    .expect("seed Meeting V1 relay membership");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize event"))
        .send()
        .await
        .expect("submit Meeting V1 event");
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

fn canonical_object_id(body: &str) -> String {
    let response: Value = serde_json::from_str(body).expect("parse Relay write response");
    let payload = response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .expect("parse Meeting V1 response payload");
    payload["canonical_object_id"]
        .as_str()
        .expect("Meeting V1 response canonical object id")
        .to_string()
}

async fn query(keys: &Keys, filters: Value) -> Vec<Value> {
    let response = reqwest::Client::new()
        .post(format!("{}/query", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(filters.to_string())
        .send()
        .await
        .expect("query Meeting V1 events");
    let status = response.status();
    let body = response.text().await.expect("read query response");
    assert!(
        status.is_success(),
        "Meeting V1 query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse Meeting V1 query response")
}

fn state_content(event: &Value) -> Value {
    serde_json::from_str(
        event["content"]
            .as_str()
            .expect("Meeting V1 State content string"),
    )
    .expect("Meeting V1 State JSON")
}

fn state_revision(event: &Value) -> u64 {
    state_content(event)["state_revision"].as_u64().unwrap_or(0)
}

async fn wait_for_state(
    keys: &Keys,
    meeting_id: Uuid,
    minimum_revision: u64,
    timeout: Duration,
) -> Value {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let latest = query(
            keys,
            json!([{
                "kinds": [KIND_MEETING_STATE],
                "#h": [meeting_id.to_string()],
                "limit": 200
            }]),
        )
        .await
        .into_iter()
        .max_by_key(state_revision);
        if let Some(state) = latest.filter(|state| state_revision(state) >= minimum_revision) {
            return state;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for Meeting V1 State revision {minimum_revision}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn state_u64(state: &Value, field: &str) -> u64 {
    state_content(state)[field]
        .as_u64()
        .unwrap_or_else(|| panic!("Meeting V1 State is missing {field}"))
}

async fn submit_intent(
    keys: &Keys,
    meeting_id: Uuid,
    basis_speech_revision: u64,
    summary: &str,
) -> Event {
    let event = buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
        session_id: meeting_id,
        basis_speech_revision,
        addressed_to: None,
        summary,
    })
    .expect("build Meeting V1 Intent")
    .sign_with_keys(keys)
    .expect("sign Meeting V1 Intent");
    let (status, body) = post_event(keys, &event).await;
    assert_accepted(status, &body);
    assert_eq!(canonical_object_id(&body), event.id.to_hex());
    event
}

async fn select_intent(
    moderator: &Keys,
    meeting_id: Uuid,
    intent: &Event,
    state: &Value,
) -> String {
    let event = buzz_sdk::build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
        session_id: meeting_id,
        selection: MeetingV1Selection::Intent {
            intent_id: &intent.id.to_hex(),
        },
        expected_control_epoch: state_u64(state, "control_epoch"),
        expected_decision_epoch: state_u64(state, "decision_epoch"),
        expected_intent_revision: state_u64(state, "intent_revision"),
        expected_speech_revision: state_u64(state, "speech_revision"),
        selection_reason: Some("next relevant contribution"),
        deferrals: &[],
        attempt_id: None,
        expected_source_event_id: None,
    })
    .expect("build Meeting V1 Select")
    .sign_with_keys(moderator)
    .expect("sign Meeting V1 Select");
    let (status, body) = post_event(moderator, &event).await;
    assert_accepted(status, &body);
    canonical_object_id(&body)
}

async fn select_handoff(
    moderator: &Keys,
    meeting_id: Uuid,
    handoff_id: &str,
    expected_attempt_count: u64,
    state: &Value,
) -> String {
    let event = buzz_sdk::build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
        session_id: meeting_id,
        selection: MeetingV1Selection::Handoff {
            handoff_id,
            expected_attempt_count,
        },
        expected_control_epoch: state_u64(state, "control_epoch"),
        expected_decision_epoch: state_u64(state, "decision_epoch"),
        expected_intent_revision: state_u64(state, "intent_revision"),
        expected_speech_revision: state_u64(state, "speech_revision"),
        selection_reason: Some("restore the interrupted directed question"),
        deferrals: &[],
        attempt_id: None,
        expected_source_event_id: None,
    })
    .expect("build Meeting V1 Handoff Select")
    .sign_with_keys(moderator)
    .expect("sign Meeting V1 Handoff Select");
    let (status, body) = post_event(moderator, &event).await;
    assert_accepted(status, &body);
    canonical_object_id(&body)
}

async fn ack_offer(keys: &Keys, meeting_id: Uuid, offer_id: &str) -> String {
    let event = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
        session_id: meeting_id,
        offer_id,
    })
    .expect("build Meeting V1 Offer ACK")
    .sign_with_keys(keys)
    .expect("sign Meeting V1 Offer ACK");
    let (status, body) = post_event(keys, &event).await;
    assert_accepted(status, &body);
    canonical_object_id(&body)
}

async fn speak(
    keys: &Keys,
    meeting_id: Uuid,
    grant_id: &str,
    speech_revision: u64,
    content: &str,
    handoff: Option<MeetingV1DirectedHandoff<'_>>,
) -> Event {
    let event = buzz_sdk::build_meeting_v1_speech(MeetingV1SpeechParams {
        session_id: meeting_id,
        grant_id,
        speech_revision,
        content,
        mentions: &[],
        handoff,
    })
    .expect("build Meeting V1 Speech")
    .sign_with_keys(keys)
    .expect("sign Meeting V1 Speech");
    let (status, body) = post_event(keys, &event).await;
    assert_accepted(status, &body);
    assert_eq!(canonical_object_id(&body), event.id.to_hex());
    event
}

async fn request_human_floor(keys: &Keys, meeting_id: Uuid, created_at_offset_secs: u64) -> Event {
    let builder =
        buzz_sdk::build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        })
        .expect("build Meeting V1 Human Floor Request");
    let builder = if created_at_offset_secs == 0 {
        builder
    } else {
        builder.custom_created_at(Timestamp::from(
            Timestamp::now().as_secs() + created_at_offset_secs,
        ))
    };
    let event = builder
        .sign_with_keys(keys)
        .expect("sign Meeting V1 Human Floor Request");
    let (status, body) = post_event(keys, &event).await;
    assert_accepted(status, &body);
    assert_eq!(canonical_object_id(&body), event.id.to_hex());
    event
}

async fn wait_for_next_state(keys: &Keys, meeting_id: Uuid, previous: &Value) -> Value {
    wait_for_state(
        keys,
        meeting_id,
        state_revision(previous) + 1,
        Duration::from_secs(5),
    )
    .await
}

fn assert_state_history_is_canonical(states: &mut [Value], expected_speeches: &[Event]) {
    states.sort_by_key(state_revision);
    assert!(
        !states.is_empty(),
        "Meeting V1 must publish at least one State"
    );

    let mut previous_floor_revision = 0;
    let mut previous_intent_revision = 0;
    let mut previous_speech_revision = 0;
    let mut speech_ids = Vec::new();
    for (index, state) in states.iter().enumerate() {
        let expected_state_revision = u64::try_from(index).expect("state index fits u64") + 1;
        let content = state_content(state);
        assert_eq!(
            content["state_revision"], expected_state_revision,
            "canonical State revisions must be gap-free and unique"
        );

        let floor_revision = content["floor_revision"].as_u64().expect("floor revision");
        let intent_revision = content["intent_revision"]
            .as_u64()
            .expect("intent revision");
        let speech_revision = content["speech_revision"]
            .as_u64()
            .expect("speech revision");
        assert!(floor_revision >= previous_floor_revision);
        assert!(intent_revision >= previous_intent_revision);
        assert!(speech_revision >= previous_speech_revision);
        previous_floor_revision = floor_revision;
        previous_intent_revision = intent_revision;
        previous_speech_revision = speech_revision;

        let phase = content["phase"].as_str().expect("Meeting V1 phase");
        let has_offer = !content["offer"].is_null();
        let has_grant = !content["grant"].is_null();
        assert!(
            !(has_offer && has_grant),
            "a canonical State must never expose an Offer and Grant together"
        );
        match phase {
            "offered" => assert!(
                has_offer && !has_grant,
                "the offered phase must expose exactly one active Offer"
            ),
            "granted" => assert!(
                has_grant && !has_offer,
                "the granted phase must expose exactly one active Grant"
            ),
            _ => assert!(
                !has_offer && !has_grant,
                "phase {phase} must not retain an active Offer or Grant"
            ),
        }

        if content["transition"]["primary_type"] == "speech_accepted" {
            speech_ids.push(
                content["transition"]["caused_by_event_id"]
                    .as_str()
                    .expect("speech transition cause")
                    .to_string(),
            );
        }
    }

    let expected_ids = expected_speeches
        .iter()
        .map(|speech| speech.id.to_hex())
        .collect::<Vec<_>>();
    assert_eq!(
        speech_ids, expected_ids,
        "State transitions must preserve the canonical speech order"
    );
    assert_eq!(
        previous_speech_revision,
        u64::try_from(expected_speeches.len()).expect("speech count fits u64")
    );
}

#[tokio::test]
#[ignore = "requires a running Relay with BUZZ_MEETING_V1_CREATE_ENABLED=true, Postgres, and Redis"]
async fn moderated_baton_closes_agent_handoff_human_privacy_and_timeout_paths() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let owner = Keys::generate();
    let moderator = Keys::generate();
    let agent = Keys::generate();
    let human = Keys::generate();
    let outsider = Keys::generate();
    seed_identity(&pool, community_id, &owner, "owner", None).await;
    seed_identity(&pool, community_id, &moderator, "member", None).await;
    seed_identity(&pool, community_id, &agent, "member", Some(&owner)).await;
    seed_identity(&pool, community_id, &human, "member", None).await;
    seed_identity(&pool, community_id, &outsider, "member", None).await;

    let meeting_id = Uuid::new_v4();
    let moderator_pubkey = moderator.public_key().to_hex();
    let agent_pubkey = agent.public_key().to_hex();
    let human_pubkey = human.public_key().to_hex();
    let participant_pubkeys = [
        moderator_pubkey.as_str(),
        agent_pubkey.as_str(),
        human_pubkey.as_str(),
    ];
    let owner_pubkey = owner.public_key().to_hex();
    let create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Moderated Baton E2E",
        description: Some("Agent, direct handoff, Human, receipt, and timeout proof"),
        source_channel_id: None,
        author_pubkey: &owner_pubkey,
        moderator_pubkey: &moderator_pubkey,
        participant_pubkeys: &participant_pubkeys,
    })
    .expect("build Meeting V1 Create")
    .sign_with_keys(&owner)
    .expect("sign Meeting V1 Create");
    let (status, body) = post_event(&owner, &create).await;
    assert_accepted(status, &body);
    let initial = wait_for_state(&moderator, meeting_id, 1, Duration::from_secs(5)).await;
    assert_eq!(state_content(&initial)["phase"], "moderator_idle");

    // Agent Intent -> moderator Select -> ACK -> Progress -> Speech.
    let first_intent = submit_intent(&agent, meeting_id, 0, "Report the dependency risk").await;
    let after_intent = wait_for_state(&moderator, meeting_id, 2, Duration::from_secs(5)).await;
    let first_offer = select_intent(&moderator, meeting_id, &first_intent, &after_intent).await;
    let first_grant = ack_offer(&agent, meeting_id, &first_offer).await;
    let progress = buzz_sdk::build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
        session_id: meeting_id,
        grant_id: &first_grant,
        progress_seq: 1,
        stage: MeetingV1ProgressStage::ToolUse,
    })
    .expect("build Meeting V1 Progress")
    .sign_with_keys(&agent)
    .expect("sign Meeting V1 Progress");
    let (status, body) = post_event(&agent, &progress).await;
    assert_accepted(status, &body);

    let first_speech = speak(
        &agent,
        meeting_id,
        &first_grant,
        1,
        "The dependency is stale; can you verify the mitigation?",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &human_pubkey,
            handoff_type: MeetingV1HandoffType::Review,
            reason: "Verify the proposed mitigation before we continue",
        }),
    )
    .await;
    let after_handoff = wait_for_state(&human, meeting_id, 6, Duration::from_secs(5)).await;
    let handoff_state = state_content(&after_handoff);
    assert_eq!(handoff_state["phase"], "offered");
    assert_eq!(handoff_state["offer"]["target_pubkey"], human_pubkey);
    assert_eq!(
        handoff_state["unresolved_handoffs"][0]["handoff_id"],
        first_speech.id.to_hex()
    );
    let human_offer = handoff_state["offer"]["offer_id"]
        .as_str()
        .expect("direct handoff Offer ID")
        .to_string();
    let human_grant = ack_offer(&human, meeting_id, &human_offer).await;
    speak(
        &human,
        meeting_id,
        &human_grant,
        2,
        "I verified the mitigation and it is safe.",
        None,
    )
    .await;

    // An idle Human request receives priority immediately.
    let request =
        buzz_sdk::build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        })
        .expect("build Meeting V1 Human Floor Request")
        .sign_with_keys(&owner)
        .expect("sign Meeting V1 Human Floor Request");
    let (status, body) = post_event(&owner, &request).await;
    assert_accepted(status, &body);
    let after_request = wait_for_state(&owner, meeting_id, 9, Duration::from_secs(5)).await;
    let request_state = state_content(&after_request);
    assert_eq!(request_state["phase"], "offered");
    assert_eq!(request_state["offer"]["target_pubkey"], owner_pubkey);
    let owner_offer = request_state["offer"]["offer_id"]
        .as_str()
        .expect("Human request Offer ID")
        .to_string();
    let owner_grant = ack_offer(&owner, meeting_id, &owner_offer).await;
    speak(
        &owner,
        meeting_id,
        &owner_grant,
        3,
        "Proceed with the verified mitigation.",
        None,
    )
    .await;

    // Keep a new Offer active, then submit an old Offer ACK. The rejection is
    // durable and replayable only as a private author-bound receipt.
    let second_intent = submit_intent(
        &agent,
        meeting_id,
        3,
        "Add the mitigation to the rollout notes",
    )
    .await;
    let after_second_intent =
        wait_for_state(&moderator, meeting_id, 12, Duration::from_secs(5)).await;
    let second_offer =
        select_intent(&moderator, meeting_id, &second_intent, &after_second_intent).await;
    let offered = wait_for_state(&agent, meeting_id, 13, Duration::from_secs(5)).await;
    assert_eq!(state_content(&offered)["offer"]["offer_id"], second_offer);

    let stale_ack = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
        session_id: meeting_id,
        offer_id: &first_offer,
    })
    .expect("build stale Meeting V1 Offer ACK")
    // The first ACK may have been signed in the same one-second Nostr timestamp
    // bucket with identical tags/content. Force a distinct event id so this is
    // a new stale command, not an idempotent replay of the accepted ACK.
    .custom_created_at(Timestamp::from(Timestamp::now().as_secs() + 1))
    .sign_with_keys(&agent)
    .expect("sign stale Meeting V1 Offer ACK");
    let (first_status, first_body) = post_event(&agent, &stale_ack).await;
    assert_eq!(first_status, reqwest::StatusCode::CONFLICT);
    assert!(
        first_body.contains("offer_not_active"),
        "expected terminal stale-Offer code, got: {first_body}"
    );
    let (replay_status, replay_body) = post_event(&agent, &stale_ack).await;
    assert_eq!(replay_status, reqwest::StatusCode::CONFLICT);
    assert_eq!(
        replay_body, first_body,
        "author replay must recover the same private receipt"
    );

    let receipt: (Vec<u8>, bool, String) = sqlx::query_as(
        "SELECT author_pubkey, accepted, outcome_code \
         FROM meeting_v1_command_receipts \
         WHERE community_id = $1 AND command_event_id = $2",
    )
    .bind(community_id)
    .bind(stale_ack.id.as_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .expect("load stale ACK private receipt");
    assert_eq!(receipt.0, agent.public_key().to_bytes());
    assert!(!receipt.1);
    assert_eq!(receipt.2, "offer_not_active");

    let public_event_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM events WHERE community_id = $1 AND id = $2 \
         )",
    )
    .bind(community_id)
    .bind(stale_ack.id.as_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .expect("check rejected command public persistence");
    assert!(
        !public_event_exists,
        "a rejected command must never enter the shared event log"
    );
    let public_outbox_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM meeting_event_outbox \
             WHERE community_id = $1 AND event_id = $2 \
         )",
    )
    .bind(community_id)
    .bind(stale_ack.id.as_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .expect("check rejected command public outbox");
    assert!(
        !public_outbox_exists,
        "a rejected command must never enter the shared outbox"
    );
    let public_offer_commands = query(
        &agent,
        json!([{
            "kinds": [KIND_MEETING_OFFER_RESPONSE],
            "#h": [meeting_id.to_string()],
            "limit": 200
        }]),
    )
    .await;
    assert!(
        public_offer_commands
            .iter()
            .all(|event| event["id"] != stale_ack.id.to_hex()),
        "a rejected command must not be visible in participant history"
    );

    // A different authenticated identity cannot replay the event to retrieve
    // the receipt, and a non-participant cannot query any shared Meeting log.
    let (outsider_status, outsider_body) = post_event(&outsider, &stale_ack).await;
    assert!(!outsider_status.is_success());
    assert!(
        !outsider_body.contains("offer_not_active"),
        "non-author must fail before private receipt disclosure: {outsider_body}"
    );
    assert!(
        query(
            &outsider,
            json!([{
                "kinds": [KIND_MEETING_STATE, KIND_MEETING_OFFER_RESPONSE],
                "#h": [meeting_id.to_string()],
                "limit": 200
            }])
        )
        .await
        .is_empty(),
        "non-participant must not read shared Meeting events"
    );

    // Force only this test Offer due. The Relay sweeper must publish a coherent
    // timeout transition without waiting for the configured ACK duration.
    sqlx::query(
        "UPDATE meeting_baton_offers \
         SET ack_deadline = clock_timestamp() - INTERVAL '1 second' \
         WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(hex::decode(&second_offer).expect("decode second Offer ID"))
    .execute(&pool)
    .await
    .expect("force Meeting V1 Offer deadline");
    sqlx::query(
        "UPDATE meeting_baton_state \
         SET next_action_at = clock_timestamp() - INTERVAL '1 second' \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .execute(&pool)
    .await
    .expect("force Meeting V1 next action");
    let timed_out = wait_for_state(&moderator, meeting_id, 14, Duration::from_secs(5)).await;
    let timed_out_content = state_content(&timed_out);
    assert_eq!(
        timed_out_content["phase"], "moderator_control",
        "the timed-out Intent remains a deterministic fallback candidate, so \
         control returns to the moderator with an active decision window"
    );
    assert_eq!(
        timed_out_content["transition"]["primary_type"],
        "offer_timed_out"
    );
    assert_eq!(
        timed_out_content["transition"]["deadline_type"],
        "offer_ack"
    );

    let end = buzz_sdk::build_meeting_v1_end(MeetingV1EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
    })
    .expect("build Meeting V1 End")
    .sign_with_keys(&owner)
    .expect("sign Meeting V1 End");
    let (status, body) = post_event(&owner, &end).await;
    assert_accepted(status, &body);
    let ended = query(
        &owner,
        json!([{
            "kinds": [KIND_MEETING_END],
            "#h": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert!(
        ended.iter().any(|event| event["id"] == end.id.to_hex()),
        "manual Meeting V1 End must reach the shared private log"
    );
}

#[tokio::test]
#[ignore = "requires a running Relay with BUZZ_MEETING_V1_CREATE_ENABLED=true, Postgres, and Redis"]
async fn two_humans_and_two_agents_share_one_ordered_baton_timeline() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let moderator = Keys::generate();
    let human = Keys::generate();
    let first_agent = Keys::generate();
    let second_agent = Keys::generate();
    seed_identity(&pool, community_id, &moderator, "owner", None).await;
    seed_identity(&pool, community_id, &human, "member", None).await;
    seed_identity(
        &pool,
        community_id,
        &first_agent,
        "member",
        Some(&moderator),
    )
    .await;
    seed_identity(
        &pool,
        community_id,
        &second_agent,
        "member",
        Some(&moderator),
    )
    .await;

    let meeting_id = Uuid::new_v4();
    let moderator_pubkey = moderator.public_key().to_hex();
    let human_pubkey = human.public_key().to_hex();
    let first_agent_pubkey = first_agent.public_key().to_hex();
    let second_agent_pubkey = second_agent.public_key().to_hex();
    let participants = [
        human_pubkey.as_str(),
        first_agent_pubkey.as_str(),
        second_agent_pubkey.as_str(),
    ];
    let create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Two Humans and Two Agents",
        description: Some("Ordered shared-baton acceptance scenario"),
        source_channel_id: None,
        author_pubkey: &moderator_pubkey,
        moderator_pubkey: &moderator_pubkey,
        participant_pubkeys: &participants,
    })
    .expect("build four-participant Meeting V1 Create")
    .sign_with_keys(&moderator)
    .expect("sign four-participant Meeting V1 Create");
    let (status, body) = post_event(&moderator, &create).await;
    assert_accepted(status, &body);

    let initial = wait_for_state(&moderator, meeting_id, 1, Duration::from_secs(5)).await;
    let initial_content = state_content(&initial);
    let roster = initial_content["participants"]
        .as_array()
        .expect("Meeting V1 participant roster");
    assert_eq!(roster.len(), 4);
    assert_eq!(
        roster
            .iter()
            .filter(|participant| participant["participant_type"] == "human")
            .count(),
        2
    );
    assert_eq!(
        roster
            .iter()
            .filter(|participant| participant["participant_type"] == "agent")
            .count(),
        2
    );

    let first_intent = submit_intent(
        &first_agent,
        meeting_id,
        0,
        "Present the dependency evidence",
    )
    .await;
    let after_first_intent =
        wait_for_state(&moderator, meeting_id, 2, Duration::from_secs(5)).await;
    let first_offer =
        select_intent(&moderator, meeting_id, &first_intent, &after_first_intent).await;
    let first_grant = ack_offer(&first_agent, meeting_id, &first_offer).await;

    // Keep the first Agent's Grant active while another Agent and a Human
    // independently write. Their asynchronous control-plane actions must not
    // wait for the slow/full-speech turn to finish.
    let second_intent = submit_intent(
        &second_agent,
        meeting_id,
        0,
        "Review the dependency evidence",
    )
    .await;
    let human_request =
        buzz_sdk::build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        })
        .expect("build Human Floor Request during Agent Grant")
        .sign_with_keys(&human)
        .expect("sign Human Floor Request during Agent Grant");
    let (status, body) = post_event(&human, &human_request).await;
    assert_accepted(status, &body);
    let writes_during_grant =
        wait_for_state(&moderator, meeting_id, 6, Duration::from_secs(5)).await;
    let writes_during_grant = state_content(&writes_during_grant);
    assert_eq!(writes_during_grant["phase"], "granted");
    assert_eq!(
        writes_during_grant["grant"]["grant_id"], first_grant,
        "asynchronous writes must not revoke the current speaker"
    );
    assert!(
        writes_during_grant["pending_intents"]
            .as_array()
            .is_some_and(|intents| intents
                .iter()
                .any(|intent| intent["intent_id"] == second_intent.id.to_hex())),
        "the second Agent Intent must be visible while the first Agent still owns the Grant"
    );

    let first_speech = speak(
        &first_agent,
        meeting_id,
        &first_grant,
        1,
        "The dependency evidence is ready.",
        None,
    )
    .await;
    let human_offered = wait_for_state(&human, meeting_id, 7, Duration::from_secs(5)).await;
    let human_offer = state_content(&human_offered)["offer"]["offer_id"]
        .as_str()
        .expect("Human Offer ID")
        .to_string();
    let human_grant = ack_offer(&human, meeting_id, &human_offer).await;
    let human_speech = speak(
        &human,
        meeting_id,
        &human_grant,
        2,
        "The human decision is to proceed.",
        None,
    )
    .await;
    let after_human = wait_for_state(&moderator, meeting_id, 9, Duration::from_secs(5)).await;

    let second_offer = select_intent(&moderator, meeting_id, &second_intent, &after_human).await;
    let second_grant = ack_offer(&second_agent, meeting_id, &second_offer).await;
    let second_speech = speak(
        &second_agent,
        meeting_id,
        &second_grant,
        3,
        "The dependency evidence is consistent.",
        None,
    )
    .await;
    wait_for_state(&moderator, meeting_id, 12, Duration::from_secs(5)).await;

    // The moderator is the second Human identity. Exercise its ordinary
    // self-Intent path as well, so every one of the four participants actually
    // obtains a Grant and contributes to the same canonical timeline.
    let moderator_intent = submit_intent(
        &moderator,
        meeting_id,
        3,
        "Close the discussion with a summary",
    )
    .await;
    let after_moderator_intent =
        wait_for_state(&moderator, meeting_id, 13, Duration::from_secs(5)).await;
    let moderator_offer = select_intent(
        &moderator,
        meeting_id,
        &moderator_intent,
        &after_moderator_intent,
    )
    .await;
    let moderator_grant = ack_offer(&moderator, meeting_id, &moderator_offer).await;
    let moderator_speech = speak(
        &moderator,
        meeting_id,
        &moderator_grant,
        4,
        "The moderator summary confirms the decision.",
        None,
    )
    .await;
    let final_state = wait_for_state(&moderator, meeting_id, 16, Duration::from_secs(5)).await;
    assert_eq!(state_content(&final_state)["speech_revision"], 4);

    let end = buzz_sdk::build_meeting_v1_end(MeetingV1EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
    })
    .expect("build four-participant Meeting V1 End")
    .sign_with_keys(&moderator)
    .expect("sign four-participant Meeting V1 End");
    let (status, body) = post_event(&moderator, &end).await;
    assert_accepted(status, &body);
    let ended_state = wait_for_state(&moderator, meeting_id, 17, Duration::from_secs(5)).await;
    assert_eq!(state_content(&ended_state)["phase"], "ended");

    let rejected_after_end =
        buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
            session_id: meeting_id,
            basis_speech_revision: 4,
            addressed_to: None,
            summary: "This must not reopen an ended Meeting",
        })
        .expect("build post-End Intent")
        .sign_with_keys(&second_agent)
        .expect("sign post-End Intent");
    let (status, body) = post_event(&second_agent, &rejected_after_end).await;
    assert!(
        !status.is_success(),
        "an ended Meeting must reject every new control write: {body}"
    );

    let speeches = query(
        &moderator,
        json!([{
            "kinds": [KIND_STREAM_MESSAGE],
            "#h": [meeting_id.to_string()],
            "limit": 50
        }]),
    )
    .await;
    for expected in [
        first_speech.id,
        second_speech.id,
        human_speech.id,
        moderator_speech.id,
    ] {
        assert!(
            speeches
                .iter()
                .any(|speech| speech["id"] == expected.to_hex()),
            "every accepted speech must remain readable after End"
        );
    }
}

#[tokio::test]
#[ignore = "requires a running Relay with BUZZ_MEETING_V1_CREATE_ENABLED=true, Postgres, and Redis"]
async fn four_agents_and_two_humans_complete_repeated_rounds_at_capacity() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let moderator = Keys::generate();
    let human = Keys::generate();
    let first_agent = Keys::generate();
    let second_agent = Keys::generate();
    let third_agent = Keys::generate();
    let fourth_agent = Keys::generate();
    let fifth_agent = Keys::generate();
    seed_identity(&pool, community_id, &moderator, "owner", None).await;
    seed_identity(&pool, community_id, &human, "member", None).await;
    for agent in [
        &first_agent,
        &second_agent,
        &third_agent,
        &fourth_agent,
        &fifth_agent,
    ] {
        seed_identity(&pool, community_id, agent, "member", Some(&moderator)).await;
    }

    let moderator_pubkey = moderator.public_key().to_hex();
    let human_pubkey = human.public_key().to_hex();
    let first_agent_pubkey = first_agent.public_key().to_hex();
    let second_agent_pubkey = second_agent.public_key().to_hex();
    let third_agent_pubkey = third_agent.public_key().to_hex();
    let fourth_agent_pubkey = fourth_agent.public_key().to_hex();
    let fifth_agent_pubkey = fifth_agent.public_key().to_hex();

    // The valid scenario below deliberately fills the four-Agent capacity.
    // Prove that registering one more Agent does not silently widen the
    // protocol boundary.
    let over_capacity_id = Uuid::new_v4();
    let over_capacity_participants = [
        human_pubkey.as_str(),
        first_agent_pubkey.as_str(),
        second_agent_pubkey.as_str(),
        third_agent_pubkey.as_str(),
        fourth_agent_pubkey.as_str(),
        fifth_agent_pubkey.as_str(),
    ];
    let over_capacity_create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: over_capacity_id,
        title: "Five Agent Capacity Rejection",
        description: Some("The fifth managed Agent must be rejected"),
        source_channel_id: None,
        author_pubkey: &moderator_pubkey,
        moderator_pubkey: &moderator_pubkey,
        participant_pubkeys: &over_capacity_participants,
    })
    .expect("build over-capacity Meeting V1 Create")
    .sign_with_keys(&moderator)
    .expect("sign over-capacity Meeting V1 Create");
    let (status, body) = post_event(&moderator, &over_capacity_create).await;
    assert!(
        !status.is_success() && body.contains("meeting supports at most 4 agents"),
        "the fifth managed Agent must be rejected explicitly, got HTTP {status}: {body}"
    );
    assert!(
        query(
            &moderator,
            json!([{
                "kinds": [KIND_MEETING_STATE],
                "#h": [over_capacity_id.to_string()],
                "limit": 10
            }])
        )
        .await
        .is_empty(),
        "a rejected over-capacity Create must not publish Meeting State"
    );

    let meeting_id = Uuid::new_v4();
    let participants = [
        human_pubkey.as_str(),
        first_agent_pubkey.as_str(),
        second_agent_pubkey.as_str(),
        third_agent_pubkey.as_str(),
        fourth_agent_pubkey.as_str(),
    ];
    let create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Four Agents and Two Humans",
        description: Some("Repeated moderated, Human-priority, and handoff rounds"),
        source_channel_id: None,
        author_pubkey: &moderator_pubkey,
        moderator_pubkey: &moderator_pubkey,
        participant_pubkeys: &participants,
    })
    .expect("build six-participant Meeting V1 Create")
    .sign_with_keys(&moderator)
    .expect("sign six-participant Meeting V1 Create");
    let (status, body) = post_event(&moderator, &create).await;
    assert_accepted(status, &body);

    let mut state = wait_for_state(&moderator, meeting_id, 1, Duration::from_secs(5)).await;
    let roster = state_content(&state)["participants"]
        .as_array()
        .expect("six-participant Meeting roster")
        .clone();
    assert_eq!(roster.len(), 6);
    assert_eq!(
        roster
            .iter()
            .filter(|participant| participant["participant_type"] == "human")
            .count(),
        2
    );
    assert_eq!(
        roster
            .iter()
            .filter(|participant| participant["participant_type"] == "agent")
            .count(),
        4
    );

    let mut speeches = Vec::with_capacity(12);

    // Round 1: the moderator selects Agent 1, who directly hands the baton to
    // Agent 2. Agent 2 returns control to the moderator by omitting a handoff.
    let first_intent = submit_intent(
        &first_agent,
        meeting_id,
        state_u64(&state, "speech_revision"),
        "Open with the dependency inventory",
    )
    .await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let first_offer = select_intent(&moderator, meeting_id, &first_intent, &state).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(
        state_content(&state)["offer"]["target_pubkey"],
        first_agent_pubkey
    );
    let first_grant = ack_offer(&first_agent, meeting_id, &first_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &first_agent,
        meeting_id,
        &first_grant,
        1,
        "Agent 1 found two dependency risks and asks Agent 2 to verify them.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &second_agent_pubkey,
            handoff_type: MeetingV1HandoffType::Question,
            reason: "Verify the two dependency risks before the agenda continues",
        }),
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let state_content_after_handoff = state_content(&state);
    assert_eq!(state_content_after_handoff["phase"], "offered");
    assert_eq!(
        state_content_after_handoff["offer"]["target_pubkey"],
        second_agent_pubkey
    );
    assert_eq!(
        state_content_after_handoff["offer"]["handoff_context"]["reason_text"],
        "Verify the two dependency risks before the agenda continues"
    );
    let second_offer = state_content_after_handoff["offer"]["offer_id"]
        .as_str()
        .expect("Agent 2 direct Offer")
        .to_string();
    let second_grant = ack_offer(&second_agent, meeting_id, &second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &second_agent,
        meeting_id,
        &second_grant,
        2,
        "Agent 2 verified both risks and returns the baton to the moderator.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let returned = state_content(&state);
    assert_eq!(returned["phase"], "moderator_idle");
    assert_eq!(returned["handoff_depth"], 0);

    // Round 2: Agent 3 owns a Grant while Agent 4 submits an Intent and a
    // Human requests the floor. The current Grant remains stable; after the
    // speech, Human priority wins over Agent 3's directed question to Agent 4.
    let third_intent = submit_intent(
        &third_agent,
        meeting_id,
        state_u64(&state, "speech_revision"),
        "Present the rollout risk",
    )
    .await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let third_offer = select_intent(&moderator, meeting_id, &third_intent, &state).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let third_grant = ack_offer(&third_agent, meeting_id, &third_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let fourth_intent = submit_intent(
        &fourth_agent,
        meeting_id,
        state_u64(&state, "speech_revision"),
        "Answer the rollout question after Human review",
    )
    .await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    request_human_floor(&human, meeting_id, 0).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let concurrent_writes = state_content(&state);
    assert_eq!(concurrent_writes["phase"], "granted");
    assert_eq!(concurrent_writes["grant"]["grant_id"], third_grant);
    assert!(concurrent_writes["pending_intents"]
        .as_array()
        .is_some_and(|intents| intents
            .iter()
            .any(|intent| intent["intent_id"] == fourth_intent.id.to_hex())));
    assert_eq!(
        concurrent_writes["human_queue"][0]["requester_pubkey"],
        human_pubkey
    );

    let third_speech = speak(
        &third_agent,
        meeting_id,
        &third_grant,
        3,
        "Agent 3 identifies a rollout question for Agent 4.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &fourth_agent_pubkey,
            handoff_type: MeetingV1HandoffType::Question,
            reason: "Explain whether the rollout remains reversible",
        }),
    )
    .await;
    speeches.push(third_speech.clone());
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let human_priority = state_content(&state);
    assert_eq!(human_priority["phase"], "offered");
    assert_eq!(human_priority["offer"]["target_pubkey"], human_pubkey);
    assert_eq!(
        human_priority["offer"]["allocation_source"],
        "human_request"
    );
    assert!(
        human_priority["unresolved_handoffs"]
            .as_array()
            .is_some_and(|handoffs| handoffs.iter().any(|handoff| {
                handoff["handoff_id"] == third_speech.id.to_hex()
                    && handoff["to_pubkey"] == fourth_agent_pubkey
                    && handoff["blocked_by"] == "human_request"
            })),
        "Human priority must preserve the interrupted directed question"
    );
    let human_offer = human_priority["offer"]["offer_id"]
        .as_str()
        .expect("Human priority Offer")
        .to_string();
    let human_grant = ack_offer(&human, meeting_id, &human_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &human,
        meeting_id,
        &human_grant,
        4,
        "The Human approves continuing once Agent 4 answers the question.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(state_content(&state)["phase"], "moderator_control");

    // The moderator resumes the exact handoff which Human priority interrupted.
    let fourth_offer =
        select_handoff(&moderator, meeting_id, &third_speech.id.to_hex(), 0, &state).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(
        state_content(&state)["offer"]["target_pubkey"],
        fourth_agent_pubkey
    );
    let fourth_grant = ack_offer(&fourth_agent, meeting_id, &fourth_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &fourth_agent,
        meeting_id,
        &fourth_grant,
        5,
        "Agent 4 confirms that the rollout can be reversed.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(state_content(&state)["phase"], "moderator_control");

    // Round 3: the moderator takes an ordinary self turn, then hands directly
    // to Agent 1 for that Agent's second contribution.
    let moderator_intent = submit_intent(
        &moderator,
        meeting_id,
        state_u64(&state, "speech_revision"),
        "Summarize the first half and direct the next check",
    )
    .await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let moderator_offer = select_intent(&moderator, meeting_id, &moderator_intent, &state).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let moderator_grant = ack_offer(&moderator, meeting_id, &moderator_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &moderator,
        meeting_id,
        &moderator_grant,
        6,
        "The moderator asks Agent 1 to turn the findings into an action.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &first_agent_pubkey,
            handoff_type: MeetingV1HandoffType::Question,
            reason: "Convert the verified findings into one concrete action",
        }),
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let first_agent_second_offer = state_content(&state)["offer"]["offer_id"]
        .as_str()
        .expect("Agent 1 second Offer")
        .to_string();
    let first_agent_second_grant =
        ack_offer(&first_agent, meeting_id, &first_agent_second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &first_agent,
        meeting_id,
        &first_agent_second_grant,
        7,
        "Agent 1 records the concrete rollback-validation action.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(state_content(&state)["phase"], "moderator_control");
    assert_eq!(state_content(&state)["handoff_depth"], 0);

    // Round 4: select Agent 4's still-pending Intent, then exercise a two-hop
    // direct chain through Agents 3 and 2 before control returns.
    let fourth_intent_offer = select_intent(&moderator, meeting_id, &fourth_intent, &state).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let fourth_intent_grant = ack_offer(&fourth_agent, meeting_id, &fourth_intent_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &fourth_agent,
        meeting_id,
        &fourth_intent_grant,
        8,
        "Agent 4 gives the rollout evidence to Agent 3 for a final risk check.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &third_agent_pubkey,
            handoff_type: MeetingV1HandoffType::Review,
            reason: "Perform the final risk check on the rollout evidence",
        }),
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let third_agent_second_offer = state_content(&state)["offer"]["offer_id"]
        .as_str()
        .expect("Agent 3 second Offer")
        .to_string();
    let third_agent_second_grant =
        ack_offer(&third_agent, meeting_id, &third_agent_second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &third_agent,
        meeting_id,
        &third_agent_second_grant,
        9,
        "Agent 3 finds no new risk and asks Agent 2 to confirm the dependency lock.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &second_agent_pubkey,
            handoff_type: MeetingV1HandoffType::Question,
            reason: "Confirm that the dependency lock is reproducible",
        }),
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let second_agent_second_offer = state_content(&state)["offer"]["offer_id"]
        .as_str()
        .expect("Agent 2 second Offer")
        .to_string();
    let second_agent_second_grant =
        ack_offer(&second_agent, meeting_id, &second_agent_second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &second_agent,
        meeting_id,
        &second_agent_second_grant,
        10,
        "Agent 2 confirms the dependency lock and returns control.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(state_content(&state)["phase"], "moderator_idle");
    assert_eq!(state_content(&state)["handoff_depth"], 0);

    // Round 5: the Human takes a second turn and hands directly to the
    // moderator. This gives all six identities exactly two canonical speeches.
    request_human_floor(&human, meeting_id, 2).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let human_second_offer = state_content(&state)["offer"]["offer_id"]
        .as_str()
        .expect("Human second Offer")
        .to_string();
    let human_second_grant = ack_offer(&human, meeting_id, &human_second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &human,
        meeting_id,
        &human_second_grant,
        11,
        "The Human asks the moderator to close with the agreed decision.",
        Some(MeetingV1DirectedHandoff {
            target_pubkey: &moderator_pubkey,
            handoff_type: MeetingV1HandoffType::Question,
            reason: "State the final decision and close the meeting",
        }),
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let moderator_second_offer = state_content(&state)["offer"]["offer_id"]
        .as_str()
        .expect("moderator second Offer")
        .to_string();
    let moderator_second_grant = ack_offer(&moderator, meeting_id, &moderator_second_offer).await;
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let speech = speak(
        &moderator,
        meeting_id,
        &moderator_second_grant,
        12,
        "The moderator records the decision and closes discussion.",
        None,
    )
    .await;
    speeches.push(speech);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    let final_active_state = state_content(&state);
    assert_eq!(final_active_state["phase"], "moderator_idle");
    assert_eq!(final_active_state["speech_revision"], 12);
    assert_eq!(final_active_state["handoff_depth"], 0);
    assert!(final_active_state["pending_intents"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert!(final_active_state["human_queue"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert!(final_active_state["unresolved_handoffs"]
        .as_array()
        .is_some_and(Vec::is_empty));

    let end = buzz_sdk::build_meeting_v1_end(MeetingV1EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
    })
    .expect("build six-participant Meeting V1 End")
    .sign_with_keys(&moderator)
    .expect("sign six-participant Meeting V1 End");
    let (status, body) = post_event(&moderator, &end).await;
    assert_accepted(status, &body);
    state = wait_for_next_state(&moderator, meeting_id, &state).await;
    assert_eq!(state_content(&state)["phase"], "ended");

    let post_end_intent = buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
        session_id: meeting_id,
        basis_speech_revision: 12,
        addressed_to: None,
        summary: "An ended Meeting must reject this Agent Intent",
    })
    .expect("build post-End Agent Intent")
    .sign_with_keys(&first_agent)
    .expect("sign post-End Agent Intent");
    let (status, body) = post_event(&first_agent, &post_end_intent).await;
    assert!(
        !status.is_success(),
        "an ended Meeting must reject a new Agent Intent: {body}"
    );
    let post_end_human_request =
        buzz_sdk::build_meeting_v1_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        })
        .expect("build post-End Human Request")
        .custom_created_at(Timestamp::from(Timestamp::now().as_secs() + 4))
        .sign_with_keys(&human)
        .expect("sign post-End Human Request");
    let (status, body) = post_event(&human, &post_end_human_request).await;
    assert!(
        !status.is_success(),
        "an ended Meeting must reject a new Human Request: {body}"
    );

    let mut canonical_states = query(
        &moderator,
        json!([{
            "kinds": [KIND_MEETING_STATE],
            "#h": [meeting_id.to_string()],
            "limit": 200
        }]),
    )
    .await;
    assert_state_history_is_canonical(&mut canonical_states, &speeches);
    assert_eq!(
        canonical_states.last().map(state_revision),
        Some(state_revision(&state))
    );
    let canonical_state_revisions = canonical_states
        .iter()
        .map(state_revision)
        .collect::<Vec<_>>();
    let expected_speech_ids = speeches
        .iter()
        .map(|speech| speech.id.to_hex())
        .collect::<Vec<_>>();
    let participant_keys = [
        &moderator,
        &human,
        &first_agent,
        &second_agent,
        &third_agent,
        &fourth_agent,
    ];
    for participant in participant_keys {
        let visible_speeches = query(
            participant,
            json!([{
                "kinds": [KIND_STREAM_MESSAGE],
                "#h": [meeting_id.to_string()],
                "limit": 50
            }]),
        )
        .await;
        assert_eq!(
            visible_speeches.len(),
            speeches.len(),
            "every participant must see every speech after End"
        );
        for expected_id in &expected_speech_ids {
            assert!(
                visible_speeches
                    .iter()
                    .any(|speech| speech["id"] == *expected_id),
                "participant {} is missing speech {expected_id}",
                participant.public_key()
            );
        }

        let mut visible_state_revisions = query(
            participant,
            json!([{
                "kinds": [KIND_MEETING_STATE],
                "#h": [meeting_id.to_string()],
                "limit": 200
            }]),
        )
        .await
        .iter()
        .map(state_revision)
        .collect::<Vec<_>>();
        visible_state_revisions.sort_unstable();
        assert_eq!(
            visible_state_revisions, canonical_state_revisions,
            "every participant must see the complete canonical State history"
        );
    }

    for expected_author in [
        &moderator_pubkey,
        &human_pubkey,
        &first_agent_pubkey,
        &second_agent_pubkey,
        &third_agent_pubkey,
        &fourth_agent_pubkey,
    ] {
        let authored_count = query(
            &moderator,
            json!([{
                "kinds": [KIND_STREAM_MESSAGE],
                "#h": [meeting_id.to_string()],
                "authors": [expected_author],
                "limit": 10
            }]),
        )
        .await
        .len();
        assert_eq!(
            authored_count, 2,
            "every Human and Agent identity must speak in multiple rounds"
        );
    }
}

#[tokio::test]
#[ignore = "requires a running Relay with BUZZ_REQUIRE_RELAY_MEMBERSHIP=true and BUZZ_MEETING_V1_CREATE_ENABLED=true, Postgres, and Redis"]
async fn relay_member_removal_disconnects_live_meeting_reader_and_blocks_reentry() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let owner = Keys::generate();
    let removed_participant = Keys::generate();
    seed_identity(&pool, community_id, &owner, "owner", None).await;
    seed_identity(&pool, community_id, &removed_participant, "member", None).await;

    let meeting_id = Uuid::new_v4();
    let owner_pubkey = owner.public_key().to_hex();
    let removed_pubkey = removed_participant.public_key().to_hex();
    let participants = [removed_pubkey.as_str()];
    let create = buzz_sdk::build_meeting_v1_create(MeetingV1CreateParams {
        session_id: meeting_id,
        title: "Relay Membership Revocation E2E",
        description: Some("live Meeting reader must be disconnected"),
        source_channel_id: None,
        author_pubkey: &owner_pubkey,
        moderator_pubkey: &owner_pubkey,
        participant_pubkeys: &participants,
    })
    .expect("build revocation Meeting V1 Create")
    .sign_with_keys(&owner)
    .expect("sign revocation Meeting V1 Create");
    let (status, body) = post_event(&owner, &create).await;
    assert_accepted(status, &body);
    wait_for_state(&owner, meeting_id, 1, Duration::from_secs(5)).await;

    // Prove the target has a live authenticated socket and can read the private
    // Meeting before its relay membership is removed.
    let mut removed_client = BuzzTestClient::connect(&relay_url(), &removed_participant)
        .await
        .expect("connect participant before relay membership removal");
    let subscription_id = format!("meeting-revocation-{}", Uuid::new_v4());
    let filter = Filter::new()
        .kind(Kind::Custom(KIND_MEETING_STATE as u16))
        .custom_tag(
            SingleLetterTag::lowercase(Alphabet::H),
            meeting_id.to_string(),
        );
    removed_client
        .subscribe(&subscription_id, vec![filter])
        .await
        .expect("subscribe to private Meeting State");
    let initial_states = removed_client
        .collect_until_eose(&subscription_id, Duration::from_secs(5))
        .await
        .expect("read private Meeting State before removal");
    assert!(
        !initial_states.is_empty(),
        "participant must read the private Meeting before revocation"
    );

    let mut owner_client = BuzzTestClient::connect(&relay_url(), &owner)
        .await
        .expect("connect relay owner");
    let remove = EventBuilder::new(Kind::Custom(9031), "")
        .tags([Tag::parse(["p", &removed_pubkey]).expect("removal p tag")])
        .sign_with_keys(&owner)
        .expect("sign relay member removal");
    let remove_id = remove.id.to_hex();
    let result = owner_client
        .send_event(remove)
        .await
        .expect("submit relay member removal");
    assert!(
        result.accepted,
        "relay member removal must be accepted: {}",
        result.message
    );

    // Connection-control is delivered on the priority queue before socket
    // cancellation, so the revoked member receives the reason-bearing OK false.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .unwrap_or(Duration::ZERO);
        assert!(
            !remaining.is_zero(),
            "timed out waiting for relay-membership disconnect"
        );
        match removed_client.recv_event(remaining).await {
            Ok(RelayMessage::Ok(ok)) if ok.event_id == remove_id => {
                assert!(!ok.accepted);
                assert!(
                    ok.message.contains("relay membership revoked"),
                    "disconnect reason must identify relay membership revocation: {}",
                    ok.message
                );
                break;
            }
            Ok(_) => {}
            Err(TestClientError::ConnectionClosed) => {
                panic!("revoked socket closed before its reason-bearing control frame")
            }
            Err(error) => panic!("failed while waiting for revocation disconnect: {error}"),
        }
    }

    match BuzzTestClient::connect(&relay_url(), &removed_participant).await {
        Err(TestClientError::AuthFailed(message)) => assert!(
            message.contains("not a relay member"),
            "re-entry must fail at relay membership auth: {message}"
        ),
        Err(error) => panic!("unexpected re-entry failure: {error}"),
        Ok(_) => panic!("removed relay member re-authenticated unexpectedly"),
    }

    let response = reqwest::Client::new()
        .post(format!("{}/query", relay_http_url()))
        .header("X-Pubkey", &removed_pubkey)
        .header("Content-Type", "application/json")
        .body(
            json!([{
                "kinds": [KIND_MEETING_STATE],
                "#h": [meeting_id.to_string()],
                "limit": 20
            }])
            .to_string(),
        )
        .send()
        .await
        .expect("query Meeting as removed relay member");
    assert_eq!(
        response.status(),
        reqwest::StatusCode::FORBIDDEN,
        "removed relay member must not re-enter through the HTTP read surface"
    );

    owner_client.disconnect().await.expect("disconnect owner");
}
