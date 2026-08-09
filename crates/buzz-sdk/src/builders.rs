//! Typed event builder functions.
//!
//! All functions return `Result<nostr::EventBuilder, SdkError>`.
//! The caller signs: `builder.sign_with_keys(&keys)?`.

use buzz_core::{
    kind::{
        KIND_AGENT_OBSERVER_FRAME, KIND_AGENT_PROFILE, KIND_APPROVAL_DENY, KIND_APPROVAL_GRANT,
        KIND_DELETION, KIND_DM_ADD_MEMBER, KIND_DM_OPEN, KIND_EMOJI_SET, KIND_GIT_ISSUE,
        KIND_GIT_PATCH, KIND_GIT_PR_UPDATE, KIND_GIT_PULL_REQUEST, KIND_GIT_REPO_ANNOUNCEMENT,
        KIND_GIT_STATUS_CLOSED, KIND_GIT_STATUS_DRAFT, KIND_GIT_STATUS_MERGED,
        KIND_GIT_STATUS_OPEN, KIND_IA_ARCHIVE_REQUEST, KIND_IA_UNARCHIVE_REQUEST,
        KIND_MEETING_ACTION_COMMAND, KIND_MEETING_BOARD_COMMAND, KIND_MEETING_CREATE,
        KIND_MEETING_END, KIND_MEETING_FLOOR_CLAIM, KIND_MEETING_FLOOR_SIGNAL,
        KIND_MEETING_GRANT_SIGNAL, KIND_MEETING_HUMAN_FLOOR_REQUEST,
        KIND_MEETING_MODERATOR_COMMAND, KIND_MEETING_OFFER_RESPONSE, KIND_MEETING_SPEECH_INTENT,
        KIND_MEETING_SUMMARY_COMMAND, KIND_MODERATION_BAN, KIND_MODERATION_RESOLVE_REPORT,
        KIND_MODERATION_TIMEOUT, KIND_MODERATION_UNBAN, KIND_MODERATION_UNTIMEOUT,
        KIND_PRESENCE_UPDATE, KIND_STREAM_MESSAGE, KIND_WORKFLOW_DEF, KIND_WORKFLOW_TRIGGER,
    },
    observer::{
        content_looks_like_nip44, OBSERVER_AGENT_TAG, OBSERVER_FRAME_CONTROL, OBSERVER_FRAME_TAG,
        OBSERVER_FRAME_TELEMETRY,
    },
};
use nostr::{EventBuilder, Kind, Tag};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ChannelKind, CustomEmoji, DiffMeta, MemberRole, SdkError, ThreadRef, Visibility, VoteDirection,
};

/// Wire schema version emitted by Meeting V1 builders.
pub const MEETING_V1_SCHEMA_VERSION: &str = "2";
/// Floor policy emitted by Meeting V1 builders.
pub const MEETING_V1_POLICY: &str = "moderated-baton-v1";
/// Wire schema version emitted by Meeting V2 builders.
pub const MEETING_V2_SCHEMA_VERSION: &str = "3";
/// Floor policy emitted by Meeting V2 builders.
pub const MEETING_V2_POLICY: &str = "moderated-board-v1";
/// Persisted policy used by ended, read-only action Meetings created before
/// renewable action leases were introduced.
pub const MEETING_V2_ACTIONS_V2_POLICY: &str = "moderated-board-actions-v2";
/// Action-capable Meeting V2 floor policy with renewable action leases.
pub const MEETING_V2_ACTIONS_POLICY: &str = "moderated-board-actions-v3";
/// Retired runtime capability retained only for historical diagnostics.
pub const MEETING_V2_ACTIONS_V2_CAPABILITY: &str = "meeting-v2-action-finalization-v2";
/// Retired exact-affinity runtime capability retained only for historical diagnostics.
pub const MEETING_V2_ACTIONS_V3_CAPABILITY: &str = "meeting-v2-action-finalization-v3";
/// Runtime capability required for logical-host action-capable Meeting V2 sessions.
pub const MEETING_V2_ACTIONS_CAPABILITY: &str = "meeting-v2-action-finalization-v4";
/// Maximum number of bounded runtime capabilities in one Agent control profile.
pub const MAX_AGENT_PROFILE_CAPABILITIES: usize = 64;
/// Initial/current Meeting V2 board format.
pub const MEETING_V2_BOARD_FORMAT: &str = "markdown";
/// Maximum UTF-8 byte size of a Meeting V2 board body.
pub const MAX_MEETING_V2_BOARD_BYTES: usize = 65_536;

/// Strict content envelope for a Meeting V2 current board projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeetingV2BoardContent {
    /// Versioned presentation format. Stage one accepts only `markdown`.
    pub format: String,
    /// Complete current board document.
    pub body: String,
}

/// Inputs for a strict Meeting V2 Create command.
///
/// `participant_pubkeys` contains every participant except `author_pubkey`.
/// The event author becomes the immutable Meeting owner and moderator.
pub struct MeetingV2CreateParams<'a> {
    /// Stable Meeting UUID; also the private backing Channel UUID.
    pub session_id: Uuid,
    /// Human-readable Meeting title.
    pub title: &'a str,
    /// Optional Meeting description.
    pub description: Option<&'a str>,
    /// Optional source Channel used only as a context/navigation reference.
    pub source_channel_id: Option<Uuid>,
    /// Pubkey that will sign the returned event and become moderator.
    pub author_pubkey: &'a str,
    /// Frozen participants other than the event author.
    pub participant_pubkeys: &'a [&'a str],
    /// Complete initial Markdown board body.
    pub initial_board: &'a str,
}

/// Inputs for a Meeting V2 Board Maintenance command.
pub struct MeetingV2BoardActionParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Current Control Token epoch observed in Relay State.
    pub expected_control_epoch: u64,
    /// Current internal Board window fencing token observed in Relay State.
    pub board_window: u64,
    /// Complete replacement Markdown body for `update`; `None` means `unchanged`.
    pub board: Option<&'a str>,
}

/// Product outcome carried by a Meeting V2 End command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV2EndOutcome {
    /// The moderator declares the goal reached with an effective conclusion.
    Closed,
    /// The moderator or an authorized operator terminates without success.
    Aborted,
}

impl MeetingV2EndOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Aborted => "aborted",
        }
    }
}

/// Inputs for a Meeting V2 close or abort command.
pub struct MeetingV2EndParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Event ID of the corresponding V2 Create command.
    pub create_event_id: &'a str,
    /// Successful close or abnormal abort.
    pub outcome: MeetingV2EndOutcome,
    /// Required closed reason for abort; absent for close.
    pub reason_code: Option<&'a str>,
    /// Optional short explanation for abort; absent for close.
    pub reason: Option<&'a str>,
}

/// Fences shared by commands targeting an active Meeting V2 action run.
#[derive(Debug, Clone, Copy)]
pub struct MeetingV2ActionRunFence<'a> {
    /// Relay-issued action run identifier.
    pub action_run_id: Uuid,
    /// Current action retry-window epoch.
    pub action_window: u64,
    /// Exact frozen final Board event for this action run.
    pub board_event_id: &'a str,
}

/// Source-owned Meeting retrieval-summary mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingSummaryMutation<'a> {
    /// Replace the current retrieval summary with the exact supplied text.
    Set(&'a str),
    /// Clear the current retrieval summary.
    Clear,
}

/// Inputs for updating Meeting retrieval metadata in an active Action window.
pub struct MeetingSummaryUpdateParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// SET or CLEAR mutation.
    pub mutation: MeetingSummaryMutation<'a>,
    /// Current action-run fences.
    pub action_fence: MeetingV2ActionRunFence<'a>,
}

/// Inputs for entering Meeting V2 action finalization.
pub struct MeetingV2ActionBeginParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Current control-token epoch observed in authoritative State.
    pub expected_control_epoch: u64,
    /// Final completed Board window.
    pub board_window: u64,
    /// Exact authoritative State event on which the decision was made.
    pub expected_state_event_id: &'a str,
    /// Exact final Board projection interpreted by the moderator.
    pub board_event_id: &'a str,
    /// Optional registered moderator decision attempt consumed by a later stage.
    pub expected_decision_attempt_id: Option<&'a str>,
}

/// Inputs for durably blocking a Meeting V2 action run.
pub struct MeetingV2ActionBlockParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Current action-run fences.
    pub fence: MeetingV2ActionRunFence<'a>,
    /// Closed machine-readable failure category.
    pub reason_code: &'a str,
    /// Optional short diagnostic safe for the private command receipt.
    pub reason: Option<&'a str>,
}

/// Inputs for a no-content action command such as retry or return-to-board.
pub struct MeetingV2ActionCommandParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Current action-run fences.
    pub fence: MeetingV2ActionRunFence<'a>,
}

/// Low-cardinality diagnostic stage reported by an Action lease renewal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV2ActionProgressStage {
    /// The host is reasoning about the frozen Board and canonical state.
    Reasoning,
    /// The host is invoking a tool or business command.
    ToolCall,
    /// The host is processing a tool or business-command result.
    ToolResult,
    /// The host is preparing the final actions-recorded attestation.
    Finalizing,
    /// A Human-hosted action is intentionally waiting for Human input.
    WaitingHuman,
}

impl MeetingV2ActionProgressStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::ToolCall => "tool_call",
            Self::ToolResult => "tool_result",
            Self::Finalizing => "finalizing",
            Self::WaitingHuman => "waiting_human",
        }
    }
}

/// Inputs for renewing the exact in-flight Meeting V2 Action window.
pub struct MeetingV2ActionLeaseRenewParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Current action-run fences.
    pub fence: MeetingV2ActionRunFence<'a>,
    /// Exact next renewal sequence for this action window.
    pub progress_seq: u64,
    /// Current low-cardinality host stage for diagnostics.
    pub stage: MeetingV2ActionProgressStage,
    /// Monotonic provider/tool activity sequence observed by the host.
    pub last_activity_seq: u64,
}

/// Action-run fence carried by the moderator's final recorded-actions attestation.
#[derive(Debug, Clone, Copy)]
pub struct MeetingV2ActionsEndFence<'a> {
    /// Relay-issued action run identifier.
    pub action_run_id: Uuid,
    /// Current action retry-window epoch.
    pub action_window: u64,
    /// Exact frozen final Board event for this action run.
    pub board_event_id: &'a str,
}

/// Inputs for closing or aborting an action-capable Meeting V2 session.
pub struct MeetingV2ActionsEndParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Event ID of the corresponding Create command.
    pub create_event_id: &'a str,
    /// Successful close or abnormal abort.
    pub outcome: MeetingV2EndOutcome,
    /// Required reason category for abort; absent for close.
    pub reason_code: Option<&'a str>,
    /// Optional short abort explanation.
    pub reason: Option<&'a str>,
    /// Required only when closing from a completed action-finalization run.
    pub action_fence: Option<MeetingV2ActionsEndFence<'a>>,
}

/// Inputs for a strict Meeting V1 Create command.
///
/// `participant_pubkeys` contains every participant except `author_pubkey`.
/// The author is supplied separately so the SDK can reject duplicate rosters
/// and require the moderator to belong to the complete frozen roster. The
/// caller must sign the returned builder with the matching author key.
pub struct MeetingV1CreateParams<'a> {
    /// Stable Meeting UUID; also the private backing Channel UUID.
    pub session_id: Uuid,
    /// Human-readable Meeting title.
    pub title: &'a str,
    /// Optional Meeting description.
    pub description: Option<&'a str>,
    /// Optional source Channel used as a context/navigation reference.
    pub source_channel_id: Option<Uuid>,
    /// Pubkey that will sign the returned event.
    pub author_pubkey: &'a str,
    /// Frozen moderator pubkey. It must be the author or one listed participant.
    pub moderator_pubkey: &'a str,
    /// Frozen participants other than the event author.
    pub participant_pubkeys: &'a [&'a str],
}

/// Inputs for a strict manual Meeting V1 End command.
pub struct MeetingV1EndParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Event ID of the corresponding V1 Create command.
    pub create_event_id: &'a str,
}

/// Inputs for submitting a Meeting V1 speech intent.
pub struct MeetingV1IntentSubmitParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Speech revision against which the summary was prepared.
    pub basis_speech_revision: u64,
    /// Optional participant whom the proposed contribution addresses.
    pub addressed_to: Option<&'a str>,
    /// One-sentence summary visible in the shared intent pool.
    pub summary: &'a str,
}

/// Inputs for compare-and-swap refreshing a Meeting V1 speech intent.
pub struct MeetingV1IntentRefreshParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Stable ID of the original submit event.
    pub intent_id: &'a str,
    /// Current canonical event ID being replaced.
    pub previous_event_id: &'a str,
    /// Speech revision against which the new summary was prepared.
    pub basis_speech_revision: u64,
    /// Optional participant whom the proposed contribution addresses.
    pub addressed_to: Option<&'a str>,
    /// Replacement one-sentence summary.
    pub summary: &'a str,
}

/// Inputs for compare-and-swap withdrawing a Meeting V1 speech intent.
pub struct MeetingV1IntentWithdrawParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Stable ID of the original submit event.
    pub intent_id: &'a str,
    /// Current canonical event ID being withdrawn.
    pub previous_event_id: &'a str,
}

/// One pending intent deferred atomically by a moderator-self selection.
pub struct MeetingV1IntentDeferral<'a> {
    /// Stable ID of the pending intent.
    pub intent_id: &'a str,
    /// Current canonical event ID used as the compare-and-swap basis.
    pub previous_event_id: &'a str,
    /// Required human-readable explanation.
    pub reason: &'a str,
}

/// Source selected by a Meeting V1 moderator.
pub enum MeetingV1Selection<'a> {
    /// Select a pending speech intent.
    Intent {
        /// Stable ID of the original intent submit event.
        intent_id: &'a str,
    },
    /// Select an unresolved directed handoff.
    Handoff {
        /// Stable handoff ID, equal to its source speech event ID.
        handoff_id: &'a str,
        /// Current persisted attempt count used as a compare-and-swap basis.
        expected_attempt_count: u64,
    },
}

/// Inputs for a Meeting V1 moderator Select command.
pub struct MeetingV1ModeratorSelectParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Intent or handoff selected by the moderator.
    pub selection: MeetingV1Selection<'a>,
    /// Expected current control epoch.
    pub expected_control_epoch: u64,
    /// Expected current decision epoch.
    pub expected_decision_epoch: u64,
    /// Expected current intent revision.
    pub expected_intent_revision: u64,
    /// Expected current speech revision.
    pub expected_speech_revision: u64,
    /// Optional short explanation for the selection.
    pub selection_reason: Option<&'a str>,
    /// Atomic deferrals; only valid when Relay confirms a moderator-self intent.
    pub deferrals: &'a [MeetingV1IntentDeferral<'a>],
    /// Registered decision attempt; required for an Agent moderator.
    pub attempt_id: Option<&'a str>,
    /// Intent event version from the registered snapshot.
    pub expected_source_event_id: Option<&'a str>,
}

/// Closed reason codes for rejecting a Meeting V1 intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1IntentRejectionReason {
    /// The proposed contribution is outside the meeting topic.
    OffTopic,
    /// The proposed contribution duplicates another one.
    Duplicate,
    /// A newer contribution replaces this one.
    Superseded,
    /// The contribution cannot be supported in this meeting.
    Unsupported,
    /// The contribution does not fit the current agenda.
    AgendaMismatch,
}

impl MeetingV1IntentRejectionReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::OffTopic => "off_topic",
            Self::Duplicate => "duplicate",
            Self::Superseded => "superseded",
            Self::Unsupported => "unsupported",
            Self::AgendaMismatch => "agenda_mismatch",
        }
    }
}

/// Inputs for a Meeting V1 moderator Reject command.
pub struct MeetingV1ModeratorRejectParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Stable ID of the pending intent.
    pub intent_id: &'a str,
    /// Current canonical intent event ID.
    pub previous_event_id: &'a str,
    /// Persisted author of the intent, used as a notification index.
    pub intent_author_pubkey: &'a str,
    /// Closed machine-readable reason.
    pub reason_code: MeetingV1IntentRejectionReason,
    /// Required human-readable explanation.
    pub reason_text: &'a str,
    /// Registered decision attempt; required for an Agent moderator.
    pub attempt_id: Option<&'a str>,
}

/// Closed reason codes for dismissing a Meeting V1 handoff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1HandoffDismissReason {
    /// A newer question or request replaces this one.
    Superseded,
    /// The question was answered through another path.
    AnsweredElsewhere,
    /// The question is outside the meeting scope.
    OutOfScope,
    /// The answer is no longer needed.
    NoLongerNeeded,
}

impl MeetingV1HandoffDismissReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Superseded => "superseded",
            Self::AnsweredElsewhere => "answered_elsewhere",
            Self::OutOfScope => "out_of_scope",
            Self::NoLongerNeeded => "no_longer_needed",
        }
    }
}

/// Inputs for a Meeting V1 moderator Dismiss Handoff command.
pub struct MeetingV1ModeratorDismissHandoffParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Stable ID of the open handoff.
    pub handoff_id: &'a str,
    /// Expected current speech revision.
    pub expected_speech_revision: u64,
    /// Expected current handoff attempt count.
    pub expected_attempt_count: u64,
    /// Closed machine-readable reason.
    pub reason_code: MeetingV1HandoffDismissReason,
    /// Required human-readable explanation.
    pub reason_text: &'a str,
    /// Registered decision attempt; required for an Agent moderator.
    pub attempt_id: Option<&'a str>,
}

/// Inputs for registering an authoritative Meeting V1 moderator attempt.
pub struct MeetingV1DecisionAttemptStartParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Expected current control epoch.
    pub expected_control_epoch: u64,
    /// Expected current decision epoch.
    pub expected_decision_epoch: u64,
    /// Expected current Intent revision.
    pub expected_intent_revision: u64,
    /// Expected current speech revision.
    pub expected_speech_revision: u64,
    /// Relay State event observed immediately before registration.
    pub expected_state_event_id: &'a str,
    /// Abandoned attempt replaced without refreshing its deadline.
    pub replacement_of_attempt_id: Option<&'a str>,
}

/// Terminal class for a Meeting V1 moderator attempt without a primary action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1DecisionAttemptFinishOutcome {
    /// Model execution completed and requires no primary action.
    Completed,
    /// Shared protocol state made the model result irrelevant.
    Discarded,
}

impl MeetingV1DecisionAttemptFinishOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Discarded => "discarded",
        }
    }
}

/// Inputs for terminalizing a Meeting V1 moderator attempt.
pub struct MeetingV1DecisionAttemptFinishParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Registered decision attempt.
    pub attempt_id: &'a str,
    /// Completed or discarded terminal class.
    pub outcome: MeetingV1DecisionAttemptFinishOutcome,
    /// Closed machine-readable terminal reason.
    pub reason_code: &'a str,
}

/// Inputs for consuming a Relay-issued Meeting V1 retry ticket.
pub struct MeetingV1DecisionRetryParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Failed decision attempt.
    pub attempt_id: &'a str,
    /// One-use Relay retry ticket.
    pub retry_ticket_id: &'a str,
    /// Signed action event whose rejection issued the ticket.
    pub failed_action_event_id: &'a str,
    /// Expected current control epoch.
    pub expected_control_epoch: u64,
    /// Expected current decision epoch.
    pub expected_decision_epoch: u64,
    /// Expected failed attempt number.
    pub expected_attempt_number: u64,
}

/// Inputs for closing an exhausted Meeting V1 Candidate Cohort.
pub struct MeetingV1CompleteCohortParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Registered decision attempt.
    pub attempt_id: &'a str,
    /// Expected current control epoch.
    pub expected_control_epoch: u64,
    /// Expected current decision epoch.
    pub expected_decision_epoch: u64,
}

/// Inputs for abandoning a Meeting V1 moderator attempt after Runtime loss.
pub struct MeetingV1DecisionAttemptAbandonParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Registered decision attempt.
    pub attempt_id: &'a str,
}

/// Inputs for an attempt-bound Agent moderator self-Intent withdrawal.
pub struct MeetingV1ModeratorWithdrawSelfParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Registered decision attempt.
    pub attempt_id: &'a str,
    /// Stable moderator self-Intent ID.
    pub intent_id: &'a str,
    /// Intent event version seen by the model.
    pub previous_event_id: &'a str,
}

/// Inputs for a Meeting V1 moderator Recall command.
pub struct MeetingV1ModeratorRecallParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Control epoch to which the recall applies.
    pub control_epoch: u64,
    /// Optional human-readable explanation.
    pub reason: Option<&'a str>,
}

/// Inputs for a Meeting V1 Human Floor Request command.
pub struct MeetingV1HumanFloorRequestParams {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
}

/// Inputs for withdrawing a Meeting V1 Human Floor Request.
pub struct MeetingV1HumanFloorWithdrawParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Stable ID of the active request event.
    pub request_id: &'a str,
}

/// Inputs for acknowledging a Meeting V1 Offer.
pub struct MeetingV1OfferAckParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Relay-generated active Offer ID.
    pub offer_id: &'a str,
}

/// Inputs for declining a Meeting V1 Offer.
pub struct MeetingV1OfferDeclineParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Relay-generated active Offer ID.
    pub offer_id: &'a str,
    /// Optional short explanation.
    pub reason: Option<&'a str>,
}

/// Observable execution stages accepted by Meeting V1 Progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1ProgressStage {
    /// Synchronizing context needed for the turn.
    ContextSync,
    /// Calling an allowed tool.
    ToolUse,
    /// Generating a response.
    Generating,
    /// A Human is composing input.
    Composing,
    /// Submitting the signed speech event.
    Submitting,
}

impl MeetingV1ProgressStage {
    fn as_str(self) -> &'static str {
        match self {
            Self::ContextSync => "context_sync",
            Self::ToolUse => "tool_use",
            Self::Generating => "generating",
            Self::Composing => "composing",
            Self::Submitting => "submitting",
        }
    }
}

/// Inputs for a Meeting V1 Grant Progress signal.
pub struct MeetingV1GrantProgressParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Relay-generated active Grant ID.
    pub grant_id: &'a str,
    /// Strictly increasing sequence number for this Grant.
    pub progress_seq: u64,
    /// Observable local execution stage.
    pub stage: MeetingV1ProgressStage,
}

/// Closed reason codes accepted by Meeting V1 Grant Yield.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1GrantYieldReason {
    /// The turn is no longer needed.
    NoLongerNeeded,
    /// The holder cannot provide a useful answer.
    UnableToAnswer,
    /// Required context is unavailable.
    InsufficientContext,
    /// An allowed tool failed.
    ToolFailure,
    /// The holder explicitly cancelled the turn.
    Cancelled,
}

impl MeetingV1GrantYieldReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NoLongerNeeded => "no_longer_needed",
            Self::UnableToAnswer => "unable_to_answer",
            Self::InsufficientContext => "insufficient_context",
            Self::ToolFailure => "tool_failure",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Inputs for a Meeting V1 Grant Yield signal.
pub struct MeetingV1GrantYieldParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Relay-generated active Grant ID.
    pub grant_id: &'a str,
    /// Optional closed machine-readable reason.
    pub reason_code: Option<MeetingV1GrantYieldReason>,
    /// Optional short human-readable explanation.
    pub reason: Option<&'a str>,
}

/// Directed handoff types carried atomically by a Meeting V1 speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV1HandoffType {
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

impl MeetingV1HandoffType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Question => "question",
            Self::InformationRequest => "information_request",
            Self::Clarification => "clarification",
            Self::Review => "review",
            Self::ResponseRequested => "response_requested",
        }
    }
}

/// Optional directed handoff attached atomically to Meeting V1 speech.
pub struct MeetingV1DirectedHandoff<'a> {
    /// Another frozen participant who should receive the next Offer.
    pub target_pubkey: &'a str,
    /// Semantic type of the handoff.
    pub handoff_type: MeetingV1HandoffType,
    /// Required explanation shown to the target.
    pub reason: &'a str,
}

/// Inputs for a Grant-bound Meeting V1 speech.
pub struct MeetingV1SpeechParams<'a> {
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Relay-generated active Grant ID.
    pub grant_id: &'a str,
    /// Exactly the current speech revision plus one.
    pub speech_revision: u64,
    /// Non-empty speech content.
    pub content: &'a str,
    /// Mentioned participant pubkeys.
    pub mentions: &'a [&'a str],
    /// Optional atomic directed handoff.
    pub handoff: Option<MeetingV1DirectedHandoff<'a>>,
}

/// Parse a tag slice, mapping errors to `SdkError::InvalidTag`.
fn tag(parts: &[&str]) -> Result<Tag, SdkError> {
    Tag::parse(parts.iter().copied()).map_err(|e| SdkError::InvalidTag(e.to_string()))
}

/// Validate content byte length.
fn check_content(content: &str, max: usize) -> Result<(), SdkError> {
    let got = content.len();
    if got > max {
        return Err(SdkError::ContentTooLarge { max, got });
    }
    Ok(())
}

/// Validate hex string has at least `min_len` hex characters.
fn check_hex_len(s: &str, min_len: usize, field: &str) -> Result<(), SdkError> {
    if s.len() < min_len || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidDiffMeta(format!(
            "{field} must be at least {min_len} hex characters (got {:?})",
            s
        )));
    }
    Ok(())
}

/// Validate a git commit-like hex id (commit, parent-commit, euc,
/// merge-commit, applied-as-commit). Git object ids are full SHA-1 (40 hex
/// chars) or SHA-256 (64 hex chars) — anything shorter is an abbreviated
/// ref that NIP-34 canonical tags shouldn't carry, since consumers resolve
/// these against the actual repo.
fn check_commit_hex(s: &str, field: &str) -> Result<(), SdkError> {
    if (s.len() != 40 && s.len() != 64) || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be a full 40-character (SHA-1) or 64-character (SHA-256) hex commit id (got {:?})",
            s
        )));
    }
    Ok(())
}

fn check_pubkey_hex(s: &str, field: &str) -> Result<String, SdkError> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be a 64-character hex pubkey"
        )));
    }
    Ok(s.to_ascii_lowercase())
}

fn check_meeting_v1_session(session_id: Uuid) -> Result<(), SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    Ok(())
}

fn check_meeting_v1_text<'a>(
    value: &'a str,
    field: &str,
    max_bytes: usize,
) -> Result<&'a str, SdkError> {
    if value.is_empty() || value.len() > max_bytes {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be 1-{max_bytes} bytes"
        )));
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(SdkError::InvalidInput(format!(
            "{field} must not have surrounding whitespace or control characters"
        )));
    }
    Ok(value)
}

fn check_meeting_v1_optional_text<'a>(
    value: Option<&'a str>,
    field: &str,
    max_bytes: usize,
) -> Result<Option<&'a str>, SdkError> {
    value
        .map(|value| check_meeting_v1_text(value, field, max_bytes))
        .transpose()
}

fn check_meeting_v1_i64(value: u64, field: &str) -> Result<(), SdkError> {
    if value > i64::MAX as u64 {
        return Err(SdkError::InvalidInput(format!(
            "{field} exceeds the signed 64-bit wire range"
        )));
    }
    Ok(())
}

fn check_meeting_v1_i32(value: u64, field: &str) -> Result<(), SdkError> {
    if value > i32::MAX as u64 {
        return Err(SdkError::InvalidInput(format!(
            "{field} exceeds the signed 32-bit wire range"
        )));
    }
    Ok(())
}

fn meeting_command_tags(
    session_id: Uuid,
    schema_version: &str,
    action: &str,
) -> Result<Vec<Tag>, SdkError> {
    check_meeting_v1_session(session_id)?;
    Ok(vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["v", schema_version])?,
        tag(&["action", action])?,
    ])
}

/// Validate an exact-length hex string (event ids), returning it lowercased.
fn check_hex_exact(s: &str, len: usize, field: &str) -> Result<String, SdkError> {
    if s.len() != len || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidInput(format!(
            "{field} must be a {len}-character hex string"
        )));
    }
    Ok(s.to_ascii_lowercase())
}

/// Validate a git repo identifier: `[a-zA-Z0-9._-]{1,64}`, no leading dot,
/// no `..`. Shared by `build_repo_announcement` and `GitRepoCoord` so a
/// repo coordinate built directly through the SDK (bypassing CLI-side
/// `validate_repo_id`) can't slip an invalid `d`-tag into an `a`-tag value.
fn check_repo_id(repo_id: &str) -> Result<(), SdkError> {
    if repo_id.is_empty() {
        return Err(SdkError::InvalidInput("repo_id must not be empty".into()));
    }
    if repo_id.len() > 64 {
        return Err(SdkError::InvalidInput(format!(
            "repo_id exceeds 64 characters (got {})",
            repo_id.len()
        )));
    }
    if !repo_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    {
        return Err(SdkError::InvalidInput(
            "repo_id may only contain [a-zA-Z0-9._-]".into(),
        ));
    }
    if repo_id.starts_with('.') {
        return Err(SdkError::InvalidInput(
            "repo_id must not start with a dot".into(),
        ));
    }
    if repo_id.contains("..") {
        return Err(SdkError::InvalidInput(
            "repo_id must not contain '..'".into(),
        ));
    }
    Ok(())
}

/// Validate and normalize a NIP-30 custom emoji shortcode.
///
/// Shortcodes are case-insensitive in Buzz's relay-global set; lowercase
/// normalization prevents `party_parrot` and `Party_Parrot` from colliding.
pub fn normalize_custom_emoji_shortcode(shortcode: &str) -> Result<String, SdkError> {
    let trimmed = shortcode.trim().trim_matches(':');
    if trimmed.is_empty() {
        return Err(SdkError::InvalidInput(
            "emoji shortcode must not be empty".into(),
        ));
    }
    if trimmed.len() > 64 {
        return Err(SdkError::InvalidInput(format!(
            "emoji shortcode exceeds 64 bytes (got {})",
            trimmed.len()
        )));
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(SdkError::InvalidInput(
            "emoji shortcode may only contain ASCII letters, digits, hyphens, and underscores"
                .into(),
        ));
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn check_custom_emoji_url(url: &str) -> Result<(), SdkError> {
    if url.is_empty() {
        return Err(SdkError::InvalidInput(
            "emoji image URL must not be empty".into(),
        ));
    }
    if url.len() > 2048 {
        return Err(SdkError::InvalidInput(format!(
            "emoji image URL exceeds 2048 bytes (got {})",
            url.len()
        )));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(SdkError::InvalidInput(
            "emoji image URL must start with http:// or https://".into(),
        ));
    }
    Ok(())
}

/// Emit NIP-10 e-tags for a `ThreadRef`.
fn thread_tags(thread_ref: &ThreadRef, tags: &mut Vec<Tag>) -> Result<(), SdkError> {
    let root = thread_ref.root_event_id.to_hex();
    let parent = thread_ref.parent_event_id.to_hex();
    if root == parent {
        // Direct reply
        tags.push(tag(&["e", &root, "", "reply"])?);
    } else {
        // Nested reply
        tags.push(tag(&["e", &root, "", "root"])?);
        tags.push(tag(&["e", &parent, "", "reply"])?);
    }
    Ok(())
}

/// Deduplicate and cap mentions, emitting p-tags.
fn mention_tags(mentions: &[&str], tags: &mut Vec<Tag>) -> Result<(), SdkError> {
    if mentions.len() > crate::mentions::MENTION_CAP {
        return Err(SdkError::TooManyMentions);
    }
    let mut seen = std::collections::HashSet::new();
    for &hex in mentions {
        let lower = hex.to_ascii_lowercase();
        if seen.insert(lower.clone()) {
            tags.push(tag(&["p", &lower])?);
        }
    }
    Ok(())
}

/// Emit imeta tags from raw tag vectors.
fn imeta_tags(media_tags: &[Vec<String>], tags: &mut Vec<Tag>) -> Result<(), SdkError> {
    for mt in media_tags {
        let parts: Vec<&str> = mt.iter().map(String::as_str).collect();
        tags.push(Tag::parse(parts).map_err(|e| SdkError::InvalidTag(e.to_string()))?);
    }
    Ok(())
}

/// Build a stream message (kind 9).
///
/// - `channel_id`: target channel UUID
/// - `content`: message text (max 64 KiB)
/// - `thread_ref`: optional NIP-10 reply context
/// - `mentions`: pubkey hex strings to p-tag (deduped, max 50)
/// - `broadcast`: if true, adds `["broadcast", "1"]` tag
/// - `media_tags`: raw imeta tag vectors
pub fn build_message(
    channel_id: Uuid,
    content: &str,
    thread_ref: Option<&ThreadRef>,
    mentions: &[&str],
    broadcast: bool,
    media_tags: &[Vec<String>],
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let mut tags = vec![tag(&["h", &channel_id.to_string()])?];
    if let Some(tr) = thread_ref {
        thread_tags(tr, &mut tags)?;
    }
    mention_tags(mentions, &mut tags)?;
    if broadcast {
        tags.push(tag(&["broadcast", "1"])?);
    }
    imeta_tags(media_tags, &mut tags)?;
    Ok(EventBuilder::new(Kind::Custom(9), content).tags(tags))
}

/// Build an encrypted agent observer frame (kind 24200).
///
/// `recipient_pubkey` is the cleartext `p` tag used by the relay for owner-only
/// routing. `agent_pubkey` identifies the managed agent whose observer stream
/// this frame belongs to. `encrypted_content` must be NIP-44 v2 ciphertext.
pub fn build_agent_observer_frame(
    recipient_pubkey: &str,
    agent_pubkey: &str,
    frame: &str,
    encrypted_content: &str,
) -> Result<EventBuilder, SdkError> {
    if frame != OBSERVER_FRAME_TELEMETRY && frame != OBSERVER_FRAME_CONTROL {
        return Err(SdkError::InvalidInput(format!(
            "observer frame must be {OBSERVER_FRAME_TELEMETRY:?} or {OBSERVER_FRAME_CONTROL:?}"
        )));
    }
    if !content_looks_like_nip44(encrypted_content) {
        return Err(SdkError::InvalidInput(
            "observer frame content must be NIP-44 v2 ciphertext".into(),
        ));
    }

    let recipient_pubkey = check_pubkey_hex(recipient_pubkey, "recipient_pubkey")?;
    let agent_pubkey = check_pubkey_hex(agent_pubkey, "agent_pubkey")?;
    let tags = vec![
        tag(&["p", &recipient_pubkey])?,
        tag(&[OBSERVER_AGENT_TAG, &agent_pubkey])?,
        tag(&[OBSERVER_FRAME_TAG, frame])?,
    ];

    Ok(EventBuilder::new(
        Kind::Custom(KIND_AGENT_OBSERVER_FRAME as u16),
        encrypted_content,
    )
    .tags(tags))
}

/// Build a forum post thread root (kind 45001).
pub fn build_forum_post(
    channel_id: Uuid,
    content: &str,
    mentions: &[&str],
    media_tags: &[Vec<String>],
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let mut tags = vec![tag(&["h", &channel_id.to_string()])?];
    mention_tags(mentions, &mut tags)?;
    imeta_tags(media_tags, &mut tags)?;
    Ok(EventBuilder::new(Kind::Custom(45001), content).tags(tags))
}

/// Build a forum comment reply (kind 45003).
pub fn build_forum_comment(
    channel_id: Uuid,
    content: &str,
    thread_ref: &ThreadRef,
    mentions: &[&str],
    media_tags: &[Vec<String>],
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let mut tags = vec![tag(&["h", &channel_id.to_string()])?];
    thread_tags(thread_ref, &mut tags)?;
    mention_tags(mentions, &mut tags)?;
    imeta_tags(media_tags, &mut tags)?;
    Ok(EventBuilder::new(Kind::Custom(45003), content).tags(tags))
}

/// Build a diff/patch message (kind 40008).
pub fn build_diff_message(
    channel_id: Uuid,
    content: &str,
    diff_meta: &DiffMeta,
    thread_ref: Option<&ThreadRef>,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 60 * 1024)?;

    // Validate DiffMeta
    if !diff_meta.repo_url.starts_with("http://") && !diff_meta.repo_url.starts_with("https://") {
        return Err(SdkError::InvalidDiffMeta(
            "repo_url must start with http:// or https://".into(),
        ));
    }
    check_hex_len(&diff_meta.commit_sha, 7, "commit_sha")?;
    if let Some(ref pc) = diff_meta.parent_commit {
        check_hex_len(pc, 7, "parent_commit")?;
    }
    match &diff_meta.branch {
        Some((src, tgt)) if src.is_empty() || tgt.is_empty() => {
            return Err(SdkError::InvalidDiffMeta(
                "branch requires both source and target to be non-empty".into(),
            ));
        }
        _ => {}
    }
    if let Some(pr) = diff_meta.pr_number {
        if pr == 0 {
            return Err(SdkError::InvalidDiffMeta(
                "pr_number must be positive".into(),
            ));
        }
    }

    let mut tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["repo", &diff_meta.repo_url])?,
        tag(&["commit", &diff_meta.commit_sha])?,
    ];
    if let Some(ref fp) = diff_meta.file_path {
        tags.push(tag(&["file", fp])?);
    }
    if let Some(ref pc) = diff_meta.parent_commit {
        tags.push(tag(&["parent-commit", pc])?);
    }
    if let Some((ref src, ref tgt)) = diff_meta.branch {
        tags.push(tag(&["branch", src, tgt])?);
    }
    if let Some(pr) = diff_meta.pr_number {
        tags.push(tag(&["pr", &pr.to_string()])?);
    }
    if let Some(ref lang) = diff_meta.language {
        tags.push(tag(&["l", lang])?);
    }
    if let Some(ref desc) = diff_meta.description {
        tags.push(tag(&["description", desc])?);
    }
    if diff_meta.truncated {
        tags.push(tag(&["truncated", "true"])?);
    }
    if let Some(ref alt) = diff_meta.alt_text {
        tags.push(tag(&["alt", alt])?);
    }
    if let Some(tr) = thread_ref {
        thread_tags(tr, &mut tags)?;
    }
    Ok(EventBuilder::new(Kind::Custom(40008), content).tags(tags))
}

/// Build an edit event targeting an existing message (kind 40003).
pub fn build_edit(
    channel_id: Uuid,
    target_event_id: nostr::EventId,
    new_content: &str,
) -> Result<EventBuilder, SdkError> {
    check_content(new_content, 64 * 1024)?;
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["e", &target_event_id.to_hex()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(40003), new_content).tags(tags))
}

/// Optional metadata for moderator delete tombstones (kind 9005).
#[derive(Debug, Clone, Default)]
pub struct DeleteMessageOptions<'a> {
    /// Audit action UUID to link from the public tombstone.
    pub action_id: Option<Uuid>,
    /// Machine-readable, public-safe reason code.
    pub reason_code: Option<&'a str>,
    /// Human-readable reason safe for the room-facing tombstone.
    pub public_reason: Option<&'a str>,
}

/// Build a Buzz-native delete event (kind 9005).
pub fn build_delete_message(
    channel_id: Uuid,
    target_event_id: nostr::EventId,
) -> Result<EventBuilder, SdkError> {
    build_delete_message_with_options(channel_id, target_event_id, DeleteMessageOptions::default())
}

/// Build a Buzz-native delete event (kind 9005) with optional moderation metadata.
pub fn build_delete_message_with_options(
    channel_id: Uuid,
    target_event_id: nostr::EventId,
    options: DeleteMessageOptions<'_>,
) -> Result<EventBuilder, SdkError> {
    let mut tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["e", &target_event_id.to_hex()])?,
    ];
    if let Some(action_id) = options.action_id {
        tags.push(tag(&["action_id", &action_id.to_string()])?);
    }
    if let Some(reason_code) = options.reason_code {
        tags.push(tag(&["reason_code", reason_code])?);
    }
    if let Some(public_reason) = options.public_reason {
        tags.push(tag(&["public_reason", public_reason])?);
    }
    Ok(EventBuilder::new(Kind::Custom(9005), "").tags(tags))
}

/// Build a NIP-09 deletion event (kind 5). The `h` tag is non-standard for
/// NIP-09 but is required so channel-scoped subscriptions observe the delete.
pub fn build_delete_compat(
    channel_id: Uuid,
    target_event_id: nostr::EventId,
) -> Result<EventBuilder, SdkError> {
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["e", &target_event_id.to_hex()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(5), "").tags(tags))
}

/// Build a forum vote event (kind 45002). Content is `"+"` or `"-"`.
pub fn build_vote(
    channel_id: Uuid,
    target_event_id: nostr::EventId,
    direction: VoteDirection,
) -> Result<EventBuilder, SdkError> {
    let content = match direction {
        VoteDirection::Up => "+",
        VoteDirection::Down => "-",
    };
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["e", &target_event_id.to_hex()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(45002), content).tags(tags))
}

/// Build a NIP-25 reaction event (kind 7). Emoji max 64 chars.
pub fn build_reaction(
    target_event_id: nostr::EventId,
    emoji: &str,
) -> Result<EventBuilder, SdkError> {
    if emoji.chars().count() > 64 {
        return Err(SdkError::EmojiTooLong);
    }
    let tags = vec![tag(&["e", &target_event_id.to_hex()])?];
    Ok(EventBuilder::new(Kind::Custom(7), emoji).tags(tags))
}

/// Build a NIP-25 reaction event using a NIP-30 custom emoji.
///
/// The reaction content is `:shortcode:` and the event carries exactly one
/// `["emoji", shortcode, url]` tag, matching NIP-25's custom emoji reaction
/// guidance.
pub fn build_custom_emoji_reaction(
    target_event_id: nostr::EventId,
    shortcode: &str,
    url: &str,
) -> Result<EventBuilder, SdkError> {
    let shortcode = normalize_custom_emoji_shortcode(shortcode)?;
    check_custom_emoji_url(url)?;
    let content = format!(":{shortcode}:");
    let tags = vec![
        tag(&["e", &target_event_id.to_hex()])?,
        tag(&["emoji", &shortcode, url])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(7), content).tags(tags))
}

/// Build a deletion event targeting a reaction (kind 5).
pub fn build_remove_reaction(reaction_event_id: nostr::EventId) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["e", &reaction_event_id.to_hex()])?];
    Ok(EventBuilder::new(Kind::Custom(5), "").tags(tags))
}

/// d-tag for a member's own custom emoji set. Each member publishes one
/// user-signed kind:30030 under this d-tag; the workspace palette is the
/// client-side union of every member's set.
pub const CUSTOM_EMOJI_SET_D_TAG: &str = "buzz:custom-emoji";

/// Build a member's own custom emoji set event (kind:30030, NIP-30/NIP-51).
///
/// User-signed and parameterized-replaceable, keyed by `(pubkey, 30030,
/// "buzz:custom-emoji")`. Replaces the caller's prior set. The workspace
/// palette shown in clients is the union of every member's set, deduped by
/// `(shortcode, url)` on read. Add/remove is read-own-set → mutate → rebuild.
pub fn build_custom_emoji_set(emojis: &[CustomEmoji]) -> Result<EventBuilder, SdkError> {
    let mut seen = std::collections::HashSet::with_capacity(emojis.len());
    let mut tags = Vec::with_capacity(emojis.len() + 1);
    tags.push(tag(&["d", CUSTOM_EMOJI_SET_D_TAG])?);
    for emoji in emojis {
        let shortcode = normalize_custom_emoji_shortcode(&emoji.shortcode)?;
        check_custom_emoji_url(&emoji.url)?;
        if !seen.insert(shortcode.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate emoji shortcode: {shortcode}"
            )));
        }
        tags.push(tag(&["emoji", &shortcode, &emoji.url])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_EMOJI_SET as u16), "").tags(tags))
}

/// Build a canvas update event (kind 40100).
pub fn build_set_canvas(channel_id: Uuid, content: &str) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["h", &channel_id.to_string()])?];
    Ok(EventBuilder::new(Kind::Custom(40100), content).tags(tags))
}

/// Build a NIP-01 profile metadata event (kind 0).
///
/// Only present (Some) fields are included in the JSON object.
pub fn build_profile(
    display_name: Option<&str>,
    name: Option<&str>,
    picture: Option<&str>,
    about: Option<&str>,
    nip05: Option<&str>,
) -> Result<EventBuilder, SdkError> {
    let mut map = serde_json::Map::new();
    if let Some(v) = display_name {
        map.insert("display_name".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = name {
        map.insert("name".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = picture {
        map.insert("picture".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = about {
        map.insert("about".into(), serde_json::Value::String(v.into()));
    }
    if let Some(v) = nip05 {
        map.insert("nip05".into(), serde_json::Value::String(v.into()));
    }
    let content = serde_json::Value::Object(map).to_string();
    Ok(EventBuilder::new(Kind::Custom(0), content).tags([]))
}

/// Build a complete Agent control profile event (kind 10100).
///
/// This replaceable profile carries the channel-add policy and the complete
/// runtime-capability snapshot together. Writers must preserve the dimension
/// they are not changing before calling this builder; emitting only one field
/// would make the signed event projection disagree with Relay state.
pub fn build_agent_profile_controls(
    channel_add_policy: &str,
    capabilities: &[&str],
    display_name: Option<&str>,
) -> Result<EventBuilder, SdkError> {
    if !matches!(channel_add_policy, "anyone" | "owner_only" | "nobody") {
        return Err(SdkError::InvalidInput(format!(
            "invalid channel_add_policy: {channel_add_policy}"
        )));
    }
    if capabilities.len() > MAX_AGENT_PROFILE_CAPABILITIES {
        return Err(SdkError::InvalidInput(format!(
            "too many Agent capabilities (max {MAX_AGENT_PROFILE_CAPABILITIES})"
        )));
    }

    let mut canonical = std::collections::BTreeSet::new();
    for capability in capabilities {
        if capability.is_empty()
            || capability.len() > 128
            || capability.trim() != *capability
            || capability.chars().any(char::is_control)
            || !canonical.insert(*capability)
        {
            return Err(SdkError::InvalidInput(
                "Agent capabilities must be unique clean strings of 1..=128 bytes".to_string(),
            ));
        }
    }

    let mut content = serde_json::json!({
        "channel_add_policy": channel_add_policy,
        "capabilities": canonical.into_iter().collect::<Vec<_>>(),
    });
    if let Some(display_name) = display_name.filter(|value| !value.trim().is_empty()) {
        content["display_name"] = serde_json::Value::String(display_name.to_string());
    }
    let content = content.to_string();
    Ok(EventBuilder::new(Kind::Custom(KIND_AGENT_PROFILE as u16), content).tags([]))
}

/// Build a NIP-29 add-member event (kind 9000).
pub fn build_add_member(
    channel_id: Uuid,
    target_pubkey: &str,
    role: Option<MemberRole>,
) -> Result<EventBuilder, SdkError> {
    check_hex_len(target_pubkey, 64, "target_pubkey")?;
    let mut tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["p", &target_pubkey.to_ascii_lowercase()])?,
    ];
    if let Some(r) = role {
        tags.push(tag(&["role", r.as_str()])?);
    }
    Ok(EventBuilder::new(Kind::Custom(9000), "").tags(tags))
}

/// Build a NIP-29 remove-member event (kind 9001).
pub fn build_remove_member(
    channel_id: Uuid,
    target_pubkey: &str,
) -> Result<EventBuilder, SdkError> {
    check_hex_len(target_pubkey, 64, "target_pubkey")?;
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["p", &target_pubkey.to_ascii_lowercase()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(9001), "").tags(tags))
}

/// Build a NIP-29 leave-request event (kind 9022).
pub fn build_leave(channel_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["h", &channel_id.to_string()])?];
    Ok(EventBuilder::new(Kind::Custom(9022), "").tags(tags))
}

/// Build a NIP-29 edit-metadata event for name/about/visibility/ttl (kind 9002).
///
/// `ttl`: outer `None` leaves it unchanged; `Some(Some(secs))` sets the
/// ephemeral timeout; `Some(None)` clears it (emits `["ttl", ""]`).
pub fn build_update_channel(
    channel_id: Uuid,
    name: Option<&str>,
    about: Option<&str>,
    visibility: Option<&str>,
    ttl: Option<Option<i32>>,
) -> Result<EventBuilder, SdkError> {
    if name.is_none() && about.is_none() && visibility.is_none() && ttl.is_none() {
        return Err(SdkError::InvalidTag(
            "at least one of name, about, visibility, or ttl must be provided".into(),
        ));
    }
    if let Some(v) = visibility {
        if v != "open" && v != "private" {
            return Err(SdkError::InvalidTag(
                "visibility must be \"open\" or \"private\"".into(),
            ));
        }
    }
    if name
        .map(buzz_core::channel::canonical_channel_name)
        .is_some_and(|name| name.trim().is_empty())
    {
        return Err(SdkError::InvalidTag("channel name is required".into()));
    }
    let mut tags = vec![tag(&["h", &channel_id.to_string()])?];
    if let Some(n) = name {
        tags.push(tag(&[
            "name",
            buzz_core::channel::canonical_channel_name(n),
        ])?);
    }
    if let Some(a) = about {
        tags.push(tag(&["about", a])?);
    }
    if let Some(v) = visibility {
        tags.push(tag(&["visibility", v])?);
    }
    if let Some(ttl) = ttl {
        match ttl {
            Some(secs) => tags.push(tag(&["ttl", &secs.to_string()])?),
            None => tags.push(tag(&["ttl", ""])?),
        }
    }
    Ok(EventBuilder::new(Kind::Custom(9002), "").tags(tags))
}

/// Build a NIP-29 edit-metadata event for topic (kind 9002).
pub fn build_set_topic(channel_id: Uuid, topic: &str) -> Result<EventBuilder, SdkError> {
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["topic", topic])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(9002), "").tags(tags))
}

/// Build a NIP-29 edit-metadata event for purpose (kind 9002).
pub fn build_set_purpose(channel_id: Uuid, purpose: &str) -> Result<EventBuilder, SdkError> {
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["purpose", purpose])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(9002), "").tags(tags))
}

/// Build a NIP-29 create-group event (kind 9007).
///
/// `ttl`: `Some(secs)` makes the channel ephemeral with that lifetime in
/// seconds (the relay archives it once the deadline passes without activity);
/// `None` leaves it permanent.
pub fn build_create_channel(
    channel_id: Uuid,
    name: &str,
    visibility: Option<Visibility>,
    channel_type: Option<ChannelKind>,
    about: Option<&str>,
    ttl: Option<i32>,
) -> Result<EventBuilder, SdkError> {
    let name = buzz_core::channel::canonical_channel_name(name);
    if name.trim().is_empty() {
        return Err(SdkError::InvalidTag("channel name is required".into()));
    }
    let mut tags = vec![tag(&["h", &channel_id.to_string()])?, tag(&["name", name])?];
    if let Some(v) = visibility {
        tags.push(tag(&["visibility", v.as_str()])?);
    }
    if let Some(ct) = channel_type {
        tags.push(tag(&["channel_type", ct.as_str()])?);
    }
    if let Some(a) = about {
        tags.push(tag(&["about", a])?);
    }
    if let Some(secs) = ttl {
        tags.push(tag(&["ttl", &secs.to_string()])?);
    }
    Ok(EventBuilder::new(Kind::Custom(9007), "").tags(tags))
}

/// Build a NIP-29 join-request event (kind 9021).
pub fn build_join(channel_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["h", &channel_id.to_string()])?];
    Ok(EventBuilder::new(Kind::Custom(9021), "").tags(tags))
}

/// Build a NIP-29 archive event (kind 9002, `["archived", "true"]`).
pub fn build_archive(channel_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["archived", "true"])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(9002), "").tags(tags))
}

/// Build a NIP-29 unarchive event (kind 9002, `["archived", "false"]`).
pub fn build_unarchive(channel_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![
        tag(&["h", &channel_id.to_string()])?,
        tag(&["archived", "false"])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(9002), "").tags(tags))
}

/// Build a NIP-29 delete-group event (kind 9008).
pub fn build_delete_channel(channel_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["h", &channel_id.to_string()])?];
    Ok(EventBuilder::new(Kind::Custom(9008), "").tags(tags))
}

/// Build a global text note (kind:1, NIP-01).
///
/// `reply_to_event_id`: adds a single `["e", <id>, "", "reply"]` tag.
/// This is intentionally simpler than the full `ThreadRef` mechanism used
/// for channel messages — social notes use a flat reply model for now.
/// Full NIP-10 threading (root + reply + p-tags) is deferred.
pub fn build_note(
    content: &str,
    reply_to_event_id: Option<nostr::EventId>,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let mut tags = vec![];
    if let Some(reply_id) = reply_to_event_id {
        tags.push(tag(&["e", &reply_id.to_hex(), "", "reply"])?);
    }
    Ok(EventBuilder::new(Kind::Custom(1), content).tags(tags))
}

/// Maximum number of contacts allowed in a single contact list event.
const MAX_CONTACTS: usize = 10_000;

/// Build a contact list replacement event (kind:3, NIP-02).
///
/// Each contact is `(pubkey_hex, relay_url, petname)`.
/// `pubkey_hex` must be exactly 64 hex characters (any case accepted, normalized
/// to lowercase before storage). Non-hex or wrong-length pubkeys are rejected
/// with `SdkError::InvalidInput`.
/// `relay_url` and `petname` may be `None` (stored as empty string per NIP-02).
///
/// Duplicate pubkeys are silently deduplicated — the first occurrence is kept.
///
/// Replaces the entire contact list — callers must read-before-write for deltas.
pub fn build_contact_list(
    contacts: &[(&str, Option<&str>, Option<&str>)],
) -> Result<EventBuilder, SdkError> {
    if contacts.len() > MAX_CONTACTS {
        return Err(SdkError::InvalidInput(format!(
            "contact list exceeds maximum of {} contacts (got {})",
            MAX_CONTACTS,
            contacts.len()
        )));
    }
    let mut seen = std::collections::HashSet::with_capacity(contacts.len());
    let mut tags = Vec::with_capacity(contacts.len());
    for &(pubkey_hex, relay_url, petname) in contacts {
        if pubkey_hex.len() != 64 || !pubkey_hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Err(SdkError::InvalidInput(format!(
                "contact pubkey must be exactly 64 hex chars, got len={}",
                pubkey_hex.len()
            )));
        }
        if let Some(url) = relay_url {
            if url.len() > 2048 {
                return Err(SdkError::InvalidInput(format!(
                    "relay_url exceeds 2048 bytes (got {})",
                    url.len()
                )));
            }
        }
        if let Some(name) = petname {
            if name.len() > 256 {
                return Err(SdkError::InvalidInput(format!(
                    "petname exceeds 256 bytes (got {})",
                    name.len()
                )));
            }
        }
        let lower = pubkey_hex.to_ascii_lowercase();
        if !seen.insert(lower.clone()) {
            continue;
        }
        tags.push(tag(&[
            "p",
            &lower,
            relay_url.unwrap_or(""),
            petname.unwrap_or(""),
        ])?);
    }
    Ok(EventBuilder::new(Kind::Custom(3), "").tags(tags))
}

/// Extract the channel UUID from an event's `h` tag.
///
/// Returns `None` if no `h` tag is present or the value is not a valid UUID.
pub fn extract_channel_id(event: &nostr::Event) -> Option<Uuid> {
    event.tags.iter().find_map(|t| {
        let vec = t.as_slice();
        if vec.first().map(|s| s.as_str()) == Some("h") {
            vec.get(1).and_then(|v| Uuid::parse_str(v.as_str()).ok())
        } else {
            None
        }
    })
}

/// Build a git repository announcement event (kind:30617, NIP-34).
///
/// Creates or updates a repository. The `repo_id` is the unique identifier
/// (d-tag) — must be `[a-zA-Z0-9._-]{1,64}`, no leading dots, no `..`.
///
/// This is a parameterized replaceable event: publishing again with the same
/// `repo_id` updates the announcement (relay overwrites the previous one).
pub fn build_repo_announcement(
    repo_id: &str,
    name: Option<&str>,
    description: Option<&str>,
    clone_urls: &[&str],
    web_url: Option<&str>,
    relays: &[&str],
) -> Result<EventBuilder, SdkError> {
    // Validate repo_id
    check_repo_id(repo_id)?;

    // Validate optional name
    if let Some(n) = name {
        if n.len() > 128 {
            return Err(SdkError::InvalidInput(format!(
                "name exceeds 128 characters (got {})",
                n.len()
            )));
        }
    }

    // Validate optional description
    if let Some(d) = description {
        if d.len() > 1024 {
            return Err(SdkError::InvalidInput(format!(
                "description exceeds 1024 characters (got {})",
                d.len()
            )));
        }
    }

    // Validate clone_urls
    if clone_urls.len() > 5 {
        return Err(SdkError::InvalidInput(format!(
            "too many clone_urls (max 5, got {})",
            clone_urls.len()
        )));
    }
    for url in clone_urls {
        if url.is_empty() {
            return Err(SdkError::InvalidInput("clone_url must not be empty".into()));
        }
        if url.len() > 512 {
            return Err(SdkError::InvalidInput(format!(
                "clone_url exceeds 512 characters (got {})",
                url.len()
            )));
        }
    }

    // Validate web_url
    if let Some(url) = web_url {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return Err(SdkError::InvalidInput(format!(
                "web_url must start with http:// or https:// (got {:?})",
                url
            )));
        }
        if url.len() > 512 {
            return Err(SdkError::InvalidInput(format!(
                "web_url exceeds 512 characters (got {})",
                url.len()
            )));
        }
    }

    // Validate relays
    if relays.len() > 10 {
        return Err(SdkError::InvalidInput(format!(
            "too many relays (max 10, got {})",
            relays.len()
        )));
    }
    for relay in relays {
        if !relay.starts_with("ws://") && !relay.starts_with("wss://") {
            return Err(SdkError::InvalidInput(format!(
                "relay must start with ws:// or wss:// (got {:?})",
                relay
            )));
        }
        if relay.len() > 256 {
            return Err(SdkError::InvalidInput(format!(
                "relay exceeds 256 characters (got {})",
                relay.len()
            )));
        }
    }

    // Build tags
    let mut tags = vec![tag(&["d", repo_id])?];
    if let Some(n) = name {
        tags.push(tag(&["name", n])?);
    }
    if let Some(d) = description {
        tags.push(tag(&["description", d])?);
    }
    if !clone_urls.is_empty() {
        let mut clone_tag = vec!["clone"];
        clone_tag.extend_from_slice(clone_urls);
        tags.push(tag(&clone_tag)?);
    }
    if let Some(url) = web_url {
        tags.push(tag(&["web", url])?);
    }
    if !relays.is_empty() {
        let mut relay_tag = vec!["relays"];
        relay_tag.extend_from_slice(relays);
        tags.push(tag(&relay_tag)?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_REPO_ANNOUNCEMENT as u16), "").tags(tags))
}

/// Build a repository announcement while preserving caller-supplied metadata.
///
/// This is intended for read-modify-write updates to kind:30617 announcements.
/// Every supplied tag is retained except `d`; all existing `d` tags are replaced
/// with exactly one validated canonical repository identifier.
pub fn build_repo_announcement_with_tags(
    repo_id: &str,
    content: &str,
    mut tags: Vec<Tag>,
) -> Result<EventBuilder, SdkError> {
    check_repo_id(repo_id)?;
    tags.retain(|tag| tag.as_slice().first().map(String::as_str) != Some("d"));
    tags.insert(0, tag(&["d", repo_id])?);

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_REPO_ANNOUNCEMENT as u16), content).tags(tags))
}

/// Repository coordinate — owner pubkey + `d`-tag identifier.
///
/// Renders as the `a`-tag value clients use to address a kind:30617
/// announcement: `30617:<owner>:<id>`.
pub struct GitRepoCoord {
    /// 64-char hex pubkey of the repo's announcing owner.
    pub owner: String,
    /// The repo's `d`-tag identifier.
    pub id: String,
}

impl GitRepoCoord {
    fn to_a_tag_value(&self) -> Result<String, SdkError> {
        let owner = check_pubkey_hex(&self.owner, "repo owner")?;
        check_repo_id(&self.id)?;
        Ok(format!("30617:{owner}:{}", self.id))
    }
}

/// Metadata for a git patch event (kind:1617, NIP-34).
#[derive(Default)]
pub struct GitPatchMeta {
    /// Earliest-unique-commit of the repo (`r` tag, `euc` marker).
    pub euc: Option<String>,
    /// Additional pubkeys to `p`-tag besides the repo owner.
    pub recipients: Vec<String>,
    /// Previous patch in a series, or the original root patch when this is
    /// the first patch of a revision — emits `["e", id, "", "reply"]`.
    pub reply_to: Option<String>,
    /// First patch in a new series — emits `["t", "root"]`.
    pub root: bool,
    /// First patch in a revision of an existing series — emits `["t", "root-revision"]`.
    pub root_revision: bool,
    /// Commit ID this patch produces when applied (`commit` tag + `r` tag).
    pub commit: Option<String>,
    /// Parent commit ID (`parent-commit` tag).
    pub parent_commit: Option<String>,
    /// PGP signature of the commit, or `Some("")` for an explicitly unsigned commit.
    pub commit_pgp_sig: Option<String>,
    /// Committer identity: `(name, email, unix-timestamp, tz-offset-minutes)`.
    pub committer: Option<(String, String, String, String)>,
}

/// Build a git patch event (kind:1617, NIP-34).
///
/// `content` is the verbatim output of `git format-patch` — not truncated.
/// NIP-34 says patches SHOULD be used when under 60KB (PRs otherwise); this
/// builder enforces that bound rather than silently truncating a patch that
/// must remain applyable.
pub fn build_git_patch(
    repo: &GitRepoCoord,
    content: &str,
    meta: &GitPatchMeta,
) -> Result<EventBuilder, SdkError> {
    if content.trim().is_empty() {
        return Err(SdkError::InvalidInput(
            "patch content must not be empty — refusing to publish an unappliable patch".into(),
        ));
    }
    check_content(content, 60 * 1024)?;
    let a_value = repo.to_a_tag_value()?;
    let owner = check_pubkey_hex(&repo.owner, "repo owner")?;

    let mut tags = vec![tag(&["a", &a_value])?];
    if let Some(ref euc) = meta.euc {
        check_commit_hex(euc, "euc")?;
        tags.push(tag(&["r", euc, "euc"])?);
    }
    tags.push(tag(&["p", &owner])?);
    for recipient in &meta.recipients {
        let pk = check_pubkey_hex(recipient, "recipient")?;
        tags.push(tag(&["p", &pk])?);
    }
    if let Some(ref prev) = meta.reply_to {
        let event_id = check_hex_exact(prev, 64, "reply_to")?;
        tags.push(tag(&["e", &event_id, "", "reply"])?);
    }
    if meta.root && meta.root_revision {
        return Err(SdkError::InvalidInput(
            "patch cannot be both --root and --root-revision".into(),
        ));
    }
    if meta.root {
        tags.push(tag(&["t", "root"])?);
    }
    if meta.root_revision {
        tags.push(tag(&["t", "root-revision"])?);
    }
    if let Some(ref commit) = meta.commit {
        check_commit_hex(commit, "commit")?;
        tags.push(tag(&["commit", commit])?);
        tags.push(tag(&["r", commit])?);
    }
    if let Some(ref parent) = meta.parent_commit {
        check_commit_hex(parent, "parent_commit")?;
        tags.push(tag(&["parent-commit", parent])?);
    }
    if let Some(ref sig) = meta.commit_pgp_sig {
        tags.push(tag(&["commit-pgp-sig", sig])?);
    }
    if let Some((ref name, ref email, ref ts, ref tz)) = meta.committer {
        tags.push(tag(&["committer", name, email, ts, tz])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_PATCH as u16), content).tags(tags))
}

/// Metadata for a git issue event (kind:1621, NIP-34).
#[derive(Default)]
pub struct GitIssueMeta {
    /// Labels (`t` tags).
    pub labels: Vec<String>,
    /// Additional pubkeys to `p`-tag besides the repo owner.
    pub recipients: Vec<String>,
}

/// Build a git issue event (kind:1621, NIP-34). `content` is the markdown body.
pub fn build_git_issue(
    repo: &GitRepoCoord,
    subject: &str,
    content: &str,
    meta: &GitIssueMeta,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    if subject.is_empty() {
        return Err(SdkError::InvalidInput("subject must not be empty".into()));
    }
    if subject.len() > 256 {
        return Err(SdkError::InvalidInput(format!(
            "subject exceeds 256 characters (got {})",
            subject.len()
        )));
    }
    let a_value = repo.to_a_tag_value()?;
    let owner = check_pubkey_hex(&repo.owner, "repo owner")?;

    let mut tags = vec![tag(&["a", &a_value])?, tag(&["p", &owner])?];
    for recipient in &meta.recipients {
        let pk = check_pubkey_hex(recipient, "recipient")?;
        tags.push(tag(&["p", &pk])?);
    }
    tags.push(tag(&["subject", subject])?);
    for label in &meta.labels {
        tags.push(tag(&["t", label])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_ISSUE as u16), content).tags(tags))
}

/// Status to apply to a patch or issue root (kind:1630/1631/1632/1633, NIP-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitStatus {
    /// 1630 — Open (the default state).
    Open,
    /// 1631 — Applied/Merged for patches; Resolved for issues.
    AppliedOrResolved,
    /// 1632 — Closed.
    Closed,
    /// 1633 — Draft.
    Draft,
}

/// A reference to an applied/merged patch event for a status `q` tag,
/// optionally carrying a relay-url and/or pubkey hint per NIP-34:
/// `['q', <id>, <relay-url>, <pubkey>]`.
///
/// Parsed from the CLI's `--q <id>[:<relay-url>[:<pubkey>]]` syntax via
/// [`GitAppliedPatchRef::parse`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitAppliedPatchRef {
    /// The applied/merged patch event id (64-char hex).
    pub id: String,
    /// Optional relay-url hint where the patch event can be found.
    pub relay: Option<String>,
    /// Optional pubkey hint of the patch's author. Only meaningful when
    /// `relay` is also set, per NIP-34's `['q', id, relay, pubkey]` shape.
    pub pubkey: Option<String>,
}

impl GitAppliedPatchRef {
    /// Parse `<id>`, `<id>:<relay-url>`, or `<id>:<relay-url>:<pubkey>`.
    ///
    /// The relay-url segment may itself contain `:` (e.g. `wss://host:port`),
    /// so splitting is bounded to at most 3 parts rather than splitting on
    /// every colon.
    pub fn parse(spec: &str) -> Result<Self, SdkError> {
        let mut parts = spec.splitn(3, ':');
        let id = parts.next().unwrap_or_default().to_string();
        let rest = parts.next();
        match rest {
            None => Ok(GitAppliedPatchRef {
                id,
                relay: None,
                pubkey: None,
            }),
            Some(_) => {
                // Re-split with the relay url glued back together, since a
                // relay URL itself contains colons (`wss://host:port`); the
                // pubkey, if present, is always the last `:`-delimited
                // segment.
                let rest_str = &spec[id.len() + 1..];
                if let Some(idx) = rest_str.rfind(':') {
                    let candidate_pubkey = &rest_str[idx + 1..];
                    if candidate_pubkey.len() == 64
                        && candidate_pubkey.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        return Ok(GitAppliedPatchRef {
                            id,
                            relay: Some(rest_str[..idx].to_string()),
                            pubkey: Some(candidate_pubkey.to_ascii_lowercase()),
                        });
                    }
                }
                Ok(GitAppliedPatchRef {
                    id,
                    relay: Some(rest_str.to_string()),
                    pubkey: None,
                })
            }
        }
    }
}

impl GitStatus {
    fn kind(self) -> u16 {
        match self {
            GitStatus::Open => KIND_GIT_STATUS_OPEN as u16,
            GitStatus::AppliedOrResolved => KIND_GIT_STATUS_MERGED as u16,
            GitStatus::Closed => KIND_GIT_STATUS_CLOSED as u16,
            GitStatus::Draft => KIND_GIT_STATUS_DRAFT as u16,
        }
    }
}

/// Metadata for a git status event (kind:1630-1633, NIP-34). Applies to a
/// patch root, a patch revision root, an issue, or a PR.
#[derive(Default)]
pub struct GitStatusMeta {
    /// The issue/PR/original-root-patch event being given a status — required.
    pub root_event: String,
    /// When a revision was the one applied/merged, its root id.
    pub accepted_revision_root: Option<String>,
    /// Repo coordinate, included as an `a` tag for subscription efficiency.
    pub repo: Option<GitRepoCoord>,
    /// Earliest-unique-commit of the repo (`r` tag, no marker).
    pub euc: Option<String>,
    /// Additional `p` tags (root author, revision author, etc.) besides the repo owner.
    pub recipients: Vec<String>,
    /// Applied/merged patch event references (`q` tags) — kind:1631 only.
    pub applied_patches: Vec<GitAppliedPatchRef>,
    /// Merge commit id — kind:1631 only.
    pub merge_commit: Option<String>,
    /// Commit ids applied to the target branch — kind:1631 only.
    pub applied_as_commits: Vec<String>,
}

/// Build a git status event (kind:1630/1631/1632/1633, NIP-34).
/// `content` is optional markdown context for the status change.
pub fn build_git_status(
    status: GitStatus,
    content: &str,
    meta: &GitStatusMeta,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let root = check_hex_exact(&meta.root_event, 64, "root_event")?;

    let mut tags = vec![tag(&["e", &root, "", "root"])?];
    if let Some(ref revision) = meta.accepted_revision_root {
        let revision = check_hex_exact(revision, 64, "accepted_revision_root")?;
        tags.push(tag(&["e", &revision, "", "reply"])?);
    }
    for recipient in &meta.recipients {
        let pk = check_pubkey_hex(recipient, "recipient")?;
        tags.push(tag(&["p", &pk])?);
    }
    if let Some(ref repo) = meta.repo {
        let a_value = repo.to_a_tag_value()?;
        tags.push(tag(&["a", &a_value])?);
    }
    if let Some(ref euc) = meta.euc {
        check_commit_hex(euc, "euc")?;
        tags.push(tag(&["r", euc])?);
    }

    if status != GitStatus::AppliedOrResolved
        && (!meta.applied_patches.is_empty()
            || meta.merge_commit.is_some()
            || !meta.applied_as_commits.is_empty())
    {
        return Err(SdkError::InvalidInput(
            "applied_patches/merge_commit/applied_as_commits only apply to the merged/resolved status".into(),
        ));
    }
    for patch_ref in &meta.applied_patches {
        let patch_id = check_hex_exact(&patch_ref.id, 64, "applied_patch")?;
        match (&patch_ref.relay, &patch_ref.pubkey) {
            (None, None) => tags.push(tag(&["q", &patch_id])?),
            (Some(relay), None) => tags.push(tag(&["q", &patch_id, relay])?),
            (Some(relay), Some(pubkey)) => {
                let pubkey = check_pubkey_hex(pubkey, "applied_patch pubkey")?;
                tags.push(tag(&["q", &patch_id, relay, &pubkey])?)
            }
            (None, Some(_)) => {
                return Err(SdkError::InvalidInput(
                    "applied_patch pubkey hint requires a relay-url hint".into(),
                ))
            }
        }
    }
    if let Some(ref merge_commit) = meta.merge_commit {
        check_commit_hex(merge_commit, "merge_commit")?;
        tags.push(tag(&["merge-commit", merge_commit])?);
        tags.push(tag(&["r", merge_commit])?);
    }
    if !meta.applied_as_commits.is_empty() {
        let mut commits_tag = vec!["applied-as-commits"];
        for commit in &meta.applied_as_commits {
            check_commit_hex(commit, "applied_as_commit")?;
        }
        commits_tag.extend(meta.applied_as_commits.iter().map(String::as_str));
        tags.push(tag(&commits_tag)?);
        for commit in &meta.applied_as_commits {
            tags.push(tag(&["r", commit])?);
        }
    }

    Ok(EventBuilder::new(Kind::Custom(status.kind()), content).tags(tags))
}

/// Metadata for a git pull-request event (kind:1618, NIP-34).
///
/// A PR points reviewers at a branch tip they can fetch — `commit` (the tip)
/// plus at least one `clone_urls` entry where that commit is reachable. Unlike
/// a [`build_git_patch`], the change is *not* inlined; the diff is whatever the
/// tip introduces over its merge base. Per NIP-34 the tip SHOULD already be
/// pushed to `refs/nostr/<pr-event-id>` (or otherwise reachable) in the clone
/// repos before the event is signed — this builder does no network work and
/// does not verify reachability, mirroring [`build_git_patch`]'s philosophy.
#[derive(Default)]
pub struct GitPullRequestMeta {
    /// Earliest-unique-commit of the repo (`r` tag) — lets clients subscribe
    /// to all PRs against a local repo.
    pub euc: Option<String>,
    /// Additional pubkeys to `p`-tag besides the repo owner.
    pub recipients: Vec<String>,
    /// NIP-29 channel where the pull request originated (`h` tag).
    pub channel_id: Option<String>,
    /// PR subject line (`subject` tag) — required, used as the header.
    pub subject: String,
    /// Labels (`t` tags).
    pub labels: Vec<String>,
    /// Tip commit id of the PR branch (`c` tag) — required.
    pub commit: String,
    /// Clone URL(s) where the tip can be fetched (`clone` tag) — at least one.
    pub clone_urls: Vec<String>,
    /// Recommended branch name (`branch-name` tag).
    pub branch_name: Option<String>,
    /// Most recent common ancestor with the target branch (`merge-base` tag).
    pub merge_base: Option<String>,
    /// Root patch event this PR revises, which should then be closed
    /// (`e` tag) — optional.
    pub revision_of: Option<String>,
}

/// Build a git pull-request event (kind:1618, NIP-34). `content` is the
/// markdown PR description.
pub fn build_git_pull_request(
    repo: &GitRepoCoord,
    content: &str,
    meta: &GitPullRequestMeta,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    if meta.subject.is_empty() {
        return Err(SdkError::InvalidInput("subject must not be empty".into()));
    }
    if meta.subject.len() > 256 {
        return Err(SdkError::InvalidInput(format!(
            "subject exceeds 256 characters (got {})",
            meta.subject.len()
        )));
    }
    check_commit_hex(&meta.commit, "commit")?;
    if meta.clone_urls.is_empty() {
        return Err(SdkError::InvalidInput(
            "a pull request needs at least one --clone url where the tip commit can be fetched"
                .into(),
        ));
    }
    let a_value = repo.to_a_tag_value()?;
    let owner = check_pubkey_hex(&repo.owner, "repo owner")?;

    let mut tags = vec![tag(&["a", &a_value])?];
    if let Some(ref euc) = meta.euc {
        check_commit_hex(euc, "euc")?;
        tags.push(tag(&["r", euc])?);
    }
    tags.push(tag(&["p", &owner])?);
    for recipient in &meta.recipients {
        let pk = check_pubkey_hex(recipient, "recipient")?;
        tags.push(tag(&["p", &pk])?);
    }
    tags.push(tag(&["subject", &meta.subject])?);
    for label in &meta.labels {
        tags.push(tag(&["t", label])?);
    }
    tags.push(tag(&["c", &meta.commit])?);
    if let Some(ref channel_id) = meta.channel_id {
        let channel_id = Uuid::parse_str(channel_id)
            .map_err(|e| SdkError::InvalidInput(format!("channel_id must be a valid UUID: {e}")))?;
        tags.push(tag(&["h", &channel_id.to_string()])?);
    }
    let mut clone_tag = vec!["clone"];
    clone_tag.extend(meta.clone_urls.iter().map(String::as_str));
    tags.push(tag(&clone_tag)?);
    if let Some(ref branch) = meta.branch_name {
        tags.push(tag(&["branch-name", branch])?);
    }
    if let Some(ref base) = meta.merge_base {
        check_commit_hex(base, "merge_base")?;
        tags.push(tag(&["merge-base", base])?);
    }
    if let Some(ref patch) = meta.revision_of {
        let patch_id = check_hex_exact(patch, 64, "revision_of")?;
        tags.push(tag(&["e", &patch_id])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_PULL_REQUEST as u16), content).tags(tags))
}

/// Metadata for a git pull-request update event (kind:1619, NIP-34). A PR
/// update changes the tip commit of an existing PR; it references the PR via
/// NIP-22 uppercase root tags (`E`/`P`).
#[derive(Default)]
pub struct GitPrUpdateMeta {
    /// Earliest-unique-commit of the repo (`r` tag).
    pub euc: Option<String>,
    /// Additional pubkeys to `p`-tag besides the repo owner.
    pub recipients: Vec<String>,
    /// The pull-request event being updated (`E` tag, NIP-22 root) — required.
    pub pr_event: String,
    /// The pull-request author (`P` tag, NIP-22 root) — required.
    pub pr_author: String,
    /// Updated tip commit id (`c` tag) — required.
    pub commit: String,
    /// Clone URL(s) where the new tip can be fetched (`clone` tag) — at least one.
    pub clone_urls: Vec<String>,
    /// Most recent common ancestor with the target branch (`merge-base` tag).
    pub merge_base: Option<String>,
}

/// Build a git pull-request update event (kind:1619, NIP-34). `content` is
/// optional markdown context for the update.
pub fn build_git_pr_update(
    repo: &GitRepoCoord,
    content: &str,
    meta: &GitPrUpdateMeta,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let pr_event = check_hex_exact(&meta.pr_event, 64, "pr_event")?;
    let pr_author = check_pubkey_hex(&meta.pr_author, "pr_author")?;
    check_commit_hex(&meta.commit, "commit")?;
    if meta.clone_urls.is_empty() {
        return Err(SdkError::InvalidInput(
            "a pull request update needs at least one --clone url where the tip commit can be fetched"
                .into(),
        ));
    }
    let a_value = repo.to_a_tag_value()?;
    let owner = check_pubkey_hex(&repo.owner, "repo owner")?;

    let mut tags = vec![tag(&["a", &a_value])?];
    if let Some(ref euc) = meta.euc {
        check_commit_hex(euc, "euc")?;
        tags.push(tag(&["r", euc])?);
    }
    tags.push(tag(&["p", &owner])?);
    for recipient in &meta.recipients {
        let pk = check_pubkey_hex(recipient, "recipient")?;
        tags.push(tag(&["p", &pk])?);
    }
    tags.push(tag(&["E", &pr_event])?);
    tags.push(tag(&["P", &pr_author])?);
    tags.push(tag(&["c", &meta.commit])?);
    let mut clone_tag = vec!["clone"];
    clone_tag.extend(meta.clone_urls.iter().map(String::as_str));
    tags.push(tag(&clone_tag)?);
    if let Some(ref base) = meta.merge_base {
        check_commit_hex(base, "merge_base")?;
        tags.push(tag(&["merge-base", base])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_GIT_PR_UPDATE as u16), content).tags(tags))
}

/// Build a workflow definition event (kind 30620).
///
/// - `channel_id`: the channel this workflow belongs to (h-tag)
/// - `workflow_id`: unique workflow UUID (d-tag)
/// - `yaml`: workflow YAML definition as content
pub fn build_workflow_def(
    channel_id: Uuid,
    workflow_id: Uuid,
    yaml: &str,
) -> Result<EventBuilder, SdkError> {
    check_content(yaml, 64 * 1024)?;
    let tags = vec![
        tag(&["d", &workflow_id.to_string()])?,
        tag(&["h", &channel_id.to_string()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_WORKFLOW_DEF as u16), yaml).tags(tags))
}

/// Build a workflow update event (kind 30620) for an existing workflow.
///
/// Updates an existing workflow definition in-place via the parameterized
/// replaceable event mechanism — same d-tag overwrites the previous version.
/// The h-tag (channel scope) is required by the relay for authorization.
pub fn build_workflow_update(
    channel_id: Uuid,
    workflow_id: Uuid,
    yaml: &str,
) -> Result<EventBuilder, SdkError> {
    check_content(yaml, 64 * 1024)?;
    let tags = vec![
        tag(&["d", &workflow_id.to_string()])?,
        tag(&["h", &channel_id.to_string()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_WORKFLOW_DEF as u16), yaml).tags(tags))
}

/// Build a NIP-09 deletion event targeting a workflow definition (kind 5).
///
/// The `a`-tag addresses the parameterized replaceable event
/// `<KIND_WORKFLOW_DEF>:<pubkey>:<workflow_id>`.
pub fn build_workflow_delete(
    author_pubkey: &str,
    workflow_id: Uuid,
) -> Result<EventBuilder, SdkError> {
    let pk = check_pubkey_hex(author_pubkey, "author_pubkey")?;
    let tags = vec![tag(&[
        "a",
        &format!("{}:{pk}:{workflow_id}", KIND_WORKFLOW_DEF),
    ])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_DELETION as u16), "").tags(tags))
}

/// Build a workflow trigger event (kind 46020).
pub fn build_workflow_trigger(workflow_id: Uuid) -> Result<EventBuilder, SdkError> {
    let tags = vec![tag(&["d", &workflow_id.to_string()])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_WORKFLOW_TRIGGER as u16), "").tags(tags))
}

/// Build a workflow approval event — kind 46030 (grant) or 46031 (deny).
///
/// - `token_hash`: hex-encoded SHA-256 of the approval token UUID (d-tag).
///   Must be exactly 64 hex characters.
/// - `approved`: `true` emits kind 46030 (grant), `false` emits kind 46031 (deny)
/// - `note`: optional human-readable note as event content
pub fn build_workflow_approval(
    token_hash: &str,
    approved: bool,
    note: &str,
) -> Result<EventBuilder, SdkError> {
    if token_hash.len() != 64 || !token_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidInput(
            "token_hash must be a 64-character hex SHA-256 digest".into(),
        ));
    }
    let kind = if approved {
        KIND_APPROVAL_GRANT
    } else {
        KIND_APPROVAL_DENY
    };
    let tags = vec![tag(&["d", token_hash])?];
    Ok(EventBuilder::new(Kind::Custom(kind as u16), note).tags(tags))
}

/// Build a DM open event (kind 41010).
///
/// `pubkeys` must be 1–8 hex-encoded pubkeys to include in the DM conversation.
pub fn build_dm_open(pubkeys: &[&str]) -> Result<EventBuilder, SdkError> {
    if pubkeys.is_empty() || pubkeys.len() > 8 {
        return Err(SdkError::InvalidInput(
            "dm open requires 1-8 pubkeys".into(),
        ));
    }
    let mut tags = Vec::with_capacity(pubkeys.len());
    for pk in pubkeys {
        let validated = check_pubkey_hex(pk, "pubkey")?;
        tags.push(tag(&["p", &validated])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_DM_OPEN as u16), "").tags(tags))
}

/// Build a DM add-member event (kind 41011).
pub fn build_dm_add_member(channel_id: Uuid, pubkey: &str) -> Result<EventBuilder, SdkError> {
    let pk = check_pubkey_hex(pubkey, "pubkey")?;
    let tags = vec![tag(&["h", &channel_id.to_string()])?, tag(&["p", &pk])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_DM_ADD_MEMBER as u16), "").tags(tags))
}

/// Build a Meeting V0 create command (kind 42100).
///
/// `participant_pubkeys` contains every participant except the event author.
/// The relay inserts the author into the complete frozen roster and validates
/// all identities against the current community.
pub fn build_meeting_create(
    session_id: Uuid,
    title: &str,
    description: Option<&str>,
    source_channel_id: Option<Uuid>,
    participant_pubkeys: &[&str],
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    let title = buzz_core::channel::canonical_channel_name(title);
    if title.trim().is_empty() {
        return Err(SdkError::InvalidInput("meeting title is required".into()));
    }
    if title.chars().count() > 255 {
        return Err(SdkError::InvalidInput(
            "meeting title exceeds 255 characters".into(),
        ));
    }
    if participant_pubkeys.is_empty() || participant_pubkeys.len() > 11 {
        return Err(SdkError::InvalidInput(
            "meeting create requires 1-11 other participant pubkeys".into(),
        ));
    }

    let mut tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["name", title])?,
        tag(&["v", "1"])?,
    ];
    if let Some(description) = description {
        tags.push(tag(&["about", description])?);
    }
    if let Some(source_channel_id) = source_channel_id {
        if source_channel_id.is_nil() {
            return Err(SdkError::InvalidInput(
                "source channel id must not be nil".into(),
            ));
        }
        tags.push(tag(&["source", &source_channel_id.to_string()])?);
    }

    let mut seen = std::collections::HashSet::with_capacity(participant_pubkeys.len());
    for pubkey in participant_pubkeys {
        let normalized = check_pubkey_hex(pubkey, "participant pubkey")?;
        if !seen.insert(normalized.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate meeting participant: {normalized}"
            )));
        }
        tags.push(tag(&["p", &normalized])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_CREATE as u16), "").tags(tags))
}

/// Build a Meeting V0 end command (kind 42101).
pub fn build_meeting_end(
    session_id: Uuid,
    create_event_id: &str,
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    let create_event_id = check_hex_exact(create_event_id, 64, "meeting create event id")?;
    let tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["e", &create_event_id])?,
        tag(&["reason", "manual"])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), "").tags(tags))
}

/// Build a strict Meeting V1 create command (kind 42100).
///
/// The returned event always carries `v=2`, `policy=moderated-baton-v1`, and
/// an explicit moderator. V0 callers must continue using
/// [`build_meeting_create`].
pub fn build_meeting_v1_create(
    params: MeetingV1CreateParams<'_>,
) -> Result<EventBuilder, SdkError> {
    if params.session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    let title = buzz_core::channel::canonical_channel_name(params.title);
    if title.is_empty() {
        return Err(SdkError::InvalidInput("meeting title is required".into()));
    }
    if title.chars().count() > 255 {
        return Err(SdkError::InvalidInput(
            "meeting title exceeds 255 characters".into(),
        ));
    }
    if params.participant_pubkeys.is_empty() || params.participant_pubkeys.len() > 11 {
        return Err(SdkError::InvalidInput(
            "meeting create requires 1-11 other participant pubkeys".into(),
        ));
    }
    if params.source_channel_id == Some(params.session_id) {
        return Err(SdkError::InvalidInput(
            "meeting cannot use itself as its source channel".into(),
        ));
    }

    let author_pubkey = check_pubkey_hex(params.author_pubkey, "meeting author pubkey")?;
    let moderator_pubkey = check_pubkey_hex(params.moderator_pubkey, "meeting moderator pubkey")?;
    let mut seen = std::collections::HashSet::with_capacity(params.participant_pubkeys.len() + 1);
    seen.insert(author_pubkey.clone());

    let mut participant_pubkeys = Vec::with_capacity(params.participant_pubkeys.len());
    for pubkey in params.participant_pubkeys {
        let normalized = check_pubkey_hex(pubkey, "participant pubkey")?;
        if !seen.insert(normalized.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate meeting participant: {normalized}"
            )));
        }
        participant_pubkeys.push(normalized);
    }
    if !seen.contains(&moderator_pubkey) {
        return Err(SdkError::InvalidInput(
            "meeting moderator must belong to the complete frozen roster".into(),
        ));
    }

    let mut tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["name", title])?,
        tag(&["v", MEETING_V1_SCHEMA_VERSION])?,
        tag(&["policy", MEETING_V1_POLICY])?,
        tag(&["moderator", &moderator_pubkey])?,
    ];
    if let Some(description) = params.description {
        tags.push(tag(&["about", description])?);
    }
    if let Some(source_channel_id) = params.source_channel_id {
        if source_channel_id.is_nil() {
            return Err(SdkError::InvalidInput(
                "source channel id must not be nil".into(),
            ));
        }
        tags.push(tag(&["source", &source_channel_id.to_string()])?);
    }
    for participant_pubkey in &participant_pubkeys {
        tags.push(tag(&["p", participant_pubkey])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_CREATE as u16), "").tags(tags))
}

/// Parse and strictly validate a Meeting V2 board JSON envelope.
pub fn parse_meeting_v2_board_content(content: &str) -> Result<MeetingV2BoardContent, SdkError> {
    let board: MeetingV2BoardContent = serde_json::from_str(content).map_err(|error| {
        SdkError::InvalidInput(format!("invalid Meeting V2 board JSON: {error}"))
    })?;
    validate_meeting_v2_board_content(&board)?;
    Ok(board)
}

/// Build a strict Meeting V2 create command (kind 42100).
///
/// The returned event always carries `v=3` and
/// `policy=moderated-board-v1`. Its signer becomes the immutable owner and
/// moderator; V2 deliberately has no `moderator` tag.
pub fn build_meeting_v2_create(
    params: MeetingV2CreateParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_create_for_policy(params, MEETING_V2_POLICY)
}

/// Build an action-capable Meeting V2 create command (kind 42100).
///
/// The Board and roster semantics are identical to [`build_meeting_v2_create`],
/// but the persisted policy is independently discriminated so old V2 sessions
/// retain their original close behavior.
pub fn build_meeting_v2_actions_create(
    params: MeetingV2CreateParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_create_for_policy(params, MEETING_V2_ACTIONS_POLICY)
}

fn build_meeting_v2_create_for_policy(
    params: MeetingV2CreateParams<'_>,
    policy: &str,
) -> Result<EventBuilder, SdkError> {
    if params.session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    let title = buzz_core::channel::canonical_channel_name(params.title);
    if title.is_empty() {
        return Err(SdkError::InvalidInput("meeting title is required".into()));
    }
    if title.chars().count() > 255 {
        return Err(SdkError::InvalidInput(
            "meeting title exceeds 255 characters".into(),
        ));
    }
    if params.participant_pubkeys.is_empty() || params.participant_pubkeys.len() > 11 {
        return Err(SdkError::InvalidInput(
            "meeting create requires 1-11 other participant pubkeys".into(),
        ));
    }
    if params.source_channel_id == Some(params.session_id) {
        return Err(SdkError::InvalidInput(
            "meeting cannot use itself as its source channel".into(),
        ));
    }

    let author_pubkey = check_pubkey_hex(params.author_pubkey, "meeting author pubkey")?;
    let mut seen = std::collections::HashSet::with_capacity(params.participant_pubkeys.len() + 1);
    seen.insert(author_pubkey);
    let mut participant_pubkeys = Vec::with_capacity(params.participant_pubkeys.len());
    for pubkey in params.participant_pubkeys {
        let normalized = check_pubkey_hex(pubkey, "participant pubkey")?;
        if !seen.insert(normalized.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate meeting participant: {normalized}"
            )));
        }
        participant_pubkeys.push(normalized);
    }

    let board = MeetingV2BoardContent {
        format: MEETING_V2_BOARD_FORMAT.to_string(),
        body: params.initial_board.to_string(),
    };
    validate_meeting_v2_board_content(&board)?;
    let content = serde_json::to_string(&board)
        .map_err(|error| SdkError::InvalidInput(format!("serialize Meeting V2 board: {error}")))?;

    let mut tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["name", title])?,
        tag(&["v", MEETING_V2_SCHEMA_VERSION])?,
        tag(&["policy", policy])?,
    ];
    if let Some(description) = params.description {
        tags.push(tag(&["about", description])?);
    }
    if let Some(source_channel_id) = params.source_channel_id {
        if source_channel_id.is_nil() {
            return Err(SdkError::InvalidInput(
                "source channel id must not be nil".into(),
            ));
        }
        tags.push(tag(&["source", &source_channel_id.to_string()])?);
    }
    for participant_pubkey in &participant_pubkeys {
        tags.push(tag(&["p", participant_pubkey])?);
    }

    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_CREATE as u16), content).tags(tags))
}

/// Validate an already-decoded Meeting V2 board envelope.
pub fn validate_meeting_v2_board_content(board: &MeetingV2BoardContent) -> Result<(), SdkError> {
    if board.format != MEETING_V2_BOARD_FORMAT {
        return Err(SdkError::InvalidInput(format!(
            "Meeting V2 board format must be {MEETING_V2_BOARD_FORMAT}"
        )));
    }
    if board.body.trim().is_empty() {
        return Err(SdkError::InvalidInput(
            "Meeting V2 board body is required".into(),
        ));
    }
    if board.body.contains('\0') {
        return Err(SdkError::InvalidInput(
            "Meeting V2 board body must not contain NUL".into(),
        ));
    }
    if board.body.len() > MAX_MEETING_V2_BOARD_BYTES {
        return Err(SdkError::ContentTooLarge {
            max: MAX_MEETING_V2_BOARD_BYTES,
            got: board.body.len(),
        });
    }
    Ok(())
}

/// Build a strict Meeting V2 Board update or unchanged command (kind 42111).
pub fn build_meeting_v2_board_action(
    params: MeetingV2BoardActionParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_board_action_for_policy(params, MEETING_V2_POLICY)
}

/// Build an action-capable Meeting V2 Board update or unchanged command.
pub fn build_meeting_v2_actions_board_action(
    params: MeetingV2BoardActionParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_board_action_for_policy(params, MEETING_V2_ACTIONS_POLICY)
}

fn build_meeting_v2_board_action_for_policy(
    params: MeetingV2BoardActionParams<'_>,
    policy: &str,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_session(params.session_id)?;
    if params.expected_control_epoch == 0 || params.board_window == 0 {
        return Err(SdkError::InvalidInput(
            "Meeting V2 Board fencing values must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.expected_control_epoch, "expected control epoch")?;
    check_meeting_v1_i64(params.board_window, "Board window")?;
    let (action, content) = if let Some(body) = params.board {
        let board = MeetingV2BoardContent {
            format: MEETING_V2_BOARD_FORMAT.to_string(),
            body: body.to_string(),
        };
        validate_meeting_v2_board_content(&board)?;
        let content = serde_json::to_string(&board).map_err(|error| {
            SdkError::InvalidInput(format!("serialize Meeting V2 board update: {error}"))
        })?;
        ("update", content)
    } else {
        ("unchanged", String::new())
    };
    let tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["v", MEETING_V2_SCHEMA_VERSION])?,
        tag(&["policy", policy])?,
        tag(&["action", action])?,
        tag(&[
            "expected-control-epoch",
            &params.expected_control_epoch.to_string(),
        ])?,
        tag(&["board-window", &params.board_window.to_string()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_BOARD_COMMAND as u16), content).tags(tags))
}

/// Build a strict Meeting V2 normal close or abnormal abort (kind 42101).
pub fn build_meeting_v2_end(params: MeetingV2EndParams<'_>) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_end_for_policy(
        params.session_id,
        params.create_event_id,
        params.outcome,
        params.reason_code,
        params.reason,
        MEETING_V2_POLICY,
        None,
    )
}

/// Build a strict action-capable Meeting V2 normal close or abnormal abort.
pub fn build_meeting_v2_actions_end(
    params: MeetingV2ActionsEndParams<'_>,
) -> Result<EventBuilder, SdkError> {
    if params.outcome == MeetingV2EndOutcome::Aborted && params.action_fence.is_some() {
        return Err(SdkError::InvalidInput(
            "Meeting V2 abort must not carry an action completion fence".into(),
        ));
    }
    build_meeting_v2_end_for_policy(
        params.session_id,
        params.create_event_id,
        params.outcome,
        params.reason_code,
        params.reason,
        MEETING_V2_ACTIONS_POLICY,
        params.action_fence,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_meeting_v2_end_for_policy(
    session_id: Uuid,
    create_event_id: &str,
    outcome: MeetingV2EndOutcome,
    reason_code: Option<&str>,
    reason: Option<&str>,
    policy: &str,
    action_fence: Option<MeetingV2ActionsEndFence<'_>>,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_session(session_id)?;
    let create_event_id = check_hex_exact(create_event_id, 64, "meeting create event id")?;
    let reason = check_meeting_v1_optional_text(reason, "Meeting V2 abort reason", 1024)?;
    let mut tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["v", MEETING_V2_SCHEMA_VERSION])?,
        tag(&["policy", policy])?,
        tag(&["e", &create_event_id])?,
        tag(&["outcome", outcome.as_str()])?,
    ];
    match outcome {
        MeetingV2EndOutcome::Closed => {
            if reason_code.is_some() || reason.is_some() {
                return Err(SdkError::InvalidInput(
                    "Meeting V2 close cannot carry an abort reason".into(),
                ));
            }
            if let Some(fence) = action_fence {
                check_meeting_v1_session(fence.action_run_id)?;
                if fence.action_window == 0 {
                    return Err(SdkError::InvalidInput(
                        "Meeting V2 action window must be positive".into(),
                    ));
                }
                check_meeting_v1_i64(fence.action_window, "action window")?;
                let board_event_id =
                    check_hex_exact(fence.board_event_id, 64, "Meeting V2 final Board event id")?;
                tags.push(tag(&["action-run", &fence.action_run_id.to_string()])?);
                tags.push(tag(&["action-window", &fence.action_window.to_string()])?);
                tags.push(tag(&["board", &board_event_id])?);
                tags.push(tag(&["attestation", "actions-recorded"])?);
            }
        }
        MeetingV2EndOutcome::Aborted => {
            let reason_code = reason_code.ok_or_else(|| {
                SdkError::InvalidInput("Meeting V2 abort requires a reason code".into())
            })?;
            let reason_code =
                check_meeting_v1_text(reason_code, "Meeting V2 abort reason code", 128)?;
            tags.push(tag(&["reason-code", reason_code])?);
        }
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), reason.unwrap_or("")).tags(tags))
}

fn meeting_v2_action_run_tags(
    session_id: Uuid,
    action: &str,
    fence: MeetingV2ActionRunFence<'_>,
) -> Result<Vec<Tag>, SdkError> {
    check_meeting_v1_session(session_id)?;
    check_meeting_v1_session(fence.action_run_id)?;
    if fence.action_window == 0 {
        return Err(SdkError::InvalidInput(
            "Meeting V2 action window must be positive".into(),
        ));
    }
    check_meeting_v1_i64(fence.action_window, "Meeting V2 action window")?;
    let board_event_id =
        check_hex_exact(fence.board_event_id, 64, "Meeting V2 final Board event id")?;
    Ok(vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["v", MEETING_V2_SCHEMA_VERSION])?,
        tag(&["policy", MEETING_V2_ACTIONS_POLICY])?,
        tag(&["action", action])?,
        tag(&["action-run", &fence.action_run_id.to_string()])?,
        tag(&["action-window", &fence.action_window.to_string()])?,
        tag(&["board", &board_event_id])?,
    ])
}

/// Build a fenced Meeting retrieval-summary metadata command (kind 42113).
pub fn build_meeting_summary_update(
    params: MeetingSummaryUpdateParams<'_>,
) -> Result<EventBuilder, SdkError> {
    let (action, content) = match params.mutation {
        MeetingSummaryMutation::Set(summary) => {
            if summary.trim().is_empty() {
                return Err(SdkError::InvalidInput(
                    "Meeting summary must not be empty".into(),
                ));
            }
            if summary.contains('\0') {
                return Err(SdkError::InvalidInput(
                    "Meeting summary must not contain NUL".into(),
                ));
            }
            ("set", summary)
        }
        MeetingSummaryMutation::Clear => ("clear", ""),
    };
    let tags = meeting_v2_action_run_tags(params.session_id, action, params.action_fence)?;
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_SUMMARY_COMMAND as u16), content).tags(tags))
}

/// Build the `FINALIZE_ACTIONS`/`begin` command for an action-capable Meeting V2.
pub fn build_meeting_v2_action_begin(
    params: MeetingV2ActionBeginParams<'_>,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_session(params.session_id)?;
    if params.expected_control_epoch == 0 || params.board_window == 0 {
        return Err(SdkError::InvalidInput(
            "Meeting V2 action begin fencing values must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.expected_control_epoch, "expected control epoch")?;
    check_meeting_v1_i64(params.board_window, "Board window")?;
    let state_event_id = check_hex_exact(
        params.expected_state_event_id,
        64,
        "Meeting V2 expected State event id",
    )?;
    let board_event_id =
        check_hex_exact(params.board_event_id, 64, "Meeting V2 final Board event id")?;
    let mut tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["v", MEETING_V2_SCHEMA_VERSION])?,
        tag(&["policy", MEETING_V2_ACTIONS_POLICY])?,
        tag(&["action", "begin"])?,
        tag(&[
            "expected-control-epoch",
            &params.expected_control_epoch.to_string(),
        ])?,
        tag(&["board-window", &params.board_window.to_string()])?,
        tag(&["expected-state", &state_event_id])?,
        tag(&["board", &board_event_id])?,
    ];
    if let Some(attempt_id) = params.expected_decision_attempt_id {
        let attempt_id =
            check_hex_exact(attempt_id, 64, "Meeting V2 moderator decision attempt id")?;
        tags.push(tag(&["decision-attempt", &attempt_id])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_ACTION_COMMAND as u16), "").tags(tags))
}

/// Build a durable `block` command for an active Meeting V2 action run.
pub fn build_meeting_v2_action_block(
    params: MeetingV2ActionBlockParams<'_>,
) -> Result<EventBuilder, SdkError> {
    let reason_code = check_meeting_v1_text(
        params.reason_code,
        "Meeting V2 action block reason code",
        128,
    )?;
    if !matches!(
        reason_code,
        "external_operation_failed"
            | "external_state_conflict"
            | "tool_unavailable"
            | "provider_failure"
            | "action_deadline_exceeded"
            | "action_lease_expired"
            | "action_operator_deadline_exceeded"
    ) {
        return Err(SdkError::InvalidInput(format!(
            "unsupported Meeting V2 action block reason code: {reason_code}"
        )));
    }
    let reason =
        check_meeting_v1_optional_text(params.reason, "Meeting V2 action block reason", 1_024)?;
    let mut tags = meeting_v2_action_run_tags(params.session_id, "block", params.fence)?;
    tags.push(tag(&["reason-code", reason_code])?);
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_ACTION_COMMAND as u16),
        reason.unwrap_or(""),
    )
    .tags(tags))
}

fn build_meeting_v2_empty_action_command(
    params: MeetingV2ActionCommandParams<'_>,
    action: &str,
) -> Result<EventBuilder, SdkError> {
    let tags = meeting_v2_action_run_tags(params.session_id, action, params.fence)?;
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_ACTION_COMMAND as u16), "").tags(tags))
}

/// Build a `retry` command for a blocked Meeting V2 action run.
pub fn build_meeting_v2_action_retry(
    params: MeetingV2ActionCommandParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_v2_empty_action_command(params, "retry")
}

/// Build an internal host `renew` command for a runnable Action window.
///
/// This builder intentionally has no `buzz-cli` subcommand: a renewal is a
/// cooperative host-liveness signal, not a model-authored business action.
pub fn build_meeting_v2_action_lease_renew(
    params: MeetingV2ActionLeaseRenewParams<'_>,
) -> Result<EventBuilder, SdkError> {
    if params.progress_seq == 0 {
        return Err(SdkError::InvalidInput(
            "Meeting V2 action progress sequence must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.progress_seq, "Meeting V2 action progress sequence")?;
    check_meeting_v1_i64(
        params.last_activity_seq,
        "Meeting V2 action activity sequence",
    )?;
    let mut tags = meeting_v2_action_run_tags(params.session_id, "renew", params.fence)?;
    tags.push(tag(&["progress-seq", &params.progress_seq.to_string()])?);
    tags.push(tag(&["stage", params.stage.as_str()])?);
    tags.push(tag(&[
        "last-activity-seq",
        &params.last_activity_seq.to_string(),
    ])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_ACTION_COMMAND as u16), "").tags(tags))
}

/// Build a `return-to-board` command that preserves any external effects.
pub fn build_meeting_v2_action_return_to_board(
    params: MeetingV2ActionCommandParams<'_>,
) -> Result<EventBuilder, SdkError> {
    let mut tags = meeting_v2_action_run_tags(params.session_id, "return-to-board", params.fence)?;
    tags.push(tag(&["external-effects", "preserved"])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_ACTION_COMMAND as u16), "").tags(tags))
}

/// Build a strict manual Meeting V1 end command (kind 42101).
///
/// The reason is fixed to `manual`; Relay-authored security termination uses a
/// separate internal builder because it also carries the revoked participant.
pub fn build_meeting_v1_end(params: MeetingV1EndParams<'_>) -> Result<EventBuilder, SdkError> {
    if params.session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    let create_event_id = check_hex_exact(params.create_event_id, 64, "meeting create event id")?;
    let tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["v", MEETING_V1_SCHEMA_VERSION])?,
        tag(&["policy", MEETING_V1_POLICY])?,
        tag(&["e", &create_event_id])?,
        tag(&["reason", "manual"])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), "").tags(tags))
}

/// Build a Meeting V1 speech-intent Submit command (kind 42105).
pub fn build_meeting_v1_intent_submit(
    params: MeetingV1IntentSubmitParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_submit(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 speech-intent Submit command (kind 42105).
pub fn build_meeting_v2_intent_submit(
    params: MeetingV1IntentSubmitParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_submit(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_intent_submit(
    params: MeetingV1IntentSubmitParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_i64(params.basis_speech_revision, "basis speech revision")?;
    let summary = check_meeting_v1_text(params.summary, "meeting intent summary", 512)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "submit")?;
    tags.push(tag(&[
        "basis-speech-revision",
        &params.basis_speech_revision.to_string(),
    ])?);
    if let Some(addressed_to) = params.addressed_to {
        let addressed_to = check_pubkey_hex(addressed_to, "addressed-to pubkey")?;
        tags.push(tag(&["addressed-to", &addressed_to])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16), summary).tags(tags))
}

/// Build a Meeting V1 speech-intent Refresh command (kind 42105).
pub fn build_meeting_v1_intent_refresh(
    params: MeetingV1IntentRefreshParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_refresh(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 speech-intent Refresh command (kind 42105).
pub fn build_meeting_v2_intent_refresh(
    params: MeetingV1IntentRefreshParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_refresh(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_intent_refresh(
    params: MeetingV1IntentRefreshParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_i64(params.basis_speech_revision, "basis speech revision")?;
    let intent_id = check_hex_exact(params.intent_id, 64, "meeting intent id")?;
    let previous_event_id =
        check_hex_exact(params.previous_event_id, 64, "previous intent event id")?;
    let summary = check_meeting_v1_text(params.summary, "meeting intent summary", 512)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "refresh")?;
    tags.push(tag(&["intent", &intent_id])?);
    tags.push(tag(&["prev", &previous_event_id])?);
    tags.push(tag(&[
        "basis-speech-revision",
        &params.basis_speech_revision.to_string(),
    ])?);
    if let Some(addressed_to) = params.addressed_to {
        let addressed_to = check_pubkey_hex(addressed_to, "addressed-to pubkey")?;
        tags.push(tag(&["addressed-to", &addressed_to])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16), summary).tags(tags))
}

/// Build a Meeting V1 speech-intent Withdraw command (kind 42105).
pub fn build_meeting_v1_intent_withdraw(
    params: MeetingV1IntentWithdrawParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_withdraw(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 speech-intent Withdraw command (kind 42105).
pub fn build_meeting_v2_intent_withdraw(
    params: MeetingV1IntentWithdrawParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_intent_withdraw(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_intent_withdraw(
    params: MeetingV1IntentWithdrawParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let intent_id = check_hex_exact(params.intent_id, 64, "meeting intent id")?;
    let previous_event_id =
        check_hex_exact(params.previous_event_id, 64, "previous intent event id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "withdraw")?;
    tags.push(tag(&["intent", &intent_id])?);
    tags.push(tag(&["prev", &previous_event_id])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator Select command (kind 42106).
pub fn build_meeting_v1_moderator_select(
    params: MeetingV1ModeratorSelectParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_select(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator Select command (kind 42106).
pub fn build_meeting_v2_moderator_select(
    params: MeetingV1ModeratorSelectParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_select(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_moderator_select(
    params: MeetingV1ModeratorSelectParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.expected_control_epoch == 0 {
        return Err(SdkError::InvalidInput(
            "expected control epoch must be positive".into(),
        ));
    }
    for (value, field) in [
        (params.expected_control_epoch, "expected control epoch"),
        (params.expected_decision_epoch, "expected decision epoch"),
        (params.expected_intent_revision, "expected intent revision"),
        (params.expected_speech_revision, "expected speech revision"),
    ] {
        check_meeting_v1_i64(value, field)?;
    }
    let selection_reason =
        check_meeting_v1_optional_text(params.selection_reason, "selection reason", 512)?;
    if params.deferrals.len() > 12 {
        return Err(SdkError::InvalidInput(
            "meeting Select cannot defer more than 12 intents".into(),
        ));
    }
    let mut tags = meeting_command_tags(params.session_id, schema_version, "select")?;
    let selected_intent_id = match params.selection {
        MeetingV1Selection::Intent { intent_id } => {
            let intent_id = check_hex_exact(intent_id, 64, "meeting intent id")?;
            tags.push(tag(&["intent", &intent_id])?);
            Some(intent_id)
        }
        MeetingV1Selection::Handoff {
            handoff_id,
            expected_attempt_count,
        } => {
            check_meeting_v1_i32(expected_attempt_count, "expected handoff attempt count")?;
            if !params.deferrals.is_empty() {
                return Err(SdkError::InvalidInput(
                    "handoff selection cannot carry intent deferrals".into(),
                ));
            }
            let handoff_id = check_hex_exact(handoff_id, 64, "meeting handoff id")?;
            tags.push(tag(&["handoff", &handoff_id])?);
            tags.push(tag(&[
                "expected-handoff-attempt-count",
                &expected_attempt_count.to_string(),
            ])?);
            None
        }
    };
    tags.push(tag(&[
        "expected-control-epoch",
        &params.expected_control_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-decision-epoch",
        &params.expected_decision_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-intent-revision",
        &params.expected_intent_revision.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-speech-revision",
        &params.expected_speech_revision.to_string(),
    ])?);
    match (params.attempt_id, params.expected_source_event_id) {
        (Some(attempt_id), expected_source_event_id) => {
            let attempt_id = check_hex_exact(attempt_id, 64, "moderator decision attempt id")?;
            tags.push(tag(&["decision-attempt", &attempt_id])?);
            match (selected_intent_id.is_some(), expected_source_event_id) {
                (true, Some(source_event_id)) => {
                    let source_event_id =
                        check_hex_exact(source_event_id, 64, "selected Intent snapshot event id")?;
                    tags.push(tag(&["expected-source-event", &source_event_id])?);
                }
                (true, None) => {
                    return Err(SdkError::InvalidInput(
                        "attempt-bound Intent selection requires its snapshot event id".into(),
                    ));
                }
                (false, Some(_)) => {
                    return Err(SdkError::InvalidInput(
                        "Handoff selection cannot carry an Intent source event id".into(),
                    ));
                }
                (false, None) => {}
            }
        }
        (None, None) => {}
        (None, Some(_)) => {
            return Err(SdkError::InvalidInput(
                "source event binding requires a moderator decision attempt".into(),
            ));
        }
    }

    let mut seen = std::collections::HashSet::with_capacity(params.deferrals.len());
    let mut deferrals = Vec::with_capacity(params.deferrals.len());
    for deferral in params.deferrals {
        let intent_id = check_hex_exact(deferral.intent_id, 64, "deferred intent id")?;
        if selected_intent_id.as_deref() == Some(intent_id.as_str()) {
            return Err(SdkError::InvalidInput(
                "selected intent cannot defer itself".into(),
            ));
        }
        if !seen.insert(intent_id.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate deferred intent: {intent_id}"
            )));
        }
        let previous_event_id = check_hex_exact(
            deferral.previous_event_id,
            64,
            "deferred intent prev event id",
        )?;
        let reason = check_meeting_v1_text(deferral.reason, "deferral reason", 1024)?;
        deferrals.push(serde_json::json!({
            "intent_id": intent_id,
            "prev": previous_event_id,
            "reason": reason,
        }));
    }
    let content = serde_json::to_string(&serde_json::json!({
        "selection_reason": selection_reason,
        "deferrals": deferrals,
    }))
    .map_err(|error| SdkError::InvalidInput(format!("invalid moderator selection: {error}")))?;
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), content).tags(tags))
}

/// Build a Meeting V1 moderator Reject command (kind 42106).
pub fn build_meeting_v1_moderator_reject(
    params: MeetingV1ModeratorRejectParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_reject(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator Reject command (kind 42106).
pub fn build_meeting_v2_moderator_reject(
    params: MeetingV1ModeratorRejectParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_reject(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_moderator_reject(
    params: MeetingV1ModeratorRejectParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let intent_id = check_hex_exact(params.intent_id, 64, "meeting intent id")?;
    let previous_event_id =
        check_hex_exact(params.previous_event_id, 64, "previous intent event id")?;
    let author = check_pubkey_hex(params.intent_author_pubkey, "intent author pubkey")?;
    let reason_text = check_meeting_v1_text(params.reason_text, "intent rejection reason", 1024)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "reject")?;
    tags.push(tag(&["intent", &intent_id])?);
    tags.push(tag(&["prev", &previous_event_id])?);
    tags.push(tag(&["reason-code", params.reason_code.as_str()])?);
    tags.push(tag(&["p", &author])?);
    if let Some(attempt_id) = params.attempt_id {
        let attempt_id = check_hex_exact(attempt_id, 64, "moderator decision attempt id")?;
        tags.push(tag(&["decision-attempt", &attempt_id])?);
    }
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16),
        reason_text,
    )
    .tags(tags))
}

/// Build a Meeting V1 moderator Dismiss Handoff command (kind 42106).
pub fn build_meeting_v1_moderator_dismiss_handoff(
    params: MeetingV1ModeratorDismissHandoffParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_dismiss_handoff(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator Dismiss Handoff command (kind 42106).
pub fn build_meeting_v2_moderator_dismiss_handoff(
    params: MeetingV1ModeratorDismissHandoffParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_dismiss_handoff(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_moderator_dismiss_handoff(
    params: MeetingV1ModeratorDismissHandoffParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_i64(params.expected_speech_revision, "expected speech revision")?;
    check_meeting_v1_i32(
        params.expected_attempt_count,
        "expected handoff attempt count",
    )?;
    let handoff_id = check_hex_exact(params.handoff_id, 64, "meeting handoff id")?;
    let reason_text = check_meeting_v1_text(params.reason_text, "handoff dismiss reason", 1024)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "dismiss-handoff")?;
    tags.push(tag(&["handoff", &handoff_id])?);
    tags.push(tag(&[
        "expected-speech-revision",
        &params.expected_speech_revision.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-handoff-attempt-count",
        &params.expected_attempt_count.to_string(),
    ])?);
    tags.push(tag(&["reason-code", params.reason_code.as_str()])?);
    if let Some(attempt_id) = params.attempt_id {
        let attempt_id = check_hex_exact(attempt_id, 64, "moderator decision attempt id")?;
        tags.push(tag(&["decision-attempt", &attempt_id])?);
    }
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16),
        reason_text,
    )
    .tags(tags))
}

/// Build a Meeting V1 moderator DecisionAttempt Start command (kind 42106).
pub fn build_meeting_v1_decision_attempt_start(
    params: MeetingV1DecisionAttemptStartParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_start(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator DecisionAttempt Start command (kind 42106).
pub fn build_meeting_v2_decision_attempt_start(
    params: MeetingV1DecisionAttemptStartParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_start(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_decision_attempt_start(
    params: MeetingV1DecisionAttemptStartParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.expected_control_epoch == 0 {
        return Err(SdkError::InvalidInput(
            "expected control epoch must be positive".into(),
        ));
    }
    for (value, field) in [
        (params.expected_control_epoch, "expected control epoch"),
        (params.expected_decision_epoch, "expected decision epoch"),
        (params.expected_intent_revision, "expected intent revision"),
        (params.expected_speech_revision, "expected speech revision"),
    ] {
        check_meeting_v1_i64(value, field)?;
    }
    let state_event_id = check_hex_exact(
        params.expected_state_event_id,
        64,
        "expected Relay State event id",
    )?;
    let mut tags =
        meeting_command_tags(params.session_id, schema_version, "decision-attempt-start")?;
    tags.push(tag(&[
        "expected-control-epoch",
        &params.expected_control_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-decision-epoch",
        &params.expected_decision_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-intent-revision",
        &params.expected_intent_revision.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-speech-revision",
        &params.expected_speech_revision.to_string(),
    ])?);
    tags.push(tag(&["expected-state", &state_event_id])?);
    if let Some(replacement_id) = params.replacement_of_attempt_id {
        let replacement_id =
            check_hex_exact(replacement_id, 64, "replaced moderator decision attempt id")?;
        tags.push(tag(&["replacement-attempt", &replacement_id])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator DecisionAttempt Finish command (kind 42106).
pub fn build_meeting_v1_decision_attempt_finish(
    params: MeetingV1DecisionAttemptFinishParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_finish(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator DecisionAttempt Finish command (kind 42106).
pub fn build_meeting_v2_decision_attempt_finish(
    params: MeetingV1DecisionAttemptFinishParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_finish(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_decision_attempt_finish(
    params: MeetingV1DecisionAttemptFinishParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let attempt_id = check_hex_exact(params.attempt_id, 64, "moderator decision attempt id")?;
    let reason_code = check_meeting_v1_text(params.reason_code, "attempt finish reason code", 128)?;
    let supported = match params.outcome {
        MeetingV1DecisionAttemptFinishOutcome::Completed => {
            matches!(reason_code, "no_action" | "idle_wait_fallback")
        }
        MeetingV1DecisionAttemptFinishOutcome::Discarded => matches!(
            reason_code,
            "human_priority"
                | "control_changed"
                | "speech_changed"
                | "meeting_ended"
                | "moderator_changed"
                | "cas_churn"
                | "source_changed"
                | "runtime_replaced"
                | "invalid_output"
                | "provider_failure"
        ),
    };
    if !supported {
        return Err(SdkError::InvalidInput(
            "unsupported moderator DecisionAttempt finish reason".into(),
        ));
    }
    let mut tags =
        meeting_command_tags(params.session_id, schema_version, "decision-attempt-finish")?;
    tags.push(tag(&["decision-attempt", &attempt_id])?);
    tags.push(tag(&["outcome", params.outcome.as_str()])?);
    tags.push(tag(&["reason-code", reason_code])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator DecisionRetry command (kind 42106).
pub fn build_meeting_v1_decision_retry(
    params: MeetingV1DecisionRetryParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_retry(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator DecisionRetry command (kind 42106).
pub fn build_meeting_v2_decision_retry(
    params: MeetingV1DecisionRetryParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_retry(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_decision_retry(
    params: MeetingV1DecisionRetryParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.expected_control_epoch == 0
        || params.expected_decision_epoch == 0
        || params.expected_attempt_number == 0
    {
        return Err(SdkError::InvalidInput(
            "DecisionRetry expectations must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.expected_control_epoch, "expected control epoch")?;
    check_meeting_v1_i64(params.expected_decision_epoch, "expected decision epoch")?;
    check_meeting_v1_i32(params.expected_attempt_number, "expected attempt number")?;
    let attempt_id = check_hex_exact(params.attempt_id, 64, "moderator decision attempt id")?;
    let retry_ticket_id = check_hex_exact(params.retry_ticket_id, 64, "moderator retry ticket id")?;
    let failed_action_event_id = check_hex_exact(
        params.failed_action_event_id,
        64,
        "failed moderator action event id",
    )?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "decision-retry")?;
    tags.push(tag(&["decision-attempt", &attempt_id])?);
    tags.push(tag(&["retry-ticket", &retry_ticket_id])?);
    tags.push(tag(&["failed-action", &failed_action_event_id])?);
    tags.push(tag(&[
        "expected-control-epoch",
        &params.expected_control_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-decision-epoch",
        &params.expected_decision_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-attempt-number",
        &params.expected_attempt_number.to_string(),
    ])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator CompleteCohort command (kind 42106).
pub fn build_meeting_v1_complete_cohort(
    params: MeetingV1CompleteCohortParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_complete_cohort(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator CompleteCohort command (kind 42106).
pub fn build_meeting_v2_complete_cohort(
    params: MeetingV1CompleteCohortParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_complete_cohort(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_complete_cohort(
    params: MeetingV1CompleteCohortParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.expected_control_epoch == 0 || params.expected_decision_epoch == 0 {
        return Err(SdkError::InvalidInput(
            "CompleteCohort epochs must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.expected_control_epoch, "expected control epoch")?;
    check_meeting_v1_i64(params.expected_decision_epoch, "expected decision epoch")?;
    let attempt_id = check_hex_exact(params.attempt_id, 64, "moderator decision attempt id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "complete-cohort")?;
    tags.push(tag(&["decision-attempt", &attempt_id])?);
    tags.push(tag(&[
        "expected-control-epoch",
        &params.expected_control_epoch.to_string(),
    ])?);
    tags.push(tag(&[
        "expected-decision-epoch",
        &params.expected_decision_epoch.to_string(),
    ])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator DecisionAttempt Abandon command (kind 42106).
pub fn build_meeting_v1_decision_attempt_abandon(
    params: MeetingV1DecisionAttemptAbandonParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_abandon(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator DecisionAttempt Abandon command (kind 42106).
pub fn build_meeting_v2_decision_attempt_abandon(
    params: MeetingV1DecisionAttemptAbandonParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_decision_attempt_abandon(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_decision_attempt_abandon(
    params: MeetingV1DecisionAttemptAbandonParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let attempt_id = check_hex_exact(params.attempt_id, 64, "moderator decision attempt id")?;
    let mut tags = meeting_command_tags(
        params.session_id,
        schema_version,
        "decision-attempt-abandon",
    )?;
    tags.push(tag(&["decision-attempt", &attempt_id])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build an attempt-bound Meeting V1 moderator self-Intent withdrawal (kind 42106).
pub fn build_meeting_v1_moderator_withdraw_self(
    params: MeetingV1ModeratorWithdrawSelfParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_withdraw_self(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build an attempt-bound Meeting V2 moderator self-Intent withdrawal.
pub fn build_meeting_v2_moderator_withdraw_self(
    params: MeetingV1ModeratorWithdrawSelfParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_withdraw_self(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_moderator_withdraw_self(
    params: MeetingV1ModeratorWithdrawSelfParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let attempt_id = check_hex_exact(params.attempt_id, 64, "moderator decision attempt id")?;
    let intent_id = check_hex_exact(params.intent_id, 64, "meeting intent id")?;
    let previous_event_id =
        check_hex_exact(params.previous_event_id, 64, "previous intent event id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "withdraw-self")?;
    tags.push(tag(&["decision-attempt", &attempt_id])?);
    tags.push(tag(&["intent", &intent_id])?);
    tags.push(tag(&["prev", &previous_event_id])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16), "").tags(tags))
}

/// Build a Meeting V1 moderator Recall command (kind 42106).
pub fn build_meeting_v1_moderator_recall(
    params: MeetingV1ModeratorRecallParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_recall(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 moderator Recall command (kind 42106).
pub fn build_meeting_v2_moderator_recall(
    params: MeetingV1ModeratorRecallParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_moderator_recall(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_moderator_recall(
    params: MeetingV1ModeratorRecallParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.control_epoch == 0 {
        return Err(SdkError::InvalidInput(
            "meeting control epoch must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.control_epoch, "meeting control epoch")?;
    let reason = check_meeting_v1_optional_text(params.reason, "recall reason", 1024)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "recall")?;
    tags.push(tag(&["control-epoch", &params.control_epoch.to_string()])?);
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16),
        reason.unwrap_or(""),
    )
    .tags(tags))
}

/// Build a Meeting V1 Human Floor Request command (kind 42107).
pub fn build_meeting_v1_human_floor_request(
    params: MeetingV1HumanFloorRequestParams,
) -> Result<EventBuilder, SdkError> {
    build_meeting_human_floor_request(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Human Floor Request command (kind 42107).
pub fn build_meeting_v2_human_floor_request(
    params: MeetingV1HumanFloorRequestParams,
) -> Result<EventBuilder, SdkError> {
    build_meeting_human_floor_request(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_human_floor_request(
    params: MeetingV1HumanFloorRequestParams,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let tags = meeting_command_tags(params.session_id, schema_version, "request")?;
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_HUMAN_FLOOR_REQUEST as u16), "").tags(tags))
}

/// Build a Meeting V1 Human Floor Request Withdraw command (kind 42107).
pub fn build_meeting_v1_human_floor_withdraw(
    params: MeetingV1HumanFloorWithdrawParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_human_floor_withdraw(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Human Floor Request Withdraw command (kind 42107).
pub fn build_meeting_v2_human_floor_withdraw(
    params: MeetingV1HumanFloorWithdrawParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_human_floor_withdraw(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_human_floor_withdraw(
    params: MeetingV1HumanFloorWithdrawParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let request_id = check_hex_exact(params.request_id, 64, "human floor request id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "withdraw")?;
    tags.push(tag(&["request", &request_id])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_HUMAN_FLOOR_REQUEST as u16), "").tags(tags))
}

/// Build a Meeting V1 Offer ACK command (kind 42108).
pub fn build_meeting_v1_offer_ack(
    params: MeetingV1OfferAckParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_offer_ack(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Offer ACK command (kind 42108).
pub fn build_meeting_v2_offer_ack(
    params: MeetingV1OfferAckParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_offer_ack(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_offer_ack(
    params: MeetingV1OfferAckParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let offer_id = check_hex_exact(params.offer_id, 64, "meeting offer id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "ack")?;
    tags.push(tag(&["meeting-offer", &offer_id])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_OFFER_RESPONSE as u16), "").tags(tags))
}

/// Build a Meeting V1 Offer Decline command (kind 42108).
pub fn build_meeting_v1_offer_decline(
    params: MeetingV1OfferDeclineParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_offer_decline(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Offer Decline command (kind 42108).
pub fn build_meeting_v2_offer_decline(
    params: MeetingV1OfferDeclineParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_offer_decline(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_offer_decline(
    params: MeetingV1OfferDeclineParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let offer_id = check_hex_exact(params.offer_id, 64, "meeting offer id")?;
    let reason = check_meeting_v1_optional_text(params.reason, "offer decline reason", 512)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "decline")?;
    tags.push(tag(&["meeting-offer", &offer_id])?);
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_OFFER_RESPONSE as u16),
        reason.unwrap_or(""),
    )
    .tags(tags))
}

/// Build a Meeting V1 Grant Progress signal (kind 42109).
pub fn build_meeting_v1_grant_progress(
    params: MeetingV1GrantProgressParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_grant_progress(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Grant Progress signal (kind 42109).
pub fn build_meeting_v2_grant_progress(
    params: MeetingV1GrantProgressParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_grant_progress(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_grant_progress(
    params: MeetingV1GrantProgressParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    if params.progress_seq == 0 {
        return Err(SdkError::InvalidInput(
            "meeting Grant progress sequence must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.progress_seq, "meeting Grant progress sequence")?;
    let grant_id = check_hex_exact(params.grant_id, 64, "meeting grant id")?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "progress")?;
    tags.push(tag(&["meeting-grant", &grant_id])?);
    tags.push(tag(&["progress-seq", &params.progress_seq.to_string()])?);
    tags.push(tag(&["stage", params.stage.as_str()])?);
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_GRANT_SIGNAL as u16), "").tags(tags))
}

/// Build a Meeting V1 Grant Yield signal (kind 42109).
pub fn build_meeting_v1_grant_yield(
    params: MeetingV1GrantYieldParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_grant_yield(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Meeting V2 Grant Yield signal (kind 42109).
pub fn build_meeting_v2_grant_yield(
    params: MeetingV1GrantYieldParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_grant_yield(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_grant_yield(
    params: MeetingV1GrantYieldParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    let grant_id = check_hex_exact(params.grant_id, 64, "meeting grant id")?;
    let reason = check_meeting_v1_optional_text(params.reason, "meeting Grant Yield reason", 512)?;
    let mut tags = meeting_command_tags(params.session_id, schema_version, "yield")?;
    tags.push(tag(&["meeting-grant", &grant_id])?);
    if let Some(reason_code) = params.reason_code {
        tags.push(tag(&["reason-code", reason_code.as_str()])?);
    }
    Ok(EventBuilder::new(
        Kind::Custom(KIND_MEETING_GRANT_SIGNAL as u16),
        reason.unwrap_or(""),
    )
    .tags(tags))
}

/// Build a Grant-bound Meeting V1 speech message (kind 9).
pub fn build_meeting_v1_speech(
    params: MeetingV1SpeechParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_baton_speech(params, MEETING_V1_SCHEMA_VERSION)
}

/// Build a Grant-bound Meeting V2 speech message (kind 9).
pub fn build_meeting_v2_speech(
    params: MeetingV1SpeechParams<'_>,
) -> Result<EventBuilder, SdkError> {
    build_meeting_baton_speech(params, MEETING_V2_SCHEMA_VERSION)
}

fn build_meeting_baton_speech(
    params: MeetingV1SpeechParams<'_>,
    schema_version: &str,
) -> Result<EventBuilder, SdkError> {
    check_meeting_v1_session(params.session_id)?;
    if params.speech_revision == 0 {
        return Err(SdkError::InvalidInput(
            "meeting speech revision must be positive".into(),
        ));
    }
    check_meeting_v1_i64(params.speech_revision, "meeting speech revision")?;
    let grant_id = check_hex_exact(params.grant_id, 64, "meeting grant id")?;
    if params.content.trim().is_empty() {
        return Err(SdkError::InvalidInput(
            "meeting speech content is required".into(),
        ));
    }
    check_content(params.content, 256 * 1024)?;
    let mut tags = vec![
        tag(&["h", &params.session_id.to_string()])?,
        tag(&["v", schema_version])?,
        tag(&["meeting-grant", &grant_id])?,
        tag(&["speech-revision", &params.speech_revision.to_string()])?,
    ];
    if params.mentions.len() > 12 {
        return Err(SdkError::InvalidInput(
            "meeting speech cannot mention more than 12 participants".into(),
        ));
    }
    let mut seen_mentions = std::collections::HashSet::with_capacity(params.mentions.len());
    for mention in params.mentions {
        let mention = check_pubkey_hex(mention, "meeting mention")?;
        if !seen_mentions.insert(mention.clone()) {
            return Err(SdkError::InvalidInput(format!(
                "duplicate meeting mention: {mention}"
            )));
        }
        tags.push(tag(&["p", &mention])?);
    }
    if let Some(handoff) = params.handoff {
        let target = check_pubkey_hex(handoff.target_pubkey, "meeting handoff target")?;
        let reason = check_meeting_v1_text(handoff.reason, "meeting handoff reason", 1024)?;
        tags.push(tag(&["handoff-to", &target])?);
        tags.push(tag(&["handoff-type", handoff.handoff_type.as_str()])?);
        tags.push(tag(&["handoff-reason", reason])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), params.content).tags(tags))
}

/// Build a Meeting V0 floor claim (kind 42102).
pub fn build_meeting_floor_claim(
    session_id: Uuid,
    round_number: u64,
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    if round_number == 0 {
        return Err(SdkError::InvalidInput(
            "meeting round must be positive".into(),
        ));
    }
    let tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["meeting-round", &round_number.to_string()])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_FLOOR_CLAIM as u16), "").tags(tags))
}

fn build_meeting_floor_intent_signal(
    session_id: Uuid,
    round_number: u64,
    action: &str,
    intent_basis: &str,
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    if round_number == 0 {
        return Err(SdkError::InvalidInput(
            "meeting round must be positive".into(),
        ));
    }
    let intent_basis = intent_basis.trim();
    if intent_basis.is_empty() {
        return Err(SdkError::InvalidInput(
            "meeting intent basis is required".into(),
        ));
    }
    if intent_basis.len() > 512 || intent_basis.chars().any(char::is_control) {
        return Err(SdkError::InvalidInput(
            "meeting intent basis must be at most 512 bytes without control characters".into(),
        ));
    }
    let tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["meeting-round", &round_number.to_string()])?,
        tag(&["action", action])?,
        tag(&["intent-basis", intent_basis])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_FLOOR_SIGNAL as u16), "").tags(tags))
}

/// Build a Meeting V0 Ready signal (kind 42104).
///
/// Ready declares that the Agent has synchronized the current round and will
/// resolve `intent_basis` to either a Claim or a Pass.
pub fn build_meeting_floor_ready(
    session_id: Uuid,
    round_number: u64,
    intent_basis: &str,
) -> Result<EventBuilder, SdkError> {
    build_meeting_floor_intent_signal(session_id, round_number, "ready", intent_basis)
}

/// Build a Meeting V0 Pass signal (kind 42104).
///
/// Pass contains no model reasoning; `intent_basis` is an opaque idempotency
/// key shared with the preceding Ready signal.
pub fn build_meeting_floor_pass(
    session_id: Uuid,
    round_number: u64,
    intent_basis: &str,
) -> Result<EventBuilder, SdkError> {
    build_meeting_floor_intent_signal(session_id, round_number, "pass", intent_basis)
}

/// Build a Meeting V0 Yield signal (kind 42104).
///
/// Yield may only be accepted from the holder of `grant_event_id`.
pub fn build_meeting_floor_yield(
    session_id: Uuid,
    round_number: u64,
    grant_event_id: &str,
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    if round_number == 0 {
        return Err(SdkError::InvalidInput(
            "meeting round must be positive".into(),
        ));
    }
    let grant_event_id = check_hex_exact(grant_event_id, 64, "meeting grant event id")?;
    let tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["meeting-round", &round_number.to_string()])?,
        tag(&["action", "yield"])?,
        tag(&["meeting-grant", &grant_event_id])?,
    ];
    Ok(EventBuilder::new(Kind::Custom(KIND_MEETING_FLOOR_SIGNAL as u16), "").tags(tags))
}

/// Build a Grant-bound Meeting V0 speech message (kind 9).
pub fn build_meeting_speech(
    session_id: Uuid,
    round_number: u64,
    grant_event_id: &str,
    content: &str,
    mentions: &[&str],
) -> Result<EventBuilder, SdkError> {
    if session_id.is_nil() {
        return Err(SdkError::InvalidInput(
            "meeting session id must not be nil".into(),
        ));
    }
    if round_number == 0 {
        return Err(SdkError::InvalidInput(
            "meeting round must be positive".into(),
        ));
    }
    let grant_event_id = check_hex_exact(grant_event_id, 64, "meeting grant event id")?;
    if content.trim().is_empty() {
        return Err(SdkError::InvalidInput(
            "meeting speech content is required".into(),
        ));
    }
    check_content(content, 256 * 1024)?;
    let mut tags = vec![
        tag(&["h", &session_id.to_string()])?,
        tag(&["meeting-round", &round_number.to_string()])?,
        tag(&["meeting-grant", &grant_event_id])?,
    ];
    let mentions = mentions
        .iter()
        .map(|mention| check_pubkey_hex(mention, "meeting mention"))
        .collect::<Result<Vec<_>, _>>()?;
    let mention_refs = mentions.iter().map(String::as_str).collect::<Vec<_>>();
    mention_tags(&mention_refs, &mut tags)?;
    Ok(EventBuilder::new(Kind::Custom(KIND_STREAM_MESSAGE as u16), content).tags(tags))
}

/// Build a presence update event (kind 20001).
///
/// `status` must be one of: `"online"`, `"away"`, `"offline"`.
/// The status is placed in `event.content` (relay reads it there) and also
/// in a `["status", ...]` tag for structured access.
pub fn build_presence_update(status: &str) -> Result<EventBuilder, SdkError> {
    match status {
        "online" | "away" | "offline" => {}
        _ => {
            return Err(SdkError::InvalidInput(format!(
                "status must be online, away, or offline (got: {status})"
            )))
        }
    }
    let tags = vec![tag(&["status", status])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_PRESENCE_UPDATE as u16), status).tags(tags))
}

// ---------------------------------------------------------------------------
// Community moderation commands (kinds 9040–9044).
//
// These mirror the NIP-43 relay-admin 9030-series: mod-signed command events
// that the relay validates + executes directly and never stores. The tenant
// (community) is bound by the connection host, so no `h` tag is carried — a
// stray `h` would be rejected as channel-scoping a global-only command. The
// tag vocabulary below is pinned by `moderation_commands.rs` (relay and CLI
// must agree).
// ---------------------------------------------------------------------------

/// Build a community ban command (kind 9040).
///
/// `expires_at`: `None` ⇒ permanent; `Some(unix_secs)` ⇒ ban lifts at that time.
pub fn build_moderation_ban(
    target_pubkey: &str,
    expires_at: Option<u64>,
    reason: Option<&str>,
) -> Result<EventBuilder, SdkError> {
    let target_pubkey = check_pubkey_hex(target_pubkey, "target_pubkey")?;
    let mut tags = vec![tag(&["p", &target_pubkey])?];
    if let Some(exp) = expires_at {
        tags.push(tag(&["expiration", &exp.to_string()])?);
    }
    if let Some(r) = reason {
        tags.push(tag(&["reason", r])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MODERATION_BAN as u16), "").tags(tags))
}

/// Build a community unban command (kind 9041).
pub fn build_moderation_unban(target_pubkey: &str) -> Result<EventBuilder, SdkError> {
    let target_pubkey = check_pubkey_hex(target_pubkey, "target_pubkey")?;
    let tags = vec![tag(&["p", &target_pubkey])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_MODERATION_UNBAN as u16), "").tags(tags))
}

/// Build a community timeout (write-block) command (kind 9042).
///
/// `expires_at` (required) is the unix-seconds timestamp the timeout lifts at.
pub fn build_moderation_timeout(
    target_pubkey: &str,
    expires_at: u64,
    reason: Option<&str>,
) -> Result<EventBuilder, SdkError> {
    let target_pubkey = check_pubkey_hex(target_pubkey, "target_pubkey")?;
    let mut tags = vec![
        tag(&["p", &target_pubkey])?,
        tag(&["expiration", &expires_at.to_string()])?,
    ];
    if let Some(r) = reason {
        tags.push(tag(&["reason", r])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MODERATION_TIMEOUT as u16), "").tags(tags))
}

/// Build a community untimeout command (kind 9043).
pub fn build_moderation_untimeout(target_pubkey: &str) -> Result<EventBuilder, SdkError> {
    let target_pubkey = check_pubkey_hex(target_pubkey, "target_pubkey")?;
    let tags = vec![tag(&["p", &target_pubkey])?];
    Ok(EventBuilder::new(Kind::Custom(KIND_MODERATION_UNTIMEOUT as u16), "").tags(tags))
}

/// Build a resolve-report command (kind 9044).
///
/// `report_event_id`: hex event id of the kind:1984 report being resolved.
/// `status`: `resolved` | `dismissed`. `action`: `delete` | `kick` | `ban` |
/// `timeout` | `dismiss` | `escalate` (`dismiss` pairs with `dismissed`,
/// everything else with `resolved` — the relay enforces the pairing).
/// `reason`: optional; audited into `public_reason` and relayed in the
/// reporter notice DM, so it must be safe for the reporter to read.
pub fn build_moderation_resolve_report(
    report_event_id: &str,
    status: &str,
    action: &str,
    reason: Option<&str>,
) -> Result<EventBuilder, SdkError> {
    let report_event_id = check_hex_exact(report_event_id, 64, "report_event_id")?;
    match status {
        "resolved" | "dismissed" => {}
        _ => {
            return Err(SdkError::InvalidInput(format!(
                "status must be resolved or dismissed (got: {status})"
            )))
        }
    }
    match action {
        "delete" | "kick" | "ban" | "timeout" | "dismiss" | "escalate" => {}
        _ => {
            return Err(SdkError::InvalidInput(format!(
                "action must be delete, kick, ban, timeout, dismiss, or escalate (got: {action})"
            )))
        }
    }
    let mut tags = vec![
        tag(&["report", &report_event_id])?,
        tag(&["status", status])?,
        tag(&["action", action])?,
    ];
    if let Some(r) = reason {
        tags.push(tag(&["reason", r])?);
    }
    Ok(EventBuilder::new(Kind::Custom(KIND_MODERATION_RESOLVE_REPORT as u16), "").tags(tags))
}

// ---------------------------------------------------------------------------
// NIP-IA identity archival (kinds 9035/9036).
//
// kind:9035 archive request, kind:9036 unarchive request. Both protected by
// NIP-70 (`["-"]`), p-tag the target, and may carry an optional machine-
// readable `reason` code, a `replaced-by` rotation pointer (9035 only), and a
// NIP-OA `auth` tag for owner-of-agent requests. The relay verifies; this
// builder's job is to produce a well-formed, signed request — the relay
// selects the consent path (self / admin / owner). Mirrors the desktop's
// `identity_archive_tags`/`build_archive_identity_request` (events.rs:624-743)
// so both clients emit the same wire form.
// ---------------------------------------------------------------------------

/// Maximum `reason` length in UTF-8 bytes (not chars — see
/// `desktop/src-tauri/src/events.rs:635-647`, whose `.len()` check is
/// already byte-based despite its error text saying "chars").
const MAX_REASON_BYTES: usize = 64;

fn check_reason(reason: &str) -> Result<(), SdkError> {
    if reason.len() > MAX_REASON_BYTES {
        return Err(SdkError::InvalidInput(format!(
            "reason code exceeds maximum length of {MAX_REASON_BYTES} UTF-8 bytes (got {})",
            reason.len()
        )));
    }
    if reason.chars().any(|c| c.is_control()) {
        return Err(SdkError::InvalidInput(
            "reason code must not contain control characters".into(),
        ));
    }
    Ok(())
}

/// Structural check only — the relay performs full NIP-OA verification.
/// Requires the `auth` label, a 64-hex owner pubkey, and a 128-hex signature.
fn check_auth_tag_shape(auth: &[String; 4]) -> Result<(), SdkError> {
    if auth[0] != "auth" {
        return Err(SdkError::InvalidInput(format!(
            "auth tag label must be \"auth\" (got \"{}\")",
            auth[0]
        )));
    }
    check_pubkey_hex(&auth[1], "auth tag owner pubkey")?;
    if auth[3].len() != 128 || !auth[3].chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(SdkError::InvalidInput(
            "auth tag signature must be 128-character hex".into(),
        ));
    }
    Ok(())
}

fn identity_archive_tags(
    target_pubkey: &str,
    reason: Option<&str>,
    replaced_by: Option<&str>,
    auth: Option<&[String; 4]>,
) -> Result<Vec<Tag>, SdkError> {
    let target_lower = check_pubkey_hex(target_pubkey, "target_pubkey")?;

    // NIP-70: mark as protected administrative state.
    let mut tags = vec![tag(&["-"])?, tag(&["p", &target_lower])?];

    if let Some(r) = reason {
        check_reason(r)?;
        tags.push(tag(&["reason", r])?);
    }

    if let Some(rb) = replaced_by {
        let rb_lower = check_pubkey_hex(rb, "replaced_by")?;
        if rb_lower == target_lower {
            return Err(SdkError::InvalidInput(
                "replaced-by must differ from the target".into(),
            ));
        }
        tags.push(tag(&["replaced-by", &rb_lower])?);
    }

    if let Some(auth_tag) = auth {
        check_auth_tag_shape(auth_tag)?;
        tags.push(tag(&["auth", &auth_tag[1], &auth_tag[2], &auth_tag[3]])?);
    }

    Ok(tags)
}

/// Build a NIP-IA archive request (kind 9035).
///
/// `content` is an optional human-readable reason (clients MUST NOT parse
/// authorization semantics from it; capped at 64 KiB like other content
/// fields). `reason` is a machine-readable code (`rotated`, `retired`,
/// `bot-rebuilt`, `left-organization`, `spam`, ... — unknown codes are
/// allowed per spec), capped at 64 UTF-8 bytes. `replaced_by` is the
/// rotation pointer and must differ from `target_pubkey`. `auth` is a
/// NIP-OA owner-attestation tag required only for the owner-of-agent
/// consent path; full verification happens relay-side.
///
/// `.allow_self_tagging()` is required: NIP-IA's self path has
/// `actor == target`, so the request's `["p", target]` matches the signer.
/// nostr 0.44 strips matching `p` tags by default — this keeps the wire
/// form intact.
pub fn build_archive_identity_request(
    target_pubkey: &str,
    content: &str,
    reason: Option<&str>,
    replaced_by: Option<&str>,
    auth: Option<&[String; 4]>,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let tags = identity_archive_tags(target_pubkey, reason, replaced_by, auth)?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_IA_ARCHIVE_REQUEST as u16), content)
            .tags(tags)
            .allow_self_tagging(),
    )
}

/// Build a NIP-IA unarchive request (kind 9036).
///
/// Same shape as [`build_archive_identity_request`] minus `replaced-by`
/// (which has no defined meaning on unarchive per spec). `auth` is used for
/// owner-of-agent unarchive paths. See that function for the rationale on
/// `.allow_self_tagging()`.
pub fn build_unarchive_identity_request(
    target_pubkey: &str,
    content: &str,
    reason: Option<&str>,
    auth: Option<&[String; 4]>,
) -> Result<EventBuilder, SdkError> {
    check_content(content, 64 * 1024)?;
    let tags = identity_archive_tags(target_pubkey, reason, None, auth)?;
    Ok(
        EventBuilder::new(Kind::Custom(KIND_IA_UNARCHIVE_REQUEST as u16), content)
            .tags(tags)
            .allow_self_tagging(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventId, Keys};

    fn keys() -> Keys {
        Keys::generate()
    }

    fn sign(b: EventBuilder) -> nostr::Event {
        b.sign_with_keys(&keys()).expect("sign")
    }

    fn event_id() -> EventId {
        let k = keys();
        EventBuilder::new(Kind::Custom(1), "x")
            .tags([])
            .sign_with_keys(&k)
            .expect("sign")
            .id
    }

    fn uuid() -> Uuid {
        Uuid::new_v4()
    }

    fn tag_values(event: &nostr::Event, key: &str) -> Vec<String> {
        event
            .tags
            .iter()
            .filter_map(|t| {
                let s = t.as_slice();
                if s.first().map(|v| v.as_str()) == Some(key) {
                    s.get(1).map(|v| v.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    fn has_tag(event: &nostr::Event, key: &str, val: &str) -> bool {
        event.tags.iter().any(|t| {
            let s = t.as_slice();
            s.first().map(|v| v.as_str()) == Some(key) && s.get(1).map(|v| v.as_str()) == Some(val)
        })
    }

    #[test]
    fn message_happy_path() {
        let cid = uuid();
        let ev = sign(build_message(cid, "hello", None, &[], false, &[]).unwrap());
        assert_eq!(ev.kind.as_u16(), 9);
        assert_eq!(ev.content, "hello");
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn agent_observer_frame_happy_path() {
        let sender = keys();
        let recipient = keys();
        let agent = keys();
        let encrypted = buzz_core::observer::encrypt_observer_payload(
            &sender,
            &recipient.public_key(),
            &serde_json::json!({"type": "acp_read"}),
        )
        .unwrap();
        let ev = sign(
            build_agent_observer_frame(
                &recipient.public_key().to_hex(),
                &agent.public_key().to_hex(),
                OBSERVER_FRAME_TELEMETRY,
                &encrypted,
            )
            .unwrap(),
        );

        assert_eq!(ev.kind.as_u16(), KIND_AGENT_OBSERVER_FRAME as u16);
        assert_eq!(ev.content, encrypted);
        assert!(has_tag(&ev, "p", &recipient.public_key().to_hex()));
        assert!(has_tag(
            &ev,
            OBSERVER_AGENT_TAG,
            &agent.public_key().to_hex()
        ));
        assert!(has_tag(&ev, OBSERVER_FRAME_TAG, OBSERVER_FRAME_TELEMETRY));
    }

    #[test]
    fn agent_observer_frame_rejects_plaintext_content() {
        let err = build_agent_observer_frame(
            &"a".repeat(64),
            &"b".repeat(64),
            OBSERVER_FRAME_TELEMETRY,
            "not encrypted",
        )
        .unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn message_direct_reply() {
        let cid = uuid();
        let eid = event_id();
        let tr = ThreadRef {
            root_event_id: eid,
            parent_event_id: eid,
        };
        let ev = sign(build_message(cid, "reply", Some(&tr), &[], false, &[]).unwrap());
        // Direct reply: only one e-tag with "reply" marker
        let e_tags: Vec<_> = ev
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|v| v.as_str()) == Some("e"))
            .collect();
        assert_eq!(e_tags.len(), 1);
        assert_eq!(
            e_tags[0].as_slice().get(3).map(|v| v.as_str()),
            Some("reply")
        );
    }

    #[test]
    fn message_nested_reply() {
        let cid = uuid();
        let root = event_id();
        let parent = event_id();
        let tr = ThreadRef {
            root_event_id: root,
            parent_event_id: parent,
        };
        let ev = sign(build_message(cid, "nested", Some(&tr), &[], false, &[]).unwrap());
        let e_tags: Vec<_> = ev
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|v| v.as_str()) == Some("e"))
            .collect();
        assert_eq!(e_tags.len(), 2);
        let markers: Vec<_> = e_tags
            .iter()
            .filter_map(|t| t.as_slice().get(3).map(|v| v.as_str()))
            .collect();
        assert!(markers.contains(&"root"));
        assert!(markers.contains(&"reply"));
    }

    #[test]
    fn message_broadcast_flag() {
        let cid = uuid();
        let ev = sign(build_message(cid, "hi", None, &[], true, &[]).unwrap());
        assert!(has_tag(&ev, "broadcast", "1"));
    }

    #[test]
    fn message_mentions_deduped() {
        let cid = uuid();
        let hex = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let ev = sign(build_message(cid, "hi", None, &[hex, hex], false, &[]).unwrap());
        let p_tags = tag_values(&ev, "p");
        assert_eq!(p_tags.len(), 1);
    }

    #[test]
    fn message_too_many_mentions() {
        let cid = uuid();
        let hex = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let _mentions: Vec<&str> = (0..51).map(|_| hex).collect();
        // All same hex so dedup would reduce to 1, but the check is on raw len
        // Let's use 51 distinct-ish values by varying the first char
        let hexes: Vec<String> = (0..51u8)
            .map(|i| {
                format!(
                    "{:02x}cd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd12",
                    i
                )
            })
            .collect();
        let refs: Vec<&str> = hexes.iter().map(|s| s.as_str()).collect();
        let result = build_message(cid, "hi", None, &refs, false, &[]);
        assert!(matches!(result, Err(SdkError::TooManyMentions)));
    }

    #[test]
    fn message_content_too_large() {
        let cid = uuid();
        let big = "x".repeat(64 * 1024 + 1);
        let result = build_message(cid, &big, None, &[], false, &[]);
        assert!(matches!(result, Err(SdkError::ContentTooLarge { .. })));
    }

    #[test]
    fn message_max_content_ok() {
        let cid = uuid();
        let max = "x".repeat(64 * 1024);
        assert!(build_message(cid, &max, None, &[], false, &[]).is_ok());
    }

    #[test]
    fn forum_post_happy_path() {
        let cid = uuid();
        let ev = sign(build_forum_post(cid, "post body", &[], &[]).unwrap());
        assert_eq!(ev.kind.as_u16(), 45001);
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn forum_post_content_too_large() {
        let cid = uuid();
        let big = "x".repeat(64 * 1024 + 1);
        assert!(matches!(
            build_forum_post(cid, &big, &[], &[]),
            Err(SdkError::ContentTooLarge { .. })
        ));
    }

    #[test]
    fn forum_comment_happy_path() {
        let cid = uuid();
        let eid = event_id();
        let tr = ThreadRef {
            root_event_id: eid,
            parent_event_id: eid,
        };
        let ev = sign(build_forum_comment(cid, "comment", &tr, &[], &[]).unwrap());
        assert_eq!(ev.kind.as_u16(), 45003);
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    fn good_diff_meta() -> DiffMeta {
        DiffMeta {
            repo_url: "https://github.com/example/repo".into(),
            commit_sha: "abc1234".into(),
            file_path: Some("src/main.rs".into()),
            parent_commit: None,
            branch: None,
            pr_number: None,
            language: Some("rust".into()),
            description: None,
            truncated: false,
            alt_text: None,
        }
    }

    #[test]
    fn diff_message_happy_path() {
        let cid = uuid();
        let ev = sign(build_diff_message(cid, "diff content", &good_diff_meta(), None).unwrap());
        assert_eq!(ev.kind.as_u16(), 40008);
        assert!(has_tag(&ev, "repo", "https://github.com/example/repo"));
        assert!(has_tag(&ev, "commit", "abc1234"));
        assert!(has_tag(&ev, "l", "rust"));
    }

    #[test]
    fn diff_message_bad_repo_url() {
        let cid = uuid();
        let mut meta = good_diff_meta();
        meta.repo_url = "ftp://bad.url".into();
        assert!(matches!(
            build_diff_message(cid, "x", &meta, None),
            Err(SdkError::InvalidDiffMeta(_))
        ));
    }

    #[test]
    fn diff_message_short_commit_sha() {
        let cid = uuid();
        let mut meta = good_diff_meta();
        meta.commit_sha = "abc12".into(); // only 5 chars
        assert!(matches!(
            build_diff_message(cid, "x", &meta, None),
            Err(SdkError::InvalidDiffMeta(_))
        ));
    }

    #[test]
    fn diff_message_invalid_commit_sha_chars() {
        let cid = uuid();
        let mut meta = good_diff_meta();
        meta.commit_sha = "xyz1234".into(); // 'x', 'y', 'z' not hex
        assert!(matches!(
            build_diff_message(cid, "x", &meta, None),
            Err(SdkError::InvalidDiffMeta(_))
        ));
    }

    #[test]
    fn diff_message_branch_only_source() {
        let cid = uuid();
        let mut meta = good_diff_meta();
        meta.branch = Some(("main".into(), "".into())); // target empty
        assert!(matches!(
            build_diff_message(cid, "x", &meta, None),
            Err(SdkError::InvalidDiffMeta(_))
        ));
    }

    #[test]
    fn diff_message_pr_zero() {
        let cid = uuid();
        let mut meta = good_diff_meta();
        meta.pr_number = Some(0);
        assert!(matches!(
            build_diff_message(cid, "x", &meta, None),
            Err(SdkError::InvalidDiffMeta(_))
        ));
    }

    #[test]
    fn diff_message_content_too_large() {
        let cid = uuid();
        let big = "x".repeat(60 * 1024 + 1);
        assert!(matches!(
            build_diff_message(cid, &big, &good_diff_meta(), None),
            Err(SdkError::ContentTooLarge { .. })
        ));
    }

    #[test]
    fn diff_message_all_optional_fields() {
        let cid = uuid();
        let meta = DiffMeta {
            repo_url: "https://github.com/example/repo".into(),
            commit_sha: "abc1234def".into(),
            file_path: Some("src/lib.rs".into()),
            parent_commit: Some("1234567".into()),
            branch: Some(("feature".into(), "main".into())),
            pr_number: Some(42),
            language: Some("rust".into()),
            description: Some("fix bug".into()),
            truncated: true,
            alt_text: Some("patch for bug fix".into()),
        };
        let ev = sign(build_diff_message(cid, "diff", &meta, None).unwrap());
        assert!(has_tag(&ev, "file", "src/lib.rs"));
        assert!(has_tag(&ev, "parent-commit", "1234567"));
        assert!(has_tag(&ev, "pr", "42"));
        assert!(has_tag(&ev, "truncated", "true"));
        assert!(has_tag(&ev, "alt", "patch for bug fix"));
    }

    #[test]
    fn edit_happy_path() {
        let cid = uuid();
        let eid = event_id();
        let ev = sign(build_edit(cid, eid, "new content").unwrap());
        assert_eq!(ev.kind.as_u16(), 40003);
        assert!(has_tag(&ev, "e", &eid.to_hex()));
    }

    #[test]
    fn edit_content_too_large() {
        let cid = uuid();
        let eid = event_id();
        let big = "x".repeat(64 * 1024 + 1);
        assert!(matches!(
            build_edit(cid, eid, &big),
            Err(SdkError::ContentTooLarge { .. })
        ));
    }

    #[test]
    fn delete_message_happy_path() {
        let cid = uuid();
        let eid = event_id();
        let ev = sign(build_delete_message(cid, eid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9005);
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert!(has_tag(&ev, "e", &eid.to_hex()));
        assert_eq!(ev.content, "");
    }

    #[test]
    fn delete_message_with_moderation_metadata() {
        let cid = uuid();
        let eid = event_id();
        let action_id = Uuid::new_v4();
        let ev = sign(
            build_delete_message_with_options(
                cid,
                eid,
                DeleteMessageOptions {
                    action_id: Some(action_id),
                    reason_code: Some("spam"),
                    public_reason: Some("Removed for spam."),
                },
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 9005);
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert!(has_tag(&ev, "e", &eid.to_hex()));
        assert!(has_tag(&ev, "action_id", &action_id.to_string()));
        assert!(has_tag(&ev, "reason_code", "spam"));
        assert!(has_tag(&ev, "public_reason", "Removed for spam."));
    }

    #[test]
    fn delete_compat_happy_path() {
        let cid = uuid();
        let eid = event_id();
        let ev = sign(build_delete_compat(cid, eid).unwrap());
        assert_eq!(ev.kind.as_u16(), 5);
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert!(has_tag(&ev, "e", &eid.to_hex()));
        assert_eq!(ev.content, "");
    }

    #[test]
    fn vote_up() {
        let cid = uuid();
        let eid = event_id();
        let ev = sign(build_vote(cid, eid, VoteDirection::Up).unwrap());
        assert_eq!(ev.kind.as_u16(), 45002);
        assert_eq!(ev.content, "+");
    }

    #[test]
    fn vote_down() {
        let cid = uuid();
        let eid = event_id();
        let ev = sign(build_vote(cid, eid, VoteDirection::Down).unwrap());
        assert_eq!(ev.content, "-");
    }

    #[test]
    fn reaction_happy_path() {
        let eid = event_id();
        let ev = sign(build_reaction(eid, "👍").unwrap());
        assert_eq!(ev.kind.as_u16(), 7);
        assert_eq!(ev.content, "👍");
    }

    #[test]
    fn reaction_emoji_too_long() {
        let eid = event_id();
        let long_emoji = "a".repeat(65);
        assert!(matches!(
            build_reaction(eid, &long_emoji),
            Err(SdkError::EmojiTooLong)
        ));
    }

    #[test]
    fn reaction_emoji_max_len_ok() {
        let eid = event_id();
        let max_emoji = "a".repeat(64);
        assert!(build_reaction(eid, &max_emoji).is_ok());
    }

    #[test]
    fn custom_emoji_reaction_happy_path() {
        let eid = event_id();
        let ev = sign(
            build_custom_emoji_reaction(eid, ":Party_Parrot:", "https://example.com/parrot.png")
                .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 7);
        assert_eq!(ev.content, ":party_parrot:");
        assert!(has_tag(&ev, "emoji", "party_parrot"));
    }

    #[test]
    fn custom_emoji_set_happy_path() {
        let ev = sign(
            build_custom_emoji_set(&[CustomEmoji {
                shortcode: "party".to_string(),
                url: "https://example.com/party.png".to_string(),
            }])
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 30030);
        assert!(has_tag(&ev, "d", CUSTOM_EMOJI_SET_D_TAG));
        assert!(has_tag(&ev, "emoji", "party"));
    }

    #[test]
    fn remove_reaction_happy_path() {
        let eid = event_id();
        let ev = sign(build_remove_reaction(eid).unwrap());
        assert_eq!(ev.kind.as_u16(), 5);
        assert!(has_tag(&ev, "e", &eid.to_hex()));
    }

    #[test]
    fn set_canvas_happy_path() {
        let cid = uuid();
        let ev = sign(build_set_canvas(cid, "# Canvas\nHello").unwrap());
        assert_eq!(ev.kind.as_u16(), 40100);
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert_eq!(ev.content, "# Canvas\nHello");
    }

    #[test]
    fn profile_all_fields() {
        let ev = sign(
            build_profile(
                Some("Alice"),
                Some("alice"),
                Some("https://example.com/pic.jpg"),
                Some("Hello world"),
                Some("alice@example.com"),
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 0);
        let v: serde_json::Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(v["display_name"], "Alice");
        assert_eq!(v["name"], "alice");
        assert_eq!(v["nip05"], "alice@example.com");
    }

    #[test]
    fn profile_some_fields() {
        let ev = sign(build_profile(Some("Bob"), None, None, None, None).unwrap());
        let v: serde_json::Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(v["display_name"], "Bob");
        assert!(
            v.get("name").is_none()
                || !v["name"].is_null() && v.get("name") == Some(&serde_json::Value::Null)
                || !v.as_object().unwrap().contains_key("name")
        );
    }

    #[test]
    fn profile_no_fields() {
        let ev = sign(build_profile(None, None, None, None, None).unwrap());
        let v: serde_json::Value = serde_json::from_str(&ev.content).unwrap();
        assert!(v.as_object().unwrap().is_empty());
    }

    #[test]
    fn agent_profile_controls_are_complete_and_canonical() {
        let ev = sign(
            build_agent_profile_controls(
                "owner_only",
                &["z-capability", MEETING_V2_ACTIONS_CAPABILITY],
                Some("Agent One"),
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), KIND_AGENT_PROFILE as u16);
        let value: serde_json::Value = serde_json::from_str(&ev.content).unwrap();
        assert_eq!(value["channel_add_policy"], "owner_only");
        assert_eq!(value["display_name"], "Agent One");
        assert_eq!(
            value["capabilities"],
            serde_json::json!([MEETING_V2_ACTIONS_CAPABILITY, "z-capability"])
        );
    }

    #[test]
    fn meeting_action_runtime_capability_generations_are_explicit() {
        assert_eq!(
            MEETING_V2_ACTIONS_V2_CAPABILITY,
            "meeting-v2-action-finalization-v2"
        );
        assert_eq!(
            MEETING_V2_ACTIONS_V3_CAPABILITY,
            "meeting-v2-action-finalization-v3"
        );
        assert_eq!(
            MEETING_V2_ACTIONS_CAPABILITY,
            "meeting-v2-action-finalization-v4"
        );
        assert_ne!(
            MEETING_V2_ACTIONS_CAPABILITY,
            MEETING_V2_ACTIONS_V3_CAPABILITY
        );
    }

    #[test]
    fn meeting_action_block_builder_rejects_retired_affinity_reason() {
        let board_event_id = "a".repeat(64);
        let result = build_meeting_v2_action_block(MeetingV2ActionBlockParams {
            session_id: Uuid::new_v4(),
            fence: MeetingV2ActionRunFence {
                action_run_id: Uuid::new_v4(),
                action_window: 1,
                board_event_id: &board_event_id,
            },
            reason_code: "affinity_lost",
            reason: Some("historical exact-session failure"),
        });
        assert!(matches!(result, Err(SdkError::InvalidInput(_))));
    }

    #[test]
    fn agent_profile_controls_reject_invalid_or_duplicate_values() {
        assert!(build_agent_profile_controls("owner-only", &[], None).is_err());
        assert!(build_agent_profile_controls("anyone", &["duplicate", "duplicate"], None).is_err());
        assert!(build_agent_profile_controls("anyone", &[" leading-space"], None).is_err());
        let too_many = vec!["capability"; MAX_AGENT_PROFILE_CAPABILITIES + 1];
        assert!(build_agent_profile_controls("nobody", &too_many, None).is_err());
    }

    #[test]
    fn add_member_with_role() {
        let cid = uuid();
        let pubkey = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let ev = sign(build_add_member(cid, pubkey, Some(MemberRole::Admin)).unwrap());
        assert_eq!(ev.kind.as_u16(), 9000);
        assert!(has_tag(&ev, "p", pubkey));
        assert!(has_tag(&ev, "role", "admin"));
    }

    #[test]
    fn add_member_without_role() {
        let cid = uuid();
        let pubkey = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let ev = sign(build_add_member(cid, pubkey, None::<MemberRole>).unwrap());
        assert_eq!(ev.kind.as_u16(), 9000);
        assert!(tag_values(&ev, "role").is_empty());
    }

    #[test]
    fn remove_member_happy_path() {
        let cid = uuid();
        let pubkey = "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234";
        let ev = sign(build_remove_member(cid, pubkey).unwrap());
        assert_eq!(ev.kind.as_u16(), 9001);
        assert!(has_tag(&ev, "p", pubkey));
    }

    #[test]
    fn leave_happy_path() {
        let cid = uuid();
        let ev = sign(build_leave(cid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9022);
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn update_channel_name_and_about() {
        let cid = uuid();
        let ev = sign(
            build_update_channel(cid, Some("new-name"), Some("new about"), None, None).unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "name", "new-name"));
        assert!(has_tag(&ev, "about", "new about"));
    }

    #[test]
    fn update_channel_strips_all_leading_hashes_from_name() {
        let ev =
            sign(build_update_channel(uuid(), Some("  ###new-name  "), None, None, None).unwrap());
        assert!(has_tag(&ev, "name", "new-name"));
    }

    #[test]
    fn update_channel_rejects_hash_only_name() {
        assert!(matches!(
            build_update_channel(uuid(), Some("  ###  "), None, None, None),
            Err(SdkError::InvalidTag(_))
        ));
    }

    #[test]
    fn update_channel_visibility_and_ttl() {
        let cid = uuid();
        let ev =
            sign(build_update_channel(cid, None, None, Some("private"), Some(Some(3600))).unwrap());
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "visibility", "private"));
        assert!(has_tag(&ev, "ttl", "3600"));
    }

    #[test]
    fn update_channel_clears_ttl() {
        let cid = uuid();
        let ev = sign(build_update_channel(cid, None, None, None, Some(None)).unwrap());
        assert!(has_tag(&ev, "ttl", ""));
    }

    #[test]
    fn update_channel_invalid_visibility_rejected() {
        let cid = uuid();
        assert!(matches!(
            build_update_channel(cid, None, None, Some("secret"), None),
            Err(SdkError::InvalidTag(_))
        ));
    }

    #[test]
    fn update_channel_no_fields_rejected() {
        let cid = uuid();
        assert!(matches!(
            build_update_channel(cid, None, None, None, None),
            Err(SdkError::InvalidTag(_))
        ));
    }

    #[test]
    fn set_topic_happy_path() {
        let cid = uuid();
        let ev = sign(build_set_topic(cid, "Rust async patterns").unwrap());
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "topic", "Rust async patterns"));
    }

    #[test]
    fn set_purpose_happy_path() {
        let cid = uuid();
        let ev = sign(build_set_purpose(cid, "Team coordination").unwrap());
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "purpose", "Team coordination"));
    }

    #[test]
    fn create_channel_all_fields() {
        let cid = uuid();
        let ev = sign(
            build_create_channel(
                cid,
                "general",
                Some(Visibility::Open),
                Some(ChannelKind::Stream),
                Some("General chat"),
                None,
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 9007);
        assert!(has_tag(&ev, "name", "general"));
        assert!(has_tag(&ev, "visibility", "open"));
        assert!(has_tag(&ev, "channel_type", "stream"));
        assert!(has_tag(&ev, "about", "General chat"));
    }

    #[test]
    fn create_channel_minimal() {
        let cid = uuid();
        let ev = sign(
            build_create_channel(
                cid,
                "dev",
                None::<Visibility>,
                None::<ChannelKind>,
                None,
                None,
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 9007);
        assert!(has_tag(&ev, "name", "dev"));
    }

    #[test]
    fn create_channel_strips_all_leading_hashes_from_name() {
        let ev = sign(
            build_create_channel(
                uuid(),
                "  ###dev  ",
                None::<Visibility>,
                None::<ChannelKind>,
                None,
                None,
            )
            .unwrap(),
        );
        assert!(has_tag(&ev, "name", "dev"));
    }

    #[test]
    fn create_channel_rejects_hash_only_name() {
        assert!(matches!(
            build_create_channel(
                uuid(),
                "  ###  ",
                None::<Visibility>,
                None::<ChannelKind>,
                None,
                None,
            ),
            Err(SdkError::InvalidTag(_))
        ));
    }

    #[test]
    fn create_channel_ephemeral_emits_ttl() {
        let cid = uuid();
        let ev = sign(
            build_create_channel(
                cid,
                "standup",
                Some(Visibility::Open),
                Some(ChannelKind::Stream),
                None,
                Some(3600),
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 9007);
        assert!(has_tag(&ev, "ttl", "3600"));
    }

    #[test]
    fn join_happy_path() {
        let cid = uuid();
        let ev = sign(build_join(cid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9021);
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn archive_happy_path() {
        let cid = uuid();
        let ev = sign(build_archive(cid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "archived", "true"));
    }

    #[test]
    fn unarchive_happy_path() {
        let cid = uuid();
        let ev = sign(build_unarchive(cid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9002);
        assert!(has_tag(&ev, "archived", "false"));
    }

    #[test]
    fn delete_channel_happy_path() {
        let cid = uuid();
        let ev = sign(build_delete_channel(cid).unwrap());
        assert_eq!(ev.kind.as_u16(), 9008);
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn extract_channel_id_present() {
        let cid = uuid();
        let ev = sign(build_join(cid).unwrap());
        assert_eq!(extract_channel_id(&ev), Some(cid));
    }

    #[test]
    fn extract_channel_id_absent() {
        // build_note (kind 1) is a global text note — no h tag.
        let ev = sign(build_note("hello", None).unwrap());
        assert_eq!(extract_channel_id(&ev), None);
    }

    #[test]
    fn extract_channel_id_invalid_uuid() {
        // Build an event with a malformed h-tag value
        let tags = vec![Tag::parse(["h", "not-a-uuid"]).unwrap()];
        let ev = EventBuilder::new(Kind::Custom(9), "x")
            .tags(tags)
            .sign_with_keys(&keys())
            .unwrap();
        assert_eq!(extract_channel_id(&ev), None);
    }

    #[test]
    fn meeting_create_emits_versioned_frozen_roster_shape() {
        let session_id = uuid();
        let participant_a = "11".repeat(32);
        let participant_b = "22".repeat(32);
        let source_id = uuid();
        let event = sign(
            build_meeting_create(
                session_id,
                "# Design review",
                Some("Stage one"),
                Some(source_id),
                &[&participant_a, &participant_b],
            )
            .unwrap(),
        );

        assert_eq!(event.kind.as_u16(), KIND_MEETING_CREATE as u16);
        assert!(has_tag(&event, "h", &session_id.to_string()));
        assert!(has_tag(&event, "name", "Design review"));
        assert!(has_tag(&event, "about", "Stage one"));
        assert!(has_tag(&event, "source", &source_id.to_string()));
        assert!(has_tag(&event, "v", "1"));
        assert_eq!(tag_values(&event, "p"), vec![participant_a, participant_b]);
    }

    #[test]
    fn meeting_create_rejects_duplicate_or_empty_rosters() {
        let participant = "11".repeat(32);
        assert!(build_meeting_create(uuid(), "review", None, None, &[]).is_err());
        assert!(
            build_meeting_create(uuid(), "review", None, None, &[&participant, &participant])
                .is_err()
        );
    }

    #[test]
    fn meeting_v2_create_emits_implicit_moderator_and_strict_board() {
        let session_id = uuid();
        let author = "11".repeat(32);
        let participant = "22".repeat(32);
        let event = sign(
            build_meeting_v2_create(MeetingV2CreateParams {
                session_id,
                title: "# Design review",
                description: None,
                source_channel_id: None,
                author_pubkey: &author,
                participant_pubkeys: &[&participant],
                initial_board: "# Goal\nAgree on the API.",
            })
            .unwrap(),
        );

        assert_eq!(event.kind.as_u16(), KIND_MEETING_CREATE as u16);
        assert!(has_tag(&event, "h", &session_id.to_string()));
        assert!(has_tag(&event, "name", "Design review"));
        assert!(has_tag(&event, "v", MEETING_V2_SCHEMA_VERSION));
        assert!(has_tag(&event, "policy", MEETING_V2_POLICY));
        assert!(tag_values(&event, "moderator").is_empty());
        assert_eq!(tag_values(&event, "p"), vec![participant]);
        assert_eq!(
            parse_meeting_v2_board_content(&event.content).unwrap(),
            MeetingV2BoardContent {
                format: MEETING_V2_BOARD_FORMAT.to_string(),
                body: "# Goal\nAgree on the API.".to_string(),
            }
        );
    }

    #[test]
    fn meeting_v2_board_rejects_empty_oversized_nul_and_unknown_fields() {
        let author = "11".repeat(32);
        let participant = "22".repeat(32);
        let build = |body: &str| {
            build_meeting_v2_create(MeetingV2CreateParams {
                session_id: uuid(),
                title: "Review",
                description: None,
                source_channel_id: None,
                author_pubkey: &author,
                participant_pubkeys: &[&participant],
                initial_board: body,
            })
        };

        assert!(build(" \n ").is_err());
        assert!(build("bad\0board").is_err());
        assert!(matches!(
            build(&"x".repeat(MAX_MEETING_V2_BOARD_BYTES + 1)),
            Err(SdkError::ContentTooLarge { .. })
        ));
        assert!(parse_meeting_v2_board_content(
            r#"{"format":"markdown","body":"ok","revision":1}"#
        )
        .is_err());
        assert!(parse_meeting_v2_board_content(r#"{"format":"html","body":"ok"}"#).is_err());
    }

    #[test]
    fn meeting_v2_actions_wire_is_direct_and_board_fenced() {
        let session_id = uuid();
        let action_run_id = uuid();
        let author = "11".repeat(32);
        let participant = "22".repeat(32);
        let create = sign(
            build_meeting_v2_actions_create(MeetingV2CreateParams {
                session_id,
                title: "Action review",
                description: None,
                source_channel_id: None,
                author_pubkey: &author,
                participant_pubkeys: &[&participant],
                initial_board: "# Goal\nAgree and assign the implementation.",
            })
            .unwrap(),
        );
        assert!(has_tag(&create, "v", MEETING_V2_SCHEMA_VERSION));
        assert!(has_tag(&create, "policy", MEETING_V2_ACTIONS_POLICY));
        assert!(!has_tag(&create, "policy", MEETING_V2_POLICY));

        let state_event_id = "aa".repeat(32);
        let board_event_id = "bb".repeat(32);
        let begin = sign(
            build_meeting_v2_action_begin(MeetingV2ActionBeginParams {
                session_id,
                expected_control_epoch: 4,
                board_window: 7,
                expected_state_event_id: &state_event_id,
                board_event_id: &board_event_id,
                expected_decision_attempt_id: None,
            })
            .unwrap(),
        );
        assert_eq!(begin.kind.as_u16() as u32, KIND_MEETING_ACTION_COMMAND);
        assert!(has_tag(&begin, "policy", MEETING_V2_ACTIONS_POLICY));
        assert!(has_tag(&begin, "action", "begin"));
        assert!(has_tag(&begin, "expected-control-epoch", "4"));
        assert!(has_tag(&begin, "board-window", "7"));
        assert!(has_tag(&begin, "expected-state", &state_event_id));
        assert!(has_tag(&begin, "board", &board_event_id));

        let retry = sign(
            build_meeting_v2_action_retry(MeetingV2ActionCommandParams {
                session_id,
                fence: MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    board_event_id: &board_event_id,
                },
            })
            .unwrap(),
        );
        assert!(has_tag(&retry, "action", "retry"));
        assert!(has_tag(&retry, "action-run", &action_run_id.to_string()));
        assert!(has_tag(&retry, "action-window", "1"));
        assert!(has_tag(&retry, "board", &board_event_id));
        assert!(!retry
            .tags
            .iter()
            .any(|tag| tag.as_slice()[0] == "action-plan"));

        let renewal = sign(
            build_meeting_v2_action_lease_renew(MeetingV2ActionLeaseRenewParams {
                session_id,
                fence: MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    board_event_id: &board_event_id,
                },
                progress_seq: 3,
                stage: MeetingV2ActionProgressStage::ToolResult,
                last_activity_seq: 8,
            })
            .unwrap(),
        );
        assert!(has_tag(&renewal, "policy", MEETING_V2_ACTIONS_POLICY));
        assert!(has_tag(&renewal, "action", "renew"));
        assert!(has_tag(&renewal, "progress-seq", "3"));
        assert!(has_tag(&renewal, "stage", "tool_result"));
        assert!(has_tag(&renewal, "last-activity-seq", "8"));
        assert!(renewal.content.is_empty());
        assert!(
            build_meeting_v2_action_lease_renew(MeetingV2ActionLeaseRenewParams {
                session_id,
                fence: MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    board_event_id: &board_event_id,
                },
                progress_seq: 0,
                stage: MeetingV2ActionProgressStage::Reasoning,
                last_activity_seq: 0,
            })
            .is_err()
        );

        let returned = sign(
            build_meeting_v2_action_return_to_board(MeetingV2ActionCommandParams {
                session_id,
                fence: MeetingV2ActionRunFence {
                    action_run_id,
                    action_window: 1,
                    board_event_id: &board_event_id,
                },
            })
            .unwrap(),
        );
        assert!(has_tag(&returned, "external-effects", "preserved"));

        let close = sign(
            build_meeting_v2_actions_end(MeetingV2ActionsEndParams {
                session_id,
                create_event_id: &create.id.to_hex(),
                outcome: MeetingV2EndOutcome::Closed,
                reason_code: None,
                reason: None,
                action_fence: Some(MeetingV2ActionsEndFence {
                    action_run_id,
                    action_window: 1,
                    board_event_id: &board_event_id,
                }),
            })
            .unwrap(),
        );
        assert!(has_tag(&close, "policy", MEETING_V2_ACTIONS_POLICY));
        assert!(has_tag(&close, "action-run", &action_run_id.to_string()));
        assert!(has_tag(&close, "action-window", "1"));
        assert!(has_tag(&close, "board", &board_event_id));
        assert!(has_tag(&close, "attestation", "actions-recorded"));
        assert!(!close
            .tags
            .iter()
            .any(|tag| tag.as_slice()[0] == "action-plan"));
    }

    #[test]
    fn meeting_summary_update_is_fenced_metadata_not_control() {
        let session_id = uuid();
        let action_run_id = uuid();
        let board_event_id = "bb".repeat(32);
        let fence = MeetingV2ActionRunFence {
            action_run_id,
            action_window: 2,
            board_event_id: &board_event_id,
        };
        let set = sign(
            build_meeting_summary_update(MeetingSummaryUpdateParams {
                session_id,
                mutation: MeetingSummaryMutation::Set("Decision and materialized outputs."),
                action_fence: fence,
            })
            .unwrap(),
        );
        assert_eq!(set.kind.as_u16() as u32, KIND_MEETING_SUMMARY_COMMAND);
        assert_eq!(set.content, "Decision and materialized outputs.");
        assert!(has_tag(&set, "h", &session_id.to_string()));
        assert!(has_tag(&set, "policy", MEETING_V2_ACTIONS_POLICY));
        assert!(has_tag(&set, "action", "set"));
        assert!(has_tag(&set, "action-run", &action_run_id.to_string()));
        assert!(has_tag(&set, "action-window", "2"));
        assert!(has_tag(&set, "board", &board_event_id));

        let clear = sign(
            build_meeting_summary_update(MeetingSummaryUpdateParams {
                session_id,
                mutation: MeetingSummaryMutation::Clear,
                action_fence: fence,
            })
            .unwrap(),
        );
        assert!(has_tag(&clear, "action", "clear"));
        assert!(clear.content.is_empty());
        assert!(build_meeting_summary_update(MeetingSummaryUpdateParams {
            session_id,
            mutation: MeetingSummaryMutation::Set(" \n "),
            action_fence: fence,
        })
        .is_err());
        assert!(build_meeting_summary_update(MeetingSummaryUpdateParams {
            session_id,
            mutation: MeetingSummaryMutation::Set("bad\0summary"),
            action_fence: fence,
        })
        .is_err());
    }

    #[test]
    fn meeting_end_references_create_event_and_manual_reason() {
        let session_id = uuid();
        let create_event_id = "ab".repeat(32);
        let event = sign(build_meeting_end(session_id, &create_event_id).unwrap());

        assert_eq!(event.kind.as_u16(), KIND_MEETING_END as u16);
        assert!(has_tag(&event, "h", &session_id.to_string()));
        assert!(has_tag(&event, "e", &create_event_id));
        assert!(has_tag(&event, "reason", "manual"));
    }

    #[test]
    fn meeting_floor_claim_signals_and_speech_have_strict_round_shapes() {
        let session_id = uuid();
        let grant_event_id = "cd".repeat(32);

        let claim = sign(build_meeting_floor_claim(session_id, 7).unwrap());
        assert_eq!(claim.kind.as_u16(), KIND_MEETING_FLOOR_CLAIM as u16);
        assert!(has_tag(&claim, "h", &session_id.to_string()));
        assert!(has_tag(&claim, "meeting-round", "7"));
        assert!(claim.content.is_empty());

        let ready = sign(build_meeting_floor_ready(session_id, 7, "speech:abc").unwrap());
        assert_eq!(ready.kind.as_u16(), KIND_MEETING_FLOOR_SIGNAL as u16);
        assert!(has_tag(&ready, "action", "ready"));
        assert!(has_tag(&ready, "intent-basis", "speech:abc"));

        let pass = sign(build_meeting_floor_pass(session_id, 7, "speech:abc").unwrap());
        assert!(has_tag(&pass, "action", "pass"));
        assert!(has_tag(&pass, "intent-basis", "speech:abc"));

        let yielded = sign(build_meeting_floor_yield(session_id, 7, &grant_event_id).unwrap());
        assert!(has_tag(&yielded, "action", "yield"));
        assert!(has_tag(&yielded, "meeting-grant", &grant_event_id));

        let mention = "ab".repeat(32);
        let speech = sign(
            build_meeting_speech(
                session_id,
                7,
                &grant_event_id,
                "I have a proposal.",
                &[mention.as_str()],
            )
            .unwrap(),
        );
        assert_eq!(speech.kind.as_u16(), KIND_STREAM_MESSAGE as u16);
        assert!(has_tag(&speech, "h", &session_id.to_string()));
        assert!(has_tag(&speech, "meeting-round", "7"));
        assert!(has_tag(&speech, "meeting-grant", &grant_event_id));
        assert!(has_tag(&speech, "p", &mention));
        assert_eq!(speech.tags.len(), 4);
    }

    #[test]
    fn meeting_floor_signals_reject_invalid_basis_and_grant() {
        let session_id = uuid();
        assert!(build_meeting_floor_ready(session_id, 1, "").is_err());
        assert!(build_meeting_floor_pass(session_id, 1, "line\nbreak").is_err());
        assert!(build_meeting_floor_yield(session_id, 1, "bad").is_err());
    }

    #[test]
    fn build_note_happy_path() {
        let builder = build_note("hello world", None).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind, Kind::Custom(1));
        assert_eq!(event.content, "hello world");
        assert!(event.tags.is_empty());
    }

    #[test]
    fn build_note_with_reply() {
        let keys = nostr::Keys::generate();
        // Create a dummy event to get a valid EventId
        let dummy = EventBuilder::new(Kind::Custom(1), "dummy")
            .tags(vec![])
            .sign_with_keys(&keys)
            .unwrap();
        let builder = build_note("reply text", Some(dummy.id)).unwrap();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind, Kind::Custom(1));
        assert_eq!(event.content, "reply text");
        assert_eq!(event.tags.len(), 1);
        let tag = event.tags.iter().next().unwrap();
        assert_eq!(tag.as_slice()[0], "e");
        assert_eq!(tag.as_slice()[1], dummy.id.to_hex());
        assert_eq!(tag.as_slice()[3], "reply");
    }

    #[test]
    fn build_note_content_too_large() {
        let big = "x".repeat(64 * 1024 + 1);
        let err = build_note(&big, None).unwrap_err();
        assert!(matches!(err, SdkError::ContentTooLarge { .. }));
    }

    #[test]
    fn build_note_empty_content() {
        // Empty content is valid per NIP-01.
        let builder = build_note("", None).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind, Kind::Custom(1));
        assert_eq!(event.content, "");
        assert!(event.tags.is_empty());
    }

    #[test]
    fn build_contact_list_happy_path() {
        let pubkey = "a".repeat(64);
        let contacts = vec![(pubkey.as_str(), None, None)];
        let builder = build_contact_list(&contacts).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind, Kind::Custom(3));
        assert_eq!(event.content, "");
        assert_eq!(event.tags.len(), 1);
        let tag = event.tags.iter().next().unwrap();
        assert_eq!(tag.as_slice()[0], "p");
        assert_eq!(tag.as_slice()[1], pubkey);
    }

    #[test]
    fn build_contact_list_normalizes_uppercase() {
        let upper = "A".repeat(64);
        let contacts = vec![(upper.as_str(), None, None)];
        let builder = build_contact_list(&contacts).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        let tag = event.tags.iter().next().unwrap();
        assert_eq!(tag.as_slice()[1], "a".repeat(64));
    }

    #[test]
    fn build_contact_list_with_relay_and_petname() {
        let pubkey = "b".repeat(64);
        let contacts = vec![(
            pubkey.as_str(),
            Some("wss://relay.example.com"),
            Some("alice"),
        )];
        let builder = build_contact_list(&contacts).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        let tag = event.tags.iter().next().unwrap();
        assert_eq!(tag.as_slice()[0], "p");
        assert_eq!(tag.as_slice()[2], "wss://relay.example.com");
        assert_eq!(tag.as_slice()[3], "alice");
    }

    #[test]
    fn build_contact_list_empty() {
        let builder = build_contact_list(&[]).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.kind, Kind::Custom(3));
        assert!(event.tags.is_empty());
    }

    #[test]
    fn build_contact_list_rejects_short_pubkey() {
        let short = "a".repeat(63);
        let contacts = vec![(short.as_str(), None, None)];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn build_contact_list_rejects_long_pubkey() {
        let long = "a".repeat(65);
        let contacts = vec![(long.as_str(), None, None)];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn build_contact_list_rejects_non_hex() {
        let non_hex = "g".repeat(64);
        let contacts = vec![(non_hex.as_str(), None, None)];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn build_contact_list_rejects_long_relay_url() {
        let pubkey = "a".repeat(64);
        let long_url = "x".repeat(2049);
        let contacts = vec![(pubkey.as_str(), Some(long_url.as_str()), None)];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn build_contact_list_rejects_long_petname() {
        let pubkey = "a".repeat(64);
        let long_name = "x".repeat(257);
        let contacts = vec![(pubkey.as_str(), None, Some(long_name.as_str()))];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn build_contact_list_duplicate_pubkeys() {
        let pubkey = "c".repeat(64);
        // Same pubkey twice — only one p-tag should be emitted.
        let contacts = vec![
            (pubkey.as_str(), None, None),
            (
                pubkey.as_str(),
                Some("wss://relay.example.com"),
                Some("bob"),
            ),
        ];
        let builder = build_contact_list(&contacts).unwrap();
        let keys = nostr::Keys::generate();
        let event = builder.sign_with_keys(&keys).unwrap();
        assert_eq!(event.tags.len(), 1);
        let tag = event.tags.iter().next().unwrap();
        assert_eq!(tag.as_slice()[0], "p");
        assert_eq!(tag.as_slice()[1], pubkey);
    }

    #[test]
    fn build_contact_list_too_many() {
        let pubkey = "d".repeat(64);
        // MAX_CONTACTS + 1 entries (all same pubkey — uniqueness doesn't matter,
        // the cap is checked before deduplication).
        let entry = (pubkey.as_str(), None, None);
        let contacts = vec![entry; MAX_CONTACTS + 1];
        let err = build_contact_list(&contacts).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_happy_path_all_fields() {
        let ev = sign(
            build_repo_announcement(
                "my-repo",
                Some("My Repo"),
                Some("A test repository"),
                &["https://github.com/example/my-repo.git"],
                Some("https://github.com/example/my-repo"),
                &["wss://relay.example.com"],
            )
            .unwrap(),
        );
        assert_eq!(ev.kind.as_u16(), 30617);
        assert_eq!(ev.content, "");
        assert!(has_tag(&ev, "d", "my-repo"));
        assert!(has_tag(&ev, "name", "My Repo"));
        assert!(has_tag(&ev, "description", "A test repository"));
        assert!(has_tag(
            &ev,
            "clone",
            "https://github.com/example/my-repo.git"
        ));
        assert!(has_tag(&ev, "web", "https://github.com/example/my-repo"));
        // relays is a multi-value tag — check the tag key exists
        assert!(ev.tags.iter().any(|t| {
            let s = t.as_slice();
            s.first().map(|v| v.as_str()) == Some("relays")
                && s.get(1).map(|v| v.as_str()) == Some("wss://relay.example.com")
        }));
    }

    #[test]
    fn repo_announcement_happy_path_minimal() {
        let ev = sign(build_repo_announcement("bare-repo", None, None, &[], None, &[]).unwrap());
        assert_eq!(ev.kind.as_u16(), 30617);
        assert_eq!(ev.content, "");
        assert!(has_tag(&ev, "d", "bare-repo"));
        // No optional tags present
        assert!(!ev
            .tags
            .iter()
            .any(|t| t.as_slice().first().map(|v| v.as_str()) == Some("name")));
        assert!(!ev
            .tags
            .iter()
            .any(|t| t.as_slice().first().map(|v| v.as_str()) == Some("clone")));
    }

    #[test]
    fn repo_announcement_with_tags_preserves_metadata_and_canonicalizes_d() {
        let tags = vec![
            Tag::parse(["d", "wrong-repo"]).unwrap(),
            Tag::parse(["name", "Protected Repo"]).unwrap(),
            Tag::parse(["buzz-channel", "channel-id"]).unwrap(),
            Tag::parse(["future-metadata", "preserve-me"]).unwrap(),
        ];

        let ev = sign(
            build_repo_announcement_with_tags("protected-repo", "repository content", tags)
                .unwrap(),
        );

        assert_eq!(ev.kind.as_u16(), 30617);
        assert_eq!(ev.content, "repository content");
        assert_eq!(
            ev.tags
                .iter()
                .filter(|tag| tag.as_slice().first().map(String::as_str) == Some("d"))
                .count(),
            1
        );
        assert!(has_tag(&ev, "d", "protected-repo"));
        assert!(has_tag(&ev, "buzz-channel", "channel-id"));
        assert!(has_tag(&ev, "future-metadata", "preserve-me"));
    }

    #[test]
    fn repo_announcement_rejects_empty_repo_id() {
        let err = build_repo_announcement("", None, None, &[], None, &[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_rejects_leading_dot() {
        let err = build_repo_announcement(".hidden", None, None, &[], None, &[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_rejects_double_dot() {
        let err = build_repo_announcement("some..repo", None, None, &[], None, &[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_rejects_repo_id_over_64_chars() {
        let long_id = "a".repeat(65);
        let err = build_repo_announcement(&long_id, None, None, &[], None, &[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_rejects_invalid_chars_in_repo_id() {
        let err = build_repo_announcement("bad repo!", None, None, &[], None, &[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn repo_announcement_multiple_clone_urls_multi_value_tag() {
        let ev = sign(
            build_repo_announcement(
                "multi-clone",
                None,
                None,
                &[
                    "https://relay.example.com/git/abc/multi-clone",
                    "ssh://git@github.com/org/multi-clone.git",
                ],
                None,
                &[],
            )
            .unwrap(),
        );
        // clone is a multi-value tag per NIP-34: ["clone", url1, url2, ...]
        let clone_tag = ev
            .tags
            .iter()
            .find(|t| t.as_slice().first().map(|v| v.as_str()) == Some("clone"))
            .expect("clone tag missing");
        let vals: Vec<&str> = clone_tag
            .as_slice()
            .iter()
            .skip(1)
            .map(|v| v.as_str())
            .collect();
        assert_eq!(vals.len(), 2);
        assert_eq!(vals[0], "https://relay.example.com/git/abc/multi-clone");
        assert_eq!(vals[1], "ssh://git@github.com/org/multi-clone.git");
    }

    #[test]
    fn git_patch_happy_path_minimal() {
        let owner = "a".repeat(64);
        let repo = GitRepoCoord {
            owner: owner.clone(),
            id: "my-repo".to_string(),
        };
        let ev =
            sign(build_git_patch(&repo, "diff --git a/x b/x", &GitPatchMeta::default()).unwrap());
        assert_eq!(ev.kind.as_u16(), 1617);
        assert_eq!(ev.content, "diff --git a/x b/x");
        assert!(has_tag(&ev, "a", &format!("30617:{owner}:my-repo")));
        assert!(has_tag(&ev, "p", &owner));
    }

    #[test]
    fn git_patch_root_and_metadata_tags() {
        let owner = "a".repeat(64);
        let commit = "c".repeat(40);
        let parent = "d".repeat(40);
        let repo = GitRepoCoord {
            owner,
            id: "repo".to_string(),
        };
        let meta = GitPatchMeta {
            root: true,
            commit: Some(commit.clone()),
            parent_commit: Some(parent.clone()),
            ..Default::default()
        };
        let ev = sign(build_git_patch(&repo, "patch body", &meta).unwrap());
        assert!(has_tag(&ev, "t", "root"));
        assert!(has_tag(&ev, "commit", &commit));
        assert!(has_tag(&ev, "parent-commit", &parent));
        assert!(has_tag(&ev, "r", &commit));
    }

    #[test]
    fn git_patch_rejects_root_and_root_revision_together() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let meta = GitPatchMeta {
            root: true,
            root_revision: true,
            ..Default::default()
        };
        let err = build_git_patch(&repo, "x", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_patch_rejects_oversized_content() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let big = "x".repeat(60 * 1024 + 1);
        let err = build_git_patch(&repo, &big, &GitPatchMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::ContentTooLarge { .. }));
    }

    #[test]
    fn git_patch_rejects_bad_repo_owner() {
        let repo = GitRepoCoord {
            owner: "not-hex".to_string(),
            id: "repo".to_string(),
        };
        let err = build_git_patch(&repo, "x", &GitPatchMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_issue_happy_path() {
        let owner = "a".repeat(64);
        let repo = GitRepoCoord {
            owner: owner.clone(),
            id: "repo".to_string(),
        };
        let meta = GitIssueMeta {
            labels: vec!["bug".to_string(), "p1".to_string()],
            recipients: vec![],
        };
        let ev =
            sign(build_git_issue(&repo, "Crashes on startup", "steps to repro", &meta).unwrap());
        assert_eq!(ev.kind.as_u16(), 1621);
        assert_eq!(ev.content, "steps to repro");
        assert!(has_tag(&ev, "a", &format!("30617:{owner}:repo")));
        assert!(has_tag(&ev, "subject", "Crashes on startup"));
        assert!(has_tag(&ev, "t", "bug"));
        assert!(has_tag(&ev, "t", "p1"));
    }

    #[test]
    fn git_issue_rejects_empty_subject() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let err = build_git_issue(&repo, "", "body", &GitIssueMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_status_open_happy_path() {
        let root = event_id().to_hex();
        let meta = GitStatusMeta {
            root_event: root.clone(),
            ..Default::default()
        };
        let ev = sign(build_git_status(GitStatus::Open, "", &meta).unwrap());
        assert_eq!(ev.kind.as_u16(), 1630);
        assert!(has_tag(&ev, "e", &root));
    }

    #[test]
    fn git_status_merged_with_applied_patches() {
        let root = event_id().to_hex();
        let patch_id = event_id().to_hex();
        let merge_commit = "f".repeat(40);
        let meta = GitStatusMeta {
            root_event: root,
            applied_patches: vec![GitAppliedPatchRef {
                id: patch_id.clone(),
                relay: None,
                pubkey: None,
            }],
            merge_commit: Some(merge_commit.clone()),
            ..Default::default()
        };
        let ev =
            sign(build_git_status(GitStatus::AppliedOrResolved, "merged, thanks!", &meta).unwrap());
        assert_eq!(ev.kind.as_u16(), 1631);
        assert!(has_tag(&ev, "q", &patch_id));
        assert!(has_tag(&ev, "merge-commit", &merge_commit));
        assert!(has_tag(&ev, "r", &merge_commit));
    }

    #[test]
    fn git_status_rejects_merge_fields_on_non_merged_status() {
        let root = event_id().to_hex();
        let meta = GitStatusMeta {
            root_event: root,
            merge_commit: Some("f".repeat(40)),
            ..Default::default()
        };
        let err = build_git_status(GitStatus::Closed, "", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_status_rejects_bad_root_event() {
        let meta = GitStatusMeta {
            root_event: "not-an-event-id".to_string(),
            ..Default::default()
        };
        let err = build_git_status(GitStatus::Open, "", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_patch_rejects_empty_content() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let err = build_git_patch(&repo, "", &GitPatchMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_patch_rejects_whitespace_only_content() {
        // Regression: a failed `git format-patch | buzz patches send
        // --patch-file -` must not silently publish a whitespace-only
        // (i.e. unappliable) patch.
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let err = build_git_patch(&repo, "   \n\t\n", &GitPatchMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_repo_coord_rejects_invalid_repo_id_chars() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "../etc/passwd".to_string(),
        };
        let err = build_git_patch(&repo, "diff", &GitPatchMeta::default()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_patch_rejects_short_commit_hex() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let meta = GitPatchMeta {
            commit: Some("a".to_string()),
            ..Default::default()
        };
        let err = build_git_patch(&repo, "diff", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_patch_accepts_full_sha1_and_sha256_commit_hex() {
        let repo = GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        };
        let meta_sha1 = GitPatchMeta {
            commit: Some("c".repeat(40)),
            ..Default::default()
        };
        assert!(build_git_patch(&repo, "diff", &meta_sha1).is_ok());
        let meta_sha256 = GitPatchMeta {
            commit: Some("c".repeat(64)),
            ..Default::default()
        };
        assert!(build_git_patch(&repo, "diff", &meta_sha256).is_ok());
    }

    #[test]
    fn git_status_defaults_p_tag_to_repo_owner_when_repo_given() {
        // SDK-level: GitStatusMeta.recipients is the caller's responsibility
        // (the CLI defaults it), but verify the repo owner ends up p-tagged
        // when the CLI-style recipients list includes it.
        let owner = "a".repeat(64);
        let root = event_id().to_hex();
        let meta = GitStatusMeta {
            root_event: root,
            repo: Some(GitRepoCoord {
                owner: owner.clone(),
                id: "repo".to_string(),
            }),
            recipients: vec![owner.clone()],
            ..Default::default()
        };
        let ev = sign(build_git_status(GitStatus::Open, "", &meta).unwrap());
        assert!(has_tag(&ev, "p", &owner));
    }

    #[test]
    fn git_applied_patch_ref_parse_id_only() {
        let id = "a".repeat(64);
        let parsed = GitAppliedPatchRef::parse(&id).unwrap();
        assert_eq!(parsed.id, id);
        assert_eq!(parsed.relay, None);
        assert_eq!(parsed.pubkey, None);
    }

    #[test]
    fn git_applied_patch_ref_parse_id_and_relay() {
        let id = "a".repeat(64);
        let spec = format!("{id}:wss://relay.example.com");
        let parsed = GitAppliedPatchRef::parse(&spec).unwrap();
        assert_eq!(parsed.id, id);
        assert_eq!(parsed.relay, Some("wss://relay.example.com".to_string()));
        assert_eq!(parsed.pubkey, None);
    }

    #[test]
    fn git_applied_patch_ref_parse_id_relay_and_pubkey() {
        let id = "a".repeat(64);
        let pubkey = "b".repeat(64);
        let spec = format!("{id}:wss://relay.example.com:{pubkey}");
        let parsed = GitAppliedPatchRef::parse(&spec).unwrap();
        assert_eq!(parsed.id, id);
        assert_eq!(parsed.relay, Some("wss://relay.example.com".to_string()));
        assert_eq!(parsed.pubkey, Some(pubkey));
    }

    #[test]
    fn git_status_q_tag_includes_relay_and_pubkey_hints() {
        let root = event_id().to_hex();
        let patch_id = event_id().to_hex();
        let pubkey = "b".repeat(64);
        let meta = GitStatusMeta {
            root_event: root,
            applied_patches: vec![GitAppliedPatchRef {
                id: patch_id.clone(),
                relay: Some("wss://relay.example.com".to_string()),
                pubkey: Some(pubkey.clone()),
            }],
            ..Default::default()
        };
        let ev = sign(build_git_status(GitStatus::AppliedOrResolved, "", &meta).unwrap());
        let q_tag = ev
            .tags
            .iter()
            .find(|t| t.as_slice().first().map(|v| v.as_str()) == Some("q"))
            .expect("q tag present");
        let parts = q_tag.as_slice();
        assert_eq!(parts.get(1).map(|v| v.as_str()), Some(patch_id.as_str()));
        assert_eq!(
            parts.get(2).map(|v| v.as_str()),
            Some("wss://relay.example.com")
        );
        assert_eq!(parts.get(3).map(|v| v.as_str()), Some(pubkey.as_str()));
    }

    #[test]
    fn workflow_def_happy_path() {
        let cid = uuid();
        let wid = uuid();
        let ev = sign(build_workflow_def(cid, wid, "name: test\ntrigger:\n  on: webhook").unwrap());
        assert_eq!(ev.kind.as_u16(), 30620);
        assert!(has_tag(&ev, "d", &wid.to_string()));
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert!(ev.content.contains("name: test"));
    }

    #[test]
    fn workflow_def_rejects_oversized_yaml() {
        let big = "x".repeat(65 * 1024);
        let err = build_workflow_def(uuid(), uuid(), &big).unwrap_err();
        assert!(matches!(err, SdkError::ContentTooLarge { .. }));
    }

    #[test]
    fn workflow_update_includes_h_tag() {
        let cid = uuid();
        let wid = uuid();
        let ev = sign(build_workflow_update(cid, wid, "name: updated").unwrap());
        assert_eq!(ev.kind.as_u16(), 30620);
        assert!(has_tag(&ev, "d", &wid.to_string()));
        assert!(has_tag(&ev, "h", &cid.to_string()));
    }

    #[test]
    fn workflow_update_rejects_oversized_yaml() {
        let big = "x".repeat(65 * 1024);
        let err = build_workflow_update(uuid(), uuid(), &big).unwrap_err();
        assert!(matches!(err, SdkError::ContentTooLarge { .. }));
    }

    #[test]
    fn workflow_delete_happy_path() {
        let pk = "a".repeat(64);
        let wid = uuid();
        let ev = sign(build_workflow_delete(&pk, wid).unwrap());
        assert_eq!(ev.kind.as_u16(), 5);
        let a_vals = tag_values(&ev, "a");
        assert_eq!(a_vals.len(), 1);
        assert!(a_vals[0].starts_with("30620:"));
        assert!(a_vals[0].contains(&wid.to_string()));
    }

    #[test]
    fn workflow_delete_rejects_bad_pubkey() {
        let err = build_workflow_delete("bad", uuid()).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn workflow_trigger_happy_path() {
        let wid = uuid();
        let ev = sign(build_workflow_trigger(wid).unwrap());
        assert_eq!(ev.kind.as_u16(), 46020);
        assert!(has_tag(&ev, "d", &wid.to_string()));
    }

    #[test]
    fn workflow_approval_grant() {
        let hash = "a".repeat(64);
        let ev = sign(build_workflow_approval(&hash, true, "lgtm").unwrap());
        assert_eq!(ev.kind.as_u16(), 46030);
        assert!(has_tag(&ev, "d", &hash));
        assert_eq!(ev.content, "lgtm");
    }

    #[test]
    fn workflow_approval_deny() {
        let hash = "b".repeat(64);
        let ev = sign(build_workflow_approval(&hash, false, "").unwrap());
        assert_eq!(ev.kind.as_u16(), 46031);
    }

    #[test]
    fn workflow_approval_rejects_bad_token_hash() {
        let err = build_workflow_approval("not-hex", true, "").unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn workflow_approval_rejects_short_hash() {
        let short = "a".repeat(32);
        let err = build_workflow_approval(&short, true, "").unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn dm_open_happy_path() {
        let pk = "a".repeat(64);
        let ev = sign(build_dm_open(&[&pk]).unwrap());
        assert_eq!(ev.kind.as_u16(), 41010);
        assert!(has_tag(&ev, "p", &pk));
    }

    #[test]
    fn dm_open_rejects_empty_pubkeys() {
        let err = build_dm_open(&[]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn dm_open_rejects_over_8_pubkeys() {
        let pk = "a".repeat(64);
        let pks: Vec<&str> = vec![pk.as_str(); 9];
        let err = build_dm_open(&pks).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn dm_open_rejects_bad_pubkey() {
        let err = build_dm_open(&["bad-hex"]).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn dm_add_member_happy_path() {
        let cid = uuid();
        let pk = "b".repeat(64);
        let ev = sign(build_dm_add_member(cid, &pk).unwrap());
        assert_eq!(ev.kind.as_u16(), 41011);
        assert!(has_tag(&ev, "h", &cid.to_string()));
        assert!(has_tag(&ev, "p", &pk));
    }

    #[test]
    fn dm_add_member_rejects_bad_pubkey() {
        let err = build_dm_add_member(uuid(), "short").unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn presence_update_content_is_status() {
        let ev = sign(build_presence_update("online").unwrap());
        assert_eq!(ev.kind.as_u16(), 20001);
        assert_eq!(ev.content, "online");
        assert!(has_tag(&ev, "status", "online"));
    }

    #[test]
    fn presence_update_away() {
        let ev = sign(build_presence_update("away").unwrap());
        assert_eq!(ev.content, "away");
    }

    #[test]
    fn presence_update_offline() {
        let ev = sign(build_presence_update("offline").unwrap());
        assert_eq!(ev.content, "offline");
    }

    #[test]
    fn presence_update_rejects_invalid_status() {
        let err = build_presence_update("dnd").unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    // ── build_git_pull_request / build_git_pr_update ──────────────────────────

    fn pr_repo() -> GitRepoCoord {
        GitRepoCoord {
            owner: "a".repeat(64),
            id: "repo".to_string(),
        }
    }

    fn full_clone_tag(event: &nostr::Event) -> Vec<String> {
        event
            .tags
            .iter()
            .find(|t| t.as_slice().first().map(|v| v.as_str()) == Some("clone"))
            .map(|t| t.as_slice()[1..].iter().map(|v| v.to_string()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn git_pr_happy_path() {
        let meta = GitPullRequestMeta {
            subject: "Add feature X".to_string(),
            commit: "c".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            branch_name: Some("feat/x".to_string()),
            labels: vec!["enhancement".to_string()],
            channel_id: Some("11111111-1111-4111-8111-111111111111".to_string()),
            ..Default::default()
        };
        let ev = sign(build_git_pull_request(&pr_repo(), "PR body", &meta).unwrap());
        assert_eq!(ev.kind.as_u16(), 1618);
        assert_eq!(ev.content, "PR body");
        assert!(has_tag(&ev, "a", &format!("30617:{}:repo", "a".repeat(64))));
        assert!(has_tag(&ev, "p", &"a".repeat(64)));
        assert!(has_tag(&ev, "subject", "Add feature X"));
        assert!(has_tag(&ev, "c", &"c".repeat(40)));
        assert!(has_tag(&ev, "t", "enhancement"));
        assert!(has_tag(&ev, "h", "11111111-1111-4111-8111-111111111111"));
        assert!(has_tag(&ev, "branch-name", "feat/x"));
        assert_eq!(
            full_clone_tag(&ev),
            vec!["https://example.com/repo.git".to_string()]
        );
    }

    #[test]
    fn git_pr_emits_multi_url_clone_tag() {
        let meta = GitPullRequestMeta {
            subject: "s".to_string(),
            commit: "c".repeat(40),
            clone_urls: vec![
                "https://a.example/repo.git".to_string(),
                "https://b.example/repo.git".to_string(),
            ],
            ..Default::default()
        };
        let ev = sign(build_git_pull_request(&pr_repo(), "", &meta).unwrap());
        assert_eq!(full_clone_tag(&ev).len(), 2);
    }

    #[test]
    fn git_pr_rejects_empty_subject() {
        let meta = GitPullRequestMeta {
            subject: String::new(),
            commit: "c".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            ..Default::default()
        };
        let err = build_git_pull_request(&pr_repo(), "body", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_pr_rejects_missing_clone_url() {
        let meta = GitPullRequestMeta {
            subject: "s".to_string(),
            commit: "c".repeat(40),
            clone_urls: vec![],
            ..Default::default()
        };
        let err = build_git_pull_request(&pr_repo(), "body", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_pr_rejects_short_commit() {
        let meta = GitPullRequestMeta {
            subject: "s".to_string(),
            commit: "abc".to_string(),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            ..Default::default()
        };
        let err = build_git_pull_request(&pr_repo(), "body", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_pr_rejects_invalid_channel_id() {
        for channel_id in ["not-a-uuid", " 11111111-1111-4111-8111-111111111111 "] {
            let meta = GitPullRequestMeta {
                subject: "s".to_string(),
                commit: "c".repeat(40),
                clone_urls: vec!["https://example.com/repo.git".to_string()],
                channel_id: Some(channel_id.to_string()),
                ..Default::default()
            };
            let err = build_git_pull_request(&pr_repo(), "body", &meta).unwrap_err();
            assert!(matches!(err, SdkError::InvalidInput(_)));
        }
    }

    #[test]
    fn git_pr_canonicalizes_channel_id() {
        let meta = GitPullRequestMeta {
            subject: "s".to_string(),
            commit: "c".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            channel_id: Some("AAAAAAAA-BBBB-4CCC-8DDD-EEEEEEEEEEEE".to_string()),
            ..Default::default()
        };
        let ev = sign(build_git_pull_request(&pr_repo(), "body", &meta).unwrap());
        assert!(has_tag(&ev, "h", "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"));
    }

    #[test]
    fn git_pr_revision_of_emits_e_tag() {
        let patch = event_id().to_hex();
        let meta = GitPullRequestMeta {
            subject: "s".to_string(),
            commit: "c".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            revision_of: Some(patch.clone()),
            ..Default::default()
        };
        let ev = sign(build_git_pull_request(&pr_repo(), "", &meta).unwrap());
        assert!(has_tag(&ev, "e", &patch));
    }

    #[test]
    fn git_pr_update_happy_path() {
        let pr = event_id().to_hex();
        let meta = GitPrUpdateMeta {
            pr_event: pr.clone(),
            pr_author: "b".repeat(64),
            commit: "d".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            merge_base: Some("e".repeat(40)),
            ..Default::default()
        };
        let ev = sign(build_git_pr_update(&pr_repo(), "rebased", &meta).unwrap());
        assert_eq!(ev.kind.as_u16(), 1619);
        assert!(has_tag(&ev, "E", &pr));
        assert!(has_tag(&ev, "P", &"b".repeat(64)));
        assert!(has_tag(&ev, "c", &"d".repeat(40)));
        assert!(has_tag(&ev, "merge-base", &"e".repeat(40)));
        assert_eq!(full_clone_tag(&ev).len(), 1);
    }

    #[test]
    fn git_pr_update_rejects_bad_pr_event() {
        let meta = GitPrUpdateMeta {
            pr_event: "not-hex".to_string(),
            pr_author: "b".repeat(64),
            commit: "d".repeat(40),
            clone_urls: vec!["https://example.com/repo.git".to_string()],
            ..Default::default()
        };
        let err = build_git_pr_update(&pr_repo(), "", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn git_pr_update_rejects_missing_clone_url() {
        let meta = GitPrUpdateMeta {
            pr_event: event_id().to_hex(),
            pr_author: "b".repeat(64),
            commit: "d".repeat(40),
            clone_urls: vec![],
            ..Default::default()
        };
        let err = build_git_pr_update(&pr_repo(), "", &meta).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    // --- community moderation commands (9040–9044) ------------------------

    #[test]
    fn moderation_ban_permanent() {
        let pk = "a".repeat(64);
        let ev = sign(build_moderation_ban(&pk, None, None).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_BAN as u16);
        assert!(has_tag(&ev, "p", &pk));
        assert!(tag_values(&ev, "expiration").is_empty());
        assert!(tag_values(&ev, "reason").is_empty());
    }

    #[test]
    fn moderation_ban_temporary_with_reason() {
        let pk = "b".repeat(64);
        let ev = sign(build_moderation_ban(&pk, Some(1783500000), Some("spam")).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_BAN as u16);
        assert!(has_tag(&ev, "p", &pk));
        assert!(has_tag(&ev, "expiration", "1783500000"));
        assert!(has_tag(&ev, "reason", "spam"));
    }

    #[test]
    fn moderation_ban_lowercases_pubkey() {
        let ev = sign(build_moderation_ban(&"A".repeat(64), None, None).unwrap());
        assert!(has_tag(&ev, "p", &"a".repeat(64)));
    }

    #[test]
    fn moderation_ban_rejects_short_pubkey() {
        let err = build_moderation_ban("abc", None, None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn moderation_ban_rejects_overlong_pubkey() {
        // Relay `extract_p_tag_bytes` requires exactly 64 hex; the SDK must
        // reject 65+ hex here rather than sign a `p` tag the relay drops.
        let err = build_moderation_ban(&"a".repeat(65), None, None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn moderation_unban_shape() {
        let pk = "c".repeat(64);
        let ev = sign(build_moderation_unban(&pk).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_UNBAN as u16);
        assert!(has_tag(&ev, "p", &pk));
    }

    #[test]
    fn moderation_timeout_shape() {
        let pk = "d".repeat(64);
        let ev = sign(build_moderation_timeout(&pk, 1783500000, Some("cool off")).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_TIMEOUT as u16);
        assert!(has_tag(&ev, "p", &pk));
        assert!(has_tag(&ev, "expiration", "1783500000"));
        assert!(has_tag(&ev, "reason", "cool off"));
    }

    #[test]
    fn moderation_untimeout_shape() {
        let pk = "e".repeat(64);
        let ev = sign(build_moderation_untimeout(&pk).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_UNTIMEOUT as u16);
        assert!(has_tag(&ev, "p", &pk));
    }

    #[test]
    fn moderation_resolve_shape() {
        let rid = event_id().to_hex();
        let ev =
            sign(build_moderation_resolve_report(&rid, "resolved", "ban", Some("rule 3")).unwrap());
        assert_eq!(ev.kind.as_u16(), KIND_MODERATION_RESOLVE_REPORT as u16);
        assert!(has_tag(&ev, "report", &rid));
        assert!(has_tag(&ev, "status", "resolved"));
        assert!(has_tag(&ev, "action", "ban"));
        assert!(has_tag(&ev, "reason", "rule 3"));
    }

    #[test]
    fn moderation_resolve_dismiss_no_reason() {
        let rid = event_id().to_hex();
        let ev = sign(build_moderation_resolve_report(&rid, "dismissed", "dismiss", None).unwrap());
        assert!(has_tag(&ev, "status", "dismissed"));
        assert!(has_tag(&ev, "action", "dismiss"));
        assert!(tag_values(&ev, "reason").is_empty());
    }

    #[test]
    fn moderation_resolve_rejects_bad_status() {
        let rid = event_id().to_hex();
        let err = build_moderation_resolve_report(&rid, "escalated", "ban", None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn moderation_resolve_rejects_bad_action() {
        let rid = event_id().to_hex();
        let err = build_moderation_resolve_report(&rid, "resolved", "nuke", None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn moderation_resolve_rejects_short_report_id() {
        let err = build_moderation_resolve_report("abc", "resolved", "ban", None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn moderation_resolve_rejects_overlong_report_id() {
        // Relay `extract_report_tag` requires exactly 64 hex; the SDK must
        // reject 65+ hex here rather than sign a `report` tag the relay drops.
        let err =
            build_moderation_resolve_report(&"a".repeat(65), "resolved", "ban", None).unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    // ── NIP-IA identity archival (kinds 9035/9036) ────────────────────────
    //
    // Mirrors `desktop/src-tauri/src/events.rs`'s own tests so both clients
    // are pinned to the same wire form. See NIP-IA.md §Vector 1.

    const IA_OWNER_HEX: &str = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
    const IA_TARGET_HEX: &str = "c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
    const IA_CONDITIONS: &str = "kind=1&created_at<1713957000";
    const IA_SIG: &str = "8b7df2575caf0a108374f8471722b233c53f9ff827a8b0f91861966c3b9dd5cb2e189eae9f49d72187674c2f5bd244145e10ff86c9f257ffe65a1ee5f108b369";

    #[test]
    fn archive_identity_request_matches_spec_vector_1_layout() {
        let auth: [String; 4] = [
            "auth".into(),
            IA_OWNER_HEX.into(),
            IA_CONDITIONS.into(),
            IA_SIG.into(),
        ];
        let ev = sign(
            build_archive_identity_request(
                IA_TARGET_HEX,
                "Archiving zombie agent after rebuild.",
                Some("bot-rebuilt"),
                None,
                Some(&auth),
            )
            .unwrap(),
        );

        let tags: Vec<Vec<String>> = ev.tags.iter().map(|t| t.as_slice().to_vec()).collect();
        assert_eq!(ev.kind, Kind::Custom(KIND_IA_ARCHIVE_REQUEST as u16));
        // Spec layout: ["-"], ["p", target], ["reason", code], ["auth", ...]
        assert_eq!(tags[0], vec!["-"]);
        assert_eq!(tags[1], vec!["p", IA_TARGET_HEX]);
        assert_eq!(tags[2], vec!["reason", "bot-rebuilt"]);
        assert_eq!(tags[3], vec!["auth", IA_OWNER_HEX, IA_CONDITIONS, IA_SIG]);
        assert_eq!(tags.len(), 4);
    }

    #[test]
    fn archive_request_rejects_replaced_by_equal_target() {
        let err =
            build_archive_identity_request(IA_TARGET_HEX, "", None, Some(IA_TARGET_HEX), None)
                .unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn unarchive_request_layout_self_path() {
        // Self-unarchive: actor == target, so the `p` tag points at the
        // signer. Pins that `.allow_self_tagging()` survives nostr 0.44's
        // default same-pubkey `p`-tag scrub, and that no `auth` tag rides
        // along on the self path.
        let builder = build_unarchive_identity_request(
            IA_TARGET_HEX,
            "I am active again.",
            Some("returned"),
            None,
        )
        .unwrap();
        let target_secret = nostr::SecretKey::from_hex(
            "0000000000000000000000000000000000000000000000000000000000000002",
        )
        .unwrap();
        let ev = builder
            .sign_with_keys(&Keys::new(target_secret))
            .expect("sign");
        let tags: Vec<Vec<String>> = ev.tags.iter().map(|t| t.as_slice().to_vec()).collect();
        assert_eq!(ev.kind, Kind::Custom(KIND_IA_UNARCHIVE_REQUEST as u16));
        assert_eq!(tags[0], vec!["-"]);
        assert_eq!(tags[1], vec!["p", IA_TARGET_HEX]);
        assert_eq!(tags[2], vec!["reason", "returned"]);
        assert_eq!(tags.len(), 3, "self unarchive must not carry an auth tag");
        assert_eq!(ev.pubkey.to_hex(), IA_TARGET_HEX);
    }

    #[test]
    fn identity_archive_reason_accepts_64_bytes() {
        // check_reason compares `.len()` (bytes), not chars — use a
        // multi-byte char so a chars-based off-by-one would slip through.
        let reason: String = "é".repeat(32); // 32 * 2 bytes = 64 bytes exactly
        assert_eq!(reason.len(), 64);
        build_archive_identity_request(IA_TARGET_HEX, "", Some(&reason), None, None)
            .expect("64-byte reason must be accepted");
    }

    #[test]
    fn identity_archive_reason_rejects_65_bytes() {
        let reason: String = "a".repeat(65);
        let err = build_archive_identity_request(IA_TARGET_HEX, "", Some(&reason), None, None)
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn identity_archive_reason_rejects_control_chars() {
        let err = build_unarchive_identity_request(IA_TARGET_HEX, "", Some("bad\nreason"), None)
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn identity_archive_rejects_malformed_auth_tag() {
        let bad_auth: [String; 4] = [
            "auth".into(),
            IA_OWNER_HEX.into(),
            IA_CONDITIONS.into(),
            "not-hex".into(),
        ];
        let err = build_archive_identity_request(IA_TARGET_HEX, "", None, None, Some(&bad_auth))
            .unwrap_err();
        assert!(matches!(err, SdkError::InvalidInput(_)));
    }

    #[test]
    fn unarchive_request_has_no_replaced_by_param() {
        // 9036 has no `replaced_by` parameter at all (unlike 9035) — this is
        // a compile-time guarantee, but pin the tag count so a future
        // signature change that reintroduces it doesn't silently widen the
        // wire form without a test failing.
        let ev = sign(
            build_unarchive_identity_request(IA_TARGET_HEX, "", Some("returned"), None).unwrap(),
        );
        assert!(!ev
            .tags
            .iter()
            .any(|t| t.as_slice().first().map(String::as_str) == Some("replaced-by")));
    }

    #[test]
    fn meeting_v1_attempt_finish_rejects_reason_from_the_wrong_terminal_class() {
        let attempt_id = event_id().to_hex();
        let error =
            build_meeting_v1_decision_attempt_finish(MeetingV1DecisionAttemptFinishParams {
                session_id: uuid(),
                attempt_id: &attempt_id,
                outcome: MeetingV1DecisionAttemptFinishOutcome::Completed,
                reason_code: "human_priority",
            })
            .unwrap_err();
        assert!(matches!(error, SdkError::InvalidInput(_)));
    }

    #[test]
    fn meeting_v1_attempt_finish_cannot_replace_complete_cohort() {
        let attempt_id = event_id().to_hex();
        let error =
            build_meeting_v1_decision_attempt_finish(MeetingV1DecisionAttemptFinishParams {
                session_id: uuid(),
                attempt_id: &attempt_id,
                outcome: MeetingV1DecisionAttemptFinishOutcome::Completed,
                reason_code: "cohort_complete",
            })
            .unwrap_err();
        assert!(matches!(error, SdkError::InvalidInput(_)));
    }
}
