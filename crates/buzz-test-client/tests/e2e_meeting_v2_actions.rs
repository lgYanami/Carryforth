//! Deterministic three-Agent proof for the action-capable Meeting V2 policy.
//!
//! Requires a disposable Relay database and a Relay started with both
//! `BUZZ_MEETING_V2_CREATE_ENABLED=true` and
//! `BUZZ_MEETING_V2_ACTIONS_CREATE_ENABLED=true`.

use buzz_audit::{AuditAction, AuditService, NewAuditEntry};
use buzz_core::kind::{KIND_MEETING_BOARD, KIND_MEETING_STATE};
use buzz_core::CommunityId;
use buzz_db::project_view_v2::{ProjectViewV2AdminAssignment, ProjectViewV2CutoverPlan};
use buzz_project_view::v2::{ProjectObjectCommand, RoleCommand, RoleCommandRequest};
use buzz_project_view::{
    CreateMutation, InitializeGoal, InitializeMutation, Mutation, MutationRequest,
    NewProjectViewObject, ObjectRef, Priority, ProjectProfile, ProjectViewObjectType,
    RequirementStatus, WorkStatus,
};
use buzz_sdk::{
    MeetingV2ActionBeginParams, MeetingV2ActionCommandParams, MeetingV2ActionItem,
    MeetingV2ActionPlan, MeetingV2ActionPlanParams, MeetingV2ActionRunFence, MeetingV2ActionStep,
    MeetingV2ActionStepAppliedParams, MeetingV2ActionStepKind, MeetingV2ActionStepPreparedParams,
    MeetingV2ActionsEndFence, MeetingV2ActionsEndParams, MeetingV2BoardActionParams,
    MeetingV2CreateParams, MeetingV2EndOutcome,
};
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag};
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
        .expect("connect to Meeting action E2E database")
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
    .expect("ensure Meeting action E2E Community");
    let id: Uuid = sqlx::query_scalar("SELECT id FROM communities WHERE lower(host) = lower($1)")
        .bind(host)
        .fetch_one(pool)
        .await
        .expect("resolve Meeting action E2E Community");
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
    .expect("seed Meeting action E2E user");
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
    .expect("add Meeting action E2E Agent member");
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
    .expect("advertise Meeting action E2E Agent capability");
}

async fn post_event(keys: &Keys, event: &Event) -> (reqwest::StatusCode, String) {
    let response = reqwest::Client::new()
        .post(format!("{}/events", relay_http_url()))
        .header("X-Pubkey", keys.public_key().to_hex())
        .header("Content-Type", "application/json")
        .body(serde_json::to_string(event).expect("serialize Meeting action E2E event"))
        .send()
        .await
        .expect("submit Meeting action E2E event");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read Meeting action E2E response");
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
        .expect("query Meeting action E2E events");
    let status = response.status();
    let body = response
        .text()
        .await
        .expect("read Meeting action E2E query response");
    assert!(
        status.is_success(),
        "query failed with HTTP {status}: {body}"
    );
    serde_json::from_str(&body).expect("parse Meeting action E2E query")
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
    .expect("latest Meeting action State")
}

fn mutation_event(keys: &Keys, mutation: &Mutation) -> Event {
    EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_PROJECT_VIEW_MUTATION as u16),
        serde_json::to_string(mutation).expect("serialize Project View mutation"),
    )
    .tags([
        Tag::parse(["-"]).expect("protected Project View tag"),
        Tag::parse(["t", "buzz-project-view-mutation"]).expect("Project View type tag"),
    ])
    .sign_with_keys(keys)
    .expect("sign Project View mutation")
}

async fn initialize_project_view(
    pool: &PgPool,
    db: &buzz_db::Db,
    community: CommunityId,
    owner: &Keys,
    moderator: &Keys,
    relay_keys: &Keys,
) -> (u64, Uuid, Uuid) {
    db.set_project_view_enabled(community, true)
        .await
        .expect("enable Project View before v1 initialization");
    let initialize = Mutation::new(
        0,
        MutationRequest::Initialize(InitializeMutation {
            profile: ProjectProfile {
                name: "Meeting action E2E".to_owned(),
                positioning: "One deterministic materialization target".to_owned(),
                purpose: "Prove Meeting actions through the Relay".to_owned(),
                problem: "Meeting outcomes need durable work".to_owned(),
                scope: "Backend qualification".to_owned(),
            },
            goals: vec![InitializeGoal {
                id: Uuid::new_v4(),
                title: "Complete action lifecycle".to_owned(),
                desired_outcome: "The Meeting closes only after materialization".to_owned(),
                directions: Vec::new(),
            }],
        }),
    );
    let initialized = mutation_event(moderator, &initialize);
    let (status, body) = post_event(moderator, &initialized).await;
    assert_accepted(status, &body);

    let role_id = Uuid::new_v4();
    let role = Mutation::new(
        1,
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Role {
                id: role_id,
                name: "Meeting action moderator".to_owned(),
                purpose: "Materialize accepted Meeting outcomes".to_owned(),
                responsibilities: vec!["Maintain the accepted action boundary".to_owned()],
                boundaries: vec!["Only execute the frozen plan".to_owned()],
                active: true,
            },
        }),
    );
    let role_event = mutation_event(moderator, &role);
    let (status, body) = post_event(moderator, &role_event).await;
    assert_accepted(status, &body);

    db.set_project_view_enabled(community, false)
        .await
        .expect("pause Project View for v2 cutover");
    let audit = AuditService::new(pool.clone());
    let cutover_audit = audit
        .log(NewAuditEntry {
            community_id: community,
            action: AuditAction::ProjectViewCutover,
            actor_pubkey: Some(owner.public_key().to_bytes().to_vec()),
            object_id: Some(community.to_string()),
            detail: json!({"test": "Meeting V2 action three-Agent E2E"}),
        })
        .await
        .expect("append Project View cutover audit fact");
    let downgraded_admins = sqlx::query_scalar::<_, String>(
        "SELECT pubkey FROM relay_members \
         WHERE community_id = $1 AND role = 'admin' AND pubkey <> $2 \
         ORDER BY pubkey",
    )
    .bind(community.as_uuid())
    .bind(moderator.public_key().to_hex())
    .fetch_all(pool)
    .await
    .expect("load non-moderator admins for explicit cutover downgrade")
    .into_iter()
    .map(|pubkey| PublicKey::from_hex(&pubkey).expect("parse cutover admin pubkey"))
    .collect();
    let cutover = db
        .cutover_project_view_v2(
            community,
            &ProjectViewV2CutoverPlan {
                admin_assignments: vec![ProjectViewV2AdminAssignment {
                    member_pubkey: moderator.public_key(),
                    role_id,
                }],
                downgraded_admins,
                audit_seq: cutover_audit.seq,
                idempotency_key_hash: [0x54; 32],
            },
            relay_keys,
        )
        .await
        .expect("cut Project View over to v2");
    db.set_project_view_enabled_checked(community, true, Some(&relay_keys.public_key()))
        .await
        .expect("enable Project View v2");
    let assignment_id: Uuid = sqlx::query_scalar(
        "SELECT assignment_id FROM project_role_assignments \
         WHERE community_id = $1 AND role_id = $2 AND member_pubkey = $3 \
           AND ended_at IS NULL",
    )
    .bind(community.as_uuid())
    .bind(role_id)
    .bind(moderator.public_key().to_hex())
    .fetch_one(pool)
    .await
    .expect("resolve moderator Project View Assignment");
    (cutover.project_revision, assignment_id, role_id)
}

async fn materialize_step(
    moderator: &Keys,
    meeting_id: Uuid,
    action_run_id: Uuid,
    plan_event_id: &str,
    step_id: Uuid,
    expected_project_revision: u64,
    project_event: &Event,
) -> u64 {
    let project_event_json = serde_json::to_value(project_event).expect("encode Project event");
    let prepared =
        buzz_sdk::build_meeting_v2_action_step_prepared(MeetingV2ActionStepPreparedParams {
            session_id: meeting_id,
            fence: MeetingV2ActionRunFence {
                action_run_id,
                action_window: 1,
                plan_event_id: Some(plan_event_id),
            },
            step_id,
            attempt: 1,
            project_event_id: &project_event.id.to_hex(),
            expected_project_revision,
            signed_project_event: &project_event_json,
        })
        .expect("build prepared action step")
        .sign_with_keys(moderator)
        .expect("sign prepared action step");
    let (status, body) = post_event(moderator, &prepared).await;
    assert_accepted(status, &body);

    let (status, body) = post_event(moderator, project_event).await;
    assert_accepted(status, &body);
    let accepted_project_revision = response_payload(&body)["project_revision"]
        .as_u64()
        .expect("Project View receipt revision");

    let applied =
        buzz_sdk::build_meeting_v2_action_step_applied(MeetingV2ActionStepAppliedParams {
            session_id: meeting_id,
            fence: MeetingV2ActionRunFence {
                action_run_id,
                action_window: 1,
                plan_event_id: Some(plan_event_id),
            },
            step_id,
            project_event_id: &project_event.id.to_hex(),
            accepted_project_revision,
        })
        .expect("build applied action step")
        .sign_with_keys(moderator)
        .expect("sign applied action step");
    let (status, body) = post_event(moderator, &applied).await;
    assert_accepted(status, &body);
    accepted_project_revision
}

#[tokio::test]
#[ignore = "requires a disposable Relay with action-capable Meeting V2 creation enabled"]
async fn three_agents_materialize_one_frozen_plan_before_normal_close() {
    let pool = test_pool().await;
    let community = ensure_community(&pool).await;
    let db = buzz_db::Db::from_pool(pool.clone());
    let owner = Keys::generate();
    let moderator = Keys::generate();
    let participant_a = Keys::generate();
    let participant_b = Keys::generate();
    let relay_keys = Keys::parse(
        &std::env::var("BUZZ_RELAY_PRIVATE_KEY")
            .expect("BUZZ_RELAY_PRIVATE_KEY must match the running Relay"),
    )
    .expect("parse Meeting action E2E Relay key");

    seed_user(&pool, community, &owner).await;
    db.bootstrap_owner(community, &owner.public_key().to_hex())
        .await
        .expect("bootstrap Meeting action E2E owner");
    seed_agent(&pool, &db, community, &moderator, &owner, "admin").await;
    seed_agent(&pool, &db, community, &participant_a, &owner, "member").await;
    seed_agent(&pool, &db, community, &participant_b, &owner, "member").await;

    sqlx::query(
        "UPDATE users SET capabilities = NULL \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(participant_b.public_key().to_bytes().as_slice())
    .execute(&pool)
    .await
    .expect("withhold one Agent capability for the roster gate probe");

    let capability_probe_id = Uuid::new_v4();
    let capability_probe = buzz_sdk::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: capability_probe_id,
        title: "Incomplete action capability roster",
        description: None,
        source_channel_id: None,
        author_pubkey: &moderator.public_key().to_hex(),
        participant_pubkeys: &[
            participant_a.public_key().to_hex().as_str(),
            participant_b.public_key().to_hex().as_str(),
        ],
        initial_board: "# Goal\nProve the action capability gate fails closed.",
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
    let advertised: Value = sqlx::query_scalar(
        "SELECT capabilities FROM users WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(participant_b.public_key().to_bytes().as_slice())
    .fetch_one(&pool)
    .await
    .expect("read Agent capability profile side effect");
    assert_eq!(advertised, json!([buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY]));

    let (initial_project_revision, assignment_id, role_id) =
        initialize_project_view(&pool, &db, community, &owner, &moderator, &relay_keys).await;

    let meeting_id = Uuid::new_v4();
    let moderator_hex = moderator.public_key().to_hex();
    let participant_a_hex = participant_a.public_key().to_hex();
    let participant_b_hex = participant_b.public_key().to_hex();
    let create = buzz_sdk::build_meeting_v2_actions_create(MeetingV2CreateParams {
        session_id: meeting_id,
        title: "Three-Agent action lifecycle",
        description: Some("deterministic Stage 4 backend acceptance"),
        source_channel_id: None,
        author_pubkey: &moderator_hex,
        participant_pubkeys: &[participant_a_hex.as_str(), participant_b_hex.as_str()],
        initial_board: "# Goal\nMaterialize the accepted backend action.\n\n## Actions\n- Create one requirement and one owned work item.",
    })
    .expect("build action-capable Meeting Create")
    .sign_with_keys(&moderator)
    .expect("sign action-capable Meeting Create");
    let (status, body) = post_event(&moderator, &create).await;
    assert_accepted(status, &body);
    let create_response = response_payload(&body);
    assert_eq!(
        create_response["floor_policy_version"],
        buzz_sdk::MEETING_V2_ACTIONS_POLICY
    );
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
    .expect("build action begin")
    .sign_with_keys(&moderator)
    .expect("sign action begin");
    let (status, body) = post_event(&moderator, &begin).await;
    assert_accepted(status, &body);
    let action_run_id = Uuid::parse_str(
        response_payload(&body)["action_run_id"]
            .as_str()
            .expect("action run id"),
    )
    .expect("parse action run id");

    let action_id = Uuid::new_v4();
    let requirement_id = Uuid::new_v4();
    let work_id = Uuid::new_v4();
    let requirement_step_id = Uuid::new_v4();
    let work_step_id = Uuid::new_v4();
    let responsibility_step_id = Uuid::new_v4();
    let plan = MeetingV2ActionPlan {
        version: buzz_sdk::MEETING_V2_ACTION_PLAN_VERSION,
        action_run_id,
        board_event_id: board_event_id.clone(),
        items: vec![MeetingV2ActionItem {
            action_id,
            summary: "Implement the accepted backend action".to_owned(),
            assignee_pubkey: moderator_hex.clone(),
        }],
        steps: vec![
            MeetingV2ActionStep {
                step_id: requirement_step_id,
                action_id: None,
                kind: MeetingV2ActionStepKind::ProjectViewCreateRequirement,
                target_object_id: requirement_id,
                payload: json!({"title": "Accepted Meeting requirement"}),
            },
            MeetingV2ActionStep {
                step_id: work_step_id,
                action_id: Some(action_id),
                kind: MeetingV2ActionStepKind::ProjectViewCreateWork,
                target_object_id: work_id,
                payload: json!({
                    "title": "Implement the accepted backend action",
                    "requirement_id": requirement_id
                }),
            },
            MeetingV2ActionStep {
                step_id: responsibility_step_id,
                action_id: Some(action_id),
                kind: MeetingV2ActionStepKind::ProjectViewSetWorkResponsibility,
                target_object_id: work_id,
                payload: json!({}),
            },
        ],
    };
    let plan_event = buzz_sdk::build_meeting_v2_action_plan(MeetingV2ActionPlanParams {
        session_id: meeting_id,
        fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 1,
            plan_event_id: None,
        },
        plan: &plan,
    })
    .expect("build frozen action plan")
    .sign_with_keys(&moderator)
    .expect("sign frozen action plan");
    let (status, body) = post_event(&moderator, &plan_event).await;
    assert_accepted(status, &body);
    let plan_event_id = plan_event.id.to_hex();

    let requirement_command = ProjectObjectCommand::new(
        initial_project_revision,
        Some(assignment_id),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Requirement {
                id: requirement_id,
                title: "Accepted Meeting requirement".to_owned(),
                description: "Accepted Meeting requirement".to_owned(),
                status: RequirementStatus::Ready,
                priority: Priority::Normal,
                planned_in_stage_id: None,
            },
        }),
    );
    let requirement_event =
        buzz_sdk::project_view_v2::build_project_object_command(requirement_command)
            .expect("build Requirement command")
            .sign_with_keys(&moderator)
            .expect("sign Requirement command");
    let work_revision = materialize_step(
        &moderator,
        meeting_id,
        action_run_id,
        &plan_event_id,
        requirement_step_id,
        initial_project_revision,
        &requirement_event,
    )
    .await;

    let work_command = ProjectObjectCommand::new(
        work_revision,
        Some(assignment_id),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Work {
                id: work_id,
                title: "Implement the accepted backend action".to_owned(),
                description: "Implement the accepted backend action".to_owned(),
                status: WorkStatus::Pending,
                priority: Priority::Normal,
                handles: ObjectRef {
                    object_type: ProjectViewObjectType::Requirement,
                    object_id: requirement_id,
                },
            },
        }),
    );
    let work_event = buzz_sdk::project_view_v2::build_project_object_command(work_command)
        .expect("build Work command")
        .sign_with_keys(&moderator)
        .expect("sign Work command");
    let responsibility_revision = materialize_step(
        &moderator,
        meeting_id,
        action_run_id,
        &plan_event_id,
        work_step_id,
        work_revision,
        &work_event,
    )
    .await;

    let responsibility_command = RoleCommand::new(
        responsibility_revision,
        Some(assignment_id),
        RoleCommandRequest::SetWorkResponsibility {
            work_id,
            responsible_role_id: Some(role_id),
        },
    );
    let responsibility_event =
        buzz_sdk::project_view_v2::build_role_command(responsibility_command)
            .expect("build Work responsibility command")
            .sign_with_keys(&moderator)
            .expect("sign Work responsibility command");
    let final_project_revision = materialize_step(
        &moderator,
        meeting_id,
        action_run_id,
        &plan_event_id,
        responsibility_step_id,
        responsibility_revision,
        &responsibility_event,
    )
    .await;

    let complete = buzz_sdk::build_meeting_v2_action_complete(MeetingV2ActionCommandParams {
        session_id: meeting_id,
        fence: MeetingV2ActionRunFence {
            action_run_id,
            action_window: 1,
            plan_event_id: Some(&plan_event_id),
        },
    })
    .expect("build action completion")
    .sign_with_keys(&moderator)
    .expect("sign action completion");
    let (status, body) = post_event(&moderator, &complete).await;
    assert_accepted(status, &body);

    let end = buzz_sdk::build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
        session_id: meeting_id,
        create_event_id: &create.id.to_hex(),
        outcome: MeetingV2EndOutcome::Closed,
        reason_code: None,
        reason: None,
        action_fence: Some(MeetingV2ActionsEndFence {
            action_run_id,
            action_window: 1,
            plan_event_id: &plan_event_id,
        }),
    })
    .expect("build action-gated Meeting close")
    .sign_with_keys(&moderator)
    .expect("sign action-gated Meeting close");
    let (status, body) = post_event(&moderator, &end).await;
    assert_accepted(status, &body);

    let terminal: (String, String, String, i64, i64, Option<Uuid>) = sqlx::query_as(
        "SELECT session.status, run.terminal_status, runtime.runtime_phase, \
                count(step.step_id)::BIGINT, \
                count(step.step_id) FILTER (WHERE step.status = 'applied')::BIGINT, \
                work.responsible_role_id \
         FROM meeting_sessions session \
         JOIN meeting_v2_bootstrap_state runtime \
           ON runtime.community_id = session.community_id \
          AND runtime.session_id = session.session_id \
         JOIN meeting_v2_action_runs run \
           ON run.community_id = session.community_id AND run.session_id = session.session_id \
         JOIN meeting_v2_action_steps step \
           ON step.community_id = run.community_id AND step.session_id = run.session_id \
          AND step.action_run_id = run.action_run_id \
         JOIN project_view_objects work \
           ON work.community_id = session.community_id AND work.object_id = $4 \
         WHERE session.community_id = $1 AND session.session_id = $2 \
           AND run.action_run_id = $3 \
         GROUP BY session.status, run.terminal_status, runtime.runtime_phase, \
                  work.responsible_role_id",
    )
    .bind(community.as_uuid())
    .bind(meeting_id)
    .bind(action_run_id)
    .bind(work_id)
    .fetch_one(&pool)
    .await
    .expect("read terminal Meeting action projection");
    assert_eq!(
        terminal,
        (
            "ended".to_owned(),
            "completed_closed".to_owned(),
            "ended".to_owned(),
            3,
            3,
            Some(role_id),
        )
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT project_revision FROM project_view_state WHERE community_id = $1",
        )
        .bind(community.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("read final Project View revision"),
        i64::try_from(final_project_revision).expect("bounded Project revision")
    );
}
