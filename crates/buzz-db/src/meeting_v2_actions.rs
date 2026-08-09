//! Authoritative Meeting V2 direct action-finalization state.
//!
//! Meeting owns only the lifecycle fence. The moderator directly uses normal
//! business command surfaces, and then signs the Meeting End attestation. This
//! module deliberately has no dependency on Project View domain types.

use buzz_core::CommunityId;
use chrono::{DateTime, Duration, Utc};
use nostr::{Event, Keys};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::meeting_v2::RuntimePhase;
use crate::{Db, DbError, Result};

/// Compare-and-swap fences for one active direct action run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionRunFence {
    /// Relay-issued action run ID.
    pub action_run_id: Uuid,
    /// Current action retry-window epoch.
    pub action_window_epoch: i64,
    /// Exact frozen final Board event.
    pub board_event_id: Vec<u8>,
}

/// Source-owned Meeting retrieval-summary mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingSummaryMutation {
    /// Replace the current retrieval summary.
    Set(String),
    /// Remove the current retrieval summary.
    Clear,
}

/// Inputs for changing a Meeting retrieval summary inside an existing command
/// transaction.
pub struct MeetingSummaryUpdateTxParams<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Verified command author.
    pub actor_pubkey: &'a [u8],
    /// Exact current direct-action run/window/Board fence.
    pub fence: ActionRunFence,
    /// Requested source metadata mutation.
    pub mutation: MeetingSummaryMutation,
}

/// Canonical result of a Meeting retrieval-summary mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingSummaryUpdate {
    /// Current canonical summary after the mutation.
    pub summary: Option<String>,
    /// Whether the source field changed.
    pub changed: bool,
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
    /// Renew the exact runnable action window without granting business authority.
    Renew {
        /// Current run/window/Board fences.
        fence: ActionRunFence,
        /// Exact next per-window progress sequence.
        progress_seq: i64,
        /// Low-cardinality cooperative host stage.
        stage: ActionProgressStage,
        /// Monotonic provider/tool activity sequence observed by the host.
        last_activity_seq: i64,
    },
    /// Durably block a direct action run.
    Block {
        /// Current run/window/Board fences.
        fence: ActionRunFence,
        /// Closed low-cardinality reason code.
        reason_code: String,
    },
    /// Start a fresh action deadline for a blocked run.
    Retry {
        /// Current run/window/Board fences.
        fence: ActionRunFence,
    },
    /// Return to Board while explicitly preserving any external effects.
    ReturnToBoard {
        /// Current run/window/Board fences.
        fence: ActionRunFence,
    },
}

impl ActionCommand {
    /// Stable low-cardinality wire action label.
    pub fn action(&self) -> &'static str {
        match self {
            Self::Begin { .. } => "begin",
            Self::Renew { .. } => "renew",
            Self::Block { .. } => "block",
            Self::Retry { .. } => "retry",
            Self::ReturnToBoard { .. } => "return-to-board",
        }
    }
}

/// Closed diagnostic vocabulary carried by action lease renewals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionProgressStage {
    /// The host is reasoning over canonical state.
    Reasoning,
    /// The host is invoking a tool or business command.
    ToolCall,
    /// The host is processing a tool or business-command result.
    ToolResult,
    /// The host is preparing the final attestation.
    Finalizing,
    /// A Human-hosted action is intentionally waiting for input.
    WaitingHuman,
}

impl ActionProgressStage {
    /// Stable wire/storage label.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::Finalizing => "finalizing",
            Self::WaitingHuman => "waiting_human",
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
    action: String,
    accepted: bool,
    outcome_code: String,
    response: Value,
}

#[derive(Debug, Clone)]
struct ActionRunRow {
    action_run_id: Uuid,
    board_event_id: Vec<u8>,
    control_epoch: i64,
    action_window_epoch: i64,
    action_condition: String,
    terminal_status: Option<String>,
    last_error_code: Option<String>,
    progress_seq: i64,
    action_deadline_at: Option<DateTime<Utc>>,
    operator_hard_deadline: Option<DateTime<Utc>>,
}

/// Update the Meeting-owned retrieval summary without changing Meeting control
/// state.
///
/// The caller owns the surrounding command-event transaction. This function
/// locks and revalidates the current Action Finalization window so a delayed
/// metadata command cannot write after Return-to-Board or Meeting closure.
pub async fn update_meeting_summary_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: MeetingSummaryUpdateTxParams<'_>,
) -> Result<MeetingSummaryUpdate> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "Meeting summary command has a nil session id".to_string(),
        ));
    }
    if params.actor_pubkey.len() != 32 {
        return Err(DbError::InvalidData(
            "Meeting summary author pubkey must be 32 bytes".to_string(),
        ));
    }
    let next_summary = match params.mutation {
        MeetingSummaryMutation::Set(summary) => {
            if summary.trim().is_empty() || summary.contains('\0') {
                return Err(DbError::InvalidData(
                    "Meeting summary SET requires non-blank text without NUL".to_string(),
                ));
            }
            Some(summary)
        }
        MeetingSummaryMutation::Clear => None,
    };

    let session =
        crate::meeting_baton::lock_baton_session_tx(tx, params.community_id, params.session_id)
            .await?;
    if session.protocol != crate::meeting_baton::BatonProtocol::V2Actions {
        return Err(DbError::InvalidData(
            "Meeting summary is only writable for the current action-finalization policy"
                .to_string(),
        ));
    }
    if session.status != "active" {
        return Err(DbError::InvalidData(
            "conflict: Meeting is no longer active".to_string(),
        ));
    }
    if params.actor_pubkey != session.host_pubkey.as_slice() {
        return Err(DbError::AccessDenied(
            "only the immutable Meeting moderator can update its summary".to_string(),
        ));
    }
    if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        tx,
        params.community_id,
        params.session_id,
        params.actor_pubkey,
    )
    .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting summary author was durably revoked from this Session".to_string(),
        ));
    }
    if !crate::meeting::actor_security_active_tx(tx, params.community_id, params.actor_pubkey)
        .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting summary author is no longer an active writable principal".to_string(),
        ));
    }

    let runtime =
        crate::meeting_v2::load_runtime_tx(tx, params.community_id, params.session_id, true)
            .await?;
    if runtime.phase != RuntimePhase::FinalizingActions {
        return Err(DbError::InvalidData(
            "conflict: Meeting is not in Action Finalization".to_string(),
        ));
    }
    let run = load_active_run_tx(tx, params.community_id, params.session_id, true)
        .await?
        .ok_or_else(|| {
            DbError::InvalidData("conflict: Meeting has no active action run".to_string())
        })?;
    if let Some(reason) = validate_run_fence(&run, &params.fence) {
        return Err(DbError::InvalidData(format!("conflict: {reason}")));
    }
    if run.action_condition != "runnable" {
        return Err(DbError::InvalidData(
            "conflict: Meeting action run is not runnable".to_string(),
        ));
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    if run
        .action_deadline_at
        .is_none_or(|deadline| deadline <= now)
        || run
            .operator_hard_deadline
            .is_some_and(|deadline| deadline <= now)
    {
        return Err(DbError::InvalidData(
            "conflict: Meeting action window has expired".to_string(),
        ));
    }

    let current_summary: Option<String> = sqlx::query_scalar(
        "SELECT summary FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .fetch_one(tx.as_mut())
    .await?;
    let changed = current_summary != next_summary;
    if changed {
        let updated = sqlx::query(
            "UPDATE meeting_sessions SET summary = $3 \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(params.community_id.as_uuid())
        .bind(params.session_id)
        .bind(&next_summary)
        .execute(tx.as_mut())
        .await?;
        if updated.rows_affected() != 1 {
            return Err(DbError::NotFound(format!("meeting {}", params.session_id)));
        }
    }

    Ok(MeetingSummaryUpdate {
        summary: next_summary,
        changed,
    })
}

/// Block one action window whose independent database deadline has elapsed.
///
/// Callers must already hold the Meeting Session row lock. The transition and
/// Relay-signed State/outbox rows commit in the caller's transaction.
pub(crate) async fn recover_due_action_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    now: DateTime<Utc>,
) -> Result<Option<crate::meeting_baton::BatonTransitionResult>> {
    let due: Option<(Uuid, bool)> = sqlx::query_as(
        "SELECT action_run_id, \
                (operator_hard_deadline IS NOT NULL AND operator_hard_deadline <= $3) \
                    AS operator_expired \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 \
           AND terminal_status IS NULL AND action_condition = 'runnable' \
           AND ((action_deadline_at IS NOT NULL AND action_deadline_at <= $3) \
             OR (operator_hard_deadline IS NOT NULL AND operator_hard_deadline <= $3)) \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some((action_run_id, operator_expired)) = due else {
        return Ok(None);
    };
    let reason_code = if operator_expired {
        "action_operator_deadline_exceeded"
    } else {
        "action_lease_expired"
    };
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_condition = 'blocked', action_deadline_at = NULL, \
             last_error_code = $4, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_condition = 'runnable'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(reason_code)
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
        reason_code,
        "action",
        "runnable",
        "blocked",
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

/// Execute one direct action command under the Meeting Session lock.
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
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(tx.as_mut())
            .await?;
        let mut response = receipt.response;
        if matches!(receipt.action.as_str(), "begin" | "renew") && receipt.accepted {
            decorate_action_timing_response_tx(&mut tx, params.community_id, &mut response, now)
                .await?;
        }
        tx.commit().await?;
        return Ok(ActionCommandCommit {
            accepted: receipt.accepted,
            duplicate: true,
            outcome_code: receipt.outcome_code,
            response,
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
    let mut response = json!({
        "meeting_id": params.session_id,
        "accepted": applied.accepted,
        "outcome": applied.outcome_code,
        "action_run_id": applied.action_run_id,
        "action_window_epoch": applied.action_window_epoch,
        "state_revision": applied.state_revision,
        "details": applied.extra,
    });
    if applied.accepted
        && matches!(
            &params.command,
            ActionCommand::Begin { .. } | ActionCommand::Renew { .. }
        )
    {
        decorate_action_timing_response_tx(&mut tx, params.community_id, &mut response, now)
            .await?;
    }
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
        ActionCommand::Renew {
            fence,
            progress_seq,
            stage,
            last_activity_seq,
        } => {
            apply_renew_tx(
                tx,
                params,
                fence,
                *progress_seq,
                *stage,
                *last_activity_seq,
                now,
            )
            .await
        }
        ActionCommand::Block { fence, reason_code } => {
            apply_block_tx(tx, params, fence, reason_code, now).await
        }
        ActionCommand::Retry { fence } => apply_retry_tx(tx, params, fence, now).await,
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

    let (duration_ms, operator_hard_cap_ms) =
        action_lease_config_tx(tx, params.community_id, params.session_id).await?;
    let deadline = now + Duration::milliseconds(duration_ms);
    let operator_hard_deadline =
        operator_hard_cap_ms.map(|cap_ms| now + Duration::milliseconds(cap_ms));
    let action_run_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO meeting_v2_action_runs \
             (community_id, session_id, action_run_id, begin_event_id, board_event_id, \
              control_epoch, board_window, action_window_epoch, action_condition, \
              action_deadline_at, operator_hard_deadline, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, 1, 'runnable', $8, $9, $10, $10)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(action_run_id)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(board_event_id)
    .bind(expected_control_epoch)
    .bind(board_window)
    .bind(deadline)
    .bind(operator_hard_deadline)
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
        "runnable",
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
            "operator_hard_deadline_ms": operator_hard_deadline.map(|value| value.timestamp_millis()),
            "decision_attempt_id": expected_decision_attempt_id.map(hex::encode),
            "frozen_intent_count": frozen_floor.map(|counts| counts.0).unwrap_or(0),
            "frozen_handoff_count": frozen_floor.map(|counts| counts.1).unwrap_or(0),
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
async fn apply_renew_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &ActionCommandTxParams<'_>,
    fence: &ActionRunFence,
    progress_seq: i64,
    stage: ActionProgressStage,
    last_activity_seq: i64,
    now: DateTime<Utc>,
) -> Result<AppliedCommand> {
    if progress_seq <= 0 || last_activity_seq < 0 {
        return Err(DbError::InvalidData(
            "Meeting V2 action renewal sequences are malformed".to_string(),
        ));
    }
    let Some(run) = load_active_run_tx(tx, params.community_id, params.session_id, true).await?
    else {
        return Ok(AppliedCommand::rejected("no_active_action_run", None));
    };
    if let Some(rejection) = validate_run_fence(&run, fence) {
        return Ok(AppliedCommand::rejected(rejection, Some(&run)));
    }
    if run.action_condition != "runnable" {
        return Ok(AppliedCommand::rejected("action_not_runnable", Some(&run)));
    }
    if run
        .action_deadline_at
        .is_none_or(|deadline| now >= deadline)
        || run
            .operator_hard_deadline
            .is_some_and(|deadline| now >= deadline)
    {
        // `execute_action_command` performs lazy recovery before dispatch. Reaching
        // this guard means the row changed unexpectedly inside this transaction.
        return Ok(AppliedCommand::rejected("action_lease_expired", Some(&run)));
    }
    let expected_progress_seq = run
        .progress_seq
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("Meeting V2 action progress overflow".to_string()))?;
    if progress_seq != expected_progress_seq {
        return Ok(AppliedCommand::rejected(
            "progress_sequence_conflict",
            Some(&run),
        ));
    }

    let (duration_ms, _) =
        action_lease_config_tx(tx, params.community_id, params.session_id).await?;
    let requested_deadline = now + Duration::milliseconds(duration_ms);
    let deadline = run
        .operator_hard_deadline
        .map_or(requested_deadline, |operator| {
            requested_deadline.min(operator)
        });
    if deadline <= now {
        return Ok(AppliedCommand::rejected(
            "action_operator_deadline_exceeded",
            Some(&run),
        ));
    }

    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET progress_seq = $4, last_progress_stage = $5, last_progress_at = $6, \
             action_deadline_at = $7, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND action_window_epoch = $8 AND terminal_status IS NULL \
           AND action_condition = 'runnable' AND progress_seq = $9",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(progress_seq)
    .bind(stage.as_str())
    .bind(now)
    .bind(deadline)
    .bind(run.action_window_epoch)
    .bind(run.progress_seq)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 action changed while renewing its lease".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO meeting_v2_action_lease_renewals \
             (community_id, session_id, action_run_id, action_window_epoch, progress_seq, \
              renewal_event_id, stage, last_activity_seq, accepted_at, lease_expires_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(run.action_run_id)
    .bind(run.action_window_epoch)
    .bind(progress_seq)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(stage.as_str())
    .bind(last_activity_seq)
    .bind(now)
    .bind(deadline)
    .execute(tx.as_mut())
    .await?;

    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_lease_renewed",
        params.event.id.as_bytes(),
        "runnable",
        "runnable",
        None,
        now,
    )
    .await?;
    Ok(AppliedCommand::accepted(
        "action_lease_renewed",
        run.action_run_id,
        run.action_window_epoch,
        transition.state_revision,
        json!({
            "accepted_progress_seq": progress_seq,
            "stage": stage.as_str(),
            "last_activity_seq": last_activity_seq,
            "action_deadline_at_ms": deadline.timestamp_millis(),
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
    if run.action_condition != "runnable" {
        return Ok(AppliedCommand::rejected("action_not_runnable", Some(&run)));
    }
    sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_condition = 'blocked', action_deadline_at = NULL, \
             last_error_code = $4, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3 \
           AND terminal_status IS NULL AND action_condition = 'runnable'",
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
        "runnable",
        "blocked",
        None,
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
    if run.action_condition != "blocked" {
        return Ok(AppliedCommand::rejected("action_not_blocked", Some(&run)));
    }
    let next_window = run
        .action_window_epoch
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("Meeting V2 action window overflow".to_string()))?;
    let retry_reason = run.last_error_code.as_deref().unwrap_or("unspecified");
    if run
        .operator_hard_deadline
        .is_some_and(|deadline| now >= deadline)
    {
        return Ok(AppliedCommand::rejected(
            "action_operator_deadline_exceeded",
            Some(&run),
        ));
    }
    let (duration_ms, _) =
        action_lease_config_tx(tx, params.community_id, params.session_id).await?;
    let requested_deadline = now + Duration::milliseconds(duration_ms);
    let deadline = run
        .operator_hard_deadline
        .map_or(requested_deadline, |operator| {
            requested_deadline.min(operator)
        });
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET action_window_epoch = $4, action_condition = 'runnable', \
             action_deadline_at = $5, last_error_code = NULL, progress_seq = 0, \
             last_progress_stage = NULL, last_progress_at = NULL, updated_at = $6 \
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
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 action changed while retrying".to_string(),
        ));
    }
    let transition = crate::meeting_baton::publish_v2_action_transition_tx(
        tx,
        params.community_id,
        params.session_id,
        run.action_run_id,
        params.relay_keys,
        "action_retried",
        params.event.id.as_bytes(),
        "blocked",
        "runnable",
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
        }),
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
    let updated = sqlx::query(
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
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 action changed while returning to Board".to_string(),
        ));
    }
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
        &run.action_condition,
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
        json!({
            "board_window": board_runtime.board_window,
            "external_effects": "preserved",
            "from_condition": run.action_condition,
        }),
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
    if run.board_event_id != fence.board_event_id {
        return Some("stale_board_event");
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
        "SELECT action_run_id, board_event_id, control_epoch, action_window_epoch, \
                action_condition, terminal_status, last_error_code, progress_seq, \
                action_deadline_at, operator_hard_deadline \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL \
         FOR UPDATE"
    } else {
        "SELECT action_run_id, board_event_id, control_epoch, action_window_epoch, \
                action_condition, terminal_status, last_error_code, progress_seq, \
                action_deadline_at, operator_hard_deadline \
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
        board_event_id: row.try_get("board_event_id")?,
        control_epoch: row.try_get("control_epoch")?,
        action_window_epoch: row.try_get("action_window_epoch")?,
        action_condition: row.try_get("action_condition")?,
        terminal_status: row.try_get("terminal_status")?,
        last_error_code: row.try_get("last_error_code")?,
        progress_seq: row.try_get("progress_seq")?,
        action_deadline_at: row.try_get("action_deadline_at")?,
        operator_hard_deadline: row.try_get("operator_hard_deadline")?,
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

async fn action_lease_config_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<(i64, Option<i64>)> {
    sqlx::query_as(
        "SELECT action_finalization_ms, action_operator_hard_cap_ms \
         FROM meeting_v2_config \
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

fn is_block_reason(reason: &str) -> bool {
    matches!(
        reason,
        "external_operation_failed"
            | "external_state_conflict"
            | "tool_unavailable"
            | "provider_failure"
            | "action_deadline_exceeded"
            | "action_lease_expired"
            | "action_operator_deadline_exceeded"
    )
}

async fn load_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8],
) -> Result<Option<ActionReceipt>> {
    let row = sqlx::query(
        "SELECT author_pubkey, action, accepted, outcome_code, response_json \
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
            action: row.try_get("action")?,
            accepted: row.try_get("accepted")?,
            outcome_code: row.try_get("outcome_code")?,
            response: row.try_get("response_json")?,
        })
    })
    .transpose()
}

/// Verify the private accepted receipt that created one current action run.
///
/// Action commands intentionally do not enter the ordinary Event store. This
/// verifier is used by Project Context while holding the Meeting lifecycle
/// locks, so a finalizing Meeting can prove its Begin command without exposing
/// that private control event to public subscriptions.
pub(crate) async fn accepted_action_begin_receipt_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    command_event_id: &[u8],
    host_pubkey: &[u8],
    action_run_id: Uuid,
) -> Result<bool> {
    if command_event_id.len() != 32 || host_pubkey.len() != 32 || action_run_id.is_nil() {
        return Ok(false);
    }
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM meeting_v2_action_command_receipts \
         WHERE community_id = $1 AND session_id = $2 AND command_event_id = $3 \
           AND author_pubkey = $4 AND action = 'begin' AND action_run_id = $5 \
           AND action_window_epoch = 1 AND accepted \
           AND outcome_code = 'action_finalization_began')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(command_event_id)
    .bind(host_pubkey)
    .bind(action_run_id)
    .fetch_one(tx.as_mut())
    .await?)
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

async fn decorate_action_timing_response_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    response: &mut Value,
    now: DateTime<Utc>,
) -> Result<()> {
    let Some(object) = response.as_object_mut() else {
        return Err(DbError::InvalidData(
            "Meeting V2 action receipt response is not an object".to_string(),
        ));
    };
    let run_id = object
        .get("action_run_id")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok());
    let window = object.get("action_window_epoch").and_then(Value::as_i64);
    let timing = if let (Some(run_id), Some(window)) = (run_id, window) {
        sqlx::query(
            "SELECT action_deadline_at, operator_hard_deadline, progress_seq \
             FROM meeting_v2_action_runs \
             WHERE community_id = $1 AND action_run_id = $2 \
               AND action_window_epoch = $3",
        )
        .bind(community_id.as_uuid())
        .bind(run_id)
        .bind(window)
        .fetch_optional(tx.as_mut())
        .await?
    } else {
        None
    };
    let (lease_expires_at, operator_hard_deadline, progress_seq) = timing
        .map(|row| {
            Ok::<_, sqlx::Error>((
                row.try_get::<Option<DateTime<Utc>>, _>("action_deadline_at")?,
                row.try_get::<Option<DateTime<Utc>>, _>("operator_hard_deadline")?,
                row.try_get::<i64, _>("progress_seq")?,
            ))
        })
        .transpose()?
        .unwrap_or((None, None, 0));
    let remaining_ms = |deadline: Option<DateTime<Utc>>| {
        deadline.map(|value| (value - now).num_milliseconds().max(0))
    };
    object.insert("server_now_ms".to_string(), json!(now.timestamp_millis()));
    object.insert(
        "lease_expires_at_ms".to_string(),
        json!(lease_expires_at.map(|value| value.timestamp_millis())),
    );
    object.insert(
        "lease_ttl_ms".to_string(),
        json!(remaining_ms(lease_expires_at)),
    );
    object.insert(
        "operator_hard_remaining_ms".to_string(),
        json!(remaining_ms(operator_hard_deadline)),
    );
    let accepted_progress_seq = object
        .get("details")
        .and_then(|details| details.get("accepted_progress_seq"))
        .and_then(Value::as_i64)
        .unwrap_or(progress_seq);
    object.insert(
        "accepted_progress_seq".to_string(),
        json!(accepted_progress_seq),
    );
    Ok(())
}

pub(crate) async fn action_state_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<Value>> {
    let row = sqlx::query(
        "SELECT action_run_id, board_event_id, control_epoch, board_window, \
                action_window_epoch, action_condition, terminal_status, \
                completion_event_id, action_deadline_at, last_error_code, progress_seq, \
                last_progress_stage, last_progress_at, operator_hard_deadline, \
                created_at, updated_at, terminal_at \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 \
         ORDER BY (terminal_status IS NULL) DESC, created_at DESC LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let board_event_id: Vec<u8> = row.try_get("board_event_id")?;
    let completion_event_id: Option<Vec<u8>> = row.try_get("completion_event_id")?;
    let deadline: Option<DateTime<Utc>> = row.try_get("action_deadline_at")?;
    let last_progress_at: Option<DateTime<Utc>> = row.try_get("last_progress_at")?;
    let operator_hard_deadline: Option<DateTime<Utc>> = row.try_get("operator_hard_deadline")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let terminal_at: Option<DateTime<Utc>> = row.try_get("terminal_at")?;
    Ok(Some(json!({
        "mode": "host_direct",
        "action_run_id": row.try_get::<Uuid, _>("action_run_id")?,
        "board_event_id": hex::encode(board_event_id),
        "control_epoch": row.try_get::<i64, _>("control_epoch")?,
        "board_window": row.try_get::<i64, _>("board_window")?,
        "action_window_epoch": row.try_get::<i64, _>("action_window_epoch")?,
        "condition": row.try_get::<String, _>("action_condition")?,
        "terminal_status": row.try_get::<Option<String>, _>("terminal_status")?,
        "completion_event_id": completion_event_id.map(hex::encode),
        "action_deadline_at_ms": deadline.map(|value| value.timestamp_millis()),
        "progress_seq": row.try_get::<i64, _>("progress_seq")?,
        "last_progress_stage": row.try_get::<Option<String>, _>("last_progress_stage")?,
        "last_progress_at_ms": last_progress_at.map(|value| value.timestamp_millis()),
        "operator_hard_deadline_ms": operator_hard_deadline.map(|value| value.timestamp_millis()),
        "last_error_code": row.try_get::<Option<String>, _>("last_error_code")?,
        "created_at_ms": created_at.timestamp_millis(),
        "updated_at_ms": updated_at.timestamp_millis(),
        "terminal_at_ms": terminal_at.map(|value| value.timestamp_millis()),
    })))
}

/// Mark an active direct action run terminal as part of Meeting End.
pub(crate) async fn mark_active_run_terminal_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    terminal_status: &str,
    completion_event_id: Option<&[u8]>,
    now: DateTime<Utc>,
) -> Result<()> {
    if !matches!(terminal_status, "completed_closed" | "completed_aborted") {
        return Err(DbError::InvalidData(format!(
            "invalid Meeting V2 action terminal status: {terminal_status}"
        )));
    }
    if terminal_status == "completed_closed"
        && completion_event_id.is_some_and(|event_id| event_id.len() != 32)
    {
        return Err(DbError::InvalidData(
            "Meeting V2 action completion event must be 32 bytes".to_string(),
        ));
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
    let completion_event_id = if terminal_status == "completed_closed" {
        Some(completion_event_id.ok_or_else(|| {
            DbError::InvalidData(
                "closing an active Meeting action run requires its End event".to_string(),
            )
        })?)
    } else {
        None
    };
    let updated = sqlx::query(
        "UPDATE meeting_v2_action_runs \
         SET terminal_status = $3, completion_event_id = $4, terminal_at = $5, \
             action_deadline_at = NULL, updated_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND action_run_id = $6 \
           AND terminal_status IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(terminal_status)
    .bind(completion_event_id)
    .bind(now)
    .bind(action_run_id)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 action changed while ending Meeting".to_string(),
        ));
    }
    Ok(())
}

/// Validate the moderator's direct recorded-actions attestation fence.
pub(crate) async fn validate_close_gate_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    action_run_id: Uuid,
    action_window_epoch: i64,
    board_event_id: &[u8],
) -> Result<bool> {
    if board_event_id.len() != 32 || action_window_epoch <= 0 {
        return Ok(false);
    }
    sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_v2_action_runs run \
             WHERE run.community_id = $1 AND run.session_id = $2 \
               AND run.action_run_id = $3 AND run.action_window_epoch = $4 \
               AND run.board_event_id = $5 AND run.terminal_status IS NULL \
               AND run.action_condition = 'runnable' \
               AND run.action_deadline_at > clock_timestamp() \
               AND (run.operator_hard_deadline IS NULL \
                    OR run.operator_hard_deadline > clock_timestamp()) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(action_run_id)
    .bind(action_window_epoch)
    .bind(board_event_id)
    .fetch_one(tx.as_mut())
    .await
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_action_command_vocabulary_has_no_plan_or_steps() {
        let fence = ActionRunFence {
            action_run_id: Uuid::new_v4(),
            action_window_epoch: 1,
            board_event_id: vec![1; 32],
        };
        assert_eq!(
            ActionCommand::Retry {
                fence: fence.clone()
            }
            .action(),
            "retry"
        );
        assert_eq!(
            ActionCommand::ReturnToBoard { fence }.action(),
            "return-to-board"
        );
    }

    #[test]
    fn current_direct_block_reasons_are_target_agnostic_without_affinity() {
        for reason in [
            "external_operation_failed",
            "external_state_conflict",
            "tool_unavailable",
            "provider_failure",
            "action_deadline_exceeded",
            "action_lease_expired",
            "action_operator_deadline_exceeded",
        ] {
            assert!(is_block_reason(reason));
        }
        assert!(!is_block_reason("affinity_lost"));
        assert!(!is_block_reason("assignee_unresolved"));
    }
}
