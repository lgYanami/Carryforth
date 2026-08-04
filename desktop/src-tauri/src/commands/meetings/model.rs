//! Semantic Desktop Meeting models and strict Relay projection wire shapes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingParticipant {
    pub(super) pubkey: String,
    pub(super) participant_type: MeetingParticipantType,
    pub(super) channel_role: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingHumanFloorRequest {
    pub(super) request_id: String,
    pub(super) requester_pubkey: String,
    pub(super) queue_position: u64,
    pub(super) state: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingHandoffContext {
    pub(super) from_pubkey: String,
    pub(super) reason_type: String,
    pub(super) reason_text: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingOffer {
    pub(super) offer_id: String,
    pub(super) target_pubkey: String,
    pub(super) target_participant_type: MeetingParticipantType,
    pub(super) allocation_source: String,
    pub(super) turn_role: String,
    pub(super) selection_reason: Option<String>,
    pub(super) handoff_context: Option<MeetingHandoffContext>,
    pub(super) created_at_ms: i64,
    pub(super) ack_deadline_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingGrant {
    pub(super) grant_id: String,
    pub(super) holder_pubkey: String,
    pub(super) allocation_source: String,
    pub(super) turn_role: String,
    pub(super) selection_reason: Option<String>,
    pub(super) handoff_context: Option<MeetingHandoffContext>,
    pub(super) created_at_ms: i64,
    pub(super) soft_lease_expires_at_ms: i64,
    pub(super) hard_deadline_ms: i64,
    pub(super) progress_seq: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingFloorState {
    /// Opaque Relay-authored State identity used to fence Human actions.
    pub(super) state_event_id: String,
    pub(super) human_queue: Vec<MeetingHumanFloorRequest>,
    pub(super) offer: Option<MeetingOffer>,
    pub(super) grant: Option<MeetingGrant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum MeetingParticipantType {
    Human,
    Agent,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum FrozenParticipantType {
    Human,
    Agent,
}

impl From<FrozenParticipantType> for MeetingParticipantType {
    fn from(value: FrozenParticipantType) -> Self {
        match value {
            FrozenParticipantType::Human => Self::Human,
            FrozenParticipantType::Agent => Self::Agent,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(super) struct StateParticipant {
    pub(super) pubkey: String,
    pub(super) participant_type: FrozenParticipantType,
    pub(super) channel_role: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct HumanFloorRequestWire {
    pub(super) request_id: String,
    pub(super) requester_pubkey: String,
    pub(super) queue_position: i64,
    pub(super) state: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct HandoffContextWire {
    pub(super) from_pubkey: String,
    pub(super) reason_type: String,
    pub(super) reason_text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct OfferWire {
    pub(super) offer_id: String,
    pub(super) target_pubkey: String,
    pub(super) target_participant_type: FrozenParticipantType,
    pub(super) allocation_source: String,
    pub(super) turn_role: String,
    #[serde(default)]
    pub(super) selection_reason: Option<String>,
    #[serde(default)]
    pub(super) source_intent_id: Option<String>,
    #[serde(default)]
    pub(super) source_request_id: Option<String>,
    #[serde(default)]
    pub(super) source_handoff_id: Option<String>,
    #[serde(default)]
    pub(super) source_speech_event_id: Option<String>,
    #[serde(default)]
    pub(super) handoff_context: Option<HandoffContextWire>,
    pub(super) basis_speech_revision: u64,
    pub(super) created_at_ms: i64,
    pub(super) ack_deadline_ms: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct GrantWire {
    pub(super) grant_id: String,
    pub(super) holder_pubkey: String,
    pub(super) allocation_source: String,
    pub(super) turn_role: String,
    pub(super) source_offer_id: String,
    #[serde(default)]
    pub(super) selection_reason: Option<String>,
    #[serde(default)]
    pub(super) source_intent_id: Option<String>,
    #[serde(default)]
    pub(super) source_request_id: Option<String>,
    #[serde(default)]
    pub(super) source_handoff_id: Option<String>,
    #[serde(default)]
    pub(super) source_speech_event_id: Option<String>,
    #[serde(default)]
    pub(super) handoff_context: Option<HandoffContextWire>,
    pub(super) basis_speech_revision: u64,
    pub(super) created_at_ms: i64,
    pub(super) soft_lease_expires_at_ms: i64,
    pub(super) hard_deadline_ms: i64,
    pub(super) progress_seq: i64,
}

#[derive(Debug, Deserialize)]
pub(super) struct ActionWire {
    pub(super) action_run_id: Uuid,
    pub(super) board_event_id: String,
    pub(super) action_window_epoch: u64,
    pub(super) condition: String,
    #[serde(default)]
    pub(super) terminal_status: Option<String>,
    #[serde(default)]
    pub(super) completion_event_id: Option<String>,
    #[serde(default)]
    pub(super) action_deadline_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) last_error_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct BoardControlWire {
    pub(super) phase: String,
    #[serde(default)]
    pub(super) action: Option<ActionWire>,
}

#[derive(Debug, Deserialize)]
pub(super) struct StateWire {
    pub(super) phase: String,
    pub(super) state_revision: u64,
    pub(super) floor_revision: u64,
    pub(super) intent_revision: u64,
    pub(super) speech_revision: u64,
    pub(super) moderator_pubkey: String,
    pub(super) participants: Vec<StateParticipant>,
    #[serde(default)]
    pub(super) human_queue: Vec<HumanFloorRequestWire>,
    #[serde(default)]
    pub(super) offer: Option<OfferWire>,
    #[serde(default)]
    pub(super) grant: Option<GrantWire>,
    #[serde(default)]
    pub(super) board_control: Option<BoardControlWire>,
}

#[derive(Debug)]
pub(super) struct CreateProjection {
    pub(super) meeting_id: String,
    pub(super) title: String,
    pub(super) description: Option<String>,
    pub(super) source_channel_id: Option<String>,
    pub(super) policy: String,
    pub(super) host_pubkey: String,
    pub(super) participant_pubkeys: BTreeSet<String>,
    pub(super) event_id: String,
    pub(super) created_at: u64,
    pub(super) initial_board: buzz_sdk_pkg::MeetingV2BoardContent,
}

#[derive(Debug)]
pub(super) struct StateProjection {
    pub(super) event_id: String,
    pub(super) state: StateWire,
}
