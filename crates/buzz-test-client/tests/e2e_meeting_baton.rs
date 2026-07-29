//! End-to-end proof for the Meeting V1 moderated-baton protocol.
//!
//! Requires a running Relay with Meeting V1 enabled, Postgres, and Redis. The
//! timeout branch advances its own persisted deadline, so it does not depend on
//! short production timing configuration.

use std::time::Duration;

use buzz_core::kind::{KIND_MEETING_END, KIND_MEETING_OFFER_RESPONSE, KIND_MEETING_STATE};
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
    })
    .expect("build Meeting V1 Select")
    .sign_with_keys(moderator)
    .expect("sign Meeting V1 Select");
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
