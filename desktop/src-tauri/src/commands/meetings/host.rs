//! Human Meeting V2 host command boundary.
//!
//! React submits bounded product actions plus an opaque control token. Native
//! reloads the verified projection, derives every wire fence and object
//! version, signs once, and preserves that exact event across an indeterminate
//! Relay response.

use buzz_sdk_pkg::{
    MeetingV1HandoffDismissReason, MeetingV1IntentDeferral, MeetingV1IntentRefreshParams,
    MeetingV1IntentRejectionReason, MeetingV1IntentSubmitParams, MeetingV1IntentWithdrawParams,
    MeetingV1ModeratorDismissHandoffParams, MeetingV1ModeratorRecallParams,
    MeetingV1ModeratorRejectParams, MeetingV1ModeratorSelectParams, MeetingV1Selection,
    MeetingV2BoardActionParams, MeetingV2EndOutcome,
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
    load_meeting_snapshot_at, read_meeting_identity_at, MeetingHostState, MeetingLifecycle,
    MeetingLoadResult, MeetingParticipantType, MeetingPendingIntent, MeetingSnapshot,
};

#[path = "host/builders.rs"]
mod builders;
use builders::{build_board_action, build_end};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Bounded Human-host action input accepted by the native Meeting boundary.
pub struct MeetingHostActionInput {
    /// Stable UUID generated once and reused only for an exact retry.
    submission_id: String,
    meeting_id: String,
    /// Opaque token emitted by the verified host projection.
    expected_control_token: String,
    action: MeetingHostAction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MeetingHostAction {
    BoardUpdate {
        body: String,
    },
    BoardUnchanged,
    IntentSubmit {
        summary: String,
        addressed_to: Option<String>,
    },
    IntentRefresh {
        intent_id: String,
        summary: String,
        addressed_to: Option<String>,
    },
    IntentWithdraw {
        intent_id: String,
    },
    SelectIntent {
        intent_id: String,
        selection_reason: Option<String>,
        deferral_reason: Option<String>,
    },
    SelectHandoff {
        handoff_id: String,
        selection_reason: Option<String>,
    },
    RejectIntent {
        intent_id: String,
        reason_code: IntentRejectionReasonInput,
        reason: String,
    },
    DismissHandoff {
        handoff_id: String,
        reason_code: HandoffDismissReasonInput,
        reason: String,
    },
    Recall {
        reason: Option<String>,
    },
    Close,
    Abort {
        reason_code: AbortReasonInput,
        reason: Option<String>,
    },
}

impl MeetingHostAction {
    fn name(&self) -> &'static str {
        match self {
            Self::BoardUpdate { .. } => "board_update",
            Self::BoardUnchanged => "board_unchanged",
            Self::IntentSubmit { .. } => "intent_submit",
            Self::IntentRefresh { .. } => "intent_refresh",
            Self::IntentWithdraw { .. } => "intent_withdraw",
            Self::SelectIntent { .. } => "select_intent",
            Self::SelectHandoff { .. } => "select_handoff",
            Self::RejectIntent { .. } => "reject_intent",
            Self::DismissHandoff { .. } => "dismiss_handoff",
            Self::Recall { .. } => "recall",
            Self::Close => "close",
            Self::Abort { .. } => "abort",
        }
    }

    fn receipt_kind(&self) -> ReceiptKind {
        match self {
            Self::BoardUpdate { .. } | Self::BoardUnchanged => ReceiptKind::Board,
            Self::Close | Self::Abort { .. } => ReceiptKind::End,
            _ => ReceiptKind::Control,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum IntentRejectionReasonInput {
    OffTopic,
    Duplicate,
    Superseded,
    Unsupported,
    AgendaMismatch,
}

impl From<IntentRejectionReasonInput> for MeetingV1IntentRejectionReason {
    fn from(value: IntentRejectionReasonInput) -> Self {
        match value {
            IntentRejectionReasonInput::OffTopic => Self::OffTopic,
            IntentRejectionReasonInput::Duplicate => Self::Duplicate,
            IntentRejectionReasonInput::Superseded => Self::Superseded,
            IntentRejectionReasonInput::Unsupported => Self::Unsupported,
            IntentRejectionReasonInput::AgendaMismatch => Self::AgendaMismatch,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum HandoffDismissReasonInput {
    Superseded,
    AnsweredElsewhere,
    OutOfScope,
    NoLongerNeeded,
}

impl From<HandoffDismissReasonInput> for MeetingV1HandoffDismissReason {
    fn from(value: HandoffDismissReasonInput) -> Self {
        match value {
            HandoffDismissReasonInput::Superseded => Self::Superseded,
            HandoffDismissReasonInput::AnsweredElsewhere => Self::AnsweredElsewhere,
            HandoffDismissReasonInput::OutOfScope => Self::OutOfScope,
            HandoffDismissReasonInput::NoLongerNeeded => Self::NoLongerNeeded,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum AbortReasonInput {
    GoalUnreachable,
    InsufficientInformation,
    DiscussionBlocked,
    UnableToFormConclusion,
    ModeratorUnableToContinue,
}

impl AbortReasonInput {
    fn as_str(self) -> &'static str {
        match self {
            Self::GoalUnreachable => "goal_unreachable",
            Self::InsufficientInformation => "insufficient_information",
            Self::DiscussionBlocked => "discussion_blocked",
            Self::UnableToFormConclusion => "unable_to_form_conclusion",
            Self::ModeratorUnableToContinue => "moderator_unable_to_continue",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
/// Verified submission outcome for one Human-host Meeting action.
pub enum MeetingHostActionResult {
    Accepted {
        meeting_id: String,
        event_id: String,
        action: String,
        canonical_object_id: Option<String>,
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

#[derive(Debug, Clone, Copy)]
enum ReceiptKind {
    Control,
    Board,
    End,
}

#[derive(Debug, Deserialize)]
struct ControlReceipt {
    meeting_id: String,
    canonical_object_id: Option<String>,
    state_revision: Option<i64>,
    recovery_transitions: usize,
    duplicate: bool,
    outcome: String,
}

#[derive(Debug, Deserialize)]
struct BoardReceipt {
    meeting_id: String,
    outcome: String,
    duplicate: bool,
    state_revision: Option<i64>,
    board_event_id: Option<String>,
    recovery_transitions: usize,
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
    action: MeetingHostAction,
    fingerprint: String,
}

struct ValidatedReceipt {
    canonical_object_id: Option<String>,
    state_revision: Option<i64>,
    duplicate: bool,
}

/// Submit one Human-hosted Meeting action against a verified control window.
#[tauri::command]
pub async fn submit_meeting_host_action(
    input: MeetingHostActionInput,
    state: State<'_, AppState>,
) -> Result<MeetingHostActionResult, String> {
    execute_host_action(input, &state).await
}

async fn execute_host_action(
    input: MeetingHostActionInput,
    state: &AppState,
) -> Result<MeetingHostActionResult, String> {
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
            return Err("Meeting host controls are unavailable for this Meeting".to_string());
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

    let receipt = match validate_receipt(&response, &pending, validated.action.receipt_kind()) {
        Ok(receipt) => receipt,
        Err(message) => {
            return Ok(indeterminate_result(
                &pending,
                format!(
                    "Relay accepted the Meeting host command, but its receipt could not be verified: {message}. Retry to confirm the same signed event."
                ),
            ));
        }
    };
    remove_pending(state, &validated.submission_id, &pending.event);
    Ok(MeetingHostActionResult::Accepted {
        meeting_id: pending.meeting_id,
        event_id: pending.event.id.to_hex(),
        action: pending.action,
        canonical_object_id: receipt.canonical_object_id,
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
        context: "Meeting host",
    }
}

fn validate_input(input: MeetingHostActionInput) -> Result<ValidatedInput, String> {
    let submission_id = canonical_uuid(&input.submission_id, "Meeting host submission ID")?;
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    canonical_hex64(&input.expected_control_token, "Meeting host control token")?;
    let action = normalize_action(input.action)?;
    let fingerprint = serde_json::to_string(&(
        meeting_id.as_str(),
        input.expected_control_token.as_str(),
        &action,
    ))
    .map_err(|error| format!("serialize Meeting host fingerprint: {error}"))?;
    Ok(ValidatedInput {
        submission_id,
        meeting_id,
        expected_control_token: input.expected_control_token,
        action,
        fingerprint,
    })
}

fn normalize_action(action: MeetingHostAction) -> Result<MeetingHostAction, String> {
    match action {
        MeetingHostAction::BoardUpdate { body } => {
            if body.trim().is_empty()
                || body.len() > buzz_sdk_pkg::MAX_MEETING_V2_BOARD_BYTES
                || body.contains('\0')
            {
                return Err(format!(
                    "Meeting Board must be non-empty, NUL-free, and at most {} bytes",
                    buzz_sdk_pkg::MAX_MEETING_V2_BOARD_BYTES
                ));
            }
            Ok(MeetingHostAction::BoardUpdate { body })
        }
        MeetingHostAction::IntentSubmit {
            summary,
            addressed_to,
        } => Ok(MeetingHostAction::IntentSubmit {
            summary: required_text(summary, 512, "Meeting Intent summary")?,
            addressed_to: optional_pubkey(addressed_to, "Meeting Intent addressee")?,
        }),
        MeetingHostAction::IntentRefresh {
            intent_id,
            summary,
            addressed_to,
        } => {
            canonical_hex64(&intent_id, "Meeting Intent ID")?;
            Ok(MeetingHostAction::IntentRefresh {
                intent_id,
                summary: required_text(summary, 512, "Meeting Intent summary")?,
                addressed_to: optional_pubkey(addressed_to, "Meeting Intent addressee")?,
            })
        }
        MeetingHostAction::IntentWithdraw { intent_id } => {
            canonical_hex64(&intent_id, "Meeting Intent ID")?;
            Ok(MeetingHostAction::IntentWithdraw { intent_id })
        }
        MeetingHostAction::SelectIntent {
            intent_id,
            selection_reason,
            deferral_reason,
        } => {
            canonical_hex64(&intent_id, "Meeting Intent ID")?;
            Ok(MeetingHostAction::SelectIntent {
                intent_id,
                selection_reason: optional_text(selection_reason, 512, "speaker selection reason")?,
                deferral_reason: optional_text(
                    deferral_reason,
                    1024,
                    "self-speech deferral reason",
                )?,
            })
        }
        MeetingHostAction::SelectHandoff {
            handoff_id,
            selection_reason,
        } => {
            canonical_hex64(&handoff_id, "Meeting Handoff ID")?;
            Ok(MeetingHostAction::SelectHandoff {
                handoff_id,
                selection_reason: optional_text(selection_reason, 512, "Handoff selection reason")?,
            })
        }
        MeetingHostAction::RejectIntent {
            intent_id,
            reason_code,
            reason,
        } => {
            canonical_hex64(&intent_id, "Meeting Intent ID")?;
            Ok(MeetingHostAction::RejectIntent {
                intent_id,
                reason_code,
                reason: required_text(reason, 1024, "Intent rejection reason")?,
            })
        }
        MeetingHostAction::DismissHandoff {
            handoff_id,
            reason_code,
            reason,
        } => {
            canonical_hex64(&handoff_id, "Meeting Handoff ID")?;
            Ok(MeetingHostAction::DismissHandoff {
                handoff_id,
                reason_code,
                reason: required_text(reason, 1024, "Handoff dismissal reason")?,
            })
        }
        MeetingHostAction::Recall { reason } => Ok(MeetingHostAction::Recall {
            reason: optional_text(reason, 1024, "Meeting Recall reason")?,
        }),
        MeetingHostAction::Abort {
            reason_code,
            reason,
        } => Ok(MeetingHostAction::Abort {
            reason_code,
            reason: optional_text(reason, 1024, "Meeting abort reason")?,
        }),
        other => Ok(other),
    }
}

fn optional_pubkey(value: Option<String>, context: &str) -> Result<Option<String>, String> {
    let value = value
        .map(|candidate| candidate.trim().to_ascii_lowercase())
        .filter(|candidate| !candidate.is_empty());
    if let Some(value) = &value {
        canonical_hex64(value, context)?;
    }
    Ok(value)
}

fn optional_text(
    value: Option<String>,
    limit: usize,
    context: &str,
) -> Result<Option<String>, String> {
    value
        .map(|candidate| required_text(candidate, limit, context))
        .transpose()
}

fn required_text(value: String, limit: usize, context: &str) -> Result<String, String> {
    let value = value.trim().to_string();
    if value.is_empty()
        || value.len() > limit
        || value.contains('\0')
        || value.chars().any(char::is_control)
    {
        return Err(format!(
            "{context} must be non-empty, clean, and at most {limit} bytes"
        ));
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
    let action_phase_abort = matches!(snapshot.lifecycle, MeetingLifecycle::FinalizingActions)
        && matches!(&input.action, MeetingHostAction::Abort { .. });
    if !matches!(snapshot.lifecycle, MeetingLifecycle::Active) && !action_phase_abort {
        return Err(
            "Meeting host controls are frozen outside active discussion or action abort"
                .to_string(),
        );
    }
    let participant = snapshot
        .participants
        .iter()
        .find(|participant| participant.pubkey == signer_pubkey)
        .ok_or_else(|| "current identity is outside the frozen Meeting roster".to_string())?;
    if participant.participant_type != MeetingParticipantType::Human
        || signer_pubkey != snapshot.moderator_pubkey
    {
        return Err("only the frozen Human moderator can use Desktop host controls".to_string());
    }
    let host = snapshot
        .host
        .as_ref()
        .ok_or_else(|| "Meeting host projection is not initialized yet".to_string())?;
    if host.control_token != input.expected_control_token {
        return Err("Meeting host control changed; refresh before submitting".to_string());
    }
    validate_action_authority(&input.action, snapshot, host, signer_pubkey)?;
    let session_id = Uuid::parse_str(&input.meeting_id)
        .map_err(|error| format!("invalid Meeting ID after validation: {error}"))?;
    let builder = build_event(&input.action, snapshot, host, session_id, signer_pubkey)?;
    let event = builder
        .sign_with_keys(keys)
        .map_err(|error| format!("failed to sign Meeting host command: {error}"))?;
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
    action: &MeetingHostAction,
    snapshot: &MeetingSnapshot,
    host: &MeetingHostState,
    signer_pubkey: &str,
) -> Result<(), String> {
    let self_intent = host
        .pending_intents
        .iter()
        .find(|intent| intent.author_pubkey == signer_pubkey);
    match action {
        MeetingHostAction::BoardUpdate { .. } | MeetingHostAction::BoardUnchanged => {
            if host.board_control.phase != "board_pending" {
                return Err(
                    "the authoritative Board Maintenance window is no longer active".into(),
                );
            }
        }
        MeetingHostAction::IntentSubmit { addressed_to, .. } => {
            require_floor_decision(host)?;
            if self_intent.is_some() {
                return Err("the host already has a pending self Intent".to_string());
            }
            validate_addressee(addressed_to.as_deref(), snapshot, signer_pubkey)?;
        }
        MeetingHostAction::IntentRefresh {
            intent_id,
            addressed_to,
            ..
        } => {
            own_intent(host, intent_id, signer_pubkey)?;
            validate_addressee(addressed_to.as_deref(), snapshot, signer_pubkey)?;
        }
        MeetingHostAction::IntentWithdraw { intent_id } => {
            own_intent(host, intent_id, signer_pubkey)?;
        }
        MeetingHostAction::SelectIntent {
            intent_id,
            deferral_reason,
            ..
        } => {
            require_floor_decision(host)?;
            let selected = pending_intent(host, intent_id)?;
            if !selected.selectable {
                return Err("the selected Intent is not eligible in this Floor window".to_string());
            }
            if selected.author_pubkey != signer_pubkey && self_intent.is_some() {
                return Err("the host self Intent must be selected or withdrawn first".to_string());
            }
            let other_selectable = host.pending_intents.iter().any(|intent| {
                intent.author_pubkey != signer_pubkey && intent.selectable && !intent.deferred
            });
            if selected.author_pubkey == signer_pubkey
                && host.consecutive_moderator_speeches >= 1
                && other_selectable
                && deferral_reason.is_none()
            {
                return Err(
                    "another pending Intent requires a reason before consecutive host speech"
                        .to_string(),
                );
            }
        }
        MeetingHostAction::SelectHandoff { handoff_id, .. } => {
            require_floor_decision(host)?;
            if self_intent.is_some() {
                return Err("the host self Intent must be selected or withdrawn first".to_string());
            }
            let handoff = host
                .open_handoffs
                .iter()
                .find(|handoff| handoff.handoff_id == *handoff_id)
                .ok_or_else(|| "the selected Handoff is no longer open".to_string())?;
            if !handoff.selectable {
                return Err("the selected Handoff is not eligible in this Floor window".to_string());
            }
        }
        MeetingHostAction::RejectIntent { intent_id, .. } => {
            let intent = pending_intent(host, intent_id)?;
            if intent.author_pubkey == signer_pubkey {
                return Err("withdraw the host self Intent instead of rejecting it".to_string());
            }
        }
        MeetingHostAction::DismissHandoff { handoff_id, .. } => {
            let handoff = host
                .open_handoffs
                .iter()
                .find(|handoff| handoff.handoff_id == *handoff_id)
                .ok_or_else(|| "the selected Handoff is no longer open".to_string())?;
            if handoff.attempt_active {
                return Err("an active Handoff Offer or Grant cannot be dismissed".to_string());
            }
        }
        MeetingHostAction::Recall { .. } if !host.can_recall => {
            return Err("Meeting control cannot be recalled in the current State".to_string());
        }
        MeetingHostAction::Close if !host.can_close => {
            return Err(
                "normal close requires explicit final Board confirmation and host control"
                    .to_string(),
            );
        }
        MeetingHostAction::Abort { .. }
        | MeetingHostAction::Recall { .. }
        | MeetingHostAction::Close => {}
    }
    Ok(())
}

fn require_floor_decision(host: &MeetingHostState) -> Result<(), String> {
    if !host.can_select {
        return Err("the authoritative Floor Decision window is not active".to_string());
    }
    Ok(())
}

fn validate_addressee(
    value: Option<&str>,
    snapshot: &MeetingSnapshot,
    signer_pubkey: &str,
) -> Result<(), String> {
    if value == Some(signer_pubkey) {
        return Err("a self Intent cannot be addressed to the host".to_string());
    }
    if value.is_some_and(|pubkey| {
        !snapshot
            .participants
            .iter()
            .any(|participant| participant.pubkey == pubkey)
    }) {
        return Err("Meeting Intent addressee is outside the frozen roster".to_string());
    }
    Ok(())
}

fn pending_intent<'a>(
    host: &'a MeetingHostState,
    intent_id: &str,
) -> Result<&'a MeetingPendingIntent, String> {
    host.pending_intents
        .iter()
        .find(|intent| intent.intent_id == intent_id)
        .ok_or_else(|| "the selected Intent is no longer pending".to_string())
}

fn own_intent<'a>(
    host: &'a MeetingHostState,
    intent_id: &str,
    signer_pubkey: &str,
) -> Result<&'a MeetingPendingIntent, String> {
    let intent = pending_intent(host, intent_id)?;
    if intent.author_pubkey != signer_pubkey {
        return Err("the selected Intent does not belong to the host".to_string());
    }
    Ok(intent)
}

fn build_event(
    action: &MeetingHostAction,
    snapshot: &MeetingSnapshot,
    host: &MeetingHostState,
    session_id: Uuid,
    signer_pubkey: &str,
) -> Result<nostr::EventBuilder, String> {
    let result = match action {
        MeetingHostAction::BoardUpdate { body } => build_board_action(
            snapshot,
            MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: host.board_control.control_epoch,
                board_window: host.board_control.board_window,
                board: Some(body),
            },
        ),
        MeetingHostAction::BoardUnchanged => build_board_action(
            snapshot,
            MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: host.board_control.control_epoch,
                board_window: host.board_control.board_window,
                board: None,
            },
        ),
        MeetingHostAction::IntentSubmit {
            summary,
            addressed_to,
        } => buzz_sdk_pkg::build_meeting_v2_intent_submit(MeetingV1IntentSubmitParams {
            session_id,
            basis_speech_revision: snapshot.speech_revision,
            addressed_to: addressed_to.as_deref(),
            summary,
        }),
        MeetingHostAction::IntentRefresh {
            intent_id,
            summary,
            addressed_to,
        } => {
            let intent = own_intent(host, intent_id, signer_pubkey)?;
            buzz_sdk_pkg::build_meeting_v2_intent_refresh(MeetingV1IntentRefreshParams {
                session_id,
                intent_id,
                previous_event_id: &intent.current_event_id,
                basis_speech_revision: snapshot.speech_revision,
                addressed_to: addressed_to.as_deref(),
                summary,
            })
        }
        MeetingHostAction::IntentWithdraw { intent_id } => {
            let intent = own_intent(host, intent_id, signer_pubkey)?;
            buzz_sdk_pkg::build_meeting_v2_intent_withdraw(MeetingV1IntentWithdrawParams {
                session_id,
                intent_id,
                previous_event_id: &intent.current_event_id,
            })
        }
        MeetingHostAction::SelectIntent {
            intent_id,
            selection_reason,
            deferral_reason,
        } => {
            let selected = pending_intent(host, intent_id)?;
            let deferral_storage = if selected.author_pubkey == signer_pubkey
                && host.consecutive_moderator_speeches >= 1
            {
                deferral_reason.as_ref().map_or_else(Vec::new, |reason| {
                    host.pending_intents
                        .iter()
                        .filter(|intent| {
                            intent.author_pubkey != signer_pubkey
                                && intent.selectable
                                && !intent.deferred
                        })
                        .map(|intent| {
                            (
                                intent.intent_id.as_str(),
                                intent.current_event_id.as_str(),
                                reason.as_str(),
                            )
                        })
                        .collect()
                })
            } else {
                Vec::new()
            };
            let deferrals = deferral_storage
                .iter()
                .map(
                    |(intent_id, previous_event_id, reason)| MeetingV1IntentDeferral {
                        intent_id,
                        previous_event_id,
                        reason,
                    },
                )
                .collect::<Vec<_>>();
            buzz_sdk_pkg::build_meeting_v2_moderator_select(MeetingV1ModeratorSelectParams {
                session_id,
                selection: MeetingV1Selection::Intent { intent_id },
                expected_control_epoch: host.control_epoch,
                expected_decision_epoch: host.decision_epoch,
                expected_intent_revision: snapshot.intent_revision,
                expected_speech_revision: snapshot.speech_revision,
                selection_reason: selection_reason.as_deref(),
                deferrals: &deferrals,
                attempt_id: None,
                expected_source_event_id: None,
            })
        }
        MeetingHostAction::SelectHandoff {
            handoff_id,
            selection_reason,
        } => {
            let handoff = host
                .open_handoffs
                .iter()
                .find(|handoff| handoff.handoff_id == *handoff_id)
                .ok_or_else(|| "the selected Handoff disappeared".to_string())?;
            buzz_sdk_pkg::build_meeting_v2_moderator_select(MeetingV1ModeratorSelectParams {
                session_id,
                selection: MeetingV1Selection::Handoff {
                    handoff_id,
                    expected_attempt_count: u64::from(handoff.attempt_count),
                },
                expected_control_epoch: host.control_epoch,
                expected_decision_epoch: host.decision_epoch,
                expected_intent_revision: snapshot.intent_revision,
                expected_speech_revision: snapshot.speech_revision,
                selection_reason: selection_reason.as_deref(),
                deferrals: &[],
                attempt_id: None,
                expected_source_event_id: None,
            })
        }
        MeetingHostAction::RejectIntent {
            intent_id,
            reason_code,
            reason,
        } => {
            let intent = pending_intent(host, intent_id)?;
            buzz_sdk_pkg::build_meeting_v2_moderator_reject(MeetingV1ModeratorRejectParams {
                session_id,
                intent_id,
                previous_event_id: &intent.current_event_id,
                intent_author_pubkey: &intent.author_pubkey,
                reason_code: (*reason_code).into(),
                reason_text: reason,
                attempt_id: None,
            })
        }
        MeetingHostAction::DismissHandoff {
            handoff_id,
            reason_code,
            reason,
        } => {
            let handoff = host
                .open_handoffs
                .iter()
                .find(|handoff| handoff.handoff_id == *handoff_id)
                .ok_or_else(|| "the selected Handoff disappeared".to_string())?;
            buzz_sdk_pkg::build_meeting_v2_moderator_dismiss_handoff(
                MeetingV1ModeratorDismissHandoffParams {
                    session_id,
                    handoff_id,
                    expected_speech_revision: snapshot.speech_revision,
                    expected_attempt_count: u64::from(handoff.attempt_count),
                    reason_code: (*reason_code).into(),
                    reason_text: reason,
                    attempt_id: None,
                },
            )
        }
        MeetingHostAction::Recall { reason } => {
            buzz_sdk_pkg::build_meeting_v2_moderator_recall(MeetingV1ModeratorRecallParams {
                session_id,
                control_epoch: host.control_epoch,
                reason: reason.as_deref(),
            })
        }
        MeetingHostAction::Close => build_end(
            snapshot,
            session_id,
            MeetingV2EndOutcome::Closed,
            None,
            None,
        ),
        MeetingHostAction::Abort {
            reason_code,
            reason,
        } => build_end(
            snapshot,
            session_id,
            MeetingV2EndOutcome::Aborted,
            Some(reason_code.as_str()),
            reason.as_deref(),
        ),
    };
    result.map_err(|error| format!("invalid Meeting host command: {error}"))
}

fn validate_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
    kind: ReceiptKind,
) -> Result<ValidatedReceipt, String> {
    if response.event_id != pending.event.id.to_hex() {
        return Err("event ID does not match the signed host command".to_string());
    }
    match kind {
        ReceiptKind::Control => {
            let receipt: ControlReceipt = parse_command_response(&response.message)?;
            if receipt.meeting_id != pending.meeting_id
                || receipt.outcome.trim().is_empty()
                || receipt.recovery_transitions > 64
                || receipt.state_revision.is_some_and(|revision| revision <= 0)
            {
                return Err("control receipt fields do not match the signed command".to_string());
            }
            if let Some(object_id) = &receipt.canonical_object_id {
                canonical_hex64(object_id, "canonical Meeting host object")?;
            }
            Ok(ValidatedReceipt {
                canonical_object_id: receipt.canonical_object_id,
                state_revision: receipt.state_revision,
                duplicate: receipt.duplicate,
            })
        }
        ReceiptKind::Board => {
            let receipt: BoardReceipt = parse_command_response(&response.message)?;
            if receipt.meeting_id != pending.meeting_id
                || receipt.outcome.trim().is_empty()
                || receipt.recovery_transitions > 1
                || receipt.state_revision.is_some_and(|revision| revision <= 0)
            {
                return Err("Board receipt fields do not match the signed command".to_string());
            }
            if let Some(board_event_id) = &receipt.board_event_id {
                canonical_hex64(board_event_id, "Meeting Board projection")?;
            }
            Ok(ValidatedReceipt {
                canonical_object_id: receipt.board_event_id,
                state_revision: receipt.state_revision,
                duplicate: receipt.duplicate,
            })
        }
        ReceiptKind::End => {
            let receipt: EndReceipt = parse_command_response(&response.message)?;
            if receipt.meeting_id != pending.meeting_id
                || receipt.status != "ended"
                || !matches!(
                    receipt.terminal_outcome.as_deref(),
                    Some("closed" | "aborted")
                )
            {
                return Err("End receipt fields do not match the signed command".to_string());
            }
            Ok(ValidatedReceipt {
                canonical_object_id: None,
                state_revision: None,
                duplicate: receipt.already_ended,
            })
        }
    }
}

fn indeterminate_result(
    pending: &PendingMeetingCommand,
    message: String,
) -> MeetingHostActionResult {
    MeetingHostActionResult::Indeterminate {
        meeting_id: pending.meeting_id.clone(),
        event_id: pending.event.id.to_hex(),
        action: pending.action.clone(),
        message,
    }
}

#[cfg(test)]
#[path = "host/tests.rs"]
mod tests;
