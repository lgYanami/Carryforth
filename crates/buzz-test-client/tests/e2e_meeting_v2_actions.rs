//! Deterministic eight-Agent proof for direct Meeting V2 action finalization.
//!
//! Requires a disposable Relay database and a Relay started with both
//! `BUZZ_MEETING_V2_CREATE_ENABLED=true` and
//! `BUZZ_MEETING_V2_DIRECT_ACTIONS_CREATE_ENABLED=true`.

use buzz_core::kind::{KIND_MEETING_BOARD, KIND_MEETING_END, KIND_MEETING_STATE};
use buzz_core::CommunityId;
use buzz_sdk::{
    MeetingSummaryMutation, MeetingSummaryUpdateParams, MeetingV2ActionBeginParams,
    MeetingV2ActionBlockParams, MeetingV2ActionCommandParams, MeetingV2ActionRunFence,
    MeetingV2ActionsEndFence, MeetingV2ActionsEndParams, MeetingV2BoardActionParams,
    MeetingV2CreateParams, MeetingV2EndOutcome,
};
use nostr::{Event, EventBuilder, Keys, Kind, Tag};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn relay_url() -> String {
    std::env::var("RELAY_URL").unwrap_or_else(|_| "ws://localhost:3000".to_owned())
}

fn relay_http_url() -> String {
    relay_url()
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
        .trim_end_matches('/')
        .to_owned()
}

async fn test_pool() -> PgPool {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_owned());
    PgPool::connect(&database_url)
        .await
        .expect("connect to Meeting direct-action E2E database")
}

async fn ensure_community(pool: &PgPool) -> CommunityId {
    let host = relay_http_url()
        .split_once("://")
        .map_or_else(relay_http_url, |(_, authority)| authority.to_owned());
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO communities (id, host) VALUES ($1, $2) \
         ON CONFLICT (lower(host)) DO NOTHING",
    )
    .bind(id)
    .bind(&host)
    .execute(pool)
    .await
    .expect("ensure Meeting direct-action E2E Community");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("resolve Meeting direct-action E2E Community");
    CommunityId::from_uuid(id)
}

async fn seed_user(pool: &PgPool, community: CommunityId, keys: &Keys) {
    sqlx::query(
        "INSERT INTO users (community_id, pubkey, channel_add_policy) \
         VALUES ($1, $2, 'anyone') \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET deactivated_at = NULL",
    )
    .bind(community.as_uuid())
    .bind(keys.public_key().to_bytes().as_slice())
    .execute(pool)
    .await
    .expect("seed Meeting direct-action E2E user");
}

async fn seed_agent(
    pool: &PgPool,
    db: &buzz_db::Db,
    community: CommunityId,
    keys: &Keys,
    owner: &Keys,
    role: &str,
) {
    seed_user(pool, community, keys).await;
    db.add_relay_member(
        community,
        &keys.public_key().to_hex(),
        role,
        Some(&owner.public_key().to_hex()),
    )
    .await
    .expect("add Meeting direct-action E2E Agent member");
    sqlx::query(
        "UPDATE users \
         SET agent_owner_pubkey = $3, capabilities = jsonb_build_array($4::text) \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(keys.public_key().to_bytes().as_slice())
    .bind(owner.public_key().to_bytes().as_slice())
    .bind(buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY)
    .execute(pool)
    .await
    .expect("advertise Meeting direct-action E2E Agent capability");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize Meeting direct-action E2E event"))
        .send()
        .await
        .expect("submit Meeting direct-action E2E event");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read Meeting direct-action E2E response");
    (status, body)
}

fn assert_accepted(status: reqwest::StatusCode, body: &str) -> Value {
    let response: Value = serde_json::from_str(body).expect("parse Relay response");
    assert!(
        status.is_success() && response["accepted"].as_bool() == Some(true),
        "expected accepted event, got HTTP {status}: {body}"
    );
    response
}

fn assert_rejected(status: reqwest::StatusCode, body: &str) {
    assert!(
        !status.is_success(),
        "expected rejected event, got HTTP {status}: {body}"
    );
}

fn response_payload(body: &str) -> Value {
    let response: Value = serde_json::from_str(body).expect("parse Relay write response");
    response["message"]
        .as_str()
        .and_then(|message| message.strip_prefix("response:"))
        .and_then(|payload| serde_json::from_str(payload).ok())
        .unwrap_or_else(|| panic!("parse typed Relay response payload: {body}"))
}

async fn query(keys: &Keys, filters: Value) -> Vec<Value> {
    let response = reqwest::Client::new()
        .post(format!("{}/query", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(filters.to_string())
        .send()
        .await
        .expect("query Meeting direct-action E2E events");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read Meeting direct-action E2E query response");
    assert!(
        status.is_success(),
        "query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse Meeting direct-action E2E query")
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
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0)
    })
    .expect("latest Meeting direct-action State")
}

#[tokio::test]
#[ignore = "requires a disposable Relay with direct-action Meeting V2 creation enabled"]
async fn eight_agents_complete_one_direct_action_lifecycle() {
    let pool = test_pool().await;
    let community = ensure_community(&pool).await;
    let db = buzz_db::Db::from_pool(pool.clone());
    let owner = Keys::generate();
    let moderator = Keys::generate();
    let participant_a = Keys::generate();
    let participant_b = Keys::generate();
    let additional_agents = (0..6).map(|_| Keys::generate()).collect::<Vec<_>>();

    seed_user(&pool, community, &owner).await;
    db.bootstrap_owner(community, &owner.public_key().to_hex())
        .await
        .expect("bootstrap Meeting direct-action E2E owner");
    seed_agent(&pool, &db, community, &moderator, &owner, "admin").await;
    seed_agent(&pool, &db, community, &participant_a, &owner, "member").await;
    seed_agent(&pool, &db, community, &participant_b, &owner, "member").await;
    for agent in &additional_agents {
        seed_agent(&pool, &db, community, agent, &owner, "member").await;
    }

    sqlx::query(
        "UPDATE users SET capabilities = NULL \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(participant_b.public_key().to_bytes().as_slice())
    .execute(&pool)
    .await
    .expect("withhold one Agent capability for the roster gate probe");

    let moderator_hex = moderator.public_key().to_hex();
    let participant_a_hex = participant_a.public_key().to_hex();
    let participant_b_hex = participant_b.public_key().to_hex();
    let additional_agent_hex = additional_agents
        .iter()
        .map(|agent| agent.public_key().to_hex())
        .collect::<Vec<_>>();
    let capability_probe = buzz_sdk::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: Uuid::new_v4(),
        title: "Incomplete direct-action capability roster",
        description: None,
        source_channel_id: None,
        author_pubkey: &moderator_hex,
        participant_pubkeys: &[participant_a_hex.as_str(), participant_b_hex.as_str()],
        initial_board: "# Goal\nProve the direct-action capability gate fails closed.",
    })
    .expect("build incomplete-capability Create probe")
    .sign_with_keys(&moderator)
    .expect("sign incomplete-capability Create probe");
    let (status, body) = post_event(&moderator, &capability_probe).await;
    assert!(
        !status.is_success() && body.contains(buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY),
        "expected the incomplete Agent roster to fail closed, got HTTP {status}: {body}"
    );

    let capability_profile = EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_AGENT_PROFILE as u16),
        json!({
            "channel_add_policy": "anyone",
            "capabilities": [buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY]
        })
        .to_string(),
    )
    .tags([])
    .sign_with_keys(&participant_b)
    .expect("sign Agent capability profile");
    let (status, body) = post_event(&participant_b, &capability_profile).await;
    assert_accepted(status, &body);

    let at_capacity_participants = [
        participant_a_hex.as_str(),
        participant_b_hex.as_str(),
        additional_agent_hex[0].as_str(),
        additional_agent_hex[1].as_str(),
        additional_agent_hex[2].as_str(),
        additional_agent_hex[3].as_str(),
        additional_agent_hex[4].as_str(),
    ];
    let over_capacity_id = Uuid::new_v4();
    let over_capacity_participants = [
        participant_a_hex.as_str(),
        participant_b_hex.as_str(),
        additional_agent_hex[0].as_str(),
        additional_agent_hex[1].as_str(),
        additional_agent_hex[2].as_str(),
        additional_agent_hex[3].as_str(),
        additional_agent_hex[4].as_str(),
        additional_agent_hex[5].as_str(),
    ];
    let over_capacity = buzz_sdk::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: over_capacity_id,
        title: "Nine-Agent direct-action rejection",
        description: None,
        source_channel_id: None,
        author_pubkey: &moderator_hex,
        participant_pubkeys: &over_capacity_participants,
        initial_board: "# Goal\nReject the ninth authoritative Agent.",
    })
    .expect("build nine-Agent direct-action Create")
    .sign_with_keys(&moderator)
    .expect("sign nine-Agent direct-action Create");
    let (status, body) = post_event(&moderator, &over_capacity).await;
    assert!(
        !status.is_success() && body.contains("meeting supports at most 8 agents"),
        "expected the ninth Agent to be rejected, got HTTP {status}: {body}"
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
    let create = buzz_sdk::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: meeting_id,
        title: "Eight-Agent direct-action lifecycle",
        description: Some("deterministic backend acceptance"),
        source_channel_id: None,
        author_pubkey: &moderator_hex,
        participant_pubkeys: &at_capacity_participants,
        initial_board: "# Goal\nReach a conclusion.\n\n## Closing actions\n- Record the accepted result using the appropriate ordinary business tool.",
    })
    .expect("build direct-action Meeting Create")
    .sign_with_keys(&moderator)
    .expect("sign direct-action Meeting Create");
    let (status, body) = post_event(&moderator, &create).await;
    assert_accepted(status, &body);
    let create_response = response_payload(&body);
    assert_eq!(
        create_response["floor_policy_version"],
        buzz_sdk::MEETING_V2_ACTIONS_POLICY
    );
    assert_eq!(create_response["participant_count"].as_u64(), Some(8));
    let board_event_id = create_response["board_event_id"]
        .as_str()
        .expect("initial Board event id")
        .to_owned();

    let board_filter = json!([{
        "kinds": [KIND_MEETING_BOARD],
        "#h": [meeting_id.to_string()],
        "limit": 2
    }]);
    let moderator_board = query(&moderator, board_filter.clone()).await;
    assert_eq!(
        query(&participant_a, board_filter.clone()).await,
        moderator_board
    );
    assert_eq!(query(&participant_b, board_filter).await, moderator_board);

    let board = buzz_sdk::build_meeting_v2_actions_board_action(MeetingV2BoardActionParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        board: None,
    })
    .expect("build explicit final Board result")
    .sign_with_keys(&moderator)
    .expect("sign explicit final Board result");
    let (status, body) = post_event(&moderator, &board).await;
    assert_accepted(status, &body);

    let floor_ready = latest_state(&moderator, meeting_id).await;
    let begin = buzz_sdk::build_meeting_v2_action_begin(MeetingV2ActionBeginParams {
        session_id: meeting_id,
        expected_control_epoch: 1,
        board_window: 1,
        expected_state_event_id: floor_ready["id"]
            .as_str()
            .expect("floor-ready State event id"),
        board_event_id: &board_event_id,
        expected_decision_attempt_id: None,
    })
    .expect("build direct action begin")
    .sign_with_keys(&moderator)
    .expect("sign direct action begin");
    let (status, body) = post_event(&moderator, &begin).await;
    assert_accepted(status, &body);
    let action_run_id = Uuid::parse_str(
        response_payload(&body)["action_run_id"]
            .as_str()
            .expect("action run id"),
    )
    .expect("parse action run id");

    let action_state = latest_state(&moderator, meeting_id).await;
    let action_state_content: Value = serde_json::from_str(
        action_state["content"]
            .as_str()
            .expect("direct-action State content"),
    )
    .expect("parse direct-action State content");
    let action = &action_state_content["board_control"]["action"];
    assert_eq!(action["mode"], "host_direct");
    assert_eq!(action["board_event_id"], board_event_id);
    assert!(action.get("plan_event_id").is_none());
    assert!(action.get("steps").is_none());

    let stale_policy_event = EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_MEETING_ACTION_COMMAND as u16),
        "",
    )
    .tags([
        Tag::parse(["h", &meeting_id.to_string()]).expect("legacy session tag"),
        Tag::parse(["v", "3"]).expect("legacy version tag"),
        Tag::parse(["policy", "moderated-board-actions-v1"]).expect("legacy policy tag"),
        Tag::parse(["action", "plan"]).expect("legacy action tag"),
    ])
    .sign_with_keys(&moderator)
    .expect("sign stale planned-policy command");
    let (status, body) = post_event(&moderator, &stale_policy_event).await;
    assert_rejected(status, &body);

    let block = buzz_sdk::build_meeting_v2_action_block(MeetingV2ActionBlockParams {
        session_id: meeting_id,
        fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 1,
            board_event_id: &board_event_id,
        },
        reason_code: "tool_unavailable",
        reason: Some("simulated ordinary business tool outage"),
    })
    .expect("build direct action block")
    .sign_with_keys(&moderator)
    .expect("sign direct action block");
    let (status, body) = post_event(&moderator, &block).await;
    assert_accepted(status, &body);

    let blocked_end = buzz_sdk::build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
        action_fence: Some(MeetingV2ActionsEndFence {
            action_run_id,
            action_window: 1,
            board_event_id: &board_event_id,
        }),
    })
    .expect("build blocked direct-action close")
    .sign_with_keys(&moderator)
    .expect("sign blocked direct-action close");
    let (status, body) = post_event(&moderator, &blocked_end).await;
    assert_rejected(status, &body);

    let retry = buzz_sdk::build_meeting_v2_action_retry(MeetingV2ActionCommandParams {
        session_id: meeting_id,
        fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 1,
            board_event_id: &board_event_id,
        },
    })
    .expect("build direct action retry")
    .sign_with_keys(&moderator)
    .expect("sign direct action retry");
    let (status, body) = post_event(&moderator, &retry).await;
    assert_accepted(status, &body);
    assert_eq!(
        response_payload(&body)["action_window_epoch"].as_u64(),
        Some(2)
    );

    let retrieval_summary =
        "Records the accepted direct-action result and when its final Board is worth loading.";
    let stale_summary = buzz_sdk::build_meeting_summary_update(MeetingSummaryUpdateParams {
        session_id: meeting_id,
        mutation: MeetingSummaryMutation::Set(retrieval_summary),
        action_fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 1,
            board_event_id: &board_event_id,
        },
    })
    .expect("build stale-window Meeting summary")
    .sign_with_keys(&moderator)
    .expect("sign stale-window Meeting summary");
    let (status, body) = post_event(&moderator, &stale_summary).await;
    assert_rejected(status, &body);

    let participant_summary = buzz_sdk::build_meeting_summary_update(MeetingSummaryUpdateParams {
        session_id: meeting_id,
        mutation: MeetingSummaryMutation::Set(retrieval_summary),
        action_fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 2,
            board_event_id: &board_event_id,
        },
    })
    .expect("build participant Meeting summary")
    .sign_with_keys(&participant_a)
    .expect("sign participant Meeting summary");
    let (status, body) = post_event(&participant_a, &participant_summary).await;
    assert_rejected(status, &body);

    let summary = buzz_sdk::build_meeting_summary_update(MeetingSummaryUpdateParams {
        session_id: meeting_id,
        mutation: MeetingSummaryMutation::Set(retrieval_summary),
        action_fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 2,
            board_event_id: &board_event_id,
        },
    })
    .expect("build Meeting summary")
    .sign_with_keys(&moderator)
    .expect("sign Meeting summary");
    let (status, body) = post_event(&moderator, &summary).await;
    assert_accepted(status, &body);
    let (status, body) = post_event(&moderator, &summary).await;
    assert_accepted(status, &body);
    let stored_summary: Option<String> = sqlx::query_scalar(
        "SELECT summary FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community.as_uuid())
    .bind(meeting_id)
    .fetch_one(&pool)
    .await
    .expect("read Meeting retrieval summary");
    assert_eq!(stored_summary.as_deref(), Some(retrieval_summary));

    let end = buzz_sdk::build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
        action_fence: Some(MeetingV2ActionsEndFence {
            action_run_id,
            action_window: 2,
            board_event_id: &board_event_id,
        }),
    })
    .expect("build actions-recorded Meeting close")
    .sign_with_keys(&moderator)
    .expect("sign actions-recorded Meeting close");
    let (status, body) = post_event(&moderator, &end).await;
    assert_accepted(status, &body);
    let (status, body) = post_event(&moderator, &end).await;
    assert_accepted(status, &body);

    type TerminalProjection = (
        String,
        Option<String>,
        Option<String>,
        String,
        Option<String>,
        Option<Vec<u8>>,
    );
    let terminal: TerminalProjection = sqlx::query_as(
        "SELECT session.status, session.terminal_outcome, session.summary, runtime.runtime_phase, \
                    run.terminal_status, run.completion_event_id \
             FROM meeting_sessions session \
             JOIN meeting_v2_bootstrap_state runtime \
               ON runtime.community_id = session.community_id \
              AND runtime.session_id = session.session_id \
             JOIN meeting_v2_action_runs run \
               ON run.community_id = session.community_id \
              AND run.session_id = session.session_id \
             WHERE session.community_id = $1 AND session.session_id = $2 \
               AND run.action_run_id = $3",
    )
    .bind(community.as_uuid())
    .bind(meeting_id)
    .bind(action_run_id)
    .fetch_one(&pool)
    .await
    .expect("read terminal direct-action projection");
    assert_eq!(
        terminal,
        (
            "ended".to_owned(),
            Some("closed".to_owned()),
            Some(retrieval_summary.to_owned()),
            "ended".to_owned(),
            Some("completed_closed".to_owned()),
            Some(end.id.as_bytes().to_vec()),
        )
    );
    let legacy_tables_absent: bool = sqlx::query_scalar(
        "SELECT to_regclass('meeting_v2_action_steps') IS NULL \
             AND to_regclass('meeting_v2_action_step_attempts') IS NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("check legacy Meeting action tables");
    assert!(legacy_tables_absent);

    let ends = query(
        &moderator,
        json!([{
            "kinds": [KIND_MEETING_END],
            "#h": [meeting_id.to_string()],
            "limit": 10
        }]),
    )
    .await;
    let accepted_end = ends
        .iter()
        .find(|event| event["id"].as_str() == Some(end.id.to_hex().as_str()))
        .expect("query accepted attested End");
    assert_eq!(
        tag_value(accepted_end, "attestation"),
        Some("actions-recorded")
    );
    assert_eq!(
        tag_value(accepted_end, "board"),
        Some(board_event_id.as_str())
    );
}
