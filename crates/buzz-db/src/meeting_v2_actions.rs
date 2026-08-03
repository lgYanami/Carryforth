//! Authoritative Meeting V2 action-finalization state and command ledger.
//!
//! Stage one deliberately stops at the Meeting boundary: plans and required
//! steps are persisted and gate normal close, but no Project View mutation is
//! executed from this module.

use std::collections::{HashMap, HashSet};

use buzz_core::CommunityId;
use chrono::{DateTime, Duration, Utc};
use nostr::{Event, Keys};
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
        /// Optional attempt binding, reserved for the same-session ACP stage.
        expected_decision_attempt_id: Option<Vec<u8>>,
    },
    /// Freeze the first valid Harness-compiled plan.
    Plan {
        /// Current run/window/empty-plan fences.
        fence: ActionRunFence,
        /// Strict compiled plan.
        plan: buzz_sdk::MeetingV2ActionPlan,
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
    fn action(&self) -> &'static str {
        match self {
            Self::Begin { .. } => "begin",
            Self::Plan { .. } => "plan",
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

    let applied = if session.status != "active" {
        AppliedCommand::rejected("meeting_ended", None)
    } else {
        apply_command_tx(&mut tx, &params).await?
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
) -> Result<AppliedCommand> {
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
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
    {
        return Err(DbError::InvalidData(
            "Meeting V2 action begin has malformed fences".to_string(),
        ));
    }
    if expected_decision_attempt_id.is_some() {
        return Ok(AppliedCommand::rejected(
            "decision_attempt_binding_unavailable",
            None,
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
        "SELECT phase, state_event_id, control_epoch, active_offer_id, active_grant_id, \
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
    let active_offer_id: Option<Vec<u8>> = baton.try_get("active_offer_id")?;
    let active_grant_id: Option<Vec<u8>> = baton.try_get("active_grant_id")?;
    let active_attempt_id: Option<Vec<u8>> = baton.try_get("active_decision_attempt_id")?;
    let next_action_at: Option<DateTime<Utc>> = baton.try_get("next_action_at")?;
    if phase != "moderator_idle"
        || active_offer_id.is_some()
        || active_grant_id.is_some()
        || active_attempt_id.is_some()
        || next_action_at.is_some()
    {
        return Ok(AppliedCommand::rejected("moderator_floor_not_idle", None));
    }
    if state_event_id != expected_state_event_id {
        return Ok(AppliedCommand::rejected("stale_state_event", None));
    }
    if control_epoch != expected_control_epoch {
        return Ok(AppliedCommand::rejected("stale_control_epoch", None));
    }
    if has_unresolved_floor_work_tx(tx, params.community_id, params.session_id).await? {
        return Ok(AppliedCommand::rejected("floor_work_pending", None));
    }
    if load_active_run_tx(tx, params.community_id, params.session_id, true)
        .await?
        .is_some()
    {
        return Ok(AppliedCommand::rejected("action_run_already_active", None));
    }

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
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_blocked",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({"reason_code": reason_code}),
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
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_retried",
        run.action_run_id,
        next_window,
        transition.state_revision,
        json!({"action_deadline_at_ms": deadline.timestamp_millis()}),
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
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_phase = 'ready_to_close', action_deadline_at = NULL, updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_phase = 'applying' \
           AND action_condition = 'runnable'",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
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
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_ready_to_close",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({"required_step_count": total, "applied_step_count": applied}),
    ))
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
    let effect_count: i64 = sqlx::query_scalar(
        "SELECT \
             (SELECT count(*) FROM meeting_v2_action_steps \
              WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                AND status IN ('prepared', 'applied')) \
           + (SELECT count(*) FROM meeting_v2_action_step_attempts \
              WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
                AND status <> 'abandoned')",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .fetch_one(tx.as_mut())
    .await?;
    if effect_count != 0 {
        return Ok(AppliedCommand::rejected(
            "action_has_external_effects",
            Some(&run),
        ));
    }
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
                action_window_epoch, action_phase, action_condition, terminal_status \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL \
         FOR UPDATE"
    } else {
        "SELECT action_run_id, plan_event_id, board_event_id, control_epoch, \
                action_window_epoch, action_phase, action_condition, terminal_status \
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
            | "object_id_conflict"
            | "responsibility_conflict"
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
    row.map(|row| {
        let plan_event_id: Option<Vec<u8>> = row.try_get("plan_event_id")?;
        let board_event_id: Vec<u8> = row.try_get("board_event_id")?;
        let deadline: Option<DateTime<Utc>> = row.try_get("action_deadline_at")?;
        let created_at: DateTime<Utc> = row.try_get("created_at")?;
        let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let terminal_at: Option<DateTime<Utc>> = row.try_get("terminal_at")?;
        Ok(json!({
            "action_run_id": row.try_get::<Uuid, _>("action_run_id")?,
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
            "created_at_ms": created_at.timestamp_millis(),
            "updated_at_ms": updated_at.timestamp_millis(),
            "terminal_at_ms": terminal_at.map(|value| value.timestamp_millis()),
        }))
    })
    .transpose()
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
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET terminal_status = $3, terminal_at = $4, action_deadline_at = NULL, updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(terminal_status)
    .bind(now)
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
