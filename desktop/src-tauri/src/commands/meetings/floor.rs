//! Human Meeting V2 Floor command boundary.
//!
//! The Desktop submits closed business intents. This module reloads the
//! verified Relay projection, derives canonical Floor object IDs, signs once,
//! and retains the exact event when delivery is indeterminate.

use std::collections::BTreeSet;

use buzz_sdk_pkg::{
    MeetingV1DirectedHandoff, MeetingV1GrantYieldParams, MeetingV1GrantYieldReason,
    MeetingV1HandoffType, MeetingV1HumanFloorRequestParams, MeetingV1HumanFloorWithdrawParams,
    MeetingV1OfferAckParams, MeetingV1OfferDeclineParams, MeetingV1SpeechParams,
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
    load_meeting_snapshot_at, read_meeting_identity_at, MeetingLifecycle, MeetingLoadResult,
    MeetingParticipantType, MeetingSnapshot,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MeetingFloorActionInput {
    /// Stable UUID generated once and reused for an exact retry.
    submission_id: String,
    meeting_id: String,
    /// Opaque State event identity presented by the verified read model.
    expected_state_event_id: String,
    action: MeetingFloorAction,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MeetingFloorAction {
    Request,
    Withdraw,
    OfferAck,
    OfferDecline {
        reason: Option<String>,
    },
    GrantYield {
        reason_code: Option<GrantYieldReasonInput>,
        reason: Option<String>,
    },
    Speech {
        content: String,
        #[serde(default)]
        mentions: Vec<String>,
        handoff: Option<DirectedHandoffInput>,
    },
}

impl MeetingFloorAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Withdraw => "withdraw",
            Self::OfferAck => "offer_ack",
            Self::OfferDecline { .. } => "offer_decline",
            Self::GrantYield { .. } => "grant_yield",
            Self::Speech { .. } => "speech",
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum GrantYieldReasonInput {
    NoLongerNeeded,
    UnableToAnswer,
    InsufficientContext,
    ToolFailure,
    Cancelled,
}

impl From<GrantYieldReasonInput> for MeetingV1GrantYieldReason {
    fn from(value: GrantYieldReasonInput) -> Self {
        match value {
            GrantYieldReasonInput::NoLongerNeeded => Self::NoLongerNeeded,
            GrantYieldReasonInput::UnableToAnswer => Self::UnableToAnswer,
            GrantYieldReasonInput::InsufficientContext => Self::InsufficientContext,
            GrantYieldReasonInput::ToolFailure => Self::ToolFailure,
            GrantYieldReasonInput::Cancelled => Self::Cancelled,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DirectedHandoffInput {
    target_pubkey: String,
    handoff_type: HandoffTypeInput,
    reason: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum HandoffTypeInput {
    Question,
    InformationRequest,
    Clarification,
    Review,
    ResponseRequested,
}

impl From<HandoffTypeInput> for MeetingV1HandoffType {
    fn from(value: HandoffTypeInput) -> Self {
        match value {
            HandoffTypeInput::Question => Self::Question,
            HandoffTypeInput::InformationRequest => Self::InformationRequest,
            HandoffTypeInput::Clarification => Self::Clarification,
            HandoffTypeInput::Review => Self::Review,
            HandoffTypeInput::ResponseRequested => Self::ResponseRequested,
        }
    }
}

/// Result of one Human Meeting Floor command.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MeetingFloorActionResult {
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

#[derive(Debug, Deserialize)]
struct MeetingCommandReceipt {
    meeting_id: String,
    canonical_object_id: Option<String>,
    state_revision: Option<i64>,
    recovery_transitions: usize,
    duplicate: bool,
    outcome: String,
}

struct ValidatedInput {
    submission_id: String,
    meeting_id: String,
    expected_state_event_id: String,
    action: MeetingFloorAction,
    fingerprint: String,
}

/// Submit a Human Floor action against a verified Meeting State.
#[tauri::command]
pub async fn submit_meeting_floor_action(
    input: MeetingFloorActionInput,
    state: State<'_, AppState>,
) -> Result<MeetingFloorActionResult, String> {
    execute_floor_action(input, &state).await
}

async fn execute_floor_action(
    input: MeetingFloorActionInput,
    state: &AppState,
) -> Result<MeetingFloorActionResult, String> {
    // Capture both bindings before the first await. A Community or identity
    // switch cannot retarget an unresolved signed command.
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
            return Err("Meeting Floor is unavailable for this Meeting".to_string());
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

    let receipt = match validate_receipt(&response, &pending) {
        Ok(receipt) => receipt,
        Err(message) => {
            return Ok(indeterminate_result(
                &pending,
                format!(
                    "Relay accepted the Meeting Floor command, but its receipt could not be verified: {message}. Retry to confirm the same signed event."
                ),
            ));
        }
    };
    remove_pending(state, &validated.submission_id, &pending.event);
    Ok(MeetingFloorActionResult::Accepted {
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
        context: "Meeting Floor",
    }
}

fn validate_input(input: MeetingFloorActionInput) -> Result<ValidatedInput, String> {
    let submission_id = canonical_uuid(&input.submission_id, "Meeting Floor submission ID")?;
    let meeting_id = canonical_uuid(&input.meeting_id, "Meeting ID")?;
    canonical_hex64(&input.expected_state_event_id, "Meeting State event ID")?;
    let action = normalize_action(input.action)?;
    let fingerprint = serde_json::to_string(&(
        meeting_id.as_str(),
        input.expected_state_event_id.as_str(),
        &action,
    ))
    .map_err(|error| format!("serialize Meeting Floor fingerprint: {error}"))?;
    Ok(ValidatedInput {
        submission_id,
        meeting_id,
        expected_state_event_id: input.expected_state_event_id,
        action,
        fingerprint,
    })
}

fn normalize_action(action: MeetingFloorAction) -> Result<MeetingFloorAction, String> {
    let optional_reason = |value: Option<String>, limit: usize, context: &str| {
        let value = value
            .map(|candidate| candidate.trim().to_string())
            .filter(|candidate| !candidate.is_empty());
        if value
            .as_ref()
            .is_some_and(|candidate| candidate.len() > limit)
        {
            return Err(format!("{context} cannot exceed {limit} bytes"));
        }
        if value
            .as_ref()
            .is_some_and(|candidate| candidate.contains('\0'))
        {
            return Err(format!("{context} cannot contain NUL"));
        }
        Ok(value)
    };
    match action {
        MeetingFloorAction::OfferDecline { reason } => Ok(MeetingFloorAction::OfferDecline {
            reason: optional_reason(reason, 512, "Offer decline reason")?,
        }),
        MeetingFloorAction::GrantYield {
            reason_code,
            reason,
        } => Ok(MeetingFloorAction::GrantYield {
            reason_code,
            reason: optional_reason(reason, 512, "Grant Yield reason")?,
        }),
        MeetingFloorAction::Speech {
            content,
            mentions,
            handoff,
        } => {
            if content.trim().is_empty() || content.len() > 256 * 1024 || content.contains('\0') {
                return Err(
                    "Meeting Speech must be non-empty, NUL-free, and at most 256 KiB".to_string(),
                );
            }
            let mut unique = BTreeSet::new();
            for pubkey in &mentions {
                canonical_hex64(pubkey, "Meeting mention")?;
                if !unique.insert(pubkey.clone()) {
                    return Err(format!("duplicate Meeting mention: {pubkey}"));
                }
            }
            if mentions.len() > 12 {
                return Err("Meeting Speech cannot mention more than 12 participants".to_string());
            }
            let handoff = handoff
                .map(|mut value| {
                    canonical_hex64(&value.target_pubkey, "Meeting Handoff target")?;
                    value.reason = value.reason.trim().to_string();
                    if value.reason.is_empty()
                        || value.reason.len() > 1024
                        || value.reason.contains('\0')
                    {
                        return Err("Meeting Handoff reason must be non-empty, NUL-free, and at most 1024 bytes".to_string());
                    }
                    Ok(value)
                })
                .transpose()?;
            Ok(MeetingFloorAction::Speech {
                content,
                mentions,
                handoff,
            })
        }
        other => Ok(other),
    }
}

fn prepare_command(
    input: &ValidatedInput,
    snapshot: &MeetingSnapshot,
    api_base_url: &str,
    signer_pubkey: &str,
    keys: &nostr::Keys,
) -> Result<PendingMeetingCommand, String> {
    if !matches!(snapshot.lifecycle, MeetingLifecycle::Active) {
        return Err("Meeting Floor is frozen outside the active discussion".to_string());
    }
    let floor = snapshot
        .floor
        .as_ref()
        .ok_or_else(|| "Meeting Floor is not initialized yet".to_string())?;
    if floor.state_event_id != input.expected_state_event_id {
        return Err(
            "Meeting State changed; refresh before submitting this Floor action".to_string(),
        );
    }
    let participant = snapshot
        .participants
        .iter()
        .find(|participant| participant.pubkey == signer_pubkey)
        .ok_or_else(|| "current identity is outside the frozen Meeting roster".to_string())?;
    if participant.participant_type != MeetingParticipantType::Human {
        return Err("only a frozen Human participant can use Desktop Floor controls".to_string());
    }
    validate_action_authority(input, snapshot, signer_pubkey)?;
    let session_id = Uuid::parse_str(&input.meeting_id)
        .map_err(|error| format!("invalid Meeting ID after validation: {error}"))?;
    let builder = build_event(&input.action, snapshot, session_id, signer_pubkey)?;
    let event = builder
        .sign_with_keys(keys)
        .map_err(|error| format!("failed to sign Meeting Floor command: {error}"))?;
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
    input: &ValidatedInput,
    snapshot: &MeetingSnapshot,
    signer_pubkey: &str,
) -> Result<(), String> {
    let floor = snapshot
        .floor
        .as_ref()
        .ok_or_else(|| "Meeting Floor is not initialized yet".to_string())?;
    let own_request = floor
        .human_queue
        .iter()
        .find(|request| request.requester_pubkey == signer_pubkey);
    let own_offer = floor
        .offer
        .as_ref()
        .filter(|offer| offer.target_pubkey == signer_pubkey);
    let own_grant = floor
        .grant
        .as_ref()
        .filter(|grant| grant.holder_pubkey == signer_pubkey);
    match &input.action {
        MeetingFloorAction::Request => {
            if signer_pubkey == snapshot.moderator_pubkey {
                return Err("the Meeting host cannot use Human Floor Request".to_string());
            }
            if own_request.is_some() || own_offer.is_some() || own_grant.is_some() {
                return Err("the current Human already has an active Floor position".to_string());
            }
        }
        MeetingFloorAction::Withdraw if own_request.is_none() => {
            return Err("the current Human has no active Floor Request to withdraw".to_string());
        }
        MeetingFloorAction::OfferAck | MeetingFloorAction::OfferDecline { .. }
            if own_offer.is_none() =>
        {
            return Err("the active Floor Offer is not addressed to the current Human".to_string());
        }
        MeetingFloorAction::GrantYield { .. } | MeetingFloorAction::Speech { .. }
            if own_grant.is_none() =>
        {
            return Err("the current Human does not hold the active Floor Grant".to_string());
        }
        MeetingFloorAction::Speech {
            mentions, handoff, ..
        } => {
            let roster = snapshot
                .participants
                .iter()
                .map(|participant| participant.pubkey.as_str())
                .collect::<BTreeSet<_>>();
            if mentions
                .iter()
                .any(|pubkey| !roster.contains(pubkey.as_str()))
            {
                return Err("Meeting Speech mention is outside the frozen roster".to_string());
            }
            if handoff.as_ref().is_some_and(|handoff| {
                handoff.target_pubkey == signer_pubkey
                    || !roster.contains(handoff.target_pubkey.as_str())
            }) {
                return Err("Meeting Handoff target must be another frozen participant".to_string());
            }
        }
        _ => {}
    }
    Ok(())
}

fn build_event(
    action: &MeetingFloorAction,
    snapshot: &MeetingSnapshot,
    session_id: Uuid,
    signer_pubkey: &str,
) -> Result<nostr::EventBuilder, String> {
    let floor = snapshot
        .floor
        .as_ref()
        .ok_or_else(|| "Meeting Floor is not initialized yet".to_string())?;
    match action {
        MeetingFloorAction::Request => {
            buzz_sdk_pkg::build_meeting_v2_human_floor_request(MeetingV1HumanFloorRequestParams {
                session_id,
            })
        }
        MeetingFloorAction::Withdraw => {
            let request = floor
                .human_queue
                .iter()
                .find(|request| request.requester_pubkey == signer_pubkey)
                .ok_or_else(|| "active Human Floor Request disappeared".to_string())?;
            buzz_sdk_pkg::build_meeting_v2_human_floor_withdraw(MeetingV1HumanFloorWithdrawParams {
                session_id,
                request_id: &request.request_id,
            })
        }
        MeetingFloorAction::OfferAck => {
            let offer = own_offer(snapshot, signer_pubkey)?;
            buzz_sdk_pkg::build_meeting_v2_offer_ack(MeetingV1OfferAckParams {
                session_id,
                offer_id: &offer.offer_id,
            })
        }
        MeetingFloorAction::OfferDecline { reason } => {
            let offer = own_offer(snapshot, signer_pubkey)?;
            buzz_sdk_pkg::build_meeting_v2_offer_decline(MeetingV1OfferDeclineParams {
                session_id,
                offer_id: &offer.offer_id,
                reason: reason.as_deref(),
            })
        }
        MeetingFloorAction::GrantYield {
            reason_code,
            reason,
        } => {
            let grant = own_grant(snapshot, signer_pubkey)?;
            buzz_sdk_pkg::build_meeting_v2_grant_yield(MeetingV1GrantYieldParams {
                session_id,
                grant_id: &grant.grant_id,
                reason_code: reason_code.map(Into::into),
                reason: reason.as_deref(),
            })
        }
        MeetingFloorAction::Speech {
            content,
            mentions,
            handoff,
        } => {
            let grant = own_grant(snapshot, signer_pubkey)?;
            let speech_revision = snapshot
                .speech_revision
                .checked_add(1)
                .ok_or_else(|| "Meeting Speech revision overflow".to_string())?;
            let mention_refs = mentions.iter().map(String::as_str).collect::<Vec<_>>();
            let directed_handoff = handoff.as_ref().map(|handoff| MeetingV1DirectedHandoff {
                target_pubkey: &handoff.target_pubkey,
                handoff_type: handoff.handoff_type.into(),
                reason: &handoff.reason,
            });
            buzz_sdk_pkg::build_meeting_v2_speech(MeetingV1SpeechParams {
                session_id,
                grant_id: &grant.grant_id,
                speech_revision,
                content,
                mentions: &mention_refs,
                handoff: directed_handoff,
            })
        }
    }
    .map_err(|error| format!("invalid Meeting Floor command: {error}"))
}

fn own_offer<'a>(
    snapshot: &'a MeetingSnapshot,
    signer_pubkey: &str,
) -> Result<&'a super::MeetingOffer, String> {
    snapshot
        .floor
        .as_ref()
        .and_then(|floor| floor.offer.as_ref())
        .filter(|offer| offer.target_pubkey == signer_pubkey)
        .ok_or_else(|| "active Floor Offer disappeared".to_string())
}

fn own_grant<'a>(
    snapshot: &'a MeetingSnapshot,
    signer_pubkey: &str,
) -> Result<&'a super::MeetingGrant, String> {
    snapshot
        .floor
        .as_ref()
        .and_then(|floor| floor.grant.as_ref())
        .filter(|grant| grant.holder_pubkey == signer_pubkey)
        .ok_or_else(|| "active Floor Grant disappeared".to_string())
}

fn validate_receipt(
    response: &SubmitEventResponse,
    pending: &PendingMeetingCommand,
) -> Result<MeetingCommandReceipt, String> {
    if response.event_id != pending.event.id.to_hex() {
        return Err("event ID does not match the signed Floor command".to_string());
    }
    let receipt: MeetingCommandReceipt = parse_command_response(&response.message)?;
    if receipt.meeting_id != pending.meeting_id
        || receipt.outcome.trim().is_empty()
        || receipt.recovery_transitions > 64
        || receipt.state_revision.is_some_and(|revision| revision <= 0)
    {
        return Err("receipt fields do not match the signed Floor command".to_string());
    }
    if let Some(canonical_object_id) = &receipt.canonical_object_id {
        canonical_hex64(canonical_object_id, "canonical Meeting Floor object")?;
    }
    Ok(receipt)
}

fn indeterminate_result(
    pending: &PendingMeetingCommand,
    message: String,
) -> MeetingFloorActionResult {
    MeetingFloorActionResult::Indeterminate {
        meeting_id: pending.meeting_id.clone(),
        event_id: pending.event.id.to_hex(),
        action: pending.action.clone(),
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEETING_ID: &str = "00000000-0000-4000-8000-000000000001";

    fn test_snapshot() -> MeetingSnapshot {
        let host = "11".repeat(32);
        let human = "22".repeat(32);
        let agent = "33".repeat(32);
        MeetingSnapshot {
            meeting_id: MEETING_ID.to_string(),
            title: "Floor test".to_string(),
            description: None,
            source_channel_id: None,
            schema_version: 3,
            policy: buzz_sdk_pkg::MEETING_V2_ACTIONS_POLICY.to_string(),
            host_pubkey: host.clone(),
            moderator_pubkey: host.clone(),
            create_event_id: "44".repeat(32),
            created_at: 1,
            lifecycle: MeetingLifecycle::Active,
            phase: "moderator_control".to_string(),
            state_revision: 1,
            floor_revision: 1,
            intent_revision: 0,
            speech_revision: 4,
            current_speaker_pubkey: None,
            current_offer_pubkey: None,
            floor: Some(super::super::MeetingFloorState {
                state_event_id: "55".repeat(32),
                human_queue: Vec::new(),
                offer: None,
                grant: None,
            }),
            host: None,
            participants: vec![
                super::super::MeetingParticipant {
                    pubkey: host.clone(),
                    participant_type: MeetingParticipantType::Human,
                    channel_role: "owner".to_string(),
                },
                super::super::MeetingParticipant {
                    pubkey: human,
                    participant_type: MeetingParticipantType::Human,
                    channel_role: "member".to_string(),
                },
                super::super::MeetingParticipant {
                    pubkey: agent,
                    participant_type: MeetingParticipantType::Agent,
                    channel_role: "bot".to_string(),
                },
            ],
            board: super::super::MeetingBoard {
                event_id: "66".repeat(32),
                format: buzz_sdk_pkg::MEETING_V2_BOARD_FORMAT.to_string(),
                body: "# Goal\nTest Floor".to_string(),
                moderator_pubkey: host,
                updated_at: 1,
                source: super::super::MeetingBoardSource::Projection,
            },
            action: None,
            end: None,
            latest_speech_at: None,
            authoritative_updated_at: 1,
        }
    }

    fn validated(action: MeetingFloorAction) -> ValidatedInput {
        ValidatedInput {
            submission_id: "00000000-0000-4000-8000-000000000002".to_string(),
            meeting_id: MEETING_ID.to_string(),
            expected_state_event_id: "55".repeat(32),
            action,
            fingerprint: "fingerprint".to_string(),
        }
    }

    #[test]
    fn input_normalization_rejects_duplicate_mentions_and_empty_handoff_reason() {
        let pubkey = "ab".repeat(32);
        let duplicate = normalize_action(MeetingFloorAction::Speech {
            content: "Ready".to_string(),
            mentions: vec![pubkey.clone(), pubkey],
            handoff: None,
        });
        assert!(duplicate.is_err());

        let empty_handoff = normalize_action(MeetingFloorAction::Speech {
            content: "Ready".to_string(),
            mentions: Vec::new(),
            handoff: Some(DirectedHandoffInput {
                target_pubkey: "cd".repeat(32),
                handoff_type: HandoffTypeInput::Question,
                reason: "  ".to_string(),
            }),
        });
        assert!(empty_handoff.is_err());
    }

    #[test]
    fn pending_binding_requires_exact_intent_target_and_signer() {
        let event =
            buzz_sdk_pkg::build_meeting_v2_human_floor_request(MeetingV1HumanFloorRequestParams {
                session_id: Uuid::parse_str("00000000-0000-4000-8000-000000000001")
                    .unwrap_or_else(|error| panic!("test UUID: {error}")),
            })
            .unwrap_or_else(|error| panic!("test request: {error}"))
            .sign_with_keys(&nostr::Keys::generate())
            .unwrap_or_else(|error| panic!("sign test request: {error}"));
        let input = ValidatedInput {
            submission_id: "00000000-0000-4000-8000-000000000002".to_string(),
            meeting_id: "00000000-0000-4000-8000-000000000001".to_string(),
            expected_state_event_id: "aa".repeat(32),
            action: MeetingFloorAction::Request,
            fingerprint: "fingerprint".to_string(),
        };
        let pending = PendingMeetingCommand {
            event,
            api_base_url: "http://relay".to_string(),
            signer_pubkey: "bb".repeat(32),
            meeting_id: input.meeting_id.clone(),
            fingerprint: input.fingerprint.clone(),
            action: "request".to_string(),
        };
        let signer = "bb".repeat(32);
        let binding = pending_binding(&input, "http://relay", &signer);
        assert!(super::super::pending::validate_pending_binding(&pending, &binding).is_ok());
        let other_relay = pending_binding(&input, "http://other", &signer);
        assert!(super::super::pending::validate_pending_binding(&pending, &other_relay).is_err());
        let other_signer = "cc".repeat(32);
        let other_identity = pending_binding(&input, "http://relay", &other_signer);
        assert!(
            super::super::pending::validate_pending_binding(&pending, &other_identity).is_err()
        );
    }

    #[test]
    fn request_authority_is_limited_to_non_host_frozen_humans() {
        let snapshot = test_snapshot();
        let request = validated(MeetingFloorAction::Request);
        assert!(validate_action_authority(&request, &snapshot, &"22".repeat(32)).is_ok());
        assert!(validate_action_authority(&request, &snapshot, &"11".repeat(32)).is_err());

        let keys = nostr::Keys::generate();
        let error = prepare_command(&request, &snapshot, "http://relay", &"33".repeat(32), &keys)
            .err()
            .unwrap_or_else(|| panic!("a frozen Agent must not receive a Desktop Human command"));
        assert!(error.contains("frozen Human"));
    }

    #[test]
    fn speech_uses_the_authoritative_grant_and_next_revision() {
        let mut snapshot = test_snapshot();
        let human = "22".repeat(32);
        let grant_id = "77".repeat(32);
        let floor = snapshot
            .floor
            .as_mut()
            .unwrap_or_else(|| panic!("test Floor"));
        floor.grant = Some(super::super::MeetingGrant {
            grant_id: grant_id.clone(),
            holder_pubkey: human.clone(),
            allocation_source: "moderator_select".to_string(),
            turn_role: "participant".to_string(),
            selection_reason: None,
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            created_at_ms: 1,
            soft_lease_expires_at_ms: 2,
            hard_deadline_ms: 3,
            progress_seq: 0,
            progress_interval_ms: Some(1),
        });
        let action = MeetingFloorAction::Speech {
            content: "A canonical Speech".to_string(),
            mentions: vec!["33".repeat(32)],
            handoff: None,
        };
        let input = validated(action);
        validate_action_authority(&input, &snapshot, &human)
            .unwrap_or_else(|error| panic!("test Speech authority: {error}"));
        let event = build_event(
            &input.action,
            &snapshot,
            Uuid::parse_str(MEETING_ID).unwrap_or_else(|error| panic!("test UUID: {error}")),
            &human,
        )
        .unwrap_or_else(|error| panic!("build test Speech: {error}"))
        .sign_with_keys(&nostr::Keys::generate())
        .unwrap_or_else(|error| panic!("sign test Speech: {error}"));

        assert_eq!(
            super::super::single_tag(&event, "meeting-grant"),
            Some(grant_id.as_str())
        );
        assert_eq!(
            super::super::single_tag(&event, "speech-revision"),
            Some("5")
        );

        snapshot.speech_revision = u64::MAX;
        assert!(build_event(
            &input.action,
            &snapshot,
            Uuid::parse_str(MEETING_ID).unwrap_or_else(|error| panic!("test UUID: {error}")),
            &human,
        )
        .is_err());
    }
}
