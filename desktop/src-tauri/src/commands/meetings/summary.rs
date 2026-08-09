//! Human-hosted Meeting retrieval-summary mutation boundary.
//!
//! Summary persistence is independent from the existing action-finalization
//! Confirm command. Native derives every fence from a verified snapshot and
//! retains the exact signed event while Relay submission or projection
//! readback is indeterminate.

use buzz_sdk_pkg::{MeetingSummaryMutation, MeetingSummaryUpdateParams, MeetingV2ActionRunFence};
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

use crate::{
    app_state::AppState,
    pending_writes::PendingMeetingCommand,
    relay::{relay_api_base_url_with_override, submit_signed_event_at_with_keys},
};

use super::pending::{
    canonical_hex64, canonical_uuid, find_pending, insert_or_reuse_pending,
    is_indeterminate_submit_error, remove_pending, PendingBinding,
};
use super::{
    load_meeting_snapshot_at, read_meeting_identity_at, MeetingLifecycle, MeetingLoadResult,
    MeetingParticipantType,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateMeetingSummaryInput {
    submission_id: String,
    meeting_id: String,
    expected_control_token: String,
    mutation: MeetingSummaryMutationInput,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MeetingSummaryMutationInput {
    Set { summary: String },
    Clear {},
}

impl MeetingSummaryMutationInput {
    fn validate(&self) -> Result<(), String> {
        if let Self::Set { summary } = self {
            if summary.trim().is_empty() || summary.contains('\0') {
                return Err("Meeting summary must contain non-blank text without NUL".to_string());
            }
        }
        Ok(())
    }

    fn intended(&self) -> Option<&str> {
        match self {
            Self::Set { summary } => Some(summary),
            Self::Clear {} => None,
        }
    }

    fn action(&self) -> &'static str {
        match self {
            Self::Set { .. } => "set",
            Self::Clear {} => "clear",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UpdateMeetingSummaryResult {
    Accepted {
        meeting_id: String,
        event_id: String,
        summary: Option<String>,
    },
    Indeterminate {
        meeting_id: String,
        event_id: String,
        message: String,
    },
}

#[tauri::command]
pub async fn update_meeting_summary(
    input: UpdateMeetingSummaryInput,
    state: State<'_, AppState>,
) -> Result<UpdateMeetingSummaryResult, String> {
    execute(input, &state).await
}

async fn execute(
    input: UpdateMeetingSummaryInput,
    state: &AppState,
) -> Result<UpdateMeetingSummaryResult, String> {
    let submission_id = canonical_uuid(&input.submission_id, "Meeting summary submission ID")?;
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    canonical_hex64(
        &input.expected_control_token,
        "Meeting summary control token",
    )?;
    input.mutation.validate()?;
    let fingerprint = serde_json::to_string(&(
        meeting_id.as_str(),
        input.expected_control_token.as_str(),
        &input.mutation,
    ))
    .map_err(|error| format!("serialize Meeting summary fingerprint: {error}"))?;
    let api_base_url = relay_api_base_url_with_override(state);
    let keys = state.signing_keys()?;
    let signer_pubkey = keys.public_key().to_hex();
    let binding = PendingBinding {
        submission_id: &submission_id,
        meeting_id: &meeting_id,
        fingerprint: &fingerprint,
        api_base_url: &api_base_url,
        signer_pubkey: &signer_pubkey,
        context: "Meeting summary",
    };

    let pending = if let Some(pending) = find_pending(state, &binding)? {
        pending
    } else {
        let identity = read_meeting_identity_at(state, &api_base_url)
            .await?
            .ok_or_else(|| "unsupported: Relay does not advertise Meeting V2".to_string())?;
        if !identity.capability.supports_summary {
            return Err("unsupported: Relay does not advertise Meeting summary writes".to_string());
        }
        let loaded = load_meeting_snapshot_at(state, &identity, &meeting_id, &api_base_url, &keys)
            .await
            .map_err(super::read_error_message)?;
        let MeetingLoadResult::Ready { snapshot } = loaded else {
            return Err("Meeting summary is unavailable for this Meeting".to_string());
        };
        if snapshot.policy != buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY
            || snapshot.lifecycle != MeetingLifecycle::FinalizingActions
        {
            return Err("Meeting is not in current Action Finalization".to_string());
        }
        let participant = snapshot
            .participants
            .iter()
            .find(|participant| participant.pubkey == signer_pubkey)
            .ok_or_else(|| "current identity is outside the frozen Meeting roster".to_string())?;
        if participant.participant_type != MeetingParticipantType::Human
            || snapshot.moderator_pubkey != signer_pubkey
        {
            return Err("only the frozen Human moderator can update the Meeting summary".into());
        }
        let host = snapshot
            .host
            .as_ref()
            .ok_or_else(|| "Meeting host projection is unavailable".to_string())?;
        if host.control_token != input.expected_control_token {
            return Err("Meeting summary control changed; refresh before saving".to_string());
        }
        let action = snapshot
            .action
            .as_ref()
            .filter(|action| {
                action.condition == "runnable"
                    && action.terminal_status.is_none()
                    && action.board_event_id == snapshot.board.event_id
            })
            .ok_or_else(|| "Meeting action run is not runnable for the frozen Board".to_string())?;
        let action_run_id = Uuid::parse_str(&action.action_run_id)
            .map_err(|_| "Meeting action run has an invalid ID".to_string())?;
        let session_id = Uuid::parse_str(&meeting_id)
            .map_err(|_| "Meeting has an invalid ID after validation".to_string())?;
        let mutation = match &input.mutation {
            MeetingSummaryMutationInput::Set { summary } => MeetingSummaryMutation::Set(summary),
            MeetingSummaryMutationInput::Clear {} => MeetingSummaryMutation::Clear,
        };
        let event = buzz_sdk_pkg::build_meeting_summary_update(MeetingSummaryUpdateParams {
            session_id,
            mutation,
            action_fence: MeetingV2ActionRunFence {
                action_run_id,
                action_window: action.action_window_epoch,
                board_event_id: &action.board_event_id,
            },
        })
        .map_err(|error| format!("invalid Meeting summary update: {error}"))?
        .sign_with_keys(&keys)
        .map_err(|error| format!("failed to sign Meeting summary update: {error}"))?;
        insert_or_reuse_pending(
            state,
            PendingMeetingCommand {
                event,
                api_base_url: api_base_url.clone(),
                signer_pubkey: signer_pubkey.clone(),
                meeting_id: meeting_id.clone(),
                fingerprint: fingerprint.clone(),
                action: input.mutation.action().to_string(),
            },
            &binding,
        )?
    };

    if let Err(message) =
        submit_signed_event_at_with_keys(&pending.event, state, &pending.api_base_url, &keys).await
    {
        if is_indeterminate_submit_error(&message) {
            return Ok(UpdateMeetingSummaryResult::Indeterminate {
                meeting_id,
                event_id: pending.event.id.to_hex(),
                message,
            });
        }
        remove_pending(state, &submission_id, &pending.event);
        return Err(message);
    }

    let intended = input.mutation.intended().map(str::to_string);
    for attempt in 0..3 {
        let identity = read_meeting_identity_at(state, &api_base_url)
            .await?
            .ok_or_else(|| "unsupported: Relay no longer advertises Meeting V2".to_string())?;
        let loaded = load_meeting_snapshot_at(state, &identity, &meeting_id, &api_base_url, &keys)
            .await
            .map_err(super::read_error_message)?;
        if let MeetingLoadResult::Ready { snapshot } = loaded {
            if snapshot.summary == intended {
                remove_pending(state, &submission_id, &pending.event);
                return Ok(UpdateMeetingSummaryResult::Accepted {
                    meeting_id,
                    event_id: pending.event.id.to_hex(),
                    summary: intended,
                });
            }
        }
        if attempt < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(50_u64 << attempt)).await;
        }
    }

    Ok(UpdateMeetingSummaryResult::Indeterminate {
        meeting_id,
        event_id: pending.event.id.to_hex(),
        message: "Relay accepted the Meeting summary command, but canonical metadata readback is not yet confirmed; retry this exact submission".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_mutation_wire_is_closed_and_validated() {
        let set: MeetingSummaryMutationInput = serde_json::from_value(serde_json::json!({
            "type": "set",
            "summary": "Decision and verified outputs."
        }))
        .expect("parse SET");
        assert_eq!(set.intended(), Some("Decision and verified outputs."));
        assert!(set.validate().is_ok());

        let clear: MeetingSummaryMutationInput =
            serde_json::from_value(serde_json::json!({"type": "clear"})).expect("parse CLEAR");
        assert_eq!(clear.intended(), None);
        assert!(clear.validate().is_ok());

        for value in [
            serde_json::json!({"type": "set", "summary": "  \n "}),
            serde_json::json!({"type": "set", "summary": "bad\u{0000}summary"}),
        ] {
            let mutation: MeetingSummaryMutationInput =
                serde_json::from_value(value).expect("parse invalid SET shape");
            assert!(mutation.validate().is_err());
        }
        assert!(serde_json::from_value::<MeetingSummaryMutationInput>(
            serde_json::json!({"type": "clear", "summary": "unexpected"})
        )
        .is_err());
    }
}
