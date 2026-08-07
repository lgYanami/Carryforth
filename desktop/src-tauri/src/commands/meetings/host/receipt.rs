//! Canonical Relay receipt validation for Human host commands.

use serde::Deserialize;

use crate::{
    pending_writes::PendingMeetingCommand,
    relay::{parse_command_response, SubmitEventResponse},
};

use super::super::pending::canonical_hex64;
use super::MeetingHostAction;

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

pub(super) struct ValidatedReceipt {
    pub(super) canonical_object_id: Option<String>,
    pub(super) state_revision: Option<i64>,
    pub(super) duplicate: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReceiptValidationError {
    Unverifiable(String),
    CanonicalConflict(String),
}

impl ReceiptValidationError {
    pub(super) fn message(self) -> String {
        match self {
            Self::Unverifiable(message) | Self::CanonicalConflict(message) => message,
        }
    }
}

pub(super) fn validate_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
    action: &MeetingHostAction,
) -> Result<ValidatedReceipt, ReceiptValidationError> {
    if response.event_id != pending.event.id.to_hex() {
        return Err(ReceiptValidationError::Unverifiable(
            "event ID does not match the signed host command".to_string(),
        ));
    }
    match receipt_kind(action) {
        ReceiptKind::Control => validate_control_receipt(response, pending),
        ReceiptKind::Board => validate_board_receipt(response, pending),
        ReceiptKind::End => validate_end_receipt(response, pending, action),
    }
}

fn receipt_kind(action: &MeetingHostAction) -> ReceiptKind {
    match action {
        MeetingHostAction::BoardUpdate { .. } | MeetingHostAction::BoardUnchanged => {
            ReceiptKind::Board
        }
        MeetingHostAction::Close | MeetingHostAction::Abort { .. } => ReceiptKind::End,
        _ => ReceiptKind::Control,
    }
}

fn validate_control_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
) -> Result<ValidatedReceipt, ReceiptValidationError> {
    let receipt: ControlReceipt =
        parse_command_response(&response.message).map_err(ReceiptValidationError::Unverifiable)?;
    if receipt.meeting_id != pending.meeting_id
        || receipt.outcome.trim().is_empty()
        || receipt.recovery_transitions > 64
        || receipt.state_revision.is_some_and(|revision| revision <= 0)
    {
        return Err(ReceiptValidationError::Unverifiable(
            "control receipt fields do not match the signed command".to_string(),
        ));
    }
    if let Some(object_id) = &receipt.canonical_object_id {
        canonical_hex64(object_id, "canonical Meeting host object")
            .map_err(ReceiptValidationError::Unverifiable)?;
    }
    Ok(ValidatedReceipt {
        canonical_object_id: receipt.canonical_object_id,
        state_revision: receipt.state_revision,
        duplicate: receipt.duplicate,
    })
}

fn validate_board_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
) -> Result<ValidatedReceipt, ReceiptValidationError> {
    let receipt: BoardReceipt =
        parse_command_response(&response.message).map_err(ReceiptValidationError::Unverifiable)?;
    if receipt.meeting_id != pending.meeting_id
        || receipt.outcome.trim().is_empty()
        || receipt.recovery_transitions > 1
        || receipt.state_revision.is_some_and(|revision| revision <= 0)
    {
        return Err(ReceiptValidationError::Unverifiable(
            "Board receipt fields do not match the signed command".to_string(),
        ));
    }
    if let Some(board_event_id) = &receipt.board_event_id {
        canonical_hex64(board_event_id, "Meeting Board projection")
            .map_err(ReceiptValidationError::Unverifiable)?;
    }
    Ok(ValidatedReceipt {
        canonical_object_id: receipt.board_event_id,
        state_revision: receipt.state_revision,
        duplicate: receipt.duplicate,
    })
}

fn validate_end_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
    action: &MeetingHostAction,
) -> Result<ValidatedReceipt, ReceiptValidationError> {
    let receipt: EndReceipt =
        parse_command_response(&response.message).map_err(ReceiptValidationError::Unverifiable)?;
    if receipt.meeting_id != pending.meeting_id || receipt.status != "ended" {
        return Err(ReceiptValidationError::Unverifiable(
            "End receipt fields do not match the signed command".to_string(),
        ));
    }
    let expected_outcome = match action {
        MeetingHostAction::Close => "closed",
        MeetingHostAction::Abort { .. } => "aborted",
        _ => {
            return Err(ReceiptValidationError::Unverifiable(
                "non-terminal host command received an End receipt".to_string(),
            ));
        }
    };
    let Some(actual_outcome) = receipt.terminal_outcome.as_deref() else {
        return Err(ReceiptValidationError::Unverifiable(
            "End receipt omitted the canonical terminal outcome".to_string(),
        ));
    };
    if !matches!(actual_outcome, "closed" | "aborted") {
        return Err(ReceiptValidationError::Unverifiable(
            "End receipt has an unknown terminal outcome".to_string(),
        ));
    }
    if actual_outcome != expected_outcome {
        return Err(ReceiptValidationError::CanonicalConflict(format!(
            "Meeting already ended as `{actual_outcome}`; the requested `{}` action was not applied",
            action.name()
        )));
    }
    Ok(ValidatedReceipt {
        canonical_object_id: None,
        state_revision: None,
        duplicate: receipt.already_ended,
    })
}
