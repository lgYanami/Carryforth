//! End-to-end proof for the Meeting V2 Board-gated lifecycle.
//!
//! Requires a disposable Relay database and a Relay started with
//! `BUZZ_MEETING_V2_CREATE_ENABLED=true`.

use std::process::Command;

use buzz_core::kind::{
    KIND_MEETING_BOARD, KIND_MEETING_BOARD_COMMAND, KIND_MEETING_CREATE, KIND_MEETING_END,
    KIND_MEETING_FLOOR_CLAIM, KIND_MEETING_FLOOR_SIGNAL, KIND_MEETING_SPEECH_INTENT,
    KIND_MEETING_STATE, KIND_STREAM_MESSAGE,
};
use buzz_sdk::{
    MeetingV1HumanFloorRequestParams, MeetingV1IntentSubmitParams, MeetingV1ModeratorSelectParams,
    MeetingV1OfferAckParams, MeetingV1Selection, MeetingV1SpeechParams, MeetingV2BoardActionParams,
    MeetingV2EndOutcome, MeetingV2EndParams,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, ToBech32};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct TerminalProjection {
    schema_version: i32,
    floor_policy_version: String,
    host_pubkey: Vec<u8>,
    moderator_pubkey: Vec<u8>,
    status: String,
    terminal_outcome: String,
    terminal_reason_code: Option<String>,
    runtime_phase: String,
    control_epoch: i64,
    board_outcome: String,
    archived: bool,
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

fn run_real_buzz(keys: &Keys, args: &[&str]) -> Value {
    let buzz = std::env::var("MEETING_E2E_BUZZ_BIN")
        .expect("MEETING_E2E_BUZZ_BIN must point at the real buzz CLI");
    let output = Command::new(buzz)
        .args(["--format", "compact"])
        .args(args)
        .env("BUZZ_RELAY_URL", relay_http_url())
        .env(
            "BUZZ_PRIVATE_KEY",
            keys.secret_key().to_bech32().expect("encode E2E nsec"),
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

fn response_payload(body: &str) -> Value {
    let response: Value = serde_json::from_str(body).expect("parse Relay write response");
    response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str(payload).ok())
        .expect("parse Meeting V2 response payload")
}

fn state_content(state: &Value) -> Value {
    serde_json::from_str(
        state["content"]
            .as_str()
            .expect("Meeting V2 State content string"),
    )
    .expect("Meeting V2 State JSON")
}

fn state_u64(state: &Value, field: &str) -> u64 {
    state_content(state)[field]
        .as_u64()
        .unwrap_or_else(|| panic!("Meeting V2 State is missing {field}"))
}

async fn latest_state(keys: &Keys, meeting_id: Uuid) -> Value {
    query(
        keys,
        json!([{
            "kinds": [KIND_MEETING_STATE],
            "#h": [meeting_id.to_string()],
            "limit": 100
        }]),
    )
    .await
    .into_iter()
    .max_by_key(|state| {
        tag_value(state, "state-revision")
            .and_then(|revision| revision.parse::<u64>().ok())
            .unwrap_or(0)
    })
    .expect("current Meeting V2 State")
}

async fn create_v2_meeting(host: &Keys, participant: &Keys, title: &str) -> (Uuid, Event) {
    let meeting_id = Uuid::new_v4();
    let host_hex = host.public_key().to_hex();
    let participant_hex = participant.public_key().to_hex();
    let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
        session_id: meeting_id,
        title,
        description: Some("Meeting V2 stage-two acceptance"),
        source_channel_id: None,
        author_pubkey: &host_hex,
        participant_pubkeys: &[participant_hex.as_str()],
        initial_board: "# Goal\nProve the requested Meeting V2 lifecycle invariant.",
    })
    .expect("build Meeting V2 acceptance Create")
    .sign_with_keys(host)
    .expect("sign Meeting V2 acceptance Create");
    let (status, body) = post_event(host, &create).await;
    assert_accepted(status, &body);
    (meeting_id, create)
}

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_board_gates_each_floor_cycle_and_normal_close() {
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
    let create_payload: Value = response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str(payload).ok())
        .expect("parse Meeting V2 Create response payload");
    assert_eq!(create_payload["schema_version"], 3);
    assert_eq!(
        create_payload["floor_policy_version"],
        buzz_sdk::MEETING_V2_POLICY
    );
    assert_eq!(create_payload["moderator"], host_hex);
    let board_event_id = create_payload["board_event_id"]
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
    assert_rejected(status, &body, "schema does not match the persisted Session");

    let initial_state = latest_state(&host, meeting_id).await;
    let initial_content = state_content(&initial_state);
    assert_eq!(initial_content["phase"], "moderator_idle");
    assert_eq!(initial_content["board_control"]["phase"], "board_pending");
    assert_eq!(initial_content["board_control"]["control_epoch"], 1);
    assert_eq!(initial_content["board_control"]["board_window"], 1);

    let intent = buzz_sdk::build_meeting_v2_intent_submit(MeetingV1IntentSubmitParams {
        session_id: meeting_id,
        basis_speech_revision: 0,
        addressed_to: None,
        summary: "I can provide the release evidence.",
    })
    .expect("build V2 Intent")
    .sign_with_keys(&participant)
    .expect("sign V2 Intent");
    let (status, body) = post_event(&participant, &intent).await;
    assert_accepted(status, &body);
    assert_eq!(
        response_payload(&body)["canonical_object_id"],
        intent.id.to_hex()
    );
    let intent_state = latest_state(&host, meeting_id).await;
    let intent_content = state_content(&intent_state);
    assert_eq!(intent_content["phase"], "moderator_idle");
    assert_eq!(intent_content["intent_revision"], 1);
    assert_eq!(intent_content["board_control"]["phase"], "board_pending");

    let participant_board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: None,
    })
    .expect("build participant Board command")
    .sign_with_keys(&participant)
    .expect("sign participant Board command");
    let (status, body) = post_event(&participant, &participant_board).await;
    assert_rejected(
        status,
        &body,
        "not authorized for this moderated Meeting operation",
    );

    let stale_board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 2,
        board_window: 1,
        board: None,
    })
    .expect("build stale Board command")
    .sign_with_keys(&host)
    .expect("sign stale Board command");
    let (status, body) = post_event(&host, &stale_board).await;
    assert_rejected(status, &body, "stale_control_epoch");

    let updated_board_body =
        format!("{board_body}\n\n## Evidence\n- API compatibility is verified.");
    let board_update = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: Some(&updated_board_body),
    })
    .expect("build Board update")
    .sign_with_keys(&host)
    .expect("sign Board update");
    let (status, body) = post_event(&host, &board_update).await;
    assert_accepted(status, &body);
    let board_update_response = response_payload(&body);
    assert_eq!(board_update_response["duplicate"], false);
    assert_eq!(board_update_response["outcome"], "accepted");
    let updated_board_event_id = board_update_response["board_event_id"]
        .as_str()
        .expect("updated Board event id")
        .to_string();
    assert_ne!(updated_board_event_id, board_event_id);

    let (status, body) = post_event(&host, &board_update).await;
    assert_accepted(status, &body);
    let duplicate = response_payload(&body);
    assert_eq!(duplicate["duplicate"], true);
    assert_eq!(duplicate["outcome"], "updated");
    assert_eq!(duplicate["board_event_id"], updated_board_event_id);

    let updated_boards = query(
        &participant,
        json!([{
            "kinds": [KIND_MEETING_BOARD],
            "#h": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert_eq!(updated_boards.len(), 1);
    assert_eq!(updated_boards[0]["id"], updated_board_event_id);
    let updated_board: Event =
        serde_json::from_value(updated_boards[0].clone()).expect("decode updated Board");
    assert_eq!(
        buzz_sdk::parse_meeting_v2_board_content(&updated_board.content)
            .expect("parse updated Board")
            .body,
        updated_board_body
    );

    let selection_state = latest_state(&host, meeting_id).await;
    let selection_content = state_content(&selection_state);
    assert_eq!(selection_content["phase"], "moderator_control");
    assert_eq!(selection_content["board_control"]["phase"], "floor_ready");
    assert_eq!(
        selection_content["board_control"]["board_outcome"],
        "updated"
    );
    let select = buzz_sdk::build_meeting_v2_moderator_select(MeetingV1ModeratorSelectParams {
        session_id: meeting_id,
        selection: MeetingV1Selection::Intent {
            intent_id: &intent.id.to_hex(),
        },
        expected_control_epoch: state_u64(&selection_state, "control_epoch"),
        expected_decision_epoch: state_u64(&selection_state, "decision_epoch"),
        expected_intent_revision: state_u64(&selection_state, "intent_revision"),
        expected_speech_revision: state_u64(&selection_state, "speech_revision"),
        selection_reason: Some("share the release evidence"),
        deferrals: &[],
        attempt_id: None,
        expected_source_event_id: None,
    })
    .expect("build V2 moderator Select")
    .sign_with_keys(&host)
    .expect("sign V2 moderator Select");
    let (status, body) = post_event(&host, &select).await;
    assert_accepted(status, &body);
    let offer_id = response_payload(&body)["canonical_object_id"]
        .as_str()
        .expect("V2 Offer id")
        .to_string();

    let ack = buzz_sdk::build_meeting_v2_offer_ack(MeetingV1OfferAckParams {
        session_id: meeting_id,
        offer_id: &offer_id,
    })
    .expect("build V2 Offer ACK")
    .sign_with_keys(&participant)
    .expect("sign V2 Offer ACK");
    let (status, body) = post_event(&participant, &ack).await;
    assert_accepted(status, &body);
    let grant_id = response_payload(&body)["canonical_object_id"]
        .as_str()
        .expect("V2 Grant id")
        .to_string();

    let speech = buzz_sdk::build_meeting_v2_speech(MeetingV1SpeechParams {
        session_id: meeting_id,
        grant_id: &grant_id,
        speech_revision: 1,
        content: "The compatibility evidence supports shipping the API boundary first.",
        mentions: &[],
        handoff: None,
    })
    .expect("build V2 speech")
    .sign_with_keys(&participant)
    .expect("sign V2 speech");
    let (status, body) = post_event(&participant, &speech).await;
    assert_accepted(status, &body);
    assert_eq!(
        response_payload(&body)["canonical_object_id"],
        speech.id.to_hex()
    );

    let returned_state = latest_state(&host, meeting_id).await;
    let returned_content = state_content(&returned_state);
    assert_eq!(returned_content["phase"], "moderator_idle");
    assert_eq!(returned_content["speech_revision"], 1);
    assert_eq!(returned_content["control_epoch"], 2);
    assert_eq!(returned_content["board_control"]["phase"], "board_pending");
    assert_eq!(returned_content["board_control"]["control_epoch"], 2);
    assert_eq!(returned_content["board_control"]["board_window"], 2);

    let close = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
    })
    .expect("build V2 close")
    .sign_with_keys(&host)
    .expect("sign V2 close");
    let (status, body) = post_event(&host, &close).await;
    assert_rejected(status, &body, "explicit final Board result");

    let final_board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 2,
        board_window: 2,
        board: None,
    })
    .expect("build final unchanged Board result")
    .sign_with_keys(&host)
    .expect("sign final unchanged Board result");
    let (status, body) = post_event(&host, &final_board).await;
    assert_accepted(status, &body);
    assert_eq!(response_payload(&body)["outcome"], "accepted");

    let final_state = latest_state(&host, meeting_id).await;
    let final_content = state_content(&final_state);
    assert_eq!(final_content["phase"], "moderator_idle");
    assert_eq!(final_content["board_control"]["phase"], "floor_ready");
    assert_eq!(final_content["board_control"]["board_outcome"], "unchanged");

    let (status, body) = post_event(&host, &close).await;
    assert_accepted(status, &body);
    let close_response = response_payload(&body);
    assert_eq!(close_response["status"], "ended");
    assert_eq!(close_response["terminal_outcome"], "closed");

    let projection: TerminalProjection = sqlx::query_as(
        "SELECT s.schema_version, s.floor_policy_version, s.host_pubkey, \
                s.moderator_pubkey, s.status, s.terminal_outcome, \
                s.terminal_reason_code, b.runtime_phase, b.control_epoch, \
                b.board_outcome, c.archived_at IS NOT NULL AS archived \
         FROM meeting_sessions s \
         JOIN meeting_v2_bootstrap_state b \
           ON b.community_id = s.community_id AND b.session_id = s.session_id \
         JOIN channels c ON c.community_id = s.community_id AND c.id = s.session_id \
         WHERE s.community_id = $1 AND s.session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read terminal Meeting V2 projection");
    assert_eq!(projection.schema_version, 3);
    assert_eq!(projection.floor_policy_version, buzz_sdk::MEETING_V2_POLICY);
    assert_eq!(projection.host_pubkey, host.public_key().to_bytes());
    assert_eq!(projection.moderator_pubkey, host.public_key().to_bytes());
    assert_eq!(projection.status, "ended");
    assert_eq!(projection.terminal_outcome, "closed");
    assert_eq!(projection.terminal_reason_code, None);
    assert_eq!(projection.runtime_phase, "ended");
    assert_eq!(projection.control_epoch, 2);
    assert_eq!(projection.board_outcome, "unchanged");
    assert!(projection.archived);
    let persisted_counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT \
             count(*) FILTER (WHERE e.kind = $3), \
             count(*) FILTER (WHERE e.kind = $4), \
             count(*) FILTER (WHERE e.kind = $5), \
             count(*) FILTER (WHERE e.kind = $6), \
             count(*) FILTER (WHERE e.kind = $7), \
             count(*) FILTER (WHERE e.kind = $8) \
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
    .bind(KIND_MEETING_BOARD_COMMAND as i32)
    .fetch_one(&pool)
    .await
    .expect("count accepted and rejected Meeting V2 events");
    assert_eq!(persisted_counts, (1, 1, 1, 0, 0, 0));
    let intent_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events \
         WHERE community_id = $1 AND channel_id = $2 AND kind = $3",
    )
    .bind(community_id)
    .bind(meeting_id)
    .bind(KIND_MEETING_SPEECH_INTENT as i32)
    .fetch_one(&pool)
    .await
    .expect("count accepted V2 and rejected V1 intents");
    assert_eq!(intent_count, 1);
    let board_receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM meeting_v2_board_command_receipts \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("count private Board command receipts");
    assert_eq!(board_receipts, 3);
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

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_board_timeout_starts_a_fresh_floor_decision_window() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;
    let (meeting_id, create) =
        create_v2_meeting(&host, &participant, "Meeting V2 independent Board timeout").await;

    let intent = buzz_sdk::build_meeting_v2_intent_submit(MeetingV1IntentSubmitParams {
        session_id: meeting_id,
        basis_speech_revision: 0,
        addressed_to: None,
        summary: "Make this Intent ready only after Board recovery.",
    })
    .expect("build timeout-test V2 Intent")
    .sign_with_keys(&participant)
    .expect("sign timeout-test V2 Intent");
    let (status, body) = post_event(&participant, &intent).await;
    assert_accepted(status, &body);

    sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET board_started_at = clock_timestamp() - interval '2 seconds', \
             board_deadline_at = clock_timestamp() - interval '1 second' \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .execute(&pool)
    .await
    .expect("force due Meeting V2 Board deadline");

    let late_board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: None,
    })
    .expect("build late Board result")
    .sign_with_keys(&host)
    .expect("sign late Board result");
    let (status, body) = post_event(&host, &late_board).await;
    assert!(
        !status.is_success()
            && (body.contains("board_window_timed_out") || body.contains("board_window_inactive")),
        "expected a late Board rejection after timeout recovery, got HTTP {status}: {body}"
    );

    let recovered = latest_state(&host, meeting_id).await;
    let recovered_content = state_content(&recovered);
    assert_eq!(recovered_content["phase"], "moderator_control");
    assert_eq!(recovered_content["board_control"]["phase"], "floor_ready");
    assert_eq!(
        recovered_content["board_control"]["board_outcome"],
        "timed_out"
    );
    assert_eq!(
        recovered_content["moderator_decision_deadline_ms"],
        recovered_content["next_action_at_ms"]
    );

    let timing: (i64, i64, bool) = sqlx::query_as(
        "SELECT \
             round(extract(epoch FROM (state.moderator_decision_deadline \
                 - state.moderator_decision_started_at)) * 1000)::bigint, \
             baton.moderator_decision_ms, \
             state.moderator_decision_started_at >= runtime.board_completed_at \
         FROM meeting_baton_state state \
         JOIN meeting_baton_config baton \
           ON baton.community_id = state.community_id \
          AND baton.session_id = state.session_id \
         JOIN meeting_v2_bootstrap_state runtime \
           ON runtime.community_id = state.community_id \
          AND runtime.session_id = state.session_id \
         WHERE state.community_id = $1 AND state.session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read independent Board and floor timing windows");
    assert_eq!(timing.0, timing.1);
    assert!(timing.2);
    let timeout_transitions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM meeting_baton_state_history \
         WHERE community_id = $1 AND session_id = $2 \
           AND transition_primary_type = 'board_timed_out'",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("count canonical Board timeout transitions");
    assert_eq!(timeout_transitions, 1);

    let close = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
    })
    .expect("build close after Board timeout")
    .sign_with_keys(&host)
    .expect("sign close after Board timeout");
    let (status, body) = post_event(&host, &close).await;
    assert_rejected(status, &body, "explicit final Board result");
}

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_late_close_recovers_floor_deadline_before_end() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;

    let (meeting_id, create) =
        create_v2_meeting(&host, &participant, "Meeting V2 late close fencing").await;
    let intent = buzz_sdk::build_meeting_v2_intent_submit(MeetingV1IntentSubmitParams {
        session_id: meeting_id,
        basis_speech_revision: 0,
        addressed_to: None,
        summary: "Fallback must run before a late close.",
    })
    .expect("build late-close V2 Intent")
    .sign_with_keys(&participant)
    .expect("sign late-close V2 Intent");
    let (status, body) = post_event(&participant, &intent).await;
    assert_accepted(status, &body);

    let board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: None,
    })
    .expect("build late-close Board result")
    .sign_with_keys(&host)
    .expect("sign late-close Board result");
    let (status, body) = post_event(&host, &board).await;
    assert_accepted(status, &body);
    assert_eq!(
        state_content(&latest_state(&host, meeting_id).await)["phase"],
        "moderator_control"
    );

    sqlx::query(
        "UPDATE meeting_baton_state \
         SET moderator_decision_started_at = clock_timestamp() - interval '2 seconds', \
             moderator_decision_deadline = clock_timestamp() - interval '1 second', \
             next_action_at = clock_timestamp() - interval '1 second' \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .execute(&pool)
    .await
    .expect("force due Meeting V2 Floor deadline");

    let close = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
    })
    .expect("build late V2 close")
    .sign_with_keys(&host)
    .expect("sign late V2 close");
    let (status, body) = post_event(&host, &close).await;
    assert_rejected(status, &body, "moderator control");

    let recovered = latest_state(&host, meeting_id).await;
    let recovered_content = state_content(&recovered);
    assert_eq!(recovered_content["phase"], "offered");
    assert_eq!(
        recovered_content["transition"]["primary_type"],
        "moderator_fallback"
    );
    let meeting_status: String = sqlx::query_scalar(
        "SELECT status FROM meeting_sessions WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read late-close Meeting V2 status");
    assert_eq!(meeting_status, "active");
}

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_human_floor_request_preempts_board_maintenance() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;
    let (meeting_id, _) =
        create_v2_meeting(&host, &participant, "Meeting V2 Human Board preemption").await;

    let request =
        buzz_sdk::build_meeting_v2_human_floor_request(MeetingV1HumanFloorRequestParams {
            session_id: meeting_id,
        })
        .expect("build V2 Human Floor Request")
        .sign_with_keys(&participant)
        .expect("sign V2 Human Floor Request");
    let (status, body) = post_event(&participant, &request).await;
    assert_accepted(status, &body);

    let offered = latest_state(&participant, meeting_id).await;
    let offered_content = state_content(&offered);
    assert_eq!(offered_content["phase"], "offered");
    assert_eq!(
        offered_content["offer"]["target_pubkey"],
        participant.public_key().to_hex()
    );
    assert_eq!(offered_content["board_control"]["phase"], "floor_ready");
    assert_eq!(
        offered_content["board_control"]["board_outcome"],
        "preempted"
    );
    assert_eq!(offered_content["board_control"]["board_window"], 1);

    let stale_board = buzz_sdk::build_meeting_v2_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: None,
    })
    .expect("build Board result after Human preemption")
    .sign_with_keys(&host)
    .expect("sign Board result after Human preemption");
    let (status, body) = post_event(&host, &stale_board).await;
    assert_rejected(status, &body, "board_window_inactive");
}

#[tokio::test]
#[ignore = "requires a disposable Relay with Meeting V2 creation enabled"]
async fn meeting_v2_operator_abort_is_distinct_from_normal_close() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    let operator = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;
    seed_identity(&pool, community_id, &operator, "admin").await;

    let meeting_id = Uuid::new_v4();
    let host_hex = host.public_key().to_hex();
    let participant_hex = participant.public_key().to_hex();
    let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
        session_id: meeting_id,
        title: "Meeting V2 abnormal termination",
        description: Some("operator abort proof"),
        source_channel_id: None,
        author_pubkey: &host_hex,
        participant_pubkeys: &[participant_hex.as_str()],
        initial_board: "# Goal\nDetermine whether required evidence exists.",
    })
    .expect("build abort-test Create")
    .sign_with_keys(&host)
    .expect("sign abort-test Create");
    let (status, body) = post_event(&host, &create).await;
    assert_accepted(status, &body);

    let participant_abort = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Aborted,
        reason_code: Some("goal_unreachable"),
        reason: Some("The required evidence is unavailable."),
    })
    .expect("build unauthorized participant abort")
    .sign_with_keys(&participant)
    .expect("sign unauthorized participant abort");
    let (status, body) = post_event(&participant, &participant_abort).await;
    assert_rejected(status, &body, "moderator or a Community operator");

    let operator_abort = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Aborted,
        reason_code: Some("goal_unreachable"),
        reason: Some("The required evidence is unavailable."),
    })
    .expect("build operator abort")
    .sign_with_keys(&operator)
    .expect("sign operator abort");
    let (status, body) = post_event(&operator, &operator_abort).await;
    assert_accepted(status, &body);
    let payload = response_payload(&body);
    assert_eq!(payload["status"], "ended");
    assert_eq!(payload["terminal_outcome"], "aborted");

    let (status, body) = post_event(&operator, &operator_abort).await;
    assert_accepted(status, &body);
    let exact_replay = response_payload(&body);
    assert_eq!(exact_replay["already_ended"], true);
    assert_eq!(exact_replay["terminal_outcome"], "aborted");

    let close_after_abort = buzz_sdk::build_meeting_v2_end(MeetingV2EndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
    })
    .expect("build close after operator abort")
    .sign_with_keys(&host)
    .expect("sign close after operator abort");
    let (status, body) = post_event(&host, &close_after_abort).await;
    assert_accepted(status, &body);
    let already_ended = response_payload(&body);
    assert_eq!(already_ended["already_ended"], true);
    assert_eq!(already_ended["terminal_outcome"], "aborted");

    let terminal: (String, String, String, String, String, bool) = sqlx::query_as(
        "SELECT s.status, s.terminal_outcome, s.terminal_reason_code, \
                runtime.runtime_phase, runtime.board_outcome, \
                channel.archived_at IS NOT NULL \
         FROM meeting_sessions s \
         JOIN meeting_v2_bootstrap_state runtime \
           ON runtime.community_id = s.community_id \
          AND runtime.session_id = s.session_id \
         JOIN channels channel \
           ON channel.community_id = s.community_id AND channel.id = s.session_id \
         WHERE s.community_id = $1 AND s.session_id = $2",
    )
    .bind(community_id)
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read aborted Meeting V2 projection");
    assert_eq!(
        terminal,
        (
            "ended".into(),
            "aborted".into(),
            "goal_unreachable".into(),
            "ended".into(),
            "preempted".into(),
            true,
        )
    );

    let end_events = query(
        &host,
        json!([{
            "kinds": [KIND_MEETING_END],
            "#h": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    assert_eq!(end_events.len(), 1);
    assert_eq!(end_events[0]["id"], operator_abort.id.to_hex());
    assert_eq!(
        end_events[0]["content"],
        "The required evidence is unavailable."
    );
    assert_eq!(tag_value(&end_events[0], "outcome"), Some("aborted"));
    assert_eq!(
        tag_value(&end_events[0], "reason-code"),
        Some("goal_unreachable")
    );
}

#[tokio::test]
#[ignore = "requires a disposable Relay and MEETING_E2E_BUZZ_BIN"]
async fn meeting_v2_real_cli_completes_a_multi_identity_lifecycle() {
    let pool = test_pool().await;
    let community_id = ensure_community(&pool).await;
    let host = Keys::generate();
    let participant = Keys::generate();
    seed_identity(&pool, community_id, &host, "owner").await;
    seed_identity(&pool, community_id, &participant, "member").await;
    let host_hex = host.public_key().to_hex();
    let participant_hex = participant.public_key().to_hex();

    let create = run_real_buzz(
        &host,
        &[
            "meetings",
            "create",
            "--policy",
            "moderated-board-v1",
            "--title",
            "Meeting V2 real CLI lifecycle",
            "--board",
            "# Goal\nComplete one real CLI-controlled meeting.",
            "--participant",
            &participant_hex,
        ],
    );
    let meeting_id = create["meeting_id"]
        .as_str()
        .expect("CLI Create meeting_id")
        .to_string();

    let intent = run_real_buzz(
        &participant,
        &[
            "meetings",
            "intents",
            "submit",
            "--meeting",
            &meeting_id,
            "--summary",
            "Provide the CLI lifecycle evidence.",
        ],
    );
    let intent_id = intent["intent_id"]
        .as_str()
        .expect("CLI Intent id")
        .to_string();

    let board_ready = run_real_buzz(
        &host,
        &["meetings", "board", "unchanged", "--meeting", &meeting_id],
    );
    assert_eq!(board_ready["accepted"], true);
    let selected = run_real_buzz(
        &host,
        &[
            "meetings",
            "moderator",
            "select",
            "--meeting",
            &meeting_id,
            "--intent",
            &intent_id,
        ],
    );
    assert_eq!(selected["accepted"], true);
    let ack = run_real_buzz(
        &participant,
        &["meetings", "offer", "ack", "--meeting", &meeting_id],
    );
    assert_eq!(ack["accepted"], true);
    let speech = run_real_buzz(
        &participant,
        &[
            "meetings",
            "say",
            "--meeting",
            &meeting_id,
            "--content",
            "The participant hands the next turn directly to the moderator.",
            "--handoff-to",
            &host_hex,
            "--handoff-type",
            "review",
            "--handoff-reason",
            "Review the lifecycle evidence before closing.",
        ],
    );
    assert!(speech["speech_event_id"].as_str().is_some());

    let handed_off = run_real_buzz(
        &participant,
        &["meetings", "floor", "status", "--meeting", &meeting_id],
    );
    assert_eq!(handed_off["phase"], "offered");
    assert_eq!(
        handed_off["content"]["board_control"]["phase"],
        "floor_ready"
    );
    assert_eq!(handed_off["content"]["board_control"]["board_window"], 1);
    assert_eq!(handed_off["content"]["offer"]["target_pubkey"], host_hex);

    let moderator_ack = run_real_buzz(
        &host,
        &["meetings", "offer", "ack", "--meeting", &meeting_id],
    );
    assert_eq!(moderator_ack["accepted"], true);
    let moderator_speech = run_real_buzz(
        &host,
        &[
            "meetings",
            "say",
            "--meeting",
            &meeting_id,
            "--content",
            "The moderator reviewed the evidence and resumes Board maintenance.",
        ],
    );
    assert!(moderator_speech["speech_event_id"].as_str().is_some());
    let board_pending = run_real_buzz(
        &host,
        &["meetings", "floor", "status", "--meeting", &meeting_id],
    );
    assert_eq!(board_pending["phase"], "moderator_idle");
    assert_eq!(
        board_pending["content"]["board_control"]["phase"],
        "board_pending"
    );
    assert_eq!(board_pending["content"]["board_control"]["board_window"], 2);

    let board_body = "# Goal\nComplete one real CLI-controlled meeting.\n\n## Conclusion\n- The lifecycle succeeded.";
    let board_path = std::env::temp_dir().join(format!(
        "buzz-meeting-v2-board-{}.md",
        Uuid::new_v4().simple()
    ));
    std::fs::write(&board_path, board_body).expect("write CLI Board fixture");
    let board_path_text = board_path.to_string_lossy().into_owned();
    let updated = run_real_buzz(
        &host,
        &[
            "meetings",
            "board",
            "update",
            "--meeting",
            &meeting_id,
            "--board",
            &board_path_text,
        ],
    );
    let _ = std::fs::remove_file(&board_path);
    assert_eq!(updated["accepted"], true);
    let board = run_real_buzz(
        &participant,
        &["meetings", "board", "get", "--meeting", &meeting_id],
    );
    assert_eq!(board["body"], board_body);

    let closed = run_real_buzz(&host, &["meetings", "close", "--meeting", &meeting_id]);
    assert_eq!(closed["status"], "ended");
    assert_eq!(closed["terminal_outcome"], "closed");
    let shown = run_real_buzz(
        &participant,
        &["meetings", "show", "--meeting", &meeting_id],
    );
    assert_eq!(shown["status"], "ended");
    assert_eq!(shown["terminal_outcome"], "closed");
    assert!(shown["terminal_reason_code"].is_null());
    let terminal_board = run_real_buzz(
        &participant,
        &["meetings", "board", "get", "--meeting", &meeting_id],
    );
    assert_eq!(terminal_board["body"], board_body);
}
