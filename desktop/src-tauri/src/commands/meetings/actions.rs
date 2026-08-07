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
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

use crate::{
    app_state::AppState,
    meeting_runtime::{
        MeetingActionRenewalBinding, MeetingActionRenewalRegistration, RegisterMeetingActionRenewal,
    },
    pending_writes::PendingMeetingCommand,
    relay::{
        parse_command_response, relay_api_base_url_with_override, submit_signed_event_at_with_keys,
        submit_signed_event_at_with_keys_typed, RelayHttpError, RelayHttpErrorCategory,
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
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
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
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
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

#[derive(Debug, PartialEq, Eq)]
enum ReceiptValidationError {
    Unverifiable(String),
    CanonicalConflict(String),
}

impl ReceiptValidationError {
    fn message(self) -> String {
        match self {
            Self::Unverifiable(message) | Self::CanonicalConflict(message) => message,
        }
    }
}

const HUMAN_ACTION_RENEW_CADENCE: std::time::Duration = std::time::Duration::from_secs(25);
const HUMAN_ACTION_RENEW_RETRY: std::time::Duration = std::time::Duration::from_secs(2);
const ACTION_LEASE_SAFETY_MARGIN: std::time::Duration = std::time::Duration::from_secs(15);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Exact canonical Action window for which Desktop should retain a Human claim.
pub struct EnsureMeetingActionRenewalInput {
    meeting_id: String,
    action_run_id: String,
    action_window_epoch: u64,
    board_event_id: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EnsureMeetingActionRenewalResult {
    Started,
    AlreadyActive,
}

#[derive(Debug, Deserialize)]
struct RenewalReceipt {
    meeting_id: String,
    accepted: bool,
    outcome: String,
    action_run_id: Option<Uuid>,
    action_window_epoch: Option<i64>,
    state_revision: Option<i64>,
    lease_ttl_ms: Option<i64>,
    operator_hard_remaining_ms: Option<i64>,
    accepted_progress_seq: Option<i64>,
}

struct PreparedRenewal {
    event: nostr::Event,
    progress_seq: u64,
}

/// Ensure that the current frozen Human moderator retains the exact runnable
/// Action lease even after React navigates away from the Meeting screen.
#[tauri::command]
pub async fn ensure_meeting_action_renewal(
    input: EnsureMeetingActionRenewalInput,
    app: AppHandle,
) -> Result<EnsureMeetingActionRenewalResult, String> {
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    let action_run_id = canonical_uuid(&input.action_run_id, "Meeting action run ID")?;
    if input.action_window_epoch == 0 {
        return Err("Meeting action window must be positive".to_string());
    }
    canonical_hex64(&input.board_event_id, "Meeting action Board event")?;

    let state = app.state::<AppState>();
    let api_base_url = relay_api_base_url_with_override(&state);
    let keys = state.signing_keys()?;
    let signer_pubkey = keys.public_key().to_hex();
    let binding = MeetingActionRenewalBinding {
        api_base_url,
        signer_pubkey,
        meeting_id,
        action_run_id,
        action_window_epoch: input.action_window_epoch,
        board_event_id: input.board_event_id,
    };
    let Some(_) = load_exact_human_action_snapshot(&state, &binding).await? else {
        return Err(
            "only the frozen Human moderator can renew the current runnable Action window"
                .to_string(),
        );
    };
    match state.meeting_action_renewals.register(binding)? {
        RegisterMeetingActionRenewal::Existing => {
            Ok(EnsureMeetingActionRenewalResult::AlreadyActive)
        }
        RegisterMeetingActionRenewal::Started(registration) => {
            tauri::async_runtime::spawn(run_human_action_renewal(app.clone(), registration));
            Ok(EnsureMeetingActionRenewalResult::Started)
        }
    }
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
        Err(ReceiptValidationError::CanonicalConflict(message)) => {
            remove_pending(state, &validated.submission_id, &pending.event);
            return Err(message);
        }
        Err(error) => {
            return Ok(indeterminate_result(
                &pending,
                format!(
                    "Relay accepted the Meeting action-finalization command, but its receipt could not be verified: {}. Retry to confirm the same signed event.",
                    error.message()
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
) -> Result<ValidatedReceipt, ReceiptValidationError> {
    if response.event_id != pending.event.id.to_hex() {
        return Err(ReceiptValidationError::Unverifiable(
            "event ID does not match the signed action-finalization command".to_string(),
        ));
    }
    if let Some(expected_outcome) = action.expected_outcome() {
        let receipt: ActionReceipt = parse_command_response(&response.message)
            .map_err(ReceiptValidationError::Unverifiable)?;
        if receipt.meeting_id != pending.meeting_id
            || !receipt.accepted
            || receipt.outcome != expected_outcome
            || receipt.action_run_id.is_none_or(|run_id| run_id.is_nil())
            || receipt.action_window_epoch.is_none_or(|window| window <= 0)
            || receipt.state_revision.is_none_or(|revision| revision <= 0)
        {
            return Err(ReceiptValidationError::Unverifiable(
                "action receipt fields do not match the signed command".to_string(),
            ));
        }
        return Ok(ValidatedReceipt {
            state_revision: receipt.state_revision,
            duplicate: receipt.duplicate,
        });
    }

    let receipt: EndReceipt =
        parse_command_response(&response.message).map_err(ReceiptValidationError::Unverifiable)?;
    if receipt.meeting_id != pending.meeting_id || receipt.status != "ended" {
        return Err(ReceiptValidationError::Unverifiable(
            "action completion receipt does not identify an ended Meeting".to_string(),
        ));
    }
    match receipt.terminal_outcome.as_deref() {
        Some("closed") => {}
        Some("aborted") => {
            return Err(ReceiptValidationError::CanonicalConflict(
                "Meeting already ended as `aborted`; action output cannot be confirmed".to_string(),
            ));
        }
        _ => {
            return Err(ReceiptValidationError::Unverifiable(
                "action completion receipt has an unknown terminal outcome".to_string(),
            ));
        }
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

struct HumanActionHead {
    keys: nostr::Keys,
    progress_seq: u64,
}

async fn load_exact_human_action_snapshot(
    state: &AppState,
    binding: &MeetingActionRenewalBinding,
) -> Result<Option<HumanActionHead>, String> {
    if relay_api_base_url_with_override(state) != binding.api_base_url {
        return Ok(None);
    }
    let keys = state.signing_keys()?;
    if keys.public_key().to_hex() != binding.signer_pubkey {
        return Ok(None);
    }
    let Some(identity) = read_meeting_identity_at(state, &binding.api_base_url).await? else {
        return Ok(None);
    };
    let loaded = load_meeting_snapshot_at(
        state,
        &identity,
        &binding.meeting_id,
        &binding.api_base_url,
        &keys,
    )
    .await
    .map_err(super::read_error_message)?;
    let MeetingLoadResult::Ready { snapshot } = loaded else {
        return Ok(None);
    };
    if snapshot.policy != buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY
        || !matches!(snapshot.lifecycle, MeetingLifecycle::FinalizingActions)
        || snapshot.moderator_pubkey != binding.signer_pubkey
        || !snapshot.participants.iter().any(|participant| {
            participant.pubkey == binding.signer_pubkey
                && participant.participant_type == MeetingParticipantType::Human
        })
    {
        return Ok(None);
    }
    let Some(action) = snapshot.action.as_ref() else {
        return Ok(None);
    };
    if action.action_run_id != binding.action_run_id
        || action.action_window_epoch != binding.action_window_epoch
        || action.board_event_id != binding.board_event_id
        || action.board_event_id != snapshot.board.event_id
        || action.condition != "runnable"
        || action.terminal_status.is_some()
    {
        return Ok(None);
    }
    Ok(Some(HumanActionHead {
        keys,
        progress_seq: action.progress_seq,
    }))
}

fn prepare_human_action_renewal(
    binding: &MeetingActionRenewalBinding,
    head: &HumanActionHead,
) -> Result<PreparedRenewal, String> {
    let session_id = Uuid::parse_str(&binding.meeting_id)
        .map_err(|error| format!("invalid Meeting ID after validation: {error}"))?;
    let action_run_id = Uuid::parse_str(&binding.action_run_id)
        .map_err(|error| format!("invalid Meeting action run after validation: {error}"))?;
    let progress_seq = head
        .progress_seq
        .checked_add(1)
        .ok_or_else(|| "Meeting action progress sequence overflow".to_string())?;
    let event = buzz_sdk_pkg::build_meeting_v2_action_lease_renew(
        buzz_sdk_pkg::MeetingV2ActionLeaseRenewParams {
            session_id,
            fence: MeetingV2ActionRunFence {
                action_run_id,
                action_window: binding.action_window_epoch,
                board_event_id: &binding.board_event_id,
            },
            progress_seq,
            stage: buzz_sdk_pkg::MeetingV2ActionProgressStage::WaitingHuman,
            last_activity_seq: 0,
        },
    )
    .map_err(|error| format!("invalid Meeting Action renewal: {error}"))?
    .sign_with_keys(&head.keys)
    .map_err(|error| format!("failed to sign Meeting Action renewal: {error}"))?;
    Ok(PreparedRenewal {
        event,
        progress_seq,
    })
}

fn validated_renewal_delay(
    response: &SubmitEventResponse,
    binding: &MeetingActionRenewalBinding,
    prepared: &PreparedRenewal,
    request_started_at: std::time::Instant,
) -> Result<std::time::Duration, String> {
    if response.event_id != prepared.event.id.to_hex() {
        return Err("renewal response event ID does not match the signed event".to_string());
    }
    let receipt: RenewalReceipt = parse_command_response(&response.message)?;
    let expected_run = Uuid::parse_str(&binding.action_run_id)
        .map_err(|error| format!("invalid Meeting action run after validation: {error}"))?;
    if receipt.meeting_id != binding.meeting_id
        || !receipt.accepted
        || receipt.outcome != "action_lease_renewed"
        || receipt.action_run_id != Some(expected_run)
        || receipt.action_window_epoch != i64::try_from(binding.action_window_epoch).ok()
        || receipt.accepted_progress_seq != i64::try_from(prepared.progress_seq).ok()
        || receipt.state_revision.is_none_or(|revision| revision <= 0)
    {
        return Err("renewal receipt does not match the signed Action fence".to_string());
    }
    let lease_ttl_ms = receipt
        .lease_ttl_ms
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| "renewal receipt has no positive lease TTL".to_string())?;
    let mut local_deadline = request_started_at
        .checked_add(std::time::Duration::from_millis(lease_ttl_ms))
        .and_then(|deadline| deadline.checked_sub(ACTION_LEASE_SAFETY_MARGIN))
        .ok_or_else(|| "renewal lease is inside the local safety margin".to_string())?;
    if let Some(operator_ms) = receipt.operator_hard_remaining_ms {
        let operator_ms = u64::try_from(operator_ms)
            .map_err(|_| "renewal operator deadline has elapsed".to_string())?;
        let operator_deadline = request_started_at
            .checked_add(std::time::Duration::from_millis(operator_ms))
            .and_then(|deadline| deadline.checked_sub(ACTION_LEASE_SAFETY_MARGIN))
            .ok_or_else(|| "renewal operator deadline is inside the safety margin".to_string())?;
        local_deadline = local_deadline.min(operator_deadline);
    }
    let remaining = local_deadline.saturating_duration_since(std::time::Instant::now());
    if remaining.is_zero() {
        return Err("renewal response arrived after the local safety deadline".to_string());
    }
    let half_remaining = remaining / 2;
    Ok(HUMAN_ACTION_RENEW_CADENCE
        .min(half_remaining)
        .max(std::time::Duration::from_secs(1)))
}

async fn wait_for_renewal_tick(
    cancel: &mut tokio::sync::watch::Receiver<bool>,
    delay: std::time::Duration,
) -> bool {
    if *cancel.borrow() {
        return false;
    }
    tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        changed = cancel.changed() => changed.is_ok() && !*cancel.borrow(),
    }
}

fn definitive_renewal_error(error: &RelayHttpError) -> bool {
    !error.request_may_have_reached_relay
        && matches!(
            error.category,
            RelayHttpErrorCategory::Forbidden
                | RelayHttpErrorCategory::Conflict
                | RelayHttpErrorCategory::Http
                | RelayHttpErrorCategory::Malformed
                | RelayHttpErrorCategory::Internal
        )
}

async fn run_human_action_renewal(
    app: AppHandle,
    mut registration: MeetingActionRenewalRegistration,
) {
    let mut prepared: Option<PreparedRenewal> = None;
    let mut delay = std::time::Duration::ZERO;
    loop {
        if !wait_for_renewal_tick(&mut registration.cancel, delay).await {
            break;
        }
        let state = app.state::<AppState>();
        let head = match load_exact_human_action_snapshot(&state, &registration.binding).await {
            Ok(Some(head)) => head,
            Ok(None) => break,
            Err(error) => {
                eprintln!("buzz-desktop: Human Meeting Action renewal read failed: {error}");
                delay = HUMAN_ACTION_RENEW_RETRY;
                continue;
            }
        };
        if prepared
            .as_ref()
            .is_some_and(|pending| head.progress_seq >= pending.progress_seq)
        {
            prepared = None;
        }
        if prepared.is_none() {
            match prepare_human_action_renewal(&registration.binding, &head) {
                Ok(event) => prepared = Some(event),
                Err(error) => {
                    eprintln!("buzz-desktop: Human Meeting Action renewal stopped: {error}");
                    break;
                }
            }
        }
        let Some(pending) = prepared.as_ref() else {
            break;
        };
        let request_started_at = std::time::Instant::now();
        let submit = submit_signed_event_at_with_keys_typed(
            &pending.event,
            &state,
            &registration.binding.api_base_url,
            &head.keys,
        );
        let response = tokio::select! {
            changed = registration.cancel.changed() => {
                let _ = changed;
                break;
            }
            response = submit => response,
        };
        match response {
            Ok(response) => match validated_renewal_delay(
                &response,
                &registration.binding,
                pending,
                request_started_at,
            ) {
                Ok(next_delay) => {
                    prepared = None;
                    delay = next_delay;
                }
                Err(error) => {
                    eprintln!(
                        "buzz-desktop: Human Meeting Action renewal receipt needs reconciliation: {error}"
                    );
                    delay = HUMAN_ACTION_RENEW_RETRY;
                }
            },
            Err(error) => {
                eprintln!(
                    "buzz-desktop: Human Meeting Action renewal submit failed: {}",
                    error.message
                );
                if definitive_renewal_error(&error) {
                    match load_exact_human_action_snapshot(&state, &registration.binding).await {
                        Ok(Some(canonical))
                            if canonical.progress_seq
                                >= prepared.as_ref().map_or(0, |value| value.progress_seq) =>
                        {
                            prepared = None;
                        }
                        Ok(Some(_)) => break,
                        Ok(None) => break,
                        Err(_) => {}
                    }
                }
                delay = HUMAN_ACTION_RENEW_RETRY;
            }
        }
    }
    app.state::<AppState>()
        .meeting_action_renewals
        .finish(&registration.key, registration.generation);
}

#[cfg(test)]
#[path = "actions/tests.rs"]
mod tests;
