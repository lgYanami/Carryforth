//! Semantic Desktop Meeting models and strict Relay projection wire shapes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBoard {
    pub(super) event_id: String,
    pub(super) format: String,
    pub(super) body: String,
    pub(super) moderator_pubkey: String,
    pub(super) updated_at: u64,
    pub(super) source: MeetingBoardSource,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MeetingBoardSource {
    Projection,
    Create,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingActionState {
    pub(super) action_run_id: String,
    pub(super) board_event_id: String,
    pub(super) action_window_epoch: u64,
    pub(super) condition: String,
    pub(super) terminal_status: Option<String>,
    pub(super) completion_event_id: Option<String>,
    pub(super) action_deadline_at_ms: Option<i64>,
    pub(super) last_error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingEndState {
    pub(super) event_id: String,
    pub(super) outcome: String,
    pub(super) reason_code: Option<String>,
    pub(super) reason: Option<String>,
    pub(super) ended_by: String,
    pub(super) ended_at: u64,
    pub(super) actions_attested: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingPendingIntent {
    pub(super) intent_id: String,
    pub(super) current_event_id: String,
    pub(super) author_pubkey: String,
    pub(super) basis_speech_revision: u64,
    pub(super) summary: String,
    pub(super) addressed_to: Option<String>,
    pub(super) created_at_ms: i64,
    pub(super) deferred: bool,
    pub(super) selection_attempt_count: u32,
    pub(super) last_offer_id: Option<String>,
    pub(super) last_attempt_outcome: Option<String>,
    pub(super) eligible_decision_epoch: u64,
    pub(super) selectable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingOpenHandoff {
    pub(super) handoff_id: String,
    pub(super) source_speech_event_id: String,
    pub(super) from_pubkey: String,
    pub(super) to_pubkey: String,
    pub(super) reason_type: String,
    pub(super) reason_text: String,
    pub(super) created_at_ms: i64,
    pub(super) attempt_count: u32,
    pub(super) last_offer_id: Option<String>,
    pub(super) last_grant_id: Option<String>,
    pub(super) last_attempt_outcome: Option<String>,
    pub(super) blocked_by: Option<String>,
    pub(super) moderator_retry_blocked: bool,
    pub(super) eligible_decision_epoch: u64,
    pub(super) attempt_active: bool,
    pub(super) selectable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingBoardControl {
    pub(super) phase: String,
    pub(super) control_epoch: u64,
    pub(super) board_window: u64,
    pub(super) board_started_at_ms: Option<i64>,
    pub(super) board_deadline_at_ms: Option<i64>,
    pub(super) board_completed_at_ms: Option<i64>,
    pub(super) board_outcome: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingHostState {
    /// Opaque Desktop-issued token binding a command to its authoritative window.
    pub(super) control_token: String,
    pub(super) state_event_id: String,
    pub(super) control_epoch: u64,
    pub(super) decision_epoch: u64,
    pub(super) decision_deadline_ms: Option<i64>,
    pub(super) next_action_at_ms: Option<i64>,
    pub(super) consecutive_moderator_speeches: u32,
    pub(super) forced_return_to_moderator: bool,
    pub(super) pending_intents: Vec<MeetingPendingIntent>,
    pub(super) open_handoffs: Vec<MeetingOpenHandoff>,
    pub(super) board_control: MeetingBoardControl,
    pub(super) can_select: bool,
    pub(super) can_close: bool,
    pub(super) can_recall: bool,
}

/// Complete verified Meeting view consumed by React.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSnapshot {
    pub(super) meeting_id: String,
    pub(super) title: String,
    pub(super) description: Option<String>,
    pub(super) source_channel_id: Option<String>,
    pub(super) schema_version: u16,
    pub(super) policy: String,
    pub(super) host_pubkey: String,
    pub(super) moderator_pubkey: String,
    pub(super) create_event_id: String,
    pub(super) created_at: u64,
    pub(super) lifecycle: MeetingLifecycle,
    pub(super) phase: String,
    pub(super) state_revision: u64,
    pub(super) floor_revision: u64,
    pub(super) intent_revision: u64,
    pub(super) speech_revision: u64,
    pub(super) current_speaker_pubkey: Option<String>,
    pub(super) current_offer_pubkey: Option<String>,
    pub(super) floor: Option<MeetingFloorState>,
    pub(super) host: Option<MeetingHostState>,
    pub(super) participants: Vec<MeetingParticipant>,
    pub(super) board: MeetingBoard,
    pub(super) action: Option<MeetingActionState>,
    pub(super) end: Option<MeetingEndState>,
    pub(super) latest_speech_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingLifecycle {
    Initializing,
    Active,
    FinalizingActions,
    Closed,
    Aborted,
}

/// Safe load states for Meeting routes. Unsupported protocols stay isolated
/// from the ordinary Channel surface.
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MeetingLoadResult {
    UnsupportedRelay,
    Forbidden,
    NotFound,
    UnsupportedProtocol {
        meeting_id: String,
        schema_version: Option<String>,
        policy: Option<String>,
    },
    Ready {
        snapshot: Box<MeetingSnapshot>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingListItem {
    pub(super) meeting_id: String,
    pub(super) title: String,
    pub(super) lifecycle: Option<MeetingLifecycle>,
    pub(super) phase: Option<String>,
    pub(super) current_speaker_pubkey: Option<String>,
    pub(super) current_offer_pubkey: Option<String>,
    pub(super) human_floor_attention_pubkey: Option<String>,
    pub(super) moderator_pubkey: Option<String>,
    pub(super) policy: Option<String>,
    pub(super) updated_at: Option<u64>,
    pub(super) ended_at: Option<u64>,
    pub(super) latest_speech_at: Option<u64>,
    pub(super) compatibility: MeetingListCompatibility,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum MeetingListCompatibility {
    Ready,
    UnsupportedRelay,
    UnsupportedProtocol,
    Forbidden,
    NotFound,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeech {
    pub(super) event_id: String,
    pub(super) author_pubkey: String,
    pub(super) content: String,
    pub(super) created_at: u64,
    pub(super) speech_revision: u64,
    pub(super) grant_event_id: String,
    pub(super) mentions: Vec<String>,
    pub(super) author_participant_type: MeetingParticipantType,
    pub(super) author_is_moderator: bool,
    pub(super) handoff: Option<MeetingSpeechHandoff>,
}

/// A verified Directed Handoff carried atomically by canonical Speech.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeechHandoff {
    pub(super) target_pubkey: String,
    pub(super) handoff_type: MeetingSpeechHandoffType,
    pub(super) reason: String,
}

/// Product-level Directed Handoff type accepted by Meeting V2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingSpeechHandoffType {
    /// Ask the target a question.
    Question,
    /// Ask the target to provide information.
    InformationRequest,
    /// Ask the target to clarify a point.
    Clarification,
    /// Ask the target to review something.
    Review,
    /// Explicitly request a response from the target.
    ResponseRequested,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeechCursor {
    pub(super) before: u64,
    pub(super) before_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingSpeechPage {
    pub(super) speeches: Vec<MeetingSpeech>,
    pub(super) next_cursor: Option<MeetingSpeechCursor>,
}

/// Product-level classification for one verified Meeting control transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetingActivityKind {
    /// The host published a changed Board.
    BoardUpdated,
    /// The host completed maintenance without changing the Board.
    BoardUnchanged,
    /// Board maintenance reached its deadline.
    BoardTimedOut,
    /// A higher-priority floor action interrupted Board maintenance.
    BoardPreempted,
    /// The host offered the floor.
    FloorOffered,
    /// An offer was acknowledged and became an active Grant.
    FloorGranted,
    /// The target declined an Offer.
    OfferDeclined,
    /// An Offer expired before acknowledgement.
    OfferExpired,
    /// The Grant holder yielded the floor.
    FloorYielded,
    /// The host recalled meeting control.
    FloorRecalled,
    /// An active Grant expired.
    FloorExpired,
    /// An accepted Speech established a Directed Handoff.
    HandoffOpened,
    /// The host attempted a Directed Handoff through a floor Offer.
    HandoffAttempted,
    /// A Directed Handoff reached a stable answered or dismissed state.
    HandoffResolved,
    /// The Meeting entered action finalization.
    ActionFinalizationStarted,
    /// Action recording became blocked.
    ActionBlocked,
    /// The host retried action recording.
    ActionRetried,
    /// Action finalization returned to Board maintenance.
    ActionReturnedToBoard,
    /// Action recording reached its deadline.
    ActionDeadlineExceeded,
    /// The Meeting closed successfully.
    MeetingClosed,
    /// The Meeting ended without a successful conclusion.
    MeetingAborted,
}

/// A bounded, sanitized Meeting activity item safe for ordinary Desktop UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingActivity {
    pub(super) activity_id: String,
    pub(super) kind: MeetingActivityKind,
    pub(super) occurred_at_ms: i64,
    pub(super) actor_pubkey: Option<String>,
    pub(super) target_pubkey: Option<String>,
    pub(super) summary: String,
}

/// One bounded page of verified Meeting activity.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingActivityPage {
    pub(super) activities: Vec<MeetingActivity>,
    pub(super) next_cursor: Option<String>,
}

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
    pub(super) source_intent_id: Option<String>,
    pub(super) source_request_id: Option<String>,
    pub(super) source_handoff_id: Option<String>,
    pub(super) source_speech_event_id: Option<String>,
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
    pub(super) source_intent_id: Option<String>,
    pub(super) source_request_id: Option<String>,
    pub(super) source_handoff_id: Option<String>,
    pub(super) source_speech_event_id: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
pub(super) struct PendingIntentWire {
    pub(super) intent_id: String,
    pub(super) current_event_id: String,
    pub(super) author_pubkey: String,
    pub(super) basis_speech_revision: u64,
    pub(super) summary: String,
    #[serde(default)]
    pub(super) addressed_to: Option<String>,
    pub(super) created_at_ms: i64,
    pub(super) deferred: bool,
    pub(super) selection_attempt_count: i64,
    #[serde(default)]
    pub(super) last_offer_id: Option<String>,
    #[serde(default)]
    pub(super) last_attempt_outcome: Option<String>,
    pub(super) eligible_decision_epoch: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct OpenHandoffWire {
    pub(super) handoff_id: String,
    pub(super) source_speech_event_id: String,
    pub(super) from_pubkey: String,
    pub(super) to_pubkey: String,
    pub(super) reason_type: String,
    pub(super) reason_text: String,
    pub(super) created_at_ms: i64,
    pub(super) question_state: String,
    pub(super) attempt_count: i64,
    #[serde(default)]
    pub(super) last_offer_id: Option<String>,
    #[serde(default)]
    pub(super) last_grant_id: Option<String>,
    #[serde(default)]
    pub(super) last_attempt_outcome: Option<String>,
    #[serde(default)]
    pub(super) blocked_by: Option<String>,
    pub(super) moderator_retry_blocked: bool,
    pub(super) eligible_decision_epoch: u64,
}

#[derive(Debug, Deserialize)]
pub(super) struct TransitionEffectWire {
    #[serde(rename = "type")]
    pub(super) effect_type: String,
    pub(super) object_id: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct TransitionWire {
    pub(super) primary_type: String,
    #[serde(default)]
    pub(super) caused_by_event_id: Option<String>,
    pub(super) at_ms: i64,
    #[serde(default)]
    pub(super) effects: Vec<TransitionEffectWire>,
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
    pub(super) control_epoch: u64,
    pub(super) board_window: u64,
    #[serde(default)]
    pub(super) board_started_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) board_deadline_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) board_completed_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) board_outcome: Option<String>,
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
    pub(super) control_epoch: u64,
    pub(super) decision_epoch: u64,
    #[serde(default)]
    pub(super) moderator_decision_deadline_ms: Option<i64>,
    #[serde(default)]
    pub(super) next_action_at_ms: Option<i64>,
    pub(super) consecutive_moderator_speeches: i64,
    pub(super) forced_return_to_moderator: bool,
    pub(super) moderator_pubkey: String,
    pub(super) participants: Vec<StateParticipant>,
    #[serde(default)]
    pub(super) pending_intents: Vec<PendingIntentWire>,
    #[serde(default)]
    pub(super) human_queue: Vec<HumanFloorRequestWire>,
    #[serde(default)]
    pub(super) unresolved_handoffs: Vec<OpenHandoffWire>,
    #[serde(default)]
    pub(super) offer: Option<OfferWire>,
    #[serde(default)]
    pub(super) grant: Option<GrantWire>,
    #[serde(default)]
    pub(super) board_control: Option<BoardControlWire>,
    #[serde(default)]
    pub(super) transition: Option<TransitionWire>,
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
    pub(super) created_at: u64,
    pub(super) state: StateWire,
}
