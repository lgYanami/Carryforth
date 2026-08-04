//! Human Meeting V2 action-finalization command boundary.
//!
//! React chooses a bounded product action and returns an opaque control token.
//! Native reloads the verified Meeting snapshot, derives every action-run and
//! Board fence, signs once, and retains that exact event while the Relay result
//! is indeterminate.

use buzz_sdk_pkg::{
    MeetingV2ActionBeginParams, MeetingV2ActionBlockParams, MeetingV2ActionCommandParams,
    MeetingV2ActionRunFence, MeetingV2ActionsEndFence, MeetingV2ActionsEndParams,
    MeetingV2EndOutcome,
};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    pending_writes::PendingMeetingCommand,
    relay::{
        parse_command_response, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
        SubmitEventResponse,
    },
};

use super::pending::{
    canonical_hex64, canonical_uuid, find_pending, insert_or_reuse_pending,
    is_indeterminate_submit_error, remove_pending, PendingBinding,
};
use super::{
    load_meeting_snapshot_at, read_meeting_identity_at, MeetingActionState, MeetingLifecycle,
    MeetingLoadResult, MeetingParticipantType, MeetingSnapshot,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Bounded Human-host action-finalization input accepted by the native boundary.
pub struct MeetingActionFinalizationInput {
    /// Stable UUID generated once and reused only for an exact retry.
    submission_id: String,
    meeting_id: String,
    /// Opaque token emitted by the verified host projection.
    expected_control_token: String,
    action: MeetingActionFinalizationAction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MeetingActionFinalizationAction {
    Begin,
    Block {
        reason_code: ActionBlockReasonInput,
        reason: Option<String>,
    },
    Retry,
    ReturnToBoard,
    Confirm,
}

impl MeetingActionFinalizationAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Begin => "begin",
            Self::Block { .. } => "block",
            Self::Retry => "retry",
            Self::ReturnToBoard => "return_to_board",
            Self::Confirm => "confirm",
        }
    }

    fn expected_outcome(&self) -> Option<&'static str> {
        match self {
            Self::Begin => Some("action_finalization_began"),
            Self::Block { .. } => Some("action_blocked"),
            Self::Retry => Some("action_retried"),
            Self::ReturnToBoard => Some("action_returned_to_board"),
            Self::Confirm => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ActionBlockReasonInput {
    ExternalOperationFailed,
    ExternalStateConflict,
    ToolUnavailable,
    ProviderFailure,
    AffinityLost,
    ActionDeadlineExceeded,
}

impl ActionBlockReasonInput {
    fn as_str(self) -> &'static str {
        match self {
            Self::ExternalOperationFailed => "external_operation_failed",
            Self::ExternalStateConflict => "external_state_conflict",
            Self::ToolUnavailable => "tool_unavailable",
            Self::ProviderFailure => "provider_failure",
            Self::AffinityLost => "affinity_lost",
            Self::ActionDeadlineExceeded => "action_deadline_exceeded",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
/// Verified submission outcome for one Human action-finalization command.
pub enum MeetingActionFinalizationResult {
    Accepted {
        meeting_id: String,
        event_id: String,
        action: String,
        state_revision: Option<i64>,
        duplicate: bool,
    },
    Indeterminate {
        meeting_id: String,
        event_id: String,
        action: String,
        message: String,
    },
}

#[derive(Debug, Deserialize)]
struct ActionReceipt {
    meeting_id: String,
    accepted: bool,
    outcome: String,
    action_run_id: Option<Uuid>,
    action_window_epoch: Option<i64>,
    state_revision: Option<i64>,
    duplicate: bool,
}

#[derive(Debug, Deserialize)]
struct EndReceipt {
    meeting_id: String,
    status: String,
    already_ended: bool,
    terminal_outcome: Option<String>,
}

struct ValidatedInput {
    submission_id: String,
    meeting_id: String,
    expected_control_token: String,
    action: MeetingActionFinalizationAction,
    fingerprint: String,
}

struct ValidatedReceipt {
    state_revision: Option<i64>,
    duplicate: bool,
}

/// Submit one Human-hosted Meeting action-finalization decision.
#[tauri::command]
pub async fn submit_meeting_action_finalization(
    input: MeetingActionFinalizationInput,
    state: State<'_, AppState>,
) -> Result<MeetingActionFinalizationResult, String> {
    execute_action(input, &state).await
}

async fn execute_action(
    input: MeetingActionFinalizationInput,
    state: &AppState,
) -> Result<MeetingActionFinalizationResult, String> {
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    let signer_pubkey = keys.public_key().to_hex();
    let validated = validate_input(input)?;
    let binding = pending_binding(&validated, &api_base_url, &signer_pubkey);

    let pending = if let Some(pending) = find_pending(state, &binding)? {
        pending
    } else {
        let identity = read_meeting_identity_at(state, &api_base_url)
            .await?
            .ok_or_else(|| "unsupported: Relay does not advertise Meeting V2".to_string())?;
        if !identity.capability.supports_direct_actions {
            return Err(
                "unsupported: Relay does not advertise Meeting action finalization".to_string(),
            );
        }
        let loaded = load_meeting_snapshot_at(
            state,
            &identity,
            &validated.meeting_id,
            &api_base_url,
            &keys,
        )
        .await
        .map_err(super::read_error_message)?;
        let MeetingLoadResult::Ready { snapshot } = loaded else {
            return Err("Meeting action finalization is unavailable for this Meeting".to_string());
        };
        let prepared =
            prepare_command(&validated, &snapshot, &api_base_url, &signer_pubkey, &keys)?;
        insert_or_reuse_pending(state, prepared, &binding)?
    };

    let response =
        match submit_signed_event_at_with_keys(&pending.event, state, &pending.api_base_url, &keys)
            .await
        {
            Ok(response) => response,
            Err(message) if is_indeterminate_submit_error(&message) => {
                return Ok(indeterminate_result(&pending, message));
            }
            Err(message) => {
                remove_pending(state, &validated.submission_id, &pending.event);
                return Err(message);
            }
        };

    let receipt = match validate_receipt(&response, &pending, &validated.action) {
        Ok(receipt) => receipt,
        Err(message) => {
            return Ok(indeterminate_result(
                &pending,
                format!(
                    "Relay accepted the Meeting action-finalization command, but its receipt could not be verified: {message}. Retry to confirm the same signed event."
                ),
            ));
        }
    };
    remove_pending(state, &validated.submission_id, &pending.event);
    Ok(MeetingActionFinalizationResult::Accepted {
        meeting_id: pending.meeting_id,
        event_id: pending.event.id.to_hex(),
        action: pending.action,
        state_revision: receipt.state_revision,
        duplicate: receipt.duplicate,
    })
}

fn pending_binding<'a>(
    input: &'a ValidatedInput,
    api_base_url: &'a str,
    signer_pubkey: &'a str,
) -> PendingBinding<'a> {
    PendingBinding {
        submission_id: &input.submission_id,
        meeting_id: &input.meeting_id,
        fingerprint: &input.fingerprint,
        api_base_url,
        signer_pubkey,
        context: "Meeting action finalization",
    }
}

fn validate_input(input: MeetingActionFinalizationInput) -> Result<ValidatedInput, String> {
    let submission_id = canonical_uuid(
        &input.submission_id,
        "Meeting action-finalization submission ID",
    )?;
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    canonical_hex64(
        &input.expected_control_token,
        "Meeting action-finalization control token",
    )?;
    let action = normalize_action(input.action)?;
    let fingerprint = serde_json::to_string(&(
        meeting_id.as_str(),
        input.expected_control_token.as_str(),
        &action,
    ))
    .map_err(|error| format!("serialize Meeting action-finalization fingerprint: {error}"))?;
    Ok(ValidatedInput {
        submission_id,
        meeting_id,
        expected_control_token: input.expected_control_token,
        action,
        fingerprint,
    })
}

fn normalize_action(
    action: MeetingActionFinalizationAction,
) -> Result<MeetingActionFinalizationAction, String> {
    match action {
        MeetingActionFinalizationAction::Block {
            reason_code,
            reason,
        } => Ok(MeetingActionFinalizationAction::Block {
            reason_code,
            reason: optional_text(reason, 1_024, "Meeting action block explanation")?,
        }),
        other => Ok(other),
    }
}

fn optional_text(
    value: Option<String>,
    limit: usize,
    context: &str,
) -> Result<Option<String>, String> {
    let value = value
        .map(|candidate| candidate.trim().to_string())
        .filter(|candidate| !candidate.is_empty());
    if value.as_ref().is_some_and(|candidate| {
        candidate.len() > limit
            || candidate.contains('\0')
            || candidate.chars().any(char::is_control)
    }) {
        return Err(format!("{context} must be clean and at most {limit} bytes"));
    }
    Ok(value)
}

fn prepare_command(
    input: &ValidatedInput,
    snapshot: &MeetingSnapshot,
    api_base_url: &str,
    signer_pubkey: &str,
    keys: &nostr::Keys,
) -> Result<PendingMeetingCommand, String> {
    if snapshot.policy != buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY {
        return Err("Meeting does not support direct action finalization".to_string());
    }
    let participant = snapshot
        .participants
        .iter()
        .find(|participant| participant.pubkey == signer_pubkey)
        .ok_or_else(|| "current identity is outside the frozen Meeting roster".to_string())?;
    if participant.participant_type != MeetingParticipantType::Human
        || signer_pubkey != snapshot.moderator_pubkey
    {
        return Err(
            "only the frozen Human moderator can operate Meeting action finalization".to_string(),
        );
    }
    let host = snapshot
        .host
        .as_ref()
        .ok_or_else(|| "Meeting host projection is not initialized yet".to_string())?;
    if host.control_token != input.expected_control_token {
        return Err(
            "Meeting action-finalization control changed; refresh before submitting".to_string(),
        );
    }
    validate_action_authority(&input.action, snapshot)?;
    let session_id = Uuid::parse_str(&input.meeting_id)
        .map_err(|error| format!("invalid Meeting ID after validation: {error}"))?;
    let event = build_event(&input.action, snapshot, session_id)?
        .sign_with_keys(keys)
        .map_err(|error| format!("failed to sign Meeting action-finalization command: {error}"))?;
    Ok(PendingMeetingCommand {
        event,
        api_base_url: api_base_url.to_string(),
        signer_pubkey: signer_pubkey.to_string(),
        meeting_id: input.meeting_id.clone(),
        fingerprint: input.fingerprint.clone(),
        action: input.action.name().to_string(),
    })
}

fn validate_action_authority(
    action: &MeetingActionFinalizationAction,
    snapshot: &MeetingSnapshot,
) -> Result<(), String> {
    let host = snapshot
        .host
        .as_ref()
        .ok_or_else(|| "Meeting host projection is not initialized yet".to_string())?;
    match action {
        MeetingActionFinalizationAction::Begin => {
            if !matches!(snapshot.lifecycle, MeetingLifecycle::Active)
                || snapshot.phase != "moderator_idle"
                || !host.can_close
                || host.board_control.phase != "floor_ready"
                || !host.pending_intents.is_empty()
                || !host.open_handoffs.is_empty()
            {
                return Err(
                    "action finalization requires an explicit final Board and an idle host Floor"
                        .to_string(),
                );
            }
        }
        MeetingActionFinalizationAction::Block { .. } => {
            require_action_condition(snapshot, "runnable")?;
        }
        MeetingActionFinalizationAction::Retry => {
            require_action_condition(snapshot, "blocked")?;
        }
        MeetingActionFinalizationAction::ReturnToBoard => {
            require_active_action(snapshot)?;
        }
        MeetingActionFinalizationAction::Confirm => {
            require_action_condition(snapshot, "runnable")?;
        }
    }
    Ok(())
}

fn require_active_action(snapshot: &MeetingSnapshot) -> Result<&MeetingActionState, String> {
    if !matches!(snapshot.lifecycle, MeetingLifecycle::FinalizingActions) {
        return Err("Meeting is not finalizing actions".to_string());
    }
    let action = snapshot
        .action
        .as_ref()
        .ok_or_else(|| "Meeting has no active action run".to_string())?;
    if action.terminal_status.is_some() || action.board_event_id != snapshot.board.event_id {
        return Err("Meeting action run is no longer active for the frozen Board".to_string());
    }
    Ok(action)
}

fn require_action_condition<'a>(
    snapshot: &'a MeetingSnapshot,
    condition: &str,
) -> Result<&'a MeetingActionState, String> {
    let action = require_active_action(snapshot)?;
    if action.condition != condition {
        return Err(format!(
            "Meeting action run is not {condition}; refresh before submitting"
        ));
    }
    Ok(action)
}

fn action_fence(action: &MeetingActionState) -> Result<MeetingV2ActionRunFence<'_>, String> {
    let action_run_id = Uuid::parse_str(&action.action_run_id)
        .map_err(|_| "Meeting action run has an invalid ID".to_string())?;
    if action_run_id.is_nil() {
        return Err("Meeting action run has an invalid ID".to_string());
    }
    Ok(MeetingV2ActionRunFence {
        action_run_id,
        action_window: action.action_window_epoch,
        board_event_id: &action.board_event_id,
    })
}

fn build_event(
    action: &MeetingActionFinalizationAction,
    snapshot: &MeetingSnapshot,
    session_id: Uuid,
) -> Result<nostr::EventBuilder, String> {
    let result = match action {
        MeetingActionFinalizationAction::Begin => {
            let host = snapshot
                .host
                .as_ref()
                .ok_or_else(|| "Meeting host projection is unavailable".to_string())?;
            buzz_sdk_pkg::build_meeting_v2_action_begin(MeetingV2ActionBeginParams {
                session_id,
                expected_control_epoch: host.board_control.control_epoch,
                board_window: host.board_control.board_window,
                expected_state_event_id: &host.state_event_id,
                board_event_id: &snapshot.board.event_id,
                expected_decision_attempt_id: None,
            })
        }
        MeetingActionFinalizationAction::Block {
            reason_code,
            reason,
        } => {
            let current = require_action_condition(snapshot, "runnable")?;
            buzz_sdk_pkg::build_meeting_v2_action_block(MeetingV2ActionBlockParams {
                session_id,
                fence: action_fence(current)?,
                reason_code: reason_code.as_str(),
                reason: reason.as_deref(),
            })
        }
        MeetingActionFinalizationAction::Retry => {
            let current = require_action_condition(snapshot, "blocked")?;
            buzz_sdk_pkg::build_meeting_v2_action_retry(MeetingV2ActionCommandParams {
                session_id,
                fence: action_fence(current)?,
            })
        }
        MeetingActionFinalizationAction::ReturnToBoard => {
            let current = require_active_action(snapshot)?;
            buzz_sdk_pkg::build_meeting_v2_action_return_to_board(MeetingV2ActionCommandParams {
                session_id,
                fence: action_fence(current)?,
            })
        }
        MeetingActionFinalizationAction::Confirm => {
            let current = require_action_condition(snapshot, "runnable")?;
            let action_run_id = Uuid::parse_str(&current.action_run_id)
                .map_err(|_| "Meeting action run has an invalid ID".to_string())?;
            buzz_sdk_pkg::build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
                session_id,
                create_event_id: &snapshot.create_event_id,
                outcome: MeetingV2EndOutcome::Closed,
                reason_code: None,
                reason: None,
                action_fence: Some(MeetingV2ActionsEndFence {
                    action_run_id,
                    action_window: current.action_window_epoch,
                    board_event_id: &current.board_event_id,
                }),
            })
        }
    };
    result.map_err(|error| format!("invalid Meeting action-finalization command: {error}"))
}

fn validate_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
    action: &MeetingActionFinalizationAction,
) -> Result<ValidatedReceipt, String> {
    if response.event_id != pending.event.id.to_hex() {
        return Err("event ID does not match the signed action-finalization command".to_string());
    }
    if let Some(expected_outcome) = action.expected_outcome() {
        let receipt: ActionReceipt = parse_command_response(&response.message)?;
        if receipt.meeting_id != pending.meeting_id
            || !receipt.accepted
            || receipt.outcome != expected_outcome
            || receipt.action_run_id.is_none_or(|run_id| run_id.is_nil())
            || receipt.action_window_epoch.is_none_or(|window| window <= 0)
            || receipt.state_revision.is_none_or(|revision| revision <= 0)
        {
            return Err("action receipt fields do not match the signed command".to_string());
        }
        return Ok(ValidatedReceipt {
            state_revision: receipt.state_revision,
            duplicate: receipt.duplicate,
        });
    }

    let receipt: EndReceipt = parse_command_response(&response.message)?;
    if receipt.meeting_id != pending.meeting_id
        || receipt.status != "ended"
        || receipt.terminal_outcome.as_deref() != Some("closed")
    {
        return Err("action completion receipt does not close the Meeting".to_string());
    }
    Ok(ValidatedReceipt {
        state_revision: None,
        duplicate: receipt.already_ended,
    })
}

fn indeterminate_result(
    pending: &PendingMeetingCommand,
    message: String,
) -> MeetingActionFinalizationResult {
    MeetingActionFinalizationResult::Indeterminate {
        meeting_id: pending.meeting_id.clone(),
        event_id: pending.event.id.to_hex(),
        action: pending.action.clone(),
        message,
    }
}

#[cfg(test)]
#[path = "actions/tests.rs"]
mod tests;
