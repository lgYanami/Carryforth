//! Authoritative Meeting V2 action-finalization state and command ledger.
//!
//! The action ledger is the cross-domain recovery boundary between a Meeting
//! and Project View. Every external command is registered here before publish,
//! accepted atomically by Project View, and then bound back to its plan step.

use std::collections::{HashMap, HashSet};

use buzz_core::CommunityId;
use buzz_project_view::v2::{ProjectObjectCommand, RoleCommand, RoleCommandRequest};
use buzz_project_view::{
    CreateMutation, MutationRequest, NewProjectViewObject, ObjectRef, Priority,
    ProjectViewObjectType, ProjectWork, Requirement, RequirementStatus, WorkStatus,
};
use chrono::{DateTime, Duration, Utc};
use nostr::{Event, Keys};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::meeting_v2::RuntimePhase;
use crate::{Db, DbError, Result};

/// Compare-and-swap fences for one active action run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRunFence {
    /// Relay-issued action run ID.
    pub action_run_id: Uuid,
    /// Current action retry-window epoch.
    pub action_window_epoch: i64,
    /// Frozen plan event ID, or `None` while planning.
    pub plan_event_id: Option<Vec<u8>>,
}

/// Strict command payload produced by Relay wire parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionCommand {
    /// Enter action finalization from a completed Board/Floor cycle.
    Begin {
        /// Current control-token epoch.
        expected_control_epoch: i64,
        /// Completed Board window.
        board_window: i64,
        /// Exact authoritative State event used for the decision.
        expected_state_event_id: Vec<u8>,
        /// Exact frozen current Board event.
        board_event_id: Vec<u8>,
        /// Exact running moderator DecisionAttempt for a candidate Floor result.
        expected_decision_attempt_id: Option<Vec<u8>>,
    },
    /// Freeze the first valid Harness-compiled plan.
    Plan {
        /// Current run/window/empty-plan fences.
        fence: ActionRunFence,
        /// Strict compiled plan.
        plan: buzz_sdk::MeetingV2ActionPlan,
    },
    /// Durably register one exact signed Project View command before publish.
    StepPrepared {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
        /// Stable plan step identifier.
        step_id: Uuid,
        /// One-based attempt number.
        attempt: i32,
        /// Exact signed Project View command event ID.
        project_event_id: Vec<u8>,
        /// Revision expected by the embedded command.
        expected_project_revision: i64,
        /// Complete signed command retained for exact replay.
        signed_project_event: Event,
    },
    /// Bind one Project View acceptance receipt to its prepared plan step.
    StepApplied {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
        /// Stable plan step identifier.
        step_id: Uuid,
        /// Exact accepted Project View command event ID.
        project_event_id: Vec<u8>,
        /// Relay-authoritative accepted Project revision.
        accepted_project_revision: i64,
    },
    /// Persist a durable materializer failure.
    Block {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
        /// Closed low-cardinality reason code.
        reason_code: String,
    },
    /// Start a fresh action deadline for a blocked run.
    Retry {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
    },
    /// Declare that every required step has a verified receipt.
    Complete {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
    },
    /// End a zero-effect run and open a new Board window.
    ReturnToBoard {
        /// Current run/window/plan fences.
        fence: ActionRunFence,
    },
}

impl ActionCommand {
    /// Stable low-cardinality wire action label.
    pub fn action(&self) -> &'static str {
        match self {
            Self::Begin { .. } => "begin",
            Self::Plan { .. } => "plan",
            Self::StepPrepared { .. } => "step-prepared",
            Self::StepApplied { .. } => "step-applied",
            Self::Block { .. } => "block",
            Self::Retry { .. } => "retry",
            Self::Complete { .. } => "complete",
            Self::ReturnToBoard { .. } => "return-to-board",
        }
    }
}

/// Inputs for executing one moderator-signed action command.
pub struct ActionCommandTxParams<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Verified, strictly parsed command event.
    pub event: &'a Event,
    /// Typed action command.
    pub command: ActionCommand,
    /// Relay signer for the authoritative Meeting State projection.
    pub relay_keys: &'a Keys,
}

/// Committed result of one action command, including private receipt replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionCommandCommit {
    /// Whether the command changed or confirmed authoritative state.
    pub accepted: bool,
    /// Whether this result came from an existing identical-event receipt.
    pub duplicate: bool,
    /// Stable machine-readable outcome.
    pub outcome_code: String,
    /// Stable response persisted in the private receipt.
    pub response: Value,
}

#[derive(Debug)]
struct ActionReceipt {
    author_pubkey: Vec<u8>,
    accepted: bool,
    outcome_code: String,
    response: Value,
}

#[derive(Debug, Clone)]
struct ActionRunRow {
    action_run_id: Uuid,
    plan_event_id: Option<Vec<u8>>,
    board_event_id: Vec<u8>,
    control_epoch: i64,
    action_window_epoch: i64,
    action_phase: String,
    action_condition: String,
    terminal_status: Option<String>,
    last_error_code: Option<String>,
}

/// Block one action window whose independent database deadline has elapsed.
///
/// Callers must already hold the Meeting Session row lock. The transition and
/// Relay-signed State/outbox rows commit in the caller's transaction, making
/// sweeper and command-triggered lazy recovery converge on the same CAS.
pub(crate) async fn recover_due_action_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    now: DateTime<Utc>,
) -> Result<Option<crate::meeting_baton::BatonTransitionResult>> {
    let row = sqlx::query(
        "SELECT action_run_id, action_phase \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 \
           AND terminal_status IS NULL AND action_condition = 'runnable' \
           AND action_phase IN ('planning', 'applying') \
           AND action_deadline_at IS NOT NULL AND action_deadline_at <= $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let action_run_id: Uuid = row.try_get("action_run_id")?;
    let action_phase: String = row.try_get("action_phase")?;
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_condition = 'blocked', action_deadline_at = NULL, \
             last_error_code = 'action_deadline_exceeded', updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_condition = 'runnable' \
           AND action_phase IN ('planning', 'applying')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Ok(None);
    }
    let transition = crate::meeting_baton::publish_v2_action_deadline_transition_tx(
        tx,
        community_id,
        session_id,
        action_run_id,
        relay_keys,
        "action_deadline_exceeded",
        "action",
        &format!("{action_phase}/runnable"),
        &format!("{action_phase}/blocked"),
        now,
    )
    .await?;
    Ok(Some(transition))
}

#[derive(Debug)]
struct AppliedCommand {
    accepted: bool,
    outcome_code: &'static str,
    action_run_id: Option<Uuid>,
    action_window_epoch: Option<i64>,
    state_revision: Option<i64>,
    extra: Value,
}

impl AppliedCommand {
    fn rejected(code: &'static str, run: Option<&ActionRunRow>) -> Self {
        Self {
            accepted: false,
            outcome_code: code,
            action_run_id: run.map(|value| value.action_run_id),
            action_window_epoch: run.map(|value| value.action_window_epoch),
            state_revision: None,
            extra: json!({}),
        }
    }

    fn accepted(
        code: &'static str,
        run_id: Uuid,
        action_window_epoch: i64,
        state_revision: i64,
        extra: Value,
    ) -> Self {
        Self {
            accepted: true,
            outcome_code: code,
            action_run_id: Some(run_id),
            action_window_epoch: Some(action_window_epoch),
            state_revision: Some(state_revision),
            extra,
        }
    }
}

/// Execute one action command under the Meeting Session lock.
pub async fn execute_action_command(
    db: &Db,
    params: ActionCommandTxParams<'_>,
) -> Result<ActionCommandCommit> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "Meeting V2 action command has a nil session id".to_string(),
        ));
    }
    params.event.verify().map_err(|error| {
        DbError::InvalidData(format!("invalid Meeting V2 action event: {error}"))
    })?;
    if params.event.kind.as_u16() as u32 != buzz_core::kind::KIND_MEETING_ACTION_COMMAND {
        return Err(DbError::InvalidData(
            "Meeting V2 action command uses the wrong event kind".to_string(),
        ));
    }

    let mut tx = db.begin_transaction().await?;
    let session = crate::meeting_baton::lock_baton_session_tx(
        &mut tx,
        params.community_id,
        params.session_id,
    )
    .await?;
    if !session.protocol.has_action_finalization() {
        return Err(DbError::InvalidData(format!(
            "meeting {} does not use {}",
            params.session_id,
            crate::meeting_v2::ACTIONS_POLICY_VERSION
        )));
    }
    let author = params.event.pubkey.as_bytes();
    if author != session.host_pubkey.as_slice() {
        return Err(DbError::AccessDenied(
            "only the immutable Meeting moderator can finalize actions".to_string(),
        ));
    }
    if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        author,
    )
    .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting action author was durably revoked from this Session".to_string(),
        ));
    }
    if !crate::meeting::actor_security_active_tx(&mut tx, params.community_id, author).await? {
        return Err(DbError::AccessDenied(
            "Meeting action author is no longer an active writable principal".to_string(),
        ));
    }

    if let Some(receipt) =
        load_receipt_tx(&mut tx, params.community_id, params.event.id.as_bytes()).await?
    {
        if receipt.author_pubkey != author {
            return Err(DbError::AccessDenied(
                "not authorized for this private Meeting action receipt".to_string(),
            ));
        }
        tx.commit().await?;
        return Ok(ActionCommandCommit {
            accepted: receipt.accepted,
            duplicate: true,
            outcome_code: receipt.outcome_code,
            response: receipt.response,
        });
    }

    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    let deadline_recovered = if session.status == "active" {
        recover_due_action_locked_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
            now,
        )
        .await?
        .is_some()
    } else {
        false
    };
    let applied = if session.status != "active" {
        AppliedCommand::rejected("meeting_ended", None)
    } else if deadline_recovered {
        let run = load_active_run_tx(&mut tx, params.community_id, params.session_id, true).await?;
        AppliedCommand::rejected("action_deadline_recovered", run.as_ref())
    } else {
        apply_command_tx(&mut tx, &params, now).await?
    };
    let response = json!({
        "meeting_id": params.session_id,
        "accepted": applied.accepted,
        "outcome": applied.outcome_code,
        "action_run_id": applied.action_run_id,
        "action_window_epoch": applied.action_window_epoch,
        "state_revision": applied.state_revision,
        "details": applied.extra,
    });
    insert_receipt_tx(&mut tx, &params, &applied, &response).await?;
    tx.commit().await?;
    Ok(ActionCommandCommit {
        accepted: applied.accepted,
        duplicate: false,
        outcome_code: applied.outcome_code.to_string(),
        response,
    })
}

async fn apply_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    match &params.command {
        ActionCommand::Begin {
            expected_control_epoch,
            board_window,
            expected_state_event_id,
            board_event_id,
            expected_decision_attempt_id,
        } => {
            apply_begin_tx(
                tx,
                params,
                *expected_control_epoch,
                *board_window,
                expected_state_event_id,
                board_event_id,
                expected_decision_attempt_id.as_deref(),
                now,
            )
            .await
        }
        ActionCommand::Plan { fence, plan } => apply_plan_tx(tx, params, fence, plan, now).await,
        ActionCommand::StepPrepared {
            fence,
            step_id,
            attempt,
            project_event_id,
            expected_project_revision,
            signed_project_event,
        } => {
            apply_step_prepared_tx(
                tx,
                params,
                fence,
                *step_id,
                *attempt,
                project_event_id,
                *expected_project_revision,
                signed_project_event,
                now,
            )
            .await
        }
        ActionCommand::StepApplied {
            fence,
            step_id,
            project_event_id,
            accepted_project_revision,
        } => {
            apply_step_applied_tx(
                tx,
                params,
                fence,
                *step_id,
                project_event_id,
                *accepted_project_revision,
                now,
            )
            .await
        }
        ActionCommand::Block { fence, reason_code } => {
            apply_block_tx(tx, params, fence, reason_code, now).await
        }
        ActionCommand::Retry { fence } => apply_retry_tx(tx, params, fence, now).await,
        ActionCommand::Complete { fence } => apply_complete_tx(tx, params, fence, now).await,
        ActionCommand::ReturnToBoard { fence } => {
            apply_return_to_board_tx(tx, params, fence, now).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_begin_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    expected_control_epoch: i64,
    board_window: i64,
    expected_state_event_id: &[u8],
    board_event_id: &[u8],
    expected_decision_attempt_id: Option<&[u8]>,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    if expected_control_epoch <= 0
        || board_window <= 0
        || expected_state_event_id.len() != 32
        || board_event_id.len() != 32
        || expected_decision_attempt_id.is_some_and(|attempt_id| attempt_id.len() != 32)
    {
        return Err(DbError::InvalidData(
            "Meeting V2 action begin has malformed fences".to_string(),
        ));
    }
    let runtime =
        crate::meeting_v2::load_runtime_tx(tx, params.community_id, params.session_id, true)
            .await?;
    if runtime.phase != RuntimePhase::FloorReady {
        return Ok(AppliedCommand::rejected("floor_not_ready", None));
    }
    if !matches!(
        runtime.board_outcome.as_deref(),
        Some("updated" | "unchanged")
    ) {
        return Ok(AppliedCommand::rejected("final_board_not_explicit", None));
    }
    if runtime.control_epoch != expected_control_epoch {
        return Ok(AppliedCommand::rejected("stale_control_epoch", None));
    }
    if runtime.board_window != board_window {
        return Ok(AppliedCommand::rejected("stale_board_window", None));
    }
    let current_board_event_id: Vec<u8> = sqlx::query_scalar(
        "SELECT board_event_id FROM meeting_current_boards \
         WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .fetch_one(tx.as_mut())
    .await?;
    if current_board_event_id != board_event_id {
        return Ok(AppliedCommand::rejected("stale_board_event", None));
    }

    let baton = sqlx::query(
        "SELECT phase, state_event_id, control_epoch, decision_epoch, intent_revision, \
                speech_revision, active_offer_id, active_grant_id, \
                active_decision_attempt_id, next_action_at \
         FROM meeting_baton_state \
         WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .fetch_one(tx.as_mut())
    .await?;
    let phase: String = baton.try_get("phase")?;
    let state_event_id: Vec<u8> = baton.try_get("state_event_id")?;
    let control_epoch: i64 = baton.try_get("control_epoch")?;
    let decision_epoch: i64 = baton.try_get("decision_epoch")?;
    let intent_revision: i64 = baton.try_get("intent_revision")?;
    let speech_revision: i64 = baton.try_get("speech_revision")?;
    let active_offer_id: Option<Vec<u8>> = baton.try_get("active_offer_id")?;
    let active_grant_id: Option<Vec<u8>> = baton.try_get("active_grant_id")?;
    let active_attempt_id: Option<Vec<u8>> = baton.try_get("active_decision_attempt_id")?;
    let next_action_at: Option<DateTime<Utc>> = baton.try_get("next_action_at")?;
    if active_offer_id.is_some() || active_grant_id.is_some() {
        return Ok(AppliedCommand::rejected("moderator_floor_not_idle", None));
    }
    if state_event_id != expected_state_event_id {
        return Ok(AppliedCommand::rejected("stale_state_event", None));
    }
    if control_epoch != expected_control_epoch {
        return Ok(AppliedCommand::rejected("stale_control_epoch", None));
    }
    match expected_decision_attempt_id {
        Some(attempt_id) => {
            if !matches!(phase.as_str(), "moderator_control" | "moderator_idle")
                || active_attempt_id.as_deref() != Some(attempt_id)
            {
                return Ok(AppliedCommand::rejected("stale_decision_attempt", None));
            }
            let attempt = sqlx::query(
                "SELECT moderator_pubkey, control_epoch, decision_epoch, speech_revision, \
                        snapshot_intent_revision, state, deadline_at \
                 FROM meeting_moderator_decision_attempts \
                 WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
                 FOR UPDATE",
            )
            .bind(params.community_id.as_uuid())
            .bind(params.session_id)
            .bind(attempt_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(attempt) = attempt else {
                return Ok(AppliedCommand::rejected("stale_decision_attempt", None));
            };
            let moderator_pubkey: Vec<u8> = attempt.try_get("moderator_pubkey")?;
            let attempt_control_epoch: i64 = attempt.try_get("control_epoch")?;
            let attempt_decision_epoch: i64 = attempt.try_get("decision_epoch")?;
            let attempt_speech_revision: i64 = attempt.try_get("speech_revision")?;
            let attempt_intent_revision: i64 = attempt.try_get("snapshot_intent_revision")?;
            let attempt_state: String = attempt.try_get("state")?;
            let attempt_deadline: DateTime<Utc> = attempt.try_get("deadline_at")?;
            if moderator_pubkey != params.event.pubkey.as_bytes()
                || attempt_state != "running"
                || attempt_control_epoch != control_epoch
                || attempt_decision_epoch != decision_epoch
                || attempt_speech_revision != speech_revision
                || attempt_intent_revision != intent_revision
                || next_action_at != Some(attempt_deadline)
            {
                return Ok(AppliedCommand::rejected(
                    "decision_attempt_prerequisite_changed",
                    None,
                ));
            }
            if now >= attempt_deadline {
                return Ok(AppliedCommand::rejected("moderator_attempt_expired", None));
            }
            if has_human_floor_work_tx(tx, params.community_id, params.session_id).await? {
                return Ok(AppliedCommand::rejected("human_request_has_priority", None));
            }
        }
        None => {
            if phase != "moderator_idle" || active_attempt_id.is_some() || next_action_at.is_some()
            {
                return Ok(AppliedCommand::rejected("moderator_floor_not_idle", None));
            }
            if has_unresolved_floor_work_tx(tx, params.community_id, params.session_id).await? {
                return Ok(AppliedCommand::rejected("floor_work_pending", None));
            }
        }
    }
    if load_active_run_tx(tx, params.community_id, params.session_id, true)
        .await?
        .is_some()
    {
        return Ok(AppliedCommand::rejected("action_run_already_active", None));
    }

    let frozen_floor = if let Some(attempt_id) = expected_decision_attempt_id {
        let completed = sqlx::query(
            "UPDATE meeting_moderator_decision_attempts \
             SET state = 'completed', terminal_event_id = $4, \
                 terminal_reason = 'action_finalization', terminal_at = $5 \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
               AND state = 'running'",
        )
        .bind(params.community_id.as_uuid())
        .bind(params.session_id)
        .bind(attempt_id)
        .bind(params.event.id.as_bytes().as_slice())
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        if completed.rows_affected() != 1 {
            return Err(DbError::InvalidData(
                "Meeting moderator DecisionAttempt changed while beginning action finalization"
                    .to_string(),
            ));
        }
        Some(
            freeze_floor_work_for_actions_tx(
                tx,
                params.community_id,
                params.session_id,
                params.event.id.as_bytes(),
                now,
            )
            .await?,
        )
    } else {
        None
    };

    let duration_ms = action_duration_ms_tx(tx, params.community_id, params.session_id).await?;
    let deadline = now + Duration::milliseconds(duration_ms);
    let action_run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO meeting_v2_action_runs \
             (community_id, session_id, action_run_id, begin_event_id, board_event_id, \
              control_epoch, board_window, action_window_epoch, action_phase, \
              action_condition, action_deadline_at, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 'planning', 'runnable', $8, $9, $9)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(action_run_id)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(board_event_id)
    .bind(expected_control_epoch)
    .bind(board_window)
    .bind(deadline)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let changed = sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET runtime_phase = 'finalizing_actions', updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 \
           AND runtime_phase = 'floor_ready' AND control_epoch = $3 AND board_window = $4",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(expected_control_epoch)
    .bind(board_window)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if changed.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 runtime changed while accepting action begin".to_string(),
        ));
    }
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        action_run_id,
        params.relay_keys,
        "action_finalization_began",
        params.event.id.as_bytes(),
        "floor_ready",
        "planning/runnable",
        expected_decision_attempt_id,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_finalization_began",
        action_run_id,
        1,
        transition.state_revision,
        json!({
            "board_event_id": hex::encode(board_event_id),
            "action_deadline_at_ms": deadline.timestamp_millis(),
            "decision_attempt_id": expected_decision_attempt_id.map(hex::encode),
            "frozen_intent_count": frozen_floor.map(|counts| counts.0).unwrap_or(0),
            "frozen_handoff_count": frozen_floor.map(|counts| counts.1).unwrap_or(0),
        }),
    ))
}

async fn apply_plan_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    plan: &buzz_sdk::MeetingV2ActionPlan,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    buzz_sdk::validate_meeting_v2_action_plan(plan)
        .map_err(|error| DbError::InvalidData(error.to_string()))?;
    validate_materializer_plan(plan)?;
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if run.action_phase != "planning" || run.action_condition != "runnable" {
        return Ok(AppliedCommand::rejected("action_not_planning", Some(&run)));
    }
    if run.plan_event_id.is_some() || fence.plan_event_id.is_some() {
        return Ok(AppliedCommand::rejected(
            "action_plan_already_frozen",
            Some(&run),
        ));
    }
    if plan.action_run_id != run.action_run_id {
        return Ok(AppliedCommand::rejected("stale_action_run", Some(&run)));
    }
    let plan_board_event = hex::decode(&plan.board_event_id)
        .map_err(|_| DbError::InvalidData("invalid action plan Board event hex".to_string()))?;
    if plan_board_event != run.board_event_id {
        return Ok(AppliedCommand::rejected("stale_board_event", Some(&run)));
    }
    validate_plan_roster_tx(tx, params.community_id, params.session_id, plan).await?;
    let plan_json = serde_json::to_value(plan)?;
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET plan_event_id = $4, plan_json = $5, action_phase = 'applying', updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_phase = 'planning' \
           AND action_condition = 'runnable' AND plan_event_id IS NULL",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(&plan_json)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 action run changed while freezing its plan".to_string(),
        ));
    }
    insert_plan_steps_tx(tx, params.community_id, params.session_id, &run, plan, now).await?;
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_plan_frozen",
        params.event.id.as_bytes(),
        "planning/runnable",
        "applying/runnable",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_plan_frozen",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({
            "plan_event_id": params.event.id.to_hex(),
            "item_count": plan.items.len(),
            "step_count": plan.steps.len(),
        }),
    ))
}

fn validate_materializer_plan(plan: &buzz_sdk::MeetingV2ActionPlan) -> Result<()> {
    let expected_steps = plan
        .items
        .len()
        .checked_mul(2)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| DbError::InvalidData("Meeting action plan size overflow".to_string()))?;
    if plan.steps.len() != expected_steps {
        return Err(DbError::InvalidData(
            "Meeting action materializer requires Requirement, then Work/responsibility pairs"
                .to_string(),
        ));
    }
    let requirement = &plan.steps[0];
    if requirement.kind != buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateRequirement
        || requirement.action_id.is_some()
    {
        return Err(DbError::InvalidData(
            "Meeting action materializer must begin with one Requirement".to_string(),
        ));
    }
    let _: RequirementStepPayload = serde_json::from_value(requirement.payload.clone())?;
    for (index, item) in plan.items.iter().enumerate() {
        let create = &plan.steps[1 + index * 2];
        let responsibility = &plan.steps[2 + index * 2];
        if create.kind != buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateWork
            || responsibility.kind
                != buzz_sdk::MeetingV2ActionStepKind::ProjectViewSetWorkResponsibility
            || create.action_id != Some(item.action_id)
            || responsibility.action_id != Some(item.action_id)
            || create.target_object_id != responsibility.target_object_id
            || responsibility.payload != json!({})
        {
            return Err(DbError::InvalidData(format!(
                "Meeting action item {} does not have one ordered Work/responsibility pair",
                item.action_id
            )));
        }
        let payload: WorkStepPayload = serde_json::from_value(create.payload.clone())?;
        if payload.requirement_id != requirement.target_object_id || payload.title != item.summary {
            return Err(DbError::InvalidData(format!(
                "Meeting action Work {} is not bound to its Requirement and item summary",
                create.target_object_id
            )));
        }
    }
    Ok(())
}

#[derive(Debug)]
struct ActionStepRow {
    action_id: Option<Uuid>,
    step_order: i32,
    step_kind: String,
    desired_payload: Value,
    assignee_pubkey: Option<Vec<u8>>,
    resolved_role_id: Option<Uuid>,
    resolved_assignment_id: Option<Uuid>,
    target_object_id: Uuid,
    status: String,
    attempt_count: i32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RequirementStepPayload {
    title: String,
    #[serde(default)]
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkStepPayload {
    title: String,
    requirement_id: Uuid,
    #[serde(default)]
    description: Option<String>,
}

fn materialized_project_description(title: &str, description: Option<&str>) -> String {
    description
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(title)
        .to_owned()
}

#[allow(clippy::too_many_arguments)]
async fn apply_step_prepared_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    step_id: Uuid,
    attempt: i32,
    project_event_id: &[u8],
    expected_project_revision: i64,
    signed_project_event: &Event,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    if step_id.is_nil()
        || attempt <= 0
        || project_event_id.len() != 32
        || expected_project_revision <= 0
    {
        return Err(DbError::InvalidData(
            "invalid Meeting V2 prepared-step scalar".to_string(),
        ));
    }
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if run.action_phase != "applying"
        || run.action_condition != "runnable"
        || run.plan_event_id.is_none()
    {
        return Ok(AppliedCommand::rejected("action_not_applying", Some(&run)));
    }

    let Some(step) = load_action_step_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        step_id,
        true,
    )
    .await?
    else {
        return Ok(AppliedCommand::rejected(
            "action_step_not_found",
            Some(&run),
        ));
    };
    let next_step_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT step_id FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND status <> 'applied' ORDER BY step_order LIMIT 1",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_optional(tx.as_mut())
    .await?;
    if next_step_id != Some(step_id) {
        return Ok(AppliedCommand::rejected(
            "action_step_out_of_order",
            Some(&run),
        ));
    }
    if step.status != "pending" && step.status != "failed" {
        return Ok(AppliedCommand::rejected(
            "action_step_already_prepared",
            Some(&run),
        ));
    }
    let expected_attempt = step
        .attempt_count
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("Meeting action attempt overflow".to_string()))?;
    if attempt != expected_attempt {
        return Ok(AppliedCommand::rejected("stale_action_attempt", Some(&run)));
    }
    if let Some(rejection) = preflight_plan_assignees_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        now,
    )
    .await?
    {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    let step = load_action_step_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        step_id,
        true,
    )
    .await?
    .ok_or_else(|| DbError::InvalidData("prepared Meeting action step disappeared".to_string()))?;
    validate_prepared_project_event(
        &step,
        params.event,
        signed_project_event,
        project_event_id,
        expected_project_revision,
    )?;

    let signed_json = serde_json::to_value(signed_project_event)?;
    sqlx::query(
        "INSERT INTO meeting_v2_action_step_attempts \
             (community_id, session_id, action_run_id, step_id, action_window_epoch, \
              attempt_number, project_command_event_id, signed_project_event, \
              expected_project_revision, status, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'prepared', $10, $10)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(step_id)
    .bind(run.action_window_epoch)
    .bind(attempt)
    .bind(project_event_id)
    .bind(signed_json)
    .bind(expected_project_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_steps \
         SET status = 'prepared', attempt_count = $5, last_error_code = NULL, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND step_id = $4 AND status IN ('pending', 'failed')",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(step_id)
    .bind(attempt)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting action step changed while preparing its write".to_string(),
        ));
    }
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_step_prepared",
        params.event.id.as_bytes(),
        "applying/runnable",
        "applying/runnable",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_step_prepared",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({
            "step_id": step_id,
            "step_order": step.step_order,
            "step_kind": step.step_kind,
            "attempt": attempt,
            "project_event_id": hex::encode(project_event_id),
            "expected_project_revision": expected_project_revision,
        }),
    ))
}

async fn apply_step_applied_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    step_id: Uuid,
    project_event_id: &[u8],
    accepted_project_revision: i64,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    if step_id.is_nil() || project_event_id.len() != 32 || accepted_project_revision <= 0 {
        return Err(DbError::InvalidData(
            "invalid Meeting V2 applied-step scalar".to_string(),
        ));
    }
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if run.action_phase != "applying"
        || run.action_condition != "runnable"
        || run.plan_event_id.is_none()
    {
        return Ok(AppliedCommand::rejected("action_not_applying", Some(&run)));
    }
    let Some(step) = load_action_step_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        step_id,
        true,
    )
    .await?
    else {
        return Ok(AppliedCommand::rejected(
            "action_step_not_found",
            Some(&run),
        ));
    };
    if step.status == "applied" {
        return Ok(AppliedCommand::rejected(
            "action_step_already_applied",
            Some(&run),
        ));
    }
    if step.status != "prepared" {
        return Ok(AppliedCommand::rejected(
            "action_step_not_prepared",
            Some(&run),
        ));
    }
    let attempt_row = sqlx::query(
        "SELECT attempt_number, status, accepted_project_revision, signed_project_event \
         FROM meeting_v2_action_step_attempts \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND step_id = $4 AND project_command_event_id = $5 FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(step_id)
    .bind(project_event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(attempt_row) = attempt_row else {
        return Ok(AppliedCommand::rejected(
            "action_attempt_not_found",
            Some(&run),
        ));
    };
    let attempt_status: String = attempt_row.try_get("status")?;
    let attempt_revision: Option<i64> = attempt_row.try_get("accepted_project_revision")?;
    if attempt_status != "accepted" || attempt_revision != Some(accepted_project_revision) {
        return Ok(AppliedCommand::rejected(
            "project_receipt_not_accepted",
            Some(&run),
        ));
    }
    let change = sqlx::query(
        "SELECT actor_pubkey, operation, subject, project_revision, source_event_id, result \
         FROM project_view_changes \
         WHERE community_id = $1 AND change_id = $2",
    )
    .bind(params.community_id.as_uuid())
    .bind(project_event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(change) = change else {
        return Ok(AppliedCommand::rejected(
            "project_receipt_not_found",
            Some(&run),
        ));
    };
    let actor: Vec<u8> = change.try_get("actor_pubkey")?;
    let operation: String = change.try_get("operation")?;
    let subject: Value = change.try_get("subject")?;
    let revision: i64 = change.try_get("project_revision")?;
    let source_event_id: Option<Vec<u8>> = change.try_get("source_event_id")?;
    let result: Value = change.try_get("result")?;
    let signed_project_event: Value = attempt_row.try_get("signed_project_event")?;
    let signed_project_event: Event = serde_json::from_value(signed_project_event)?;
    if actor != params.event.pubkey.as_bytes()
        || revision != accepted_project_revision
        || source_event_id.as_deref() != Some(project_event_id)
        || !operation_matches_step(&operation, &step.step_kind)
        || !project_change_subject_matches_event(&subject, &step.step_kind, &signed_project_event)?
        || !project_receipt_result_matches_step(&result, &step, accepted_project_revision)
    {
        return Ok(AppliedCommand::rejected(
            "project_receipt_mismatch",
            Some(&run),
        ));
    }
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_steps \
         SET status = 'applied', accepted_project_revision = $5, \
             last_error_code = NULL, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND step_id = $4 AND status = 'prepared'",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(step_id)
    .bind(accepted_project_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting action step changed while applying its receipt".to_string(),
        ));
    }
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_step_applied",
        params.event.id.as_bytes(),
        "applying/runnable",
        "applying/runnable",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_step_applied",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({
            "step_id": step_id,
            "step_order": step.step_order,
            "step_kind": step.step_kind,
            "attempt": attempt_row.try_get::<i32, _>("attempt_number")?,
            "project_event_id": hex::encode(project_event_id),
            "accepted_project_revision": accepted_project_revision,
        }),
    ))
}

async fn load_action_step_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
    step_id: Uuid,
    for_update: bool,
) -> Result<Option<ActionStepRow>> {
    let query = if for_update {
        "SELECT action_id, step_order, step_kind, desired_payload, assignee_pubkey, \
                resolved_role_id, resolved_assignment_id, target_object_id, status, attempt_count \
         FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 AND step_id = $4 \
         FOR UPDATE"
    } else {
        "SELECT action_id, step_order, step_kind, desired_payload, assignee_pubkey, \
                resolved_role_id, resolved_assignment_id, target_object_id, status, attempt_count \
         FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 AND step_id = $4"
    };
    let row = sqlx::query(query)
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(action_run_id)
        .bind(step_id)
        .fetch_optional(tx.as_mut())
        .await?;
    row.map(|row| {
        Ok(ActionStepRow {
            action_id: row.try_get("action_id")?,
            step_order: row.try_get("step_order")?,
            step_kind: row.try_get("step_kind")?,
            desired_payload: row.try_get("desired_payload")?,
            assignee_pubkey: row.try_get("assignee_pubkey")?,
            resolved_role_id: row.try_get("resolved_role_id")?,
            resolved_assignment_id: row.try_get("resolved_assignment_id")?,
            target_object_id: row.try_get("target_object_id")?,
            status: row.try_get("status")?,
            attempt_count: row.try_get("attempt_count")?,
        })
    })
    .transpose()
}

async fn preflight_plan_assignees_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<&'static str>> {
    let v2_ready: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM communities community \
             JOIN project_view_state state ON state.community_id = community.id \
             WHERE community.id = $1 AND community.project_view_enabled \
               AND community.archived_at IS NULL \
               AND community.project_view_schema_version = 2 \
               AND state.schema_version = 2 \
         )",
    )
    .bind(community_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if !v2_ready {
        return Ok(Some("project_view_v2_unavailable"));
    }
    let rows = sqlx::query(
        "SELECT DISTINCT assignee_pubkey FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND assignee_pubkey IS NOT NULL ORDER BY assignee_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut resolved = Vec::with_capacity(rows.len());
    for row in rows {
        let assignee: Vec<u8> = row.try_get("assignee_pubkey")?;
        let assignment = sqlx::query(
            "SELECT assignment.assignment_id, assignment.role_id \
             FROM project_role_assignments assignment \
             JOIN project_view_objects role \
               ON role.community_id = assignment.community_id \
              AND role.object_id = assignment.role_id \
              AND role.object_type = 'role' \
              AND role.deleted_at IS NULL \
              AND role.body->'active' = 'true'::jsonb \
             WHERE assignment.community_id = $1 \
               AND assignment.member_pubkey = $2 \
               AND assignment.ended_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(&assignee))
        .fetch_optional(tx.as_mut())
        .await?;
        let Some(assignment) = assignment else {
            return Ok(Some("assignee_unresolved"));
        };
        resolved.push((
            assignee,
            assignment.try_get::<Uuid, _>("role_id")?,
            assignment.try_get::<Uuid, _>("assignment_id")?,
        ));
    }
    for (assignee, role_id, assignment_id) in &resolved {
        let mapping_changed: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
                 SELECT 1 FROM meeting_v2_action_steps \
                 WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                   AND assignee_pubkey = $4 \
                   AND (resolved_role_id IS NOT NULL OR resolved_assignment_id IS NOT NULL) \
                   AND (resolved_role_id IS DISTINCT FROM $5 \
                        OR resolved_assignment_id IS DISTINCT FROM $6) \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(action_run_id)
        .bind(assignee)
        .bind(*role_id)
        .bind(*assignment_id)
        .fetch_one(tx.as_mut())
        .await?;
        if mapping_changed {
            return Ok(Some("assignee_mapping_changed"));
        }
    }
    for (assignee, role_id, assignment_id) in resolved {
        sqlx::query(
            "UPDATE meeting_v2_action_steps \
             SET resolved_role_id = $5, resolved_assignment_id = $6, updated_at = $7 \
             WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
               AND assignee_pubkey = $4",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(action_run_id)
        .bind(&assignee)
        .bind(role_id)
        .bind(assignment_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(None)
}

fn validate_prepared_project_event(
    step: &ActionStepRow,
    meeting_event: &Event,
    project_event: &Event,
    project_event_id: &[u8],
    expected_project_revision: i64,
) -> Result<()> {
    project_event.verify().map_err(|error| {
        DbError::InvalidData(format!("invalid prepared Project View event: {error}"))
    })?;
    let mut tags = project_event.tags.iter();
    if project_event.id.as_bytes() != project_event_id
        || project_event.pubkey != meeting_event.pubkey
        || project_event.kind.as_u16() as u32 != buzz_core::kind::KIND_PROJECT_VIEW_MUTATION
        || project_event.tags.len() != 2
        || tags.next().is_none_or(|tag| tag.as_slice() != ["-"])
        || tags
            .next()
            .is_none_or(|tag| tag.as_slice() != ["t", "buzz-project-view-mutation"])
    {
        return Err(DbError::InvalidData(
            "prepared Project View event envelope does not match the Meeting step".to_string(),
        ));
    }
    let expected_revision = u64::try_from(expected_project_revision).map_err(|_| {
        DbError::InvalidData("prepared Project revision is not representable".to_string())
    })?;
    match step.step_kind.as_str() {
        "project_view.create_requirement" => {
            let payload: RequirementStepPayload =
                serde_json::from_value(step.desired_payload.clone())?;
            let command = ProjectObjectCommand::from_json(&project_event.content)
                .map_err(|error| DbError::InvalidData(error.to_string()))?;
            let MutationRequest::Create(CreateMutation {
                object:
                    NewProjectViewObject::Requirement {
                        id,
                        title,
                        description,
                        status,
                        priority,
                        planned_in_stage_id,
                    },
            }) = command.request
            else {
                return Err(DbError::InvalidData(
                    "prepared event is not the planned Requirement create".to_string(),
                ));
            };
            if command.expected_project_revision != expected_revision
                || id != step.target_object_id
                || title != payload.title
                || description
                    != materialized_project_description(
                        &payload.title,
                        payload.description.as_deref(),
                    )
                || status != RequirementStatus::Ready
                || priority != Priority::Normal
                || planned_in_stage_id.is_some()
            {
                return Err(DbError::InvalidData(
                    "prepared Requirement create differs from its frozen plan".to_string(),
                ));
            }
        }
        "project_view.create_work" => {
            let payload: WorkStepPayload = serde_json::from_value(step.desired_payload.clone())?;
            let command = ProjectObjectCommand::from_json(&project_event.content)
                .map_err(|error| DbError::InvalidData(error.to_string()))?;
            let MutationRequest::Create(CreateMutation {
                object:
                    NewProjectViewObject::Work {
                        id,
                        title,
                        description,
                        status,
                        priority,
                        handles,
                    },
            }) = command.request
            else {
                return Err(DbError::InvalidData(
                    "prepared event is not the planned Work create".to_string(),
                ));
            };
            if command.expected_project_revision != expected_revision
                || id != step.target_object_id
                || title != payload.title
                || description
                    != materialized_project_description(
                        &payload.title,
                        payload.description.as_deref(),
                    )
                || status != WorkStatus::Pending
                || priority != Priority::Normal
                || handles
                    != (ObjectRef {
                        object_type: ProjectViewObjectType::Requirement,
                        object_id: payload.requirement_id,
                    })
            {
                return Err(DbError::InvalidData(
                    "prepared Work create differs from its frozen plan".to_string(),
                ));
            }
        }
        "project_view.set_work_responsibility" => {
            if step.action_id.is_none() || step.assignee_pubkey.is_none() {
                return Err(DbError::InvalidData(
                    "responsibility step has no frozen action assignee".to_string(),
                ));
            }
            let command = RoleCommand::from_json(&project_event.content)
                .map_err(|error| DbError::InvalidData(error.to_string()))?;
            let RoleCommandRequest::SetWorkResponsibility {
                work_id,
                responsible_role_id,
            } = command.request
            else {
                return Err(DbError::InvalidData(
                    "prepared event is not the planned Work responsibility command".to_string(),
                ));
            };
            if command.expected_project_revision != expected_revision
                || work_id != step.target_object_id
                || responsible_role_id != step.resolved_role_id
                || step.resolved_role_id.is_none()
                || step.resolved_assignment_id.is_none()
            {
                return Err(DbError::InvalidData(
                    "prepared Work responsibility differs from its frozen plan".to_string(),
                ));
            }
        }
        _ => {
            return Err(DbError::InvalidData(format!(
                "unsupported Meeting action step kind: {}",
                step.step_kind
            )));
        }
    }
    Ok(())
}

fn operation_matches_step(operation: &str, step_kind: &str) -> bool {
    matches!(
        (operation, step_kind),
        (
            "create",
            "project_view.create_requirement" | "project_view.create_work"
        ) | (
            "set_work_responsibility",
            "project_view.set_work_responsibility"
        )
    )
}

fn project_change_subject_matches_event(
    subject: &Value,
    step_kind: &str,
    event: &Event,
) -> Result<bool> {
    let expected = match step_kind {
        "project_view.create_requirement" | "project_view.create_work" => {
            let command = ProjectObjectCommand::from_json(&event.content)
                .map_err(|error| DbError::InvalidData(error.to_string()))?;
            serde_json::to_value(command.request)?
        }
        "project_view.set_work_responsibility" => {
            let command = RoleCommand::from_json(&event.content)
                .map_err(|error| DbError::InvalidData(error.to_string()))?;
            serde_json::to_value(command.request)?
        }
        _ => return Ok(false),
    };
    Ok(subject == &expected)
}

fn project_receipt_result_matches_step(
    result: &Value,
    step: &ActionStepRow,
    accepted_project_revision: i64,
) -> bool {
    let Ok(accepted_project_revision) = u64::try_from(accepted_project_revision) else {
        return false;
    };
    if result.get("project_revision").and_then(Value::as_u64) != Some(accepted_project_revision) {
        return false;
    }
    match step.step_kind.as_str() {
        "project_view.create_requirement" | "project_view.create_work" => {
            result.get("operation").and_then(Value::as_str) == Some("create")
                && result
                    .get("object_id")
                    .and_then(Value::as_str)
                    .and_then(|value| Uuid::parse_str(value).ok())
                    == Some(step.target_object_id)
                && result
                    .get("object_revision")
                    .and_then(Value::as_u64)
                    .is_some_and(|revision| revision > 0)
                && result.get("deleted").and_then(Value::as_bool) == Some(false)
        }
        "project_view.set_work_responsibility" => {
            let Some(resolved_role_id) = step.resolved_role_id else {
                return false;
            };
            result.get("operation").and_then(Value::as_str) == Some("set_work_responsibility")
                && result
                    .get("changed_objects")
                    .and_then(Value::as_array)
                    .is_some_and(|objects| {
                        objects.len() == 1
                            && objects[0].get("object_type").and_then(Value::as_str) == Some("work")
                            && objects[0]
                                .get("object_id")
                                .and_then(Value::as_str)
                                .and_then(|value| Uuid::parse_str(value).ok())
                                == Some(step.target_object_id)
                            && objects[0]
                                .get("object_revision")
                                .and_then(Value::as_u64)
                                .is_some_and(|revision| revision > 0)
                            && objects[0]
                                .get("responsible_role_id")
                                .and_then(Value::as_str)
                                .and_then(|value| Uuid::parse_str(value).ok())
                                == Some(resolved_role_id)
                    })
        }
        _ => false,
    }
}

/// Locked Meeting attempt carried by an in-flight Project View transaction.
#[derive(Debug, Clone)]
pub(crate) struct PreparedActionProjectEvent {
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
    step_id: Uuid,
    attempt_number: i32,
    project_event_id: Vec<u8>,
}

/// If a Project View command was registered by a Meeting action run, lock and
/// validate that exact attempt before the Project reducer is allowed to run.
pub(crate) async fn fence_prepared_project_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event: &Event,
    expected_project_revision: u64,
) -> Result<Option<PreparedActionProjectEvent>> {
    let session_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT session_id FROM meeting_v2_action_step_attempts \
         WHERE community_id = $1 AND project_command_event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    crate::meeting_baton::lock_baton_session_tx(tx, community_id, session_id).await?;
    let row = sqlx::query(
        "SELECT attempt.session_id, attempt.action_run_id, attempt.step_id, \
                attempt.attempt_number, attempt.action_window_epoch, \
                attempt.expected_project_revision, attempt.signed_project_event, \
                attempt.status AS attempt_status, step.status AS step_status, \
                step.attempt_count, run.action_window_epoch AS current_action_window, \
                run.action_phase, run.action_condition, run.action_deadline_at, \
                run.terminal_status, clock_timestamp() AS database_now, \
                session.status AS meeting_status, session.host_pubkey \
         FROM meeting_v2_action_step_attempts attempt \
         JOIN meeting_v2_action_steps step \
           ON step.community_id = attempt.community_id \
          AND step.session_id = attempt.session_id \
          AND step.action_run_id = attempt.action_run_id \
          AND step.step_id = attempt.step_id \
         JOIN meeting_v2_action_runs run \
           ON run.community_id = attempt.community_id \
          AND run.session_id = attempt.session_id \
          AND run.action_run_id = attempt.action_run_id \
         JOIN meeting_sessions session \
           ON session.community_id = attempt.community_id \
          AND session.session_id = attempt.session_id \
         WHERE attempt.community_id = $1 AND attempt.project_command_event_id = $2 \
           AND attempt.session_id = $3 \
         FOR UPDATE OF attempt, step, run",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let signed_event: Value = row.try_get("signed_project_event")?;
    let exact_event = serde_json::to_value(event)?;
    let stored_revision: Option<i64> = row.try_get("expected_project_revision")?;
    let expected_revision = i64::try_from(expected_project_revision).map_err(|_| {
        DbError::InvalidData("Project revision exceeds Meeting ledger range".to_string())
    })?;
    let attempt_number: i32 = row.try_get("attempt_number")?;
    let attempt_count: i32 = row.try_get("attempt_count")?;
    let attempt_window: i64 = row.try_get("action_window_epoch")?;
    let current_window: i64 = row.try_get("current_action_window")?;
    let action_deadline: Option<DateTime<Utc>> = row.try_get("action_deadline_at")?;
    let database_now: DateTime<Utc> = row.try_get("database_now")?;
    let host_pubkey: Vec<u8> = row.try_get("host_pubkey")?;
    let valid = row.try_get::<String, _>("attempt_status")? == "prepared"
        && row.try_get::<String, _>("step_status")? == "prepared"
        && row.try_get::<String, _>("action_phase")? == "applying"
        && row.try_get::<String, _>("action_condition")? == "runnable"
        && action_deadline.is_some_and(|deadline| deadline > database_now)
        && row
            .try_get::<Option<String>, _>("terminal_status")?
            .is_none()
        && row.try_get::<String, _>("meeting_status")? == "active"
        && attempt_number == attempt_count
        && attempt_window == current_window
        && stored_revision == Some(expected_revision)
        && signed_event == exact_event
        && host_pubkey == event.pubkey.as_bytes();
    if !valid {
        return Err(DbError::AccessDenied(
            "prepared Meeting action Project event is no longer publishable".to_string(),
        ));
    }
    Ok(Some(PreparedActionProjectEvent {
        community_id,
        session_id,
        action_run_id: row.try_get("action_run_id")?,
        step_id: row.try_get("step_id")?,
        attempt_number,
        project_event_id: event.id.as_bytes().to_vec(),
    }))
}

/// Mark a Meeting-owned Project View event accepted inside the same
/// transaction that stores its canonical Project receipt and projections.
pub(crate) async fn accept_prepared_project_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    binding: &PreparedActionProjectEvent,
    accepted_project_revision: u64,
    now: DateTime<Utc>,
) -> Result<()> {
    let revision = i64::try_from(accepted_project_revision).map_err(|_| {
        DbError::InvalidData("accepted Project revision exceeds Meeting ledger range".to_string())
    })?;
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_step_attempts \
         SET status = 'accepted', accepted_project_revision = $8, \
             error_code = NULL, updated_at = $9 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND step_id = $4 AND attempt_number = $5 \
           AND project_command_event_id = $6 AND status = $7",
    )
    .bind(binding.community_id.as_uuid())
    .bind(binding.session_id)
    .bind(binding.action_run_id)
    .bind(binding.step_id)
    .bind(binding.attempt_number)
    .bind(&binding.project_event_id)
    .bind("prepared")
    .bind(revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting action attempt changed before Project acceptance".to_string(),
        ));
    }
    Ok(())
}

impl Db {
    /// Record a deterministic Project View rejection so the same frozen step
    /// can prepare a new event at a freshly verified revision.
    pub async fn reject_meeting_action_project_event(
        &self,
        community_id: CommunityId,
        project_event_id: &[u8],
        error_code: &str,
        relay_keys: &Keys,
    ) -> Result<bool> {
        if project_event_id.len() != 32 || error_code.is_empty() || error_code.len() > 128 {
            return Err(DbError::InvalidData(
                "invalid rejected Meeting action Project event".to_string(),
            ));
        }
        let mut tx = self.begin_transaction().await?;
        let session_id: Option<Uuid> = sqlx::query_scalar(
            "SELECT session_id \
             FROM meeting_v2_action_step_attempts \
             WHERE community_id = $1 AND project_command_event_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(project_event_id)
        .fetch_optional(tx.as_mut())
        .await?;
        let Some(session_id) = session_id else {
            tx.rollback().await?;
            return Ok(false);
        };
        crate::meeting_baton::lock_baton_session_tx(&mut tx, community_id, session_id).await?;
        let row = sqlx::query(
            "SELECT action_run_id, step_id, attempt_number \
             FROM meeting_v2_action_step_attempts \
             WHERE community_id = $1 AND session_id = $2 \
               AND project_command_event_id = $3 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(project_event_id)
        .fetch_optional(tx.as_mut())
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(false);
        };
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(tx.as_mut())
            .await?;
        let action_run_id: Uuid = row.try_get("action_run_id")?;
        let step_id: Uuid = row.try_get("step_id")?;
        let attempt_number: i32 = row.try_get("attempt_number")?;
        let updated = sqlx::query(
            "UPDATE meeting_v2_action_step_attempts \
             SET status = 'rejected', error_code = $7, updated_at = $8 \
             WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
               AND step_id = $4 AND attempt_number = $5 \
               AND project_command_event_id = $6 AND status = 'prepared'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(action_run_id)
        .bind(step_id)
        .bind(attempt_number)
        .bind(project_event_id)
        .bind(error_code)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        if updated.rows_affected() == 1 {
            sqlx::query(
                "UPDATE meeting_v2_action_steps \
                 SET status = 'pending', last_error_code = $5, updated_at = $6 \
                 WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                   AND step_id = $4 AND status = 'prepared'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(action_run_id)
            .bind(step_id)
            .bind(error_code)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            crate::meeting_baton::publish_v2_action_transition_tx(
                &mut tx,
                community_id,
                session_id,
                action_run_id,
                relay_keys,
                "action_project_event_rejected",
                project_event_id,
                "applying/runnable",
                "applying/runnable",
                None,
                now,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(updated.rows_affected() == 1)
    }
}

async fn apply_block_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    reason_code: &str,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    if !is_block_reason(reason_code) {
        return Err(DbError::InvalidData(format!(
            "unsupported Meeting V2 action block reason: {reason_code}"
        )));
    }
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if !matches!(run.action_phase.as_str(), "planning" | "applying")
        || run.action_condition != "runnable"
    {
        return Ok(AppliedCommand::rejected("action_not_runnable", Some(&run)));
    }
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_condition = 'blocked', action_deadline_at = NULL, \
             last_error_code = $4, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(reason_code)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_blocked",
        params.event.id.as_bytes(),
        &format!("{}/runnable", run.action_phase),
        &format!("{}/blocked", run.action_phase),
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_blocked",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({"reason_code": reason_code, "phase": run.action_phase}),
    ))
}

async fn apply_retry_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if !matches!(run.action_phase.as_str(), "planning" | "applying")
        || run.action_condition != "blocked"
    {
        return Ok(AppliedCommand::rejected("action_not_blocked", Some(&run)));
    }
    let next_window = run
        .action_window_epoch
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("Meeting V2 action window overflow".to_string()))?;
    let retry_reason = run.last_error_code.as_deref().unwrap_or("unspecified");
    let duration_ms = action_duration_ms_tx(tx, params.community_id, params.session_id).await?;
    let deadline = now + Duration::milliseconds(duration_ms);
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_window_epoch = $4, action_condition = 'runnable', \
             action_deadline_at = $5, last_error_code = NULL, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_condition = 'blocked'",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(next_window)
    .bind(deadline)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_retried",
        params.event.id.as_bytes(),
        &format!("{}/blocked", run.action_phase),
        &format!("{}/runnable", run.action_phase),
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_retried",
        run.action_run_id,
        next_window,
        transition.state_revision,
        json!({
            "action_deadline_at_ms": deadline.timestamp_millis(),
            "retry_reason": retry_reason,
            "phase": run.action_phase,
        }),
    ))
}

async fn apply_complete_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if run.action_phase != "applying"
        || run.action_condition != "runnable"
        || run.plan_event_id.is_none()
    {
        return Ok(AppliedCommand::rejected("action_not_applying", Some(&run)));
    }
    let (total, applied): (i64, i64) = sqlx::query_as(
        "SELECT count(*)::BIGINT, count(*) FILTER (WHERE status = 'applied')::BIGINT \
         FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_one(tx.as_mut())
    .await?;
    if total == 0 || applied != total {
        return Ok(AppliedCommand::rejected(
            "action_steps_incomplete",
            Some(&run),
        ));
    }
    let Some(completion_project_revision) = verify_action_projection_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
    )
    .await?
    else {
        return Ok(AppliedCommand::rejected(
            "action_projection_mismatch",
            Some(&run),
        ));
    };
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_phase = 'ready_to_close', completion_project_revision = $4, \
             action_deadline_at = NULL, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_phase = 'applying' \
           AND action_condition = 'runnable'",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(completion_project_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_ready_to_close",
        params.event.id.as_bytes(),
        "applying/runnable",
        "ready_to_close/runnable",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_ready_to_close",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({
            "required_step_count": total,
            "applied_step_count": applied,
            "completion_project_revision": completion_project_revision,
        }),
    ))
}

async fn verify_action_projection_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
) -> Result<Option<i64>> {
    let state = sqlx::query(
        "SELECT project_revision, schema_version FROM project_view_state \
         WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(state) = state else {
        return Ok(None);
    };
    if state.try_get::<i16, _>("schema_version")? != 2 {
        return Ok(None);
    }
    let project_revision: i64 = state.try_get("project_revision")?;
    let rows = sqlx::query(
        "SELECT step.step_kind, step.desired_payload, step.target_object_id, \
                step.resolved_role_id, attempt.project_command_event_id, \
                object.object_type, object.body, object.handles_object_id, \
                object.handles_object_type, object.responsible_role_id, \
                object.source_event_id, object.deleted_at \
         FROM meeting_v2_action_steps step \
         JOIN meeting_v2_action_step_attempts attempt \
           ON attempt.community_id = step.community_id \
          AND attempt.session_id = step.session_id \
          AND attempt.action_run_id = step.action_run_id \
          AND attempt.step_id = step.step_id \
          AND attempt.status = 'accepted' \
          AND attempt.accepted_project_revision = step.accepted_project_revision \
         LEFT JOIN project_view_objects object \
           ON object.community_id = step.community_id \
          AND object.object_id = step.target_object_id \
         WHERE step.community_id = $1 AND step.session_id = $2 \
           AND step.action_run_id = $3 AND step.status = 'applied' \
         ORDER BY step.step_order",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    for row in rows {
        let step_kind: String = row.try_get("step_kind")?;
        let body: Option<Value> = row.try_get("body")?;
        let source_event_id: Option<Vec<u8>> = row.try_get("source_event_id")?;
        let project_event_id: Vec<u8> = row.try_get("project_command_event_id")?;
        if row
            .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
            .is_some()
        {
            return Ok(None);
        }
        match step_kind.as_str() {
            "project_view.create_requirement" => {
                let payload: RequirementStepPayload =
                    serde_json::from_value(row.try_get("desired_payload")?)?;
                let Some(body) = body else {
                    return Ok(None);
                };
                let requirement: Requirement = serde_json::from_value(body)?;
                if row.try_get::<Option<String>, _>("object_type")?.as_deref()
                    != Some("requirement")
                    || requirement.title != payload.title
                    || requirement.description
                        != materialized_project_description(
                            &payload.title,
                            payload.description.as_deref(),
                        )
                    || requirement.status != RequirementStatus::Ready
                    || requirement.priority != Priority::Normal
                    || source_event_id != Some(project_event_id)
                {
                    return Ok(None);
                }
            }
            "project_view.create_work" => {
                let payload: WorkStepPayload =
                    serde_json::from_value(row.try_get("desired_payload")?)?;
                let Some(body) = body else {
                    return Ok(None);
                };
                let work: ProjectWork = serde_json::from_value(body)?;
                if row.try_get::<Option<String>, _>("object_type")?.as_deref() != Some("work")
                    || work.title != payload.title
                    || work.description
                        != materialized_project_description(
                            &payload.title,
                            payload.description.as_deref(),
                        )
                    || work.status != WorkStatus::Pending
                    || work.priority != Priority::Normal
                    || row.try_get::<Option<Uuid>, _>("handles_object_id")?
                        != Some(payload.requirement_id)
                    || row
                        .try_get::<Option<String>, _>("handles_object_type")?
                        .as_deref()
                        != Some("requirement")
                {
                    return Ok(None);
                }
            }
            "project_view.set_work_responsibility" => {
                if row.try_get::<Option<String>, _>("object_type")?.as_deref() != Some("work")
                    || row.try_get::<Option<Uuid>, _>("responsible_role_id")?
                        != row.try_get::<Option<Uuid>, _>("resolved_role_id")?
                    || source_event_id != Some(project_event_id)
                {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        }
    }
    Ok(Some(project_revision))
}

async fn apply_return_to_board_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if !matches!(run.action_phase.as_str(), "planning" | "applying") {
        return Ok(AppliedCommand::rejected("action_cannot_return", Some(&run)));
    }
    sqlx::query(
        "SELECT step_id FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
         ORDER BY step_order FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query(
        "SELECT step_id, attempt_number FROM meeting_v2_action_step_attempts \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
         ORDER BY step_id, attempt_number FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    let accepted_or_applied: bool = sqlx::query_scalar(
        "SELECT \
           EXISTS (SELECT 1 FROM meeting_v2_action_steps \
                   WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                     AND status = 'applied') \
           OR EXISTS (SELECT 1 FROM meeting_v2_action_step_attempts \
                      WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                        AND status = 'accepted')",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_one(tx.as_mut())
    .await?;
    if accepted_or_applied {
        return Ok(AppliedCommand::rejected(
            "action_has_external_effects",
            Some(&run),
        ));
    }
    let unstable_attempt: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_v2_action_step_attempts \
             WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
               AND status IN ('published', 'indeterminate'))",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_one(tx.as_mut())
    .await?;
    if unstable_attempt {
        return Ok(AppliedCommand::rejected(
            "action_attempt_in_flight",
            Some(&run),
        ));
    }
    sqlx::query(
        "UPDATE meeting_v2_action_step_attempts \
         SET status = 'abandoned', error_code = 'returned_to_board', updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND status IN ('prepared', 'rejected')",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_v2_action_steps \
         SET status = 'abandoned', last_error_code = 'returned_to_board', updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND status <> 'applied'",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET terminal_status = 'returned_to_board', terminal_at = $4, \
             action_deadline_at = NULL, updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let board_runtime = crate::meeting_v2::open_board_window_tx(
        tx,
        params.community_id,
        params.session_id,
        run.control_epoch,
        now,
    )
    .await?;
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_returned_to_board",
        params.event.id.as_bytes(),
        &format!("{}/{}", run.action_phase, run.action_condition),
        "board_pending",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_returned_to_board",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({"board_window": board_runtime.board_window}),
    ))
}

fn validate_run_fence(run: &ActionRunRow, fence: &ActionRunFence) -> Option<&'static str> {
    if run.terminal_status.is_some() {
        return Some("action_run_terminal");
    }
    if run.action_run_id != fence.action_run_id {
        return Some("stale_action_run");
    }
    if run.action_window_epoch != fence.action_window_epoch {
        return Some("stale_action_window");
    }
    if run.plan_event_id != fence.plan_event_id {
        return Some("stale_action_plan");
    }
    None
}

async fn load_active_run_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    for_update: bool,
) -> Result<Option<ActionRunRow>> {
    let sql = if for_update {
        "SELECT action_run_id, plan_event_id, board_event_id, control_epoch, \
                action_window_epoch, action_phase, action_condition, terminal_status, \
                last_error_code \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL \
         FOR UPDATE"
    } else {
        "SELECT action_run_id, plan_event_id, board_event_id, control_epoch, \
                action_window_epoch, action_phase, action_condition, terminal_status, \
                last_error_code \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL"
    };
    let row = sqlx::query(sql)
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?;
    row.map(action_run_from_row).transpose()
}

fn action_run_from_row(row: sqlx::postgres::PgRow) -> Result<ActionRunRow> {
    Ok(ActionRunRow {
        action_run_id: row.try_get("action_run_id")?,
        plan_event_id: row.try_get("plan_event_id")?,
        board_event_id: row.try_get("board_event_id")?,
        control_epoch: row.try_get("control_epoch")?,
        action_window_epoch: row.try_get("action_window_epoch")?,
        action_phase: row.try_get("action_phase")?,
        action_condition: row.try_get("action_condition")?,
        terminal_status: row.try_get("terminal_status")?,
        last_error_code: row.try_get("last_error_code")?,
    })
}

async fn has_unresolved_floor_work_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<bool> {
    sqlx::query_scalar(
        "SELECT \
            EXISTS (SELECT 1 FROM meeting_speech_intents \
                    WHERE community_id = $1 AND session_id = $2 AND state = 'pending') \
            OR EXISTS (SELECT 1 FROM meeting_human_floor_requests \
                       WHERE community_id = $1 AND session_id = $2 \
                         AND state IN ('queued', 'offered')) \
            OR EXISTS (SELECT 1 FROM meeting_directed_handoffs \
                       WHERE community_id = $1 AND session_id = $2 \
                         AND question_state IN ('open', 'blocked')) \
            OR EXISTS (SELECT 1 FROM meeting_moderator_decision_attempts \
                       WHERE community_id = $1 AND session_id = $2 AND state = 'running')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(Into::into)
}

async fn has_human_floor_work_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM meeting_human_floor_requests \
                        WHERE community_id = $1 AND session_id = $2 \
                          AND state IN ('queued', 'offered'))",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(Into::into)
}

async fn freeze_floor_work_for_actions_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    terminal_event_id: &[u8],
    now: DateTime<Utc>,
) -> Result<(u64, u64)> {
    let intents = sqlx::query(
        "UPDATE meeting_speech_intents \
         SET state = 'ended', terminal_event_id = $3, terminal_at = $4, \
             updated_at = $4, last_attempt_outcome = 'ended', \
             deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL \
         WHERE community_id = $1 AND session_id = $2 AND state = 'pending'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(terminal_event_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let handoffs = sqlx::query(
        "UPDATE meeting_directed_handoffs \
         SET question_state = 'ended', last_attempt_outcome = 'ended', terminal_at = $3 \
         WHERE community_id = $1 AND session_id = $2 \
           AND question_state IN ('open', 'blocked')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok((intents.rows_affected(), handoffs.rows_affected()))
}

async fn action_duration_ms_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT action_finalization_ms FROM meeting_v2_config \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        DbError::InvalidData(format!(
            "Meeting V2 {session_id} has no action-finalization config"
        ))
    })
}

async fn validate_plan_roster_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    plan: &buzz_sdk::MeetingV2ActionPlan,
) -> Result<()> {
    let roster: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT pubkey FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    let roster: HashSet<Vec<u8>> = roster.into_iter().collect();
    for item in &plan.items {
        let assignee = hex::decode(&item.assignee_pubkey).map_err(|_| {
            DbError::InvalidData("Meeting V2 action assignee is not hex".to_string())
        })?;
        if !roster.contains(&assignee) {
            return Err(DbError::InvalidData(format!(
                "Meeting V2 action assignee is not in the frozen roster: {}",
                item.assignee_pubkey
            )));
        }
    }
    Ok(())
}

async fn insert_plan_steps_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    run: &ActionRunRow,
    plan: &buzz_sdk::MeetingV2ActionPlan,
    now: DateTime<Utc>,
) -> Result<()> {
    let mut assignees = HashMap::with_capacity(plan.items.len());
    for item in &plan.items {
        let assignee = hex::decode(&item.assignee_pubkey).map_err(|_| {
            DbError::InvalidData("Meeting V2 action assignee is not hex".to_string())
        })?;
        assignees.insert(item.action_id, assignee);
    }
    for (index, step) in plan.steps.iter().enumerate() {
        let step_order = i32::try_from(index + 1).map_err(|_| {
            DbError::InvalidData("Meeting V2 action step order overflow".to_string())
        })?;
        let (step_kind, target_object_type) = match step.kind {
            buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateRequirement => {
                ("project_view.create_requirement", "requirement")
            }
            buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateWork => {
                ("project_view.create_work", "work")
            }
            buzz_sdk::MeetingV2ActionStepKind::ProjectViewSetWorkResponsibility => {
                ("project_view.set_work_responsibility", "work")
            }
        };
        let assignee = step.action_id.and_then(|id| assignees.get(&id));
        sqlx::query(
            "INSERT INTO meeting_v2_action_steps \
                 (community_id, session_id, action_run_id, action_id, step_id, step_order, \
                  step_kind, desired_payload, assignee_pubkey, target_object_type, \
                  target_object_id, status, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'pending', $12, $12)",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(run.action_run_id)
        .bind(step.action_id)
        .bind(step.step_id)
        .bind(step_order)
        .bind(step_kind)
        .bind(&step.payload)
        .bind(assignee)
        .bind(target_object_type)
        .bind(step.target_object_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(())
}

fn is_block_reason(reason: &str) -> bool {
    matches!(
        reason,
        "project_view_v2_unavailable"
            | "assignee_unresolved"
            | "assignee_mapping_changed"
            | "object_id_conflict"
            | "responsibility_conflict"
            | "missing_dependency"
            | "provenance_mismatch"
            | "provider_failure"
            | "affinity_lost"
            | "action_deadline_exceeded"
    )
}

async fn load_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8],
) -> Result<Option<ActionReceipt>> {
    let row = sqlx::query(
        "SELECT author_pubkey, accepted, outcome_code, response_json \
         FROM meeting_v2_action_command_receipts \
         WHERE community_id = $1 AND command_event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(|row| {
        Ok(ActionReceipt {
            author_pubkey: row.try_get("author_pubkey")?,
            accepted: row.try_get("accepted")?,
            outcome_code: row.try_get("outcome_code")?,
            response: row.try_get("response_json")?,
        })
    })
    .transpose()
}

async fn insert_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    applied: &AppliedCommand,
    response: &Value,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_v2_action_command_receipts \
             (community_id, session_id, command_event_id, author_pubkey, action, \
              action_run_id, action_window_epoch, accepted, outcome_code, response_json) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(params.event.pubkey.as_bytes())
    .bind(params.command.action())
    .bind(applied.action_run_id)
    .bind(applied.action_window_epoch)
    .bind(applied.accepted)
    .bind(applied.outcome_code)
    .bind(response)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

pub(crate) async fn action_state_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<Value>> {
    let row = sqlx::query(
        "SELECT run.action_run_id, run.plan_event_id, run.board_event_id, \
                run.control_epoch, run.board_window, run.action_window_epoch, \
                run.action_phase, run.action_condition, run.terminal_status, \
                run.completion_project_revision, run.action_deadline_at, \
                run.last_error_code, run.created_at, run.updated_at, run.terminal_at, \
                count(step.step_id)::BIGINT AS required_step_count, \
                count(step.step_id) FILTER (WHERE step.status = 'applied')::BIGINT \
                    AS applied_step_count \
         FROM meeting_v2_action_runs run \
         LEFT JOIN meeting_v2_action_steps step \
           ON step.community_id = run.community_id \
          AND step.session_id = run.session_id \
          AND step.action_run_id = run.action_run_id \
         WHERE run.community_id = $1 AND run.session_id = $2 \
         GROUP BY run.community_id, run.session_id, run.action_run_id \
         ORDER BY (run.terminal_status IS NULL) DESC, run.created_at DESC \
         LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let action_run_id: Uuid = row.try_get("action_run_id")?;
    let step_rows = sqlx::query(
        "SELECT step.action_id, step.step_id, step.step_order, step.step_kind, \
                step.desired_payload, step.assignee_pubkey, step.resolved_role_id, \
                step.resolved_assignment_id, step.target_object_type, \
                step.target_object_id, step.accepted_project_revision, step.status, \
                step.last_error_code, step.attempt_count, \
                attempt.attempt_number AS current_attempt_number, \
                attempt.project_command_event_id AS current_project_event_id, \
                attempt.expected_project_revision AS current_expected_project_revision, \
                attempt.accepted_project_revision AS current_accepted_project_revision, \
                attempt.status AS current_attempt_status, \
                attempt.error_code AS current_attempt_error_code \
         FROM meeting_v2_action_steps step \
         LEFT JOIN LATERAL ( \
             SELECT attempt_number, project_command_event_id, \
                    expected_project_revision, accepted_project_revision, status, error_code \
             FROM meeting_v2_action_step_attempts attempt \
             WHERE attempt.community_id = step.community_id \
               AND attempt.session_id = step.session_id \
               AND attempt.action_run_id = step.action_run_id \
               AND attempt.step_id = step.step_id \
             ORDER BY attempt_number DESC LIMIT 1 \
         ) attempt ON TRUE \
         WHERE step.community_id = $1 AND step.session_id = $2 \
           AND step.action_run_id = $3 ORDER BY step.step_order",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    let steps = step_rows
        .into_iter()
        .map(|step| {
            let assignee: Option<Vec<u8>> = step.try_get("assignee_pubkey")?;
            let project_event: Option<Vec<u8>> = step.try_get("current_project_event_id")?;
            let current_attempt = step
                .try_get::<Option<i32>, _>("current_attempt_number")?
                .map(|attempt_number| {
                    Ok::<Value, DbError>(json!({
                        "attempt": attempt_number,
                        "project_event_id": project_event.map(hex::encode),
                        "expected_project_revision": step.try_get::<Option<i64>, _>("current_expected_project_revision")?,
                        "accepted_project_revision": step.try_get::<Option<i64>, _>("current_accepted_project_revision")?,
                        "status": step.try_get::<Option<String>, _>("current_attempt_status")?,
                        "error_code": step.try_get::<Option<String>, _>("current_attempt_error_code")?,
                    }))
                })
                .transpose()?;
            Ok(json!({
                "action_id": step.try_get::<Option<Uuid>, _>("action_id")?,
                "step_id": step.try_get::<Uuid, _>("step_id")?,
                "step_order": step.try_get::<i32, _>("step_order")?,
                "kind": step.try_get::<String, _>("step_kind")?,
                "payload": step.try_get::<Value, _>("desired_payload")?,
                "assignee_pubkey": assignee.map(hex::encode),
                "resolved_role_id": step.try_get::<Option<Uuid>, _>("resolved_role_id")?,
                "resolved_assignment_id": step.try_get::<Option<Uuid>, _>("resolved_assignment_id")?,
                "target_object_type": step.try_get::<String, _>("target_object_type")?,
                "target_object_id": step.try_get::<Uuid, _>("target_object_id")?,
                "accepted_project_revision": step.try_get::<Option<i64>, _>("accepted_project_revision")?,
                "status": step.try_get::<String, _>("status")?,
                "last_error_code": step.try_get::<Option<String>, _>("last_error_code")?,
                "attempt_count": step.try_get::<i32, _>("attempt_count")?,
                "current_attempt": current_attempt,
            }))
        })
        .collect::<Result<Vec<_>>>()?;
    let plan_event_id: Option<Vec<u8>> = row.try_get("plan_event_id")?;
    let board_event_id: Vec<u8> = row.try_get("board_event_id")?;
    let deadline: Option<DateTime<Utc>> = row.try_get("action_deadline_at")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let terminal_at: Option<DateTime<Utc>> = row.try_get("terminal_at")?;
    Ok(Some(json!({
        "action_run_id": action_run_id,
        "plan_event_id": plan_event_id.map(hex::encode),
        "board_event_id": hex::encode(board_event_id),
        "control_epoch": row.try_get::<i64, _>("control_epoch")?,
        "board_window": row.try_get::<i64, _>("board_window")?,
        "action_window_epoch": row.try_get::<i64, _>("action_window_epoch")?,
        "phase": row.try_get::<String, _>("action_phase")?,
        "condition": row.try_get::<String, _>("action_condition")?,
        "terminal_status": row.try_get::<Option<String>, _>("terminal_status")?,
        "completion_project_revision": row.try_get::<Option<i64>, _>("completion_project_revision")?,
        "action_deadline_at_ms": deadline.map(|value| value.timestamp_millis()),
        "last_error_code": row.try_get::<Option<String>, _>("last_error_code")?,
        "required_step_count": row.try_get::<i64, _>("required_step_count")?,
        "applied_step_count": row.try_get::<i64, _>("applied_step_count")?,
        "steps": steps,
        "created_at_ms": created_at.timestamp_millis(),
        "updated_at_ms": updated_at.timestamp_millis(),
        "terminal_at_ms": terminal_at.map(|value| value.timestamp_millis()),
    })))
}

pub(crate) async fn mark_active_run_terminal_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    terminal_status: &str,
    now: DateTime<Utc>,
) -> Result<()> {
    if !matches!(terminal_status, "completed_closed" | "completed_aborted") {
        return Err(DbError::InvalidData(format!(
            "invalid Meeting V2 action terminal status: {terminal_status}"
        )));
    }
    let action_run_id: Option<Uuid> = sqlx::query_scalar(
        "SELECT action_run_id FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(action_run_id) = action_run_id else {
        return Ok(());
    };
    sqlx::query(
        "SELECT step_id FROM meeting_v2_action_steps \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
         ORDER BY step_order FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query(
        "SELECT step_id, attempt_number FROM meeting_v2_action_step_attempts \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
         ORDER BY step_id, attempt_number FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_v2_action_step_attempts \
         SET status = 'abandoned', error_code = $4, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND status <> 'accepted'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(terminal_status)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_v2_action_steps step \
         SET status = 'abandoned', last_error_code = $4, updated_at = $5 \
         WHERE step.community_id = $1 AND step.session_id = $2 \
           AND step.action_run_id = $3 AND step.status <> 'applied' \
           AND NOT EXISTS ( \
               SELECT 1 FROM meeting_v2_action_step_attempts attempt \
               WHERE attempt.community_id = step.community_id \
                 AND attempt.session_id = step.session_id \
                 AND attempt.action_run_id = step.action_run_id \
                 AND attempt.step_id = step.step_id \
                 AND attempt.status = 'accepted')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(terminal_status)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET terminal_status = $3, terminal_at = $4, action_deadline_at = NULL, updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $5 \
           AND terminal_status IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(terminal_status)
    .bind(now)
    .bind(action_run_id)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

pub(crate) async fn validate_close_gate_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
    action_window_epoch: i64,
    plan_event_id: &[u8],
) -> Result<bool> {
    if plan_event_id.len() != 32 || action_window_epoch <= 0 {
        return Ok(false);
    }
    let allowed: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_v2_action_runs run \
             WHERE run.community_id = $1 AND run.session_id = $2 \
               AND run.action_run_id = $3 AND run.action_window_epoch = $4 \
               AND run.plan_event_id = $5 AND run.terminal_status IS NULL \
               AND run.action_phase = 'ready_to_close' \
               AND run.action_condition = 'runnable' \
               AND EXISTS ( \
                   SELECT 1 FROM meeting_v2_action_steps step \
                   WHERE step.community_id = run.community_id \
                     AND step.session_id = run.session_id \
                     AND step.action_run_id = run.action_run_id \
               ) \
               AND NOT EXISTS ( \
                   SELECT 1 FROM meeting_v2_action_steps step \
                   WHERE step.community_id = run.community_id \
                     AND step.session_id = run.session_id \
                     AND step.action_run_id = run.action_run_id \
                     AND step.status <> 'applied' \
               ) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(action_window_epoch)
    .bind(plan_event_id)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(allowed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use buzz_sdk::project_view_v2::{build_project_object_command, build_role_command};

    fn materializer_plan() -> buzz_sdk::MeetingV2ActionPlan {
        let action_id = Uuid::new_v4();
        let requirement_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        buzz_sdk::MeetingV2ActionPlan {
            version: buzz_sdk::MEETING_V2_ACTION_PLAN_VERSION,
            action_run_id: Uuid::new_v4(),
            board_event_id: "ab".repeat(32),
            items: vec![buzz_sdk::MeetingV2ActionItem {
                action_id,
                summary: "Implement the accepted design".to_owned(),
                assignee_pubkey: Keys::generate().public_key().to_hex(),
            }],
            steps: vec![
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: None,
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateRequirement,
                    target_object_id: requirement_id,
                    payload: json!({
                        "title": "Accepted design",
                        "description": "Freeze the Meeting decision",
                    }),
                },
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: Some(action_id),
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateWork,
                    target_object_id: work_id,
                    payload: json!({
                        "title": "Implement the accepted design",
                        "requirement_id": requirement_id,
                    }),
                },
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: Some(action_id),
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewSetWorkResponsibility,
                    target_object_id: work_id,
                    payload: json!({}),
                },
            ],
        }
    }

    fn action_step(
        step_kind: &str,
        target_object_id: Uuid,
        desired_payload: Value,
    ) -> ActionStepRow {
        ActionStepRow {
            action_id: None,
            step_order: 1,
            step_kind: step_kind.to_owned(),
            desired_payload,
            assignee_pubkey: None,
            resolved_role_id: None,
            resolved_assignment_id: None,
            target_object_id,
            status: "pending".to_owned(),
            attempt_count: 0,
        }
    }

    #[test]
    fn materializer_plan_requires_one_ordered_work_responsibility_pair_per_item() {
        let plan = materializer_plan();
        validate_materializer_plan(&plan).expect("canonical materializer topology");

        let mut reordered = plan.clone();
        reordered.steps.swap(1, 2);
        assert!(validate_materializer_plan(&reordered).is_err());

        let mut wrong_summary = plan;
        wrong_summary.steps[1].payload["title"] = json!("Different work");
        assert!(validate_materializer_plan(&wrong_summary).is_err());
    }

    #[test]
    fn prepared_requirement_event_must_exactly_match_the_frozen_step() {
        let moderator = Keys::generate();
        let object_id = Uuid::new_v4();
        let step = action_step(
            "project_view.create_requirement",
            object_id,
            json!({"title": "Accepted design"}),
        );
        let project_event = build_project_object_command(ProjectObjectCommand::new(
            7,
            None,
            MutationRequest::Create(CreateMutation {
                object: NewProjectViewObject::Requirement {
                    id: object_id,
                    title: "Accepted design".to_owned(),
                    description: "Accepted design".to_owned(),
                    status: RequirementStatus::Ready,
                    priority: Priority::Normal,
                    planned_in_stage_id: None,
                },
            }),
        ))
        .expect("build Requirement command")
        .sign_with_keys(&moderator)
        .expect("sign Requirement command");
        let meeting_event = nostr::EventBuilder::text_note("Meeting action")
            .sign_with_keys(&moderator)
            .expect("sign Meeting action");

        validate_prepared_project_event(
            &step,
            &meeting_event,
            &project_event,
            project_event.id.as_bytes(),
            7,
        )
        .expect("exact prepared Requirement");
        assert!(validate_prepared_project_event(
            &step,
            &meeting_event,
            &project_event,
            project_event.id.as_bytes(),
            8,
        )
        .is_err());
    }

    #[test]
    fn responsibility_receipt_must_name_the_exact_work_and_resolved_role() {
        let work_id = Uuid::new_v4();
        let role_id = Uuid::new_v4();
        let mut step = action_step("project_view.set_work_responsibility", work_id, json!({}));
        step.resolved_role_id = Some(role_id);
        let result = json!({
            "project_revision": 12,
            "operation": "set_work_responsibility",
            "changed_objects": [{
                "object_type": "work",
                "object_id": work_id,
                "object_revision": 2,
                "responsible_role_id": role_id,
            }],
        });
        assert!(project_receipt_result_matches_step(&result, &step, 12));

        let mut wrong_role = result;
        wrong_role["changed_objects"][0]["responsible_role_id"] = json!(Uuid::new_v4());
        assert!(!project_receipt_result_matches_step(&wrong_role, &step, 12));
    }

    #[test]
    fn receipt_subject_is_bound_to_the_exact_signed_command() {
        let moderator = Keys::generate();
        let work_id = Uuid::new_v4();
        let role_id = Uuid::new_v4();
        let request = RoleCommandRequest::SetWorkResponsibility {
            work_id,
            responsible_role_id: Some(role_id),
        };
        let event = build_role_command(RoleCommand::new(4, None, request.clone()))
            .expect("build responsibility command")
            .sign_with_keys(&moderator)
            .expect("sign responsibility command");
        let subject = serde_json::to_value(request).expect("serialize responsibility request");
        assert!(project_change_subject_matches_event(
            &subject,
            "project_view.set_work_responsibility",
            &event,
        )
        .expect("validate receipt subject"));
        assert!(!project_change_subject_matches_event(
            &json!({"set_work_responsibility": {"work_id": Uuid::new_v4()}}),
            "project_view.set_work_responsibility",
            &event,
        )
        .expect("reject mismatched receipt subject"));
    }
}
