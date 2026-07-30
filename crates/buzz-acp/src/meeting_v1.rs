//! Meeting V1 participant and moderator controller for ACP-managed Agents.
//!
//! It owns one shared synchronizer per V1 Session, deterministic Offer
//! handling, durable prepared events, Progress heartbeats, participant Intent
//! and Grant-bound Speech turns, plus one authoritative moderator decision
//! attempt while the moderator owns the Control Token.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use buzz_core::kind::{
    KIND_MEETING_CREATE, KIND_MEETING_END, KIND_MEETING_GRANT_SIGNAL,
    KIND_MEETING_HUMAN_FLOOR_REQUEST, KIND_MEETING_MODERATOR_COMMAND, KIND_MEETING_OFFER_RESPONSE,
    KIND_MEETING_ROUND_STATE, KIND_MEETING_SPEECH_INTENT, KIND_NIP29_GROUP_MEMBERS,
    KIND_NIP29_GROUP_METADATA, KIND_STREAM_MESSAGE,
};
use buzz_sdk::{
    MeetingV1CompleteCohortParams, MeetingV1DecisionAttemptAbandonParams,
    MeetingV1DecisionAttemptFinishOutcome, MeetingV1DecisionAttemptFinishParams,
    MeetingV1DecisionAttemptStartParams, MeetingV1DecisionRetryParams, MeetingV1DirectedHandoff,
    MeetingV1GrantProgressParams, MeetingV1GrantYieldParams, MeetingV1GrantYieldReason,
    MeetingV1HandoffDismissReason, MeetingV1HandoffType, MeetingV1IntentDeferral,
    MeetingV1IntentRefreshParams, MeetingV1IntentRejectionReason, MeetingV1IntentSubmitParams,
    MeetingV1ModeratorDismissHandoffParams, MeetingV1ModeratorRejectParams,
    MeetingV1ModeratorSelectParams, MeetingV1ModeratorWithdrawSelfParams, MeetingV1OfferAckParams,
    MeetingV1OfferDeclineParams, MeetingV1ProgressStage, MeetingV1Selection, MeetingV1SpeechParams,
};
use futures_util::FutureExt;
use nostr::{Alphabet, Event, Filter, Keys, Kind, PublicKey, SingleLetterTag};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::meeting::{
    fetch_meeting_history, now_ms, sign_builder, tag_value, validate_bounded_text, MeetingTurnKind,
    MeetingTurnRequest,
};
#[cfg(feature = "meeting-v1-acceptance")]
use crate::meeting_acceptance::{
    self, AcceptanceCandidateRef, PreSubmitAcceptanceBarrier, PreSubmitBarrierFrame,
};
use crate::observer::{self, ObserverHandle};
use crate::relay::{BuzzEvent, ProtocolSubmitOutcome, ProtocolSubmitRejected, RestClient};

const LEDGER_VERSION: u32 = 4;
const PREVIOUS_LEDGER_VERSION: u32 = 3;
const SYNC_INTERVAL: Duration = Duration::from_secs(30);
const SYNC_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const SYNC_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
const TERMINAL_LEDGER_CLEANUP_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const PROTOCOL_SUBMIT_TIMEOUT: Duration = Duration::from_secs(2);
const INTENT_MAX_DURATION: Duration = Duration::from_secs(5 * 60);
const DEFAULT_GRANT_SAFETY_MARGIN: Duration = Duration::from_secs(30);
const PROMPT_SPEECH_LIMIT: usize = 100;
const PROMPT_CONTENT_LIMIT: usize = 128 * 1024;
const MAX_INTENT_SUMMARY_BYTES: usize = 512;
const MAX_REASON_BYTES: usize = 1024;
const MAX_SPEECH_BYTES: usize = 256 * 1024;
const MAX_MENTIONS: usize = 12;
const MAX_MODERATOR_CLEANUPS: usize = 8;
const MODERATOR_DEADLINE_SAFETY_MARGIN: Duration = Duration::from_secs(15);
const DEFAULT_MODERATOR_DECISION_DURATION: Duration = Duration::from_secs(3 * 60);
const MAX_MODERATOR_FAST_REBASES: u8 = 3;
const MODERATOR_REBASE_QUIESCENCE: Duration = Duration::from_millis(250);

const PARTICIPANT_INTENT_PROMPT: &str = include_str!("meeting_participant_intent_prompt.md");
const GRANTED_SPEECH_PROMPT: &str = include_str!("meeting_granted_speech_prompt.md");
const MODERATOR_PROMPT: &str = include_str!("meeting_moderator_prompt.md");

#[derive(Debug, Clone)]
struct MeetingRuntime {
    epoch: u64,
    view: Option<MeetingView>,
    last_sync: Option<Instant>,
    retry_at: Instant,
    control_retry_at: Option<Instant>,
    moderator_rebase_at: Option<Instant>,
    sync_in_flight: Option<u64>,
    sync_requested: u64,
    queued: bool,
    in_flight_turn: Option<String>,
}

impl MeetingRuntime {
    fn new(epoch: u64) -> Self {
        Self {
            epoch,
            view: None,
            last_sync: None,
            retry_at: Instant::now(),
            control_retry_at: None,
            moderator_rebase_at: None,
            sync_in_flight: None,
            sync_requested: 0,
            queued: false,
            in_flight_turn: None,
        }
    }
}

#[derive(Debug, Clone)]
struct MeetingView {
    session_id: Uuid,
    title: String,
    description: Option<String>,
    ended: bool,
    relay_pubkey: String,
    roster: BTreeMap<String, Participant>,
    speeches: Vec<Speech>,
    intents: BTreeMap<String, IntentContext>,
    speech_cursor: Option<String>,
    baton: BatonView,
}

#[derive(Debug, Clone, Serialize)]
struct Participant {
    pubkey: String,
    role: String,
    participant_type: String,
    display_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct Speech {
    event_id: String,
    author_pubkey: String,
    author_display_name: String,
    content: String,
    created_at: u64,
    speech_revision: u64,
    grant_id: String,
    mentions: Vec<String>,
    handoff: Option<SpeechHandoff>,
}

#[derive(Debug, Clone, Serialize)]
struct SpeechHandoff {
    target_pubkey: String,
    handoff_type: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct IntentContext {
    intent_id: String,
    current_event_id: String,
    author_pubkey: String,
    summary: String,
    addressed_to: Option<String>,
    basis_speech_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatonConfigView {
    progress_interval_ms: i64,
    grant_hard_deadline_ms: i64,
    agent_safety_margin_ms: i64,
    #[serde(default = "default_moderator_max_rejudgments")]
    moderator_max_rejudgments: u64,
    #[serde(default = "default_moderator_max_cas_rebases")]
    moderator_max_cas_rebases_per_attempt: u64,
}

const fn default_moderator_max_rejudgments() -> u64 {
    2
}

const fn default_moderator_max_cas_rebases() -> u64 {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingIntentView {
    intent_id: String,
    current_event_id: String,
    author_pubkey: String,
    basis_speech_revision: u64,
    summary: String,
    addressed_to: Option<String>,
    created_at_ms: i64,
    deferred: bool,
    #[serde(default)]
    selection_attempt_count: u64,
    #[serde(default)]
    last_offer_id: Option<String>,
    #[serde(default)]
    last_attempt_outcome: Option<String>,
    #[serde(default)]
    eligible_decision_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HumanQueueView {
    request_id: String,
    requester_pubkey: String,
    queue_position: i64,
    state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HandoffContextView {
    from_pubkey: String,
    reason_type: String,
    reason_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OfferView {
    offer_id: String,
    target_pubkey: String,
    target_participant_type: String,
    allocation_source: String,
    turn_role: String,
    source_intent_id: Option<String>,
    source_request_id: Option<String>,
    source_handoff_id: Option<String>,
    source_speech_event_id: Option<String>,
    handoff_context: Option<HandoffContextView>,
    created_at_ms: i64,
    ack_deadline_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantView {
    grant_id: String,
    holder_pubkey: String,
    allocation_source: String,
    turn_role: String,
    source_offer_id: String,
    source_intent_id: Option<String>,
    source_request_id: Option<String>,
    source_handoff_id: Option<String>,
    source_speech_event_id: Option<String>,
    handoff_context: Option<HandoffContextView>,
    basis_speech_revision: u64,
    soft_lease_expires_at_ms: i64,
    hard_deadline_ms: i64,
    progress_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OpenHandoffView {
    handoff_id: String,
    source_speech_event_id: String,
    from_pubkey: String,
    to_pubkey: String,
    reason_type: String,
    reason_text: String,
    question_state: String,
    attempt_count: u64,
    last_offer_id: Option<String>,
    last_grant_id: Option<String>,
    last_attempt_outcome: Option<String>,
    blocked_by: Option<String>,
    #[serde(default)]
    moderator_retry_blocked: bool,
    #[serde(default)]
    eligible_decision_epoch: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct DecisionCandidateRef {
    source_type: String,
    source_id: String,
    #[serde(default)]
    current_event_id: Option<String>,
    #[serde(default)]
    author_pubkey: Option<String>,
    #[serde(default)]
    moderator_self: bool,
    #[serde(default)]
    basis_speech_revision: Option<u64>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    addressed_to: Option<String>,
    #[serde(default)]
    source_speech_event_id: Option<String>,
    #[serde(default)]
    from_pubkey: Option<String>,
    #[serde(default)]
    target_pubkey: Option<String>,
    #[serde(default)]
    reason_type: Option<String>,
    #[serde(default)]
    reason_text: Option<String>,
    #[serde(default)]
    attempt_count: Option<u64>,
    eligible_decision_epoch: u64,
    created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ActiveDecisionAttemptView {
    attempt_id: String,
    control_epoch: u64,
    decision_epoch: u64,
    attempt_number: u64,
    speech_revision: u64,
    snapshot_intent_revision: u64,
    snapshot_state_event_id: String,
    candidate_refs: Vec<DecisionCandidateRef>,
    candidate_snapshot_hash: String,
    started_at_ms: i64,
    deadline_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BatonView {
    /// Complete Relay-signed State content retained for prompts and forward
    /// compatibility. Parsed fields below drive deterministic control logic.
    #[serde(skip_serializing)]
    raw_state: Value,
    state_event_id: String,
    phase: String,
    state_revision: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    control_epoch: u64,
    decision_epoch: u64,
    decision_attempt: u64,
    active_decision_attempt: Option<ActiveDecisionAttemptView>,
    moderator_pubkey: String,
    baton_config: BatonConfigView,
    pending_intents: Vec<PendingIntentView>,
    human_queue: Vec<HumanQueueView>,
    unresolved_handoffs: Vec<OpenHandoffView>,
    handoff_depth: u64,
    consecutive_moderator_speeches: u64,
    forced_return_to_moderator: bool,
    moderator_decision_deadline_ms: Option<i64>,
    next_action_at_ms: Option<i64>,
    offer: Option<OfferView>,
    grant: Option<GrantView>,
}

#[derive(Debug, Deserialize)]
struct RawBatonState {
    phase: String,
    state_revision: u64,
    floor_revision: u64,
    intent_revision: u64,
    speech_revision: u64,
    control_epoch: u64,
    decision_epoch: u64,
    #[serde(default)]
    decision_attempt: u64,
    #[serde(default)]
    active_decision_attempt: Option<ActiveDecisionAttemptView>,
    moderator_pubkey: String,
    baton_config: BatonConfigView,
    participants: Vec<RawParticipant>,
    #[serde(default)]
    pending_intents: Vec<PendingIntentView>,
    #[serde(default)]
    human_queue: Vec<HumanQueueView>,
    #[serde(default)]
    unresolved_handoffs: Vec<OpenHandoffView>,
    #[serde(default)]
    handoff_depth: u64,
    #[serde(default)]
    consecutive_moderator_speeches: u64,
    #[serde(default)]
    forced_return_to_moderator: bool,
    moderator_decision_deadline_ms: Option<i64>,
    next_action_at_ms: Option<i64>,
    offer: Option<OfferView>,
    grant: Option<GrantView>,
}

#[derive(Debug, Deserialize)]
struct RawParticipant {
    pubkey: String,
    participant_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeratorRejection {
    intent_id: String,
    reason_code: String,
    reason_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeratorHandoffDismissal {
    handoff_id: String,
    reason_code: String,
    reason_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeratorDeferral {
    intent_id: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModeratorNextAction {
    action: String,
    id: Option<String>,
    reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ControlOutput {
    #[serde(default)]
    rejections: Vec<ModeratorRejection>,
    #[serde(default)]
    handoff_dismissals: Vec<ModeratorHandoffDismissal>,
    #[serde(default)]
    deferrals: Vec<ModeratorDeferral>,
    next_action: ModeratorNextAction,
}

#[derive(Debug, Clone)]
enum ModeratorActionSpec {
    Reject {
        candidate: DecisionCandidateRef,
        proposal: ModeratorRejection,
    },
    Dismiss {
        candidate: DecisionCandidateRef,
        proposal: ModeratorHandoffDismissal,
    },
    SelectIntent {
        candidate: DecisionCandidateRef,
        reason: String,
        moderator_self: bool,
    },
    SelectHandoff {
        candidate: DecisionCandidateRef,
        reason: String,
    },
    WithdrawSelf {
        candidate: DecisionCandidateRef,
    },
    Idle,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AgentLedger {
    version: u32,
    agent_pubkey: String,
    meetings: BTreeMap<String, MeetingLedger>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MeetingLedger {
    session_id: String,
    agent_pubkey: String,
    meeting_synced: bool,
    state_revision: u64,
    speech_revision: u64,
    speech_cursor: Option<String>,
    seen_speech_ids: BTreeSet<String>,
    triggers: BTreeMap<String, TriggerRecord>,
    reservations: BTreeMap<String, ReservationRecord>,
    grants: BTreeMap<String, GrantRecord>,
    #[serde(default)]
    moderator_decision: Option<ModeratorDecisionRecord>,
    #[serde(default)]
    prepared_moderator_action: Option<PreparedModeratorAction>,
    #[serde(default)]
    replacement_attempt_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModeratorDecisionRecord {
    attempt: ActiveDecisionAttemptView,
    rejections: Vec<ModeratorRejection>,
    handoff_dismissals: Vec<ModeratorHandoffDismissal>,
    deferrals: Vec<ModeratorDeferral>,
    next_action: ModeratorNextAction,
    state: String,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    turn_started_at_ms: Option<i64>,
    #[serde(default)]
    cas_rebases: u8,
    #[serde(default)]
    fast_rebases: u8,
    #[serde(default)]
    pending_retry: Option<PendingModeratorRetry>,
    #[serde(default)]
    pending_finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingModeratorRetry {
    retry_ticket_id: String,
    failed_action_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedModeratorAction {
    action_kind: String,
    object_id: String,
    #[serde(default)]
    attempt_id: Option<String>,
    event: Value,
    event_id: String,
    state: String,
    created_at_ms: i64,
    #[serde(default)]
    hard_deadline_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TriggerRecord {
    trigger_id: String,
    source_event_id: Option<String>,
    basis_speech_revision: u64,
    created_at_ms: i64,
    state: String,
    prepared_event: Option<Value>,
    prepared_event_id: Option<String>,
    #[serde(default)]
    format_attempts: u8,
    #[serde(default)]
    hard_deadline_unix_ms: Option<i64>,
}

impl TriggerRecord {
    fn new(
        trigger_id: String,
        source_event_id: Option<String>,
        basis_speech_revision: u64,
    ) -> Self {
        Self {
            trigger_id,
            source_event_id,
            basis_speech_revision,
            created_at_ms: now_ms(),
            state: "pending".to_string(),
            prepared_event: None,
            prepared_event_id: None,
            format_attempts: 0,
            hard_deadline_unix_ms: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReservationRecord {
    offer_id: String,
    state: String,
    ack_event: Option<Value>,
    decline_event: Option<Value>,
    created_at_ms: i64,
    /// Conservative upper bound for holding a local slot across restart. Relay
    /// State reconciliation normally releases it earlier.
    #[serde(default)]
    capacity_expires_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PreparedProgress {
    seq: u64,
    event: Value,
    state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantRecord {
    grant_id: String,
    source_offer_id: String,
    state: String,
    basis_speech_revision: u64,
    soft_lease_expires_at_ms: i64,
    hard_deadline_ms: i64,
    progress_seq: u64,
    next_progress_at_ms: i64,
    prepared_progress: Option<PreparedProgress>,
    speech_event: Option<Value>,
    speech_event_id: Option<String>,
    yield_event: Option<Value>,
    #[serde(default)]
    format_attempts: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IntentOutput {
    action: String,
    summary: Option<String>,
    addressed_to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantedOutput {
    action: String,
    content: Option<String>,
    #[serde(default)]
    mention_pubkeys: Vec<String>,
    handoff: Option<GrantedHandoffOutput>,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantedHandoffOutput {
    target_pubkey: String,
    handoff_type: String,
    reason: String,
}

#[derive(Debug)]
enum ProtocolSubmitFailure {
    Rejected(ProtocolSubmitRejected),
    Uncertain(String),
}

impl std::fmt::Display for ProtocolSubmitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(rejection) => write!(
                formatter,
                "Relay rejected the event ({}): {}",
                rejection.code, rejection.message
            ),
            Self::Uncertain(message) => write!(formatter, "submission is uncertain: {message}"),
        }
    }
}

impl ProtocolSubmitFailure {
    fn is_uncertain(&self) -> bool {
        matches!(self, Self::Uncertain(_))
    }
}

struct SyncTaskResult {
    session_id: Uuid,
    session_epoch: u64,
    request_id: u64,
    result: std::result::Result<MeetingView, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncApplyResult {
    Applied,
    Superseded,
    Failed,
}

struct DeferredTurnResult {
    request_id: u64,
    session_epoch: u64,
    turn_id: String,
    request: MeetingTurnRequest,
    raw_output: String,
    succeeded: bool,
}

struct ProgressTaskResult {
    session_id: Uuid,
    session_epoch: u64,
    grant_id: String,
    submission_id: u64,
    event_id: String,
    progress_seq: u64,
    stage: MeetingV1ProgressStage,
    result: std::result::Result<Value, ProtocolSubmitFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProgressInFlight {
    session_epoch: u64,
    submission_id: u64,
    event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ProtocolSubmissionKey {
    Offer {
        session_id: Uuid,
        offer_id: String,
    },
    Intent {
        session_id: Uuid,
        trigger_id: String,
    },
    GrantTerminal {
        session_id: Uuid,
        grant_id: String,
    },
    Moderator {
        session_id: Uuid,
        event_id: String,
    },
}

impl ProtocolSubmissionKey {
    fn session_id(&self) -> Uuid {
        match self {
            Self::Offer { session_id, .. }
            | Self::Intent { session_id, .. }
            | Self::GrantTerminal { session_id, .. }
            | Self::Moderator { session_id, .. } => *session_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfferSubmissionAction {
    Ack,
    Decline,
}

impl OfferSubmissionAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ack => "ack",
            Self::Decline => "decline",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GrantTerminalAction {
    Speech,
    Yield,
}

impl GrantTerminalAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Speech => "speech",
            Self::Yield => "yield",
        }
    }
}

#[derive(Debug)]
enum ProtocolSubmissionContext {
    Offer {
        offer_id: String,
        action: OfferSubmissionAction,
        allocation_source: String,
        turn_role: String,
        created_at_ms: i64,
    },
    Intent {
        trigger_id: String,
        turn_id: Option<String>,
        queued_at_ms: Option<i64>,
    },
    GrantTerminal {
        grant_id: String,
        source_offer_id: String,
        action: GrantTerminalAction,
        turn_id: Option<String>,
        queued_at_ms: Option<i64>,
        grant_started_at_ms: Option<i64>,
    },
    Moderator {
        action_kind: String,
        object_id: String,
        attempt_id: Option<String>,
        turn_id: Option<String>,
        queued_at_ms: Option<i64>,
        #[cfg(feature = "meeting-v1-acceptance")]
        barrier: Option<Box<(PathBuf, PreSubmitBarrierFrame)>>,
    },
}

struct ProtocolTaskResult {
    key: ProtocolSubmissionKey,
    session_epoch: u64,
    submission_id: u64,
    event_id: String,
    context: ProtocolSubmissionContext,
    result: std::result::Result<Value, ProtocolSubmitFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolInFlight {
    session_epoch: u64,
    submission_id: u64,
    event_id: String,
}

/// Participant and moderator V1 controller.
pub(super) struct MeetingV1Coordinator {
    rest: RestClient,
    keys: Keys,
    agent_pubkey: String,
    observer: Option<ObserverHandle>,
    agent_capacity: usize,
    available_agent_slots: usize,
    auto_accept_offers: bool,
    ledger_path: PathBuf,
    ledger: AgentLedger,
    terminal_ledger_cleanup_retry_at: Option<Instant>,
    meetings: HashMap<Uuid, MeetingRuntime>,
    pending: VecDeque<MeetingTurnRequest>,
    in_flight: HashMap<String, MeetingTurnRequest>,
    in_flight_epochs: HashMap<String, u64>,
    external_reclaimable_turns: BTreeSet<Uuid>,
    preemptions: BTreeSet<Uuid>,
    next_session_epoch: u64,
    next_sync_request_id: u64,
    sync_result_tx: tokio::sync::mpsc::UnboundedSender<SyncTaskResult>,
    sync_result_rx: tokio::sync::mpsc::UnboundedReceiver<SyncTaskResult>,
    deferred_turn_results: HashMap<Uuid, DeferredTurnResult>,
    next_protocol_submission_id: u64,
    protocol_in_flight: HashMap<ProtocolSubmissionKey, ProtocolInFlight>,
    protocol_result_tx: tokio::sync::mpsc::UnboundedSender<ProtocolTaskResult>,
    protocol_result_rx: tokio::sync::mpsc::UnboundedReceiver<ProtocolTaskResult>,
    next_progress_submission_id: u64,
    progress_in_flight: HashMap<(Uuid, String), ProgressInFlight>,
    progress_waiting_for_state: HashMap<(Uuid, String), u64>,
    progress_result_tx: tokio::sync::mpsc::UnboundedSender<ProgressTaskResult>,
    progress_result_rx: tokio::sync::mpsc::UnboundedReceiver<ProgressTaskResult>,
    #[cfg(feature = "meeting-v1-acceptance")]
    acceptance_barrier: PreSubmitAcceptanceBarrier,
}

impl MeetingV1Coordinator {
    pub(super) fn new(
        rest: RestClient,
        keys: Keys,
        observer: Option<ObserverHandle>,
        agent_capacity: usize,
    ) -> Self {
        let agent_pubkey = keys.public_key().to_hex();
        let ledger_path = ledger_path_for(&agent_pubkey);
        let mut ledger = load_ledger(&ledger_path).unwrap_or_else(|error| {
            tracing::warn!(
                path = %ledger_path.display(),
                "Meeting V1 ledger could not be loaded: {error}; rebuilding from Relay State"
            );
            AgentLedger::default()
        });
        let migrated = migrate_loaded_ledger(&mut ledger, &agent_pubkey, &ledger_path);
        let (_, _, recovered) = recover_interrupted_turns(&mut ledger);
        if migrated || recovered {
            if let Err(error) = persist_ledger(&ledger_path, &ledger) {
                tracing::warn!(
                    path = %ledger_path.display(),
                    "recovered Meeting V1 ledger could not be persisted: {error}"
                );
            }
        }
        let auto_accept_offers = std::env::var("BUZZ_ACP_MEETING_V1_AUTO_ACCEPT")
            .ok()
            .is_none_or(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        let (sync_result_tx, sync_result_rx) = tokio::sync::mpsc::unbounded_channel();
        let (protocol_result_tx, protocol_result_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_result_tx, progress_result_rx) = tokio::sync::mpsc::unbounded_channel();
        Self {
            rest,
            keys,
            agent_pubkey,
            observer,
            agent_capacity,
            available_agent_slots: agent_capacity,
            auto_accept_offers,
            ledger_path,
            ledger,
            terminal_ledger_cleanup_retry_at: None,
            meetings: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: HashMap::new(),
            in_flight_epochs: HashMap::new(),
            external_reclaimable_turns: BTreeSet::new(),
            preemptions: BTreeSet::new(),
            next_session_epoch: 0,
            next_sync_request_id: 0,
            sync_result_tx,
            sync_result_rx,
            deferred_turn_results: HashMap::new(),
            next_protocol_submission_id: 0,
            protocol_in_flight: HashMap::new(),
            protocol_result_tx,
            protocol_result_rx,
            next_progress_submission_id: 0,
            progress_in_flight: HashMap::new(),
            progress_waiting_for_state: HashMap::new(),
            progress_result_tx,
            progress_result_rx,
            #[cfg(feature = "meeting-v1-acceptance")]
            acceptance_barrier: PreSubmitAcceptanceBarrier::from_env(),
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn set_available_agent_slots(&mut self, available: usize) {
        self.available_agent_slots = available.min(self.agent_capacity);
    }

    pub(super) fn set_external_reclaimable_turns(&mut self, sessions: BTreeSet<Uuid>) {
        self.external_reclaimable_turns = sessions;
    }

    pub(super) fn unassigned_reserved_slots(&self) -> usize {
        let active = self.active_reservation_count(None);
        let assigned: BTreeSet<_> = self
            .in_flight
            .values()
            .filter(|request| self.granted_request_uses_active_reservation(request))
            .map(|request| request.session_id)
            .collect();
        active
            .saturating_sub(assigned.len())
            .min(self.agent_capacity)
    }

    pub(super) fn front_kind(&self) -> Option<MeetingTurnKind> {
        self.pending.front().map(|request| request.kind)
    }

    pub(super) fn pop_pending(&mut self) -> Option<MeetingTurnRequest> {
        self.pending.pop_front()
    }

    pub(super) fn requeue_front(&mut self, request: MeetingTurnRequest) {
        self.pending.push_front(request);
    }

    pub(super) fn mark_dispatched(&mut self, turn_id: String, request: MeetingTurnRequest) {
        let turn_started_at_ms = now_ms();
        let moderator_turn = request.kind == MeetingTurnKind::V1ModeratorControl;
        let session_epoch = self
            .meetings
            .get(&request.session_id)
            .map(|runtime| runtime.epoch);
        if let Some(runtime) = self.meetings.get_mut(&request.session_id) {
            runtime.queued = false;
            runtime.in_flight_turn = Some(turn_id.clone());
        }
        match request.kind {
            MeetingTurnKind::V1Intent => {
                if let Some(trigger) = self
                    .ledger_for_mut(request.session_id)
                    .and_then(|ledger| ledger.triggers.get_mut(&request.basis_id))
                {
                    trigger.state = "running".to_string();
                }
            }
            MeetingTurnKind::V1Granted => {
                if let Some(grant_id) = request.grant_event_id.as_deref() {
                    if let Some(grant) = self
                        .ledger_for_mut(request.session_id)
                        .and_then(|ledger| ledger.grants.get_mut(grant_id))
                    {
                        grant.state = "running".to_string();
                    }
                }
            }
            MeetingTurnKind::V1ModeratorControl => {
                if let Some(decision) = self
                    .ledger_for_mut(request.session_id)
                    .and_then(|ledger| ledger.moderator_decision.as_mut())
                    .filter(|decision| decision.attempt.attempt_id == request.basis_id)
                {
                    decision.state = "running".to_string();
                    decision.turn_id = Some(turn_id.clone());
                    decision.turn_started_at_ms = Some(turn_started_at_ms);
                }
            }
            MeetingTurnKind::V0Intent | MeetingTurnKind::V0Granted => {}
        }
        self.persist_ledger_best_effort();
        if moderator_turn {
            self.emit_moderator_decision_event(
                "meeting_v1_moderator_decision_started",
                request.session_id,
                Some(turn_id.clone()),
                ("dispatched", "control_token_held"),
                None,
                json!({
                    "queued_latency_ms": turn_started_at_ms
                        .saturating_sub(request.queued_at_unix_ms),
                }),
            );
        }
        self.emit(
            "meeting_v1_turn_started",
            request.session_id,
            Some(turn_id.clone()),
            json!({
                "turn_id": turn_id,
                "turn_type": match request.kind {
                    MeetingTurnKind::V1Intent => "participant_intent",
                    MeetingTurnKind::V1ModeratorControl => "moderator_control",
                    MeetingTurnKind::V1Granted => "granted_speech",
                    _ => "invalid",
                },
                "queued_latency_ms": now_ms().saturating_sub(request.queued_at_unix_ms),
            }),
        );
        if let Some(session_epoch) = session_epoch {
            self.in_flight_epochs.insert(turn_id.clone(), session_epoch);
        }
        self.in_flight.insert(turn_id, request);
    }

    pub(super) fn owns_turn(&self, turn_id: &str) -> bool {
        self.in_flight.contains_key(turn_id)
    }

    pub(super) async fn register(&mut self, session_id: Uuid) {
        if self.register_local(session_id) {
            self.request_full_sync(session_id);
        }
    }

    fn register_local(&mut self, session_id: Uuid) -> bool {
        if self.meetings.contains_key(&session_id) {
            return false;
        }
        self.next_session_epoch = self.next_session_epoch.saturating_add(1).max(1);
        self.meetings
            .insert(session_id, MeetingRuntime::new(self.next_session_epoch));
        self.ensure_meeting_ledger(session_id);
        self.emit(
            "meeting_v1_discovered",
            session_id,
            None,
            json!({ "session_id": session_id }),
        );
        true
    }

    pub(super) fn remove(&mut self, session_id: Uuid) {
        self.pending
            .retain(|request| request.session_id != session_id);
        if self
            .in_flight
            .values()
            .any(|request| request.session_id == session_id)
        {
            self.preemptions.insert(session_id);
        } else {
            self.preemptions.remove(&session_id);
        }
        self.deferred_turn_results.remove(&session_id);
        self.protocol_in_flight
            .retain(|key, _| key.session_id() != session_id);
        self.progress_in_flight
            .retain(|(meeting_id, _), _| *meeting_id != session_id);
        self.progress_waiting_for_state
            .retain(|(meeting_id, _), _| *meeting_id != session_id);
        self.meetings.remove(&session_id);
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            // Membership/subscription removal tears down only ephemeral runtime
            // ownership. The Relay may still expose the same Session again
            // (reconnect, membership refresh, or detector retry), so keep every
            // prepared signed protocol event intact for exact replay. Only model
            // turns that cannot survive process/runtime teardown are rewound.
            recover_interrupted_meeting_turns(ledger);
        }
        self.persist_ledger_best_effort();
        self.emit(
            "meeting_v1_ended",
            session_id,
            None,
            json!({ "session_id": session_id, "reason": "membership_removed" }),
        );
    }

    /// Tear down a Session only after a validated Relay snapshot proves that
    /// its authoritative Baton phase or room metadata is terminal.
    ///
    /// Unlike membership removal, an ended Meeting can never resume prepared
    /// protocol work. Drop its entire durable ledger entry so prompts, private
    /// reasons, and signed prepared events do not accumulate indefinitely.
    /// A running Moderator Decision is not physically cancelled by this State
    /// transition. It remains indexed until its natural provider terminal; the
    /// removed runtime epoch fences that late result. Other turn kinds retain
    /// their existing terminal-session cancellation behavior.
    fn teardown_terminal_session(&mut self, session_id: Uuid) {
        self.pending
            .retain(|request| request.session_id != session_id);
        if self.in_flight.values().any(|request| {
            request.session_id == session_id && request.kind != MeetingTurnKind::V1ModeratorControl
        }) {
            self.preemptions.insert(session_id);
        } else {
            self.preemptions.remove(&session_id);
        }
        self.deferred_turn_results.remove(&session_id);
        self.protocol_in_flight
            .retain(|key, _| key.session_id() != session_id);
        self.progress_in_flight
            .retain(|(meeting_id, _), _| *meeting_id != session_id);
        self.progress_waiting_for_state
            .retain(|(meeting_id, _), _| *meeting_id != session_id);
        self.external_reclaimable_turns.remove(&session_id);
        self.meetings.remove(&session_id);
        self.ledger.meetings.remove(&session_id.to_string());
        self.persist_terminal_ledger_cleanup();
    }

    pub(super) fn mark_all_for_resync(&mut self) {
        let session_ids: Vec<_> = self.meetings.keys().copied().collect();
        for session_id in session_ids {
            self.request_full_sync(session_id);
        }
    }

    pub(super) async fn handle_event(&mut self, event: &BuzzEvent) {
        if !self.meetings.contains_key(&event.channel_id) {
            return;
        }
        let kind = event.event.kind.as_u16() as u32;
        if kind == KIND_MEETING_ROUND_STATE {
            match self.apply_live_state_event(event) {
                Ok(true) => {
                    self.reconcile(event.channel_id).await;
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        meeting = %event.channel_id,
                        "Meeting V1 live State fast path rejected an event: {error}"
                    );
                }
            }
        } else if !matches!(kind, KIND_STREAM_MESSAGE | KIND_MEETING_END) {
            // Accepted control commands always produce a Relay-signed State.
            // Waiting for that State avoids an identity/history/profile query
            // for every ACK and Progress frame. The periodic backfill remains
            // the recovery path when a State frame is missed.
            return;
        }
        self.request_full_sync(event.channel_id);
    }

    pub(super) async fn tick(&mut self) {
        self.retry_terminal_ledger_cleanup_if_due();
        self.drain_protocol_results().await;
        self.drain_progress_results();
        let now = Instant::now();
        let control_due: Vec<_> = self
            .meetings
            .iter()
            .filter_map(|(session_id, runtime)| {
                runtime
                    .control_retry_at
                    .is_some_and(|deadline| now >= deadline)
                    .then_some(*session_id)
            })
            .collect();
        for session_id in control_due {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.control_retry_at = None;
            }
            // Replay the already-signed ACK/Decline before any potentially slow
            // backfill. The retry path synchronizes after submission.
            self.reconcile(session_id).await;
        }
        let moderator_rebase_due: Vec<_> = self
            .meetings
            .iter()
            .filter_map(|(session_id, runtime)| {
                runtime
                    .moderator_rebase_at
                    .is_some_and(|deadline| now >= deadline)
                    .then_some(*session_id)
            })
            .collect();
        for session_id in moderator_rebase_due {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.moderator_rebase_at = None;
            }
            self.request_full_sync(session_id);
        }

        self.drain_sync_results().await;

        let session_ids: Vec<_> = self.meetings.keys().copied().collect();
        for session_id in session_ids {
            self.maintain_grant(session_id).await;
        }

        let due = self
            .meetings
            .iter()
            .filter_map(|(session_id, runtime)| {
                let periodic_due = runtime
                    .last_sync
                    .is_some_and(|last_sync| now.duration_since(last_sync) >= SYNC_INTERVAL);
                let retry_due = runtime.last_sync.is_none() && now >= runtime.retry_at;
                (runtime.sync_in_flight.is_none() && (periodic_due || retry_due))
                    .then_some(*session_id)
            })
            .next();
        if let Some(session_id) = due {
            self.request_full_sync(session_id);
        }
    }

    pub(super) async fn handle_turn_result(
        &mut self,
        turn_id: &str,
        raw_output: String,
        succeeded: bool,
    ) {
        let Some(request) = self.in_flight.remove(turn_id) else {
            return;
        };
        let turn_epoch = self.in_flight_epochs.remove(turn_id);
        let current_epoch = self
            .meetings
            .get(&request.session_id)
            .map(|runtime| runtime.epoch);
        let Some(turn_epoch) = turn_epoch.filter(|epoch| Some(*epoch) == current_epoch) else {
            self.emit(
                "meeting_v1_turn_result_deferred",
                request.session_id,
                Some(turn_id.to_string()),
                json!({ "reason": "registration_epoch_changed" }),
            );
            return;
        };
        if let Some(runtime) = self.meetings.get_mut(&request.session_id) {
            if runtime.in_flight_turn.as_deref() == Some(turn_id) {
                runtime.in_flight_turn = None;
            }
        }
        let session_id = request.session_id;
        if request.kind == MeetingTurnKind::V1ModeratorControl {
            let model_latency_ms = self
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .and_then(|decision| decision.turn_started_at_ms)
                .map(|started| now_ms().saturating_sub(started));
            self.emit_moderator_decision_event(
                "meeting_v1_moderator_decision_completed",
                session_id,
                Some(turn_id.to_string()),
                (
                    if succeeded {
                        "natural_terminal"
                    } else {
                        "provider_failure"
                    },
                    if succeeded {
                        "prompt_terminal"
                    } else {
                        "prompt_failed"
                    },
                ),
                model_latency_ms,
                json!({}),
            );
        }
        let Some(request_id) = self.request_full_sync(session_id) else {
            self.discard_deferred_turn_result(DeferredTurnResult {
                request_id: 0,
                session_epoch: turn_epoch,
                turn_id: turn_id.to_string(),
                request,
                raw_output,
                succeeded,
            });
            return;
        };
        let deferred = DeferredTurnResult {
            request_id,
            session_epoch: turn_epoch,
            turn_id: turn_id.to_string(),
            request,
            raw_output,
            succeeded,
        };
        if let Some(replaced) = self.deferred_turn_results.insert(session_id, deferred) {
            self.discard_deferred_turn_result(replaced);
        }
    }

    pub(super) fn take_preemptions(&mut self) -> Vec<Uuid> {
        std::mem::take(&mut self.preemptions).into_iter().collect()
    }

    pub(super) fn mark_cross_protocol_preempted(&mut self, session_id: Uuid) {
        self.preempt_intent_turn(session_id);
        // MeetingCoordinator returns this cancellation in the current drain;
        // do not leave a duplicate request for the next main-loop iteration.
        self.preemptions.remove(&session_id);
    }

    fn ensure_meeting_ledger(&mut self, session_id: Uuid) {
        let key = session_id.to_string();
        self.ledger
            .meetings
            .entry(key.clone())
            .or_insert_with(|| MeetingLedger {
                session_id: key,
                agent_pubkey: self.agent_pubkey.clone(),
                ..MeetingLedger::default()
            });
        self.persist_ledger_best_effort();
    }

    fn apply_live_state_event(&mut self, event: &BuzzEvent) -> Result<bool> {
        let Some(current) = self
            .meetings
            .get(&event.channel_id)
            .and_then(|runtime| runtime.view.clone())
        else {
            return Ok(false);
        };
        event
            .event
            .verify()
            .map_err(|error| anyhow!("invalid State signature: {error}"))?;
        if event.event.pubkey.to_hex() != current.relay_pubkey {
            return Err(anyhow!("State signer is not the pinned Meeting Relay"));
        }
        let raw_value: Value = serde_json::from_str(&event.event.content)
            .context("Meeting V1 live State content is malformed")?;
        let raw_state: RawBatonState = serde_json::from_value(raw_value.clone())
            .context("Meeting V1 live State shape is malformed")?;
        validate_baton_state_event(&event.event, event.channel_id, &raw_state)?;
        validate_live_state_roster(&raw_state, &current.roster)?;

        if raw_state.state_revision < current.baton.state_revision {
            return Ok(true);
        }
        if raw_state.state_revision == current.baton.state_revision {
            if event.event.id.to_hex() == current.baton.state_event_id {
                return Ok(true);
            }
            return Err(anyhow!(
                "conflicting Relay State events share one state revision"
            ));
        }

        let transitioned_to_ended = raw_state.phase == "ended" && !current.ended;
        let previous_intent_revision = current.baton.intent_revision;
        let projected_speech_revision = current
            .speeches
            .iter()
            .map(|speech| speech.speech_revision)
            .max()
            .unwrap_or(0);
        let mut updated = current;
        updated.ended |= raw_state.phase == "ended";
        updated.baton = baton_from_raw_state(&event.event, raw_state, raw_value);
        self.apply_view_to_ledger(&updated);
        let clear_control_retry = updated
            .baton
            .offer
            .as_ref()
            .is_none_or(|offer| offer.target_pubkey != self.agent_pubkey);
        if let Some(runtime) = self.meetings.get_mut(&event.channel_id) {
            runtime.view = Some(updated.clone());
            if updated.baton.speech_revision > projected_speech_revision
                || updated.baton.intent_revision != previous_intent_revision
            {
                runtime.last_sync = None;
                runtime.retry_at = Instant::now();
            }
            if clear_control_retry {
                runtime.control_retry_at = None;
            }
        }
        self.emit(
            "meeting_v1_state_applied",
            event.channel_id,
            None,
            json!({
                "state_revision": updated.baton.state_revision,
                "state_event_id": updated.baton.state_event_id,
                "intent_revision": updated.baton.intent_revision,
                "speech_revision": updated.baton.speech_revision,
                "control_epoch": updated.baton.control_epoch,
                "decision_epoch": updated.baton.decision_epoch,
                "phase": updated.baton.phase,
                "source": "live_fast_path",
            }),
        );
        if transitioned_to_ended {
            self.emit(
                "meeting_v1_ended",
                event.channel_id,
                None,
                json!({ "reason": "relay_state" }),
            );
        }
        Ok(true)
    }

    fn submit_protocol_in_background(
        &mut self,
        key: ProtocolSubmissionKey,
        context: ProtocolSubmissionContext,
        event: Event,
    ) -> bool {
        if self.protocol_in_flight.contains_key(&key) {
            return false;
        }
        self.next_protocol_submission_id =
            self.next_protocol_submission_id.saturating_add(1).max(1);
        let submission_id = self.next_protocol_submission_id;
        let session_epoch = self
            .meetings
            .get(&key.session_id())
            .map_or(0, |runtime| runtime.epoch);
        let event_id = event.id.to_hex();
        self.protocol_in_flight.insert(
            key.clone(),
            ProtocolInFlight {
                session_epoch,
                submission_id,
                event_id: event_id.clone(),
            },
        );
        let rest = self.rest.clone();
        let result_tx = self.protocol_result_tx.clone();
        let _task = tokio::spawn(async move {
            let attempt = AssertUnwindSafe(async {
                #[cfg(feature = "meeting-v1-acceptance")]
                if let ProtocolSubmissionContext::Moderator {
                    barrier: Some(barrier),
                    ..
                } = &context
                {
                    let (socket_path, frame) = barrier.as_ref();
                    meeting_acceptance::await_pre_submit_release(socket_path, frame)
                        .await
                        .map_err(|error| {
                            ProtocolSubmitFailure::Uncertain(format!(
                                "acceptance barrier failed before protocol submit: {error}"
                            ))
                        })?;
                }
                submit_protocol_event(&rest, &event).await
            })
            .catch_unwind()
            .await;
            let result = match attempt {
                Ok(result) => result,
                Err(_) => Err(ProtocolSubmitFailure::Uncertain(
                    "background protocol submission task panicked".to_string(),
                )),
            };
            if result_tx
                .send(ProtocolTaskResult {
                    key,
                    session_epoch,
                    submission_id,
                    event_id,
                    context,
                    result,
                })
                .is_err()
            {
                tracing::debug!(
                    submission_id,
                    "Meeting V1 coordinator stopped before protocol submission completed"
                );
            }
        });
        true
    }

    async fn drain_protocol_results(&mut self) {
        let mut completed = Vec::new();
        while let Ok(result) = self.protocol_result_rx.try_recv() {
            completed.push(result);
        }
        for result in completed {
            self.handle_protocol_result(result).await;
        }
    }

    async fn handle_protocol_result(&mut self, completed: ProtocolTaskResult) {
        let current_epoch = self
            .meetings
            .get(&completed.key.session_id())
            .map_or(0, |runtime| runtime.epoch);
        if self
            .protocol_in_flight
            .get(&completed.key)
            .is_none_or(|in_flight| {
                current_epoch != completed.session_epoch
                    || in_flight.session_epoch != completed.session_epoch
                    || in_flight.submission_id != completed.submission_id
                    || in_flight.event_id != completed.event_id
            })
        {
            return;
        }
        self.protocol_in_flight.remove(&completed.key);
        match completed.context {
            ProtocolSubmissionContext::Offer {
                offer_id,
                action,
                allocation_source,
                turn_role,
                created_at_ms,
            } => {
                let session_id = completed.key.session_id();
                if let Some(reservation) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.reservations.get_mut(&offer_id))
                {
                    let prepared_event = match action {
                        OfferSubmissionAction::Ack => reservation.ack_event.as_ref(),
                        OfferSubmissionAction::Decline => reservation.decline_event.as_ref(),
                    };
                    if prepared_event.and_then(serialized_event_id).as_deref()
                        == Some(completed.event_id.as_str())
                        && matches!(
                            reservation.state.as_str(),
                            "ack_prepared" | "ack_sent" | "decline_prepared" | "decline_sent"
                        )
                    {
                        reservation.state = match &completed.result {
                            Ok(_) => format!("{}_sent", action.as_str()),
                            Err(ProtocolSubmitFailure::Rejected(_)) => "released".to_string(),
                            Err(ProtocolSubmitFailure::Uncertain(_)) => {
                                format!("{}_prepared", action.as_str())
                            }
                        };
                    }
                }
                self.persist_ledger_best_effort();
                self.emit(
                    "meeting_v1_offer_decision",
                    session_id,
                    None,
                    json!({
                        "offer_id": offer_id,
                        "decision": action.as_str(),
                        "allocation_source": allocation_source,
                        "turn_role": turn_role,
                        "ack_latency_ms": now_ms().saturating_sub(created_at_ms),
                        "submission": protocol_submission_label(&completed.result),
                    }),
                );
                if completed.result.is_err() {
                    tracing::warn!(
                        meeting = %session_id,
                        offer = %offer_id,
                        action = action.as_str(),
                        outcome = protocol_submission_label(&completed.result),
                        "Meeting V1 Offer response was not confirmed"
                    );
                }
                if completed
                    .result
                    .as_ref()
                    .is_err_and(|error| error.is_uncertain())
                {
                    self.schedule_offer_retry_if_active(session_id, &offer_id);
                } else {
                    self.request_fast_backfill(session_id);
                }
            }
            ProtocolSubmissionContext::Intent {
                trigger_id,
                turn_id,
                queued_at_ms,
            } => {
                let session_id = completed.key.session_id();
                if let Some(trigger) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.triggers.get_mut(&trigger_id))
                {
                    let event_matches =
                        trigger.prepared_event_id.as_deref() == Some(completed.event_id.as_str());
                    if event_matches
                        && matches!(trigger.state.as_str(), "prepared" | "sent_uncertain")
                    {
                        trigger.state = match &completed.result {
                            Ok(_) => "submitted".to_string(),
                            Err(ProtocolSubmitFailure::Rejected(_)) => "rejected".to_string(),
                            Err(ProtocolSubmitFailure::Uncertain(_)) => {
                                "sent_uncertain".to_string()
                            }
                        };
                    }
                }
                self.persist_ledger_best_effort();
                self.emit(
                    "meeting_v1_intent_completed",
                    session_id,
                    turn_id,
                    json!({
                        "trigger_id": trigger_id,
                        "decision": "SUBMIT",
                        "intent_event_id": completed.event_id,
                        "outcome": protocol_submission_label(&completed.result),
                        "latency_ms": queued_at_ms
                            .map(|queued_at_ms| now_ms().saturating_sub(queued_at_ms)),
                    }),
                );
                if completed.result.is_err() {
                    tracing::warn!(
                        meeting = %session_id,
                        trigger = %trigger_id,
                        outcome = protocol_submission_label(&completed.result),
                        "Meeting V1 Intent submission was not confirmed"
                    );
                }
                self.request_fast_backfill(session_id);
            }
            ProtocolSubmissionContext::GrantTerminal {
                grant_id,
                source_offer_id,
                action,
                turn_id,
                queued_at_ms,
                grant_started_at_ms,
            } => {
                let session_id = completed.key.session_id();
                let active_grant = self
                    .meetings
                    .get(&session_id)
                    .and_then(|runtime| runtime.view.as_ref())
                    .and_then(|view| {
                        view.baton
                            .grant
                            .as_ref()
                            .filter(|grant| {
                                grant.grant_id == grant_id
                                    && grant.holder_pubkey == self.agent_pubkey
                            })
                            .cloned()
                    });
                let mut event_matches = false;
                if let Some(record) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.grants.get_mut(&grant_id))
                {
                    let prepared_event = match action {
                        GrantTerminalAction::Speech => record.speech_event.as_ref(),
                        GrantTerminalAction::Yield => record.yield_event.as_ref(),
                    };
                    event_matches = prepared_event.and_then(serialized_event_id).as_deref()
                        == Some(completed.event_id.as_str());
                    if event_matches
                        && !matches!(record.state.as_str(), "spoken" | "yielded" | "terminal")
                    {
                        record.state = match (action, &completed.result) {
                            (GrantTerminalAction::Speech, Ok(_)) => "speech_sent".to_string(),
                            (
                                GrantTerminalAction::Speech,
                                Err(ProtocolSubmitFailure::Rejected(_)),
                            ) => "speech_rejected".to_string(),
                            (
                                GrantTerminalAction::Speech,
                                Err(ProtocolSubmitFailure::Uncertain(_)),
                            ) => "speech_sent_uncertain".to_string(),
                            (GrantTerminalAction::Yield, Ok(_)) => "yield_sent".to_string(),
                            (
                                GrantTerminalAction::Yield,
                                Err(ProtocolSubmitFailure::Rejected(_)),
                            ) => "terminal".to_string(),
                            (
                                GrantTerminalAction::Yield,
                                Err(ProtocolSubmitFailure::Uncertain(_)),
                            ) => "yield_sent_uncertain".to_string(),
                        };
                    }
                }
                self.persist_ledger_best_effort();
                if event_matches && completed.result.is_ok() {
                    self.release_reservation(
                        session_id,
                        &source_offer_id,
                        if action == GrantTerminalAction::Speech {
                            "speech_accepted"
                        } else {
                            "yield_accepted"
                        },
                    );
                }
                let mut telemetry = json!({
                    "grant_id": grant_id,
                    "action": action.as_str(),
                    "outcome": protocol_submission_label(&completed.result),
                    "grant_duration_ms": grant_started_at_ms
                        .map(|started| now_ms().saturating_sub(started)),
                    "turn_latency_ms": queued_at_ms
                        .map(|queued| now_ms().saturating_sub(queued)),
                });
                if let Some(object) = telemetry.as_object_mut() {
                    object.insert(
                        if action == GrantTerminalAction::Speech {
                            "speech_event_id"
                        } else {
                            "yield_event_id"
                        }
                        .to_string(),
                        json!(completed.event_id),
                    );
                }
                self.emit(
                    if action == GrantTerminalAction::Speech {
                        "meeting_v1_speech_submitted"
                    } else {
                        "meeting_v1_grant_yielded"
                    },
                    session_id,
                    turn_id,
                    telemetry,
                );
                if completed.result.is_err() {
                    tracing::warn!(
                        meeting = %session_id,
                        grant = %grant_id,
                        action = action.as_str(),
                        outcome = protocol_submission_label(&completed.result),
                        "Meeting V1 Grant terminal action was not confirmed"
                    );
                }
                let should_yield = event_matches
                    && action == GrantTerminalAction::Speech
                    && matches!(completed.result, Err(ProtocolSubmitFailure::Rejected(_)));
                if should_yield {
                    if let Some(grant) = active_grant {
                        self.prepare_and_submit_yield(
                            session_id,
                            &grant,
                            MeetingV1GrantYieldReason::UnableToAnswer,
                            "Relay rejected the prepared speech",
                        )
                        .await;
                    }
                } else {
                    self.request_fast_backfill(session_id);
                }
            }
            ProtocolSubmissionContext::Moderator {
                action_kind,
                object_id,
                attempt_id,
                turn_id,
                queued_at_ms,
                ..
            } => {
                let session_id = completed.key.session_id();
                let event_matches = self
                    .ledger_for(session_id)
                    .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
                    .is_some_and(|prepared| prepared.event_id == completed.event_id);
                if event_matches {
                    self.handle_moderator_protocol_outcome(
                        session_id,
                        &action_kind,
                        &object_id,
                        &completed.event_id,
                        &completed.result,
                    );
                }
                self.persist_ledger_best_effort();
                if event_matches
                    && completed.result.is_ok()
                    && matches!(
                        action_kind.as_str(),
                        "select_intent" | "select_handoff" | "moderator_speak" | "withdraw_self"
                    )
                {
                    self.emit_moderator_decision_event(
                        "meeting_v1_moderator_decision_committed",
                        session_id,
                        turn_id.clone(),
                        ("accepted", "relay_committed"),
                        None,
                        json!({
                            "action": action_kind,
                            "object_id": object_id,
                            "attempt_id": attempt_id,
                            "event_id": completed.event_id,
                        }),
                    );
                }
                self.emit(
                    "meeting_v1_moderator_action_submitted",
                    session_id,
                    turn_id,
                    json!({
                        "action": action_kind,
                        "object_id": object_id,
                        "attempt_id": attempt_id,
                        "event_id": completed.event_id,
                        "outcome": protocol_submission_label(&completed.result),
                        "latency_ms": queued_at_ms
                            .map(|queued| now_ms().saturating_sub(queued)),
                    }),
                );
                if completed.result.is_err() {
                    tracing::warn!(
                        meeting = %session_id,
                        action = %action_kind,
                        object = %object_id,
                        outcome = protocol_submission_label(&completed.result),
                        "Meeting V1 moderator action was not confirmed"
                    );
                }
                // Even an uncertain transport result may already have committed
                // at the Relay. Reconcile canonical State before replaying the
                // exact prepared event.
                self.request_fast_backfill(session_id);
            }
        }
    }

    fn handle_moderator_protocol_outcome(
        &mut self,
        session_id: Uuid,
        action_kind: &str,
        object_id: &str,
        event_id: &str,
        result: &std::result::Result<Value, ProtocolSubmitFailure>,
    ) {
        match result {
            Ok(_) => {
                if matches!(action_kind, "reject" | "dismiss_handoff") {
                    if let Some(decision) = self
                        .ledger_for_mut(session_id)
                        .and_then(|ledger| ledger.moderator_decision.as_mut())
                    {
                        if action_kind == "reject" {
                            decision
                                .rejections
                                .retain(|proposal| proposal.intent_id != object_id);
                        } else {
                            decision
                                .handoff_dismissals
                                .retain(|proposal| proposal.handoff_id != object_id);
                        }
                        decision.state = "ready".to_string();
                    }
                    if let Some(ledger) = self.ledger_for_mut(session_id) {
                        ledger.prepared_moderator_action = None;
                    }
                } else if let Some(prepared) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.prepared_moderator_action.as_mut())
                {
                    prepared.state = "sent".to_string();
                }
            }
            Err(ProtocolSubmitFailure::Uncertain(_)) => {
                if let Some(prepared) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.prepared_moderator_action.as_mut())
                {
                    // Preserve and replay this exact signed event, but only
                    // after the requested Full Sync fails to prove a commit.
                    prepared.state = "prepared".to_string();
                }
            }
            Err(ProtocolSubmitFailure::Rejected(rejection)) => {
                if let Some(ledger) = self.ledger_for_mut(session_id) {
                    ledger.prepared_moderator_action = None;
                    if action_kind == "decision_attempt_start" {
                        if let Some(decision) = ledger.moderator_decision.as_mut() {
                            decision.state = "terminal".to_string();
                        }
                    }
                }
                match rejection.code.as_str() {
                    "dependency_stale" if matches!(action_kind, "reject" | "dismiss_handoff") => {
                        self.skip_moderator_cleanup(
                            session_id,
                            action_kind,
                            object_id,
                            "dependency_stale",
                        );
                    }
                    "stale_moderator_revision"
                        if matches!(
                            action_kind,
                            "select_intent" | "select_handoff" | "moderator_speak"
                        ) =>
                    {
                        self.schedule_moderator_rebase(session_id);
                    }
                    "selected_source_changed"
                        if matches!(
                            action_kind,
                            "select_intent" | "select_handoff" | "moderator_speak"
                        ) =>
                    {
                        if let Some(ticket) = rejection.retry_ticket_id.clone() {
                            if let Some(decision) = self
                                .ledger_for_mut(session_id)
                                .and_then(|ledger| ledger.moderator_decision.as_mut())
                            {
                                decision.pending_retry = Some(PendingModeratorRetry {
                                    retry_ticket_id: ticket.clone(),
                                    failed_action_event_id: event_id.to_string(),
                                });
                                decision.state = "retry_pending".to_string();
                            }
                            self.emit_moderator_decision_event(
                                "meeting_v1_moderator_decision_retry_requested",
                                session_id,
                                None,
                                ("retry_required", "selected_source_changed"),
                                None,
                                json!({
                                    "attempt_id": self
                                        .ledger_for(session_id)
                                        .and_then(|ledger| ledger.moderator_decision.as_ref())
                                        .map(|decision| decision.attempt.attempt_id.clone()),
                                    "retry_ticket_id": ticket,
                                    "failed_action_event_id": event_id,
                                    "rejection_code": rejection.code,
                                }),
                            );
                        } else {
                            self.mark_moderator_result_stale(session_id, "source_changed");
                        }
                    }
                    "human_request_has_priority" | "active_human_request_exists" => {
                        self.mark_moderator_result_stale(session_id, "human_priority");
                    }
                    "meeting_ended" => {
                        self.mark_moderator_result_stale(session_id, "meeting_ended");
                    }
                    "participant_revoked"
                    | "moderator_attempt_actor_mismatch"
                    | "agent_moderator_required" => {
                        self.mark_moderator_result_stale(session_id, "moderator_changed");
                    }
                    "stale_speech_revision" | "moderator_attempt_prerequisite_changed" => {
                        self.mark_moderator_result_stale(session_id, "speech_changed");
                    }
                    "moderator_does_not_hold_control" => {
                        self.mark_moderator_result_stale(session_id, "control_changed");
                    }
                    "moderator_attempt_not_active"
                    | "moderator_attempt_expired"
                    | "moderator_attempt_already_terminal"
                    | "moderator_attempt_limit_reached"
                    | "retry_ticket_already_consumed"
                    | "retry_ticket_expired" => {
                        if let Some(ledger) = self.ledger_for_mut(session_id) {
                            if let Some(decision) = ledger.moderator_decision.as_mut() {
                                decision.state = "terminal".to_string();
                            }
                            if action_kind == "decision_attempt_start" {
                                ledger.replacement_attempt_id = None;
                            }
                        }
                    }
                    "current_cohort_not_empty" if action_kind == "complete_cohort" => {
                        self.set_moderator_finish_reason(session_id, "idle_wait_fallback");
                    }
                    "stale_moderator_revision" if action_kind == "complete_cohort" => {
                        if let Some(decision) = self
                            .ledger_for_mut(session_id)
                            .and_then(|ledger| ledger.moderator_decision.as_mut())
                        {
                            decision.state = "ready".to_string();
                        }
                    }
                    "stale_moderator_revision" if action_kind == "decision_retry" => {
                        self.mark_moderator_result_stale(session_id, "control_changed");
                    }
                    "stale_moderator_revision"
                    | "moderator_attempt_already_running"
                    | "no_current_cohort_candidates"
                    | "moderator_attempt_already_started"
                    | "replacement_attempt_not_found"
                    | "replacement_attempt_not_eligible"
                    | "retry_conflict_no_longer_present" => {
                        if matches!(
                            rejection.code.as_str(),
                            "no_current_cohort_candidates"
                                | "moderator_attempt_already_started"
                                | "replacement_attempt_not_found"
                                | "replacement_attempt_not_eligible"
                        ) {
                            if let Some(ledger) = self.ledger_for_mut(session_id) {
                                ledger.replacement_attempt_id = None;
                            }
                        }
                        // The following Full Sync decides whether a canonical
                        // attempt exists or whether this window is already
                        // closed. No provider retry is scheduled here.
                    }
                    _ if matches!(action_kind, "reject" | "dismiss_handoff") => {
                        self.skip_moderator_cleanup(
                            session_id,
                            action_kind,
                            object_id,
                            &rejection.code,
                        );
                    }
                    _ => {
                        self.mark_moderator_result_stale(session_id, "control_changed");
                    }
                }
            }
        }
    }

    fn schedule_moderator_rebase(&mut self, session_id: Uuid) {
        let max_rebases = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .map_or(default_moderator_max_cas_rebases(), |view| {
                view.baton
                    .baton_config
                    .moderator_max_cas_rebases_per_attempt
            });
        let mut delayed = false;
        let mut exhausted = false;
        let mut attempt_id = None;
        let mut rebase_count = 0;
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.cas_rebases = decision.cas_rebases.saturating_add(1);
            decision.fast_rebases = decision.fast_rebases.saturating_add(1);
            rebase_count = decision.cas_rebases;
            attempt_id = Some(decision.attempt.attempt_id.clone());
            exhausted = u64::from(decision.cas_rebases) >= max_rebases;
            if exhausted {
                decision.pending_finish_reason = Some("cas_churn".to_string());
                decision.state = "result_stale".to_string();
            } else {
                decision.state = "rebasing".to_string();
                if decision.fast_rebases >= MAX_MODERATOR_FAST_REBASES {
                    decision.fast_rebases = 0;
                    delayed = true;
                }
            }
        }
        if delayed {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.moderator_rebase_at = Some(Instant::now() + MODERATOR_REBASE_QUIESCENCE);
            }
        } else if exhausted {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.moderator_rebase_at = None;
            }
        }
        self.emit_moderator_decision_event(
            "meeting_v1_moderator_decision_rebased",
            session_id,
            None,
            (
                if exhausted { "discarded" } else { "rebasing" },
                if exhausted {
                    "cas_churn"
                } else if delayed {
                    "cas_quiescence"
                } else {
                    "stale_moderator_revision"
                },
            ),
            None,
            json!({
                "attempt_id": attempt_id,
                "rebase_count": rebase_count,
                "quiescence_ms": delayed.then_some(MODERATOR_REBASE_QUIESCENCE.as_millis()),
                "exhausted": exhausted,
            }),
        );
    }

    /// Request a fresh Relay snapshot without blocking the ACP event loop.
    ///
    /// Calls made while a snapshot is running coalesce into exactly one newer
    /// request. This matters for model results: a result must observe a
    /// snapshot started after that result arrived, rather than borrowing an
    /// older periodic snapshot that happened to still be in flight.
    fn request_full_sync(&mut self, session_id: Uuid) -> Option<u64> {
        let (in_flight, requested) = self
            .meetings
            .get(&session_id)
            .map(|runtime| (runtime.sync_in_flight, runtime.sync_requested))?;
        if let Some(in_flight) = in_flight {
            if requested > in_flight {
                return Some(requested);
            }
        }

        self.next_sync_request_id = self.next_sync_request_id.saturating_add(1).max(1);
        let request_id = self.next_sync_request_id;
        let should_start = in_flight.is_none();
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.sync_requested = request_id;
            runtime.last_sync = None;
            runtime.retry_at = Instant::now();
        }
        if should_start {
            self.start_full_sync(session_id, request_id);
        }
        Some(request_id)
    }

    fn start_full_sync(&mut self, session_id: Uuid, request_id: u64) {
        let Some(runtime) = self.meetings.get_mut(&session_id) else {
            return;
        };
        if runtime.sync_in_flight.is_some() {
            return;
        }
        let session_epoch = runtime.epoch;
        runtime.sync_in_flight = Some(request_id);
        self.emit(
            "meeting_v1_sync_started",
            session_id,
            None,
            json!({
                "session_id": session_id,
                "request_id": request_id,
                "source": "background",
            }),
        );
        let rest = self.rest.clone();
        let result_tx = self.sync_result_tx.clone();
        let _task = tokio::spawn(async move {
            let attempt = AssertUnwindSafe(tokio::time::timeout(
                SYNC_ATTEMPT_TIMEOUT,
                fetch_meeting_view(&rest, session_id),
            ))
            .catch_unwind()
            .await;
            let result = match attempt {
                Ok(Ok(Ok(view))) => Ok(view),
                Ok(Ok(Err(error))) => Err(error.to_string()),
                Ok(Err(_)) => Err(format!(
                    "Meeting V1 sync exceeded the {}ms controller budget",
                    SYNC_ATTEMPT_TIMEOUT.as_millis()
                )),
                Err(_) => Err("Meeting V1 background sync task panicked".to_string()),
            };
            if result_tx
                .send(SyncTaskResult {
                    session_id,
                    session_epoch,
                    request_id,
                    result,
                })
                .is_err()
            {
                tracing::debug!(
                    meeting = %session_id,
                    request_id,
                    "Meeting V1 coordinator stopped before background sync completed"
                );
            }
        });
    }

    async fn drain_sync_results(&mut self) {
        let mut completed = Vec::new();
        while let Ok(result) = self.sync_result_rx.try_recv() {
            completed.push(result);
        }
        for result in completed {
            self.handle_sync_result(result).await;
        }
    }

    async fn handle_sync_result(&mut self, completed: SyncTaskResult) {
        let session_id = completed.session_id;
        let Some(runtime) = self.meetings.get_mut(&session_id) else {
            return;
        };
        if runtime.epoch != completed.session_epoch
            || runtime.sync_in_flight != Some(completed.request_id)
        {
            return;
        }
        runtime.sync_in_flight = None;

        let applied = match completed.result {
            Ok(view) => self.apply_synced_view(session_id, view),
            Err(error) => {
                tracing::warn!(meeting = %session_id, "Meeting V1 sync failed: {error}");
                self.schedule_sync_retry(session_id);
                self.emit(
                    "meeting_v1_sync_failed",
                    session_id,
                    None,
                    json!({
                        "request_id": completed.request_id,
                        "error": error,
                    }),
                );
                SyncApplyResult::Failed
            }
        };
        if applied == SyncApplyResult::Applied {
            self.progress_waiting_for_state
                .retain(|(meeting_id, _), required_request_id| {
                    *meeting_id != session_id || *required_request_id > completed.request_id
                });
        }

        let requested_after = self
            .meetings
            .get(&session_id)
            .map(|runtime| runtime.sync_requested)
            .filter(|requested| *requested > completed.request_id);
        if let Some(request_id) = requested_after {
            self.start_full_sync(session_id, request_id);
            return;
        }

        match applied {
            SyncApplyResult::Superseded => {
                self.request_full_sync(session_id);
            }
            SyncApplyResult::Failed => {
                // Keep a naturally completed provider result until a later
                // authoritative Full Sync succeeds. Dropping it here would
                // either lose an attempt-bound decision or force an
                // unregistered duplicate model call.
            }
            SyncApplyResult::Applied => {
                let pending = self
                    .deferred_turn_results
                    .get(&session_id)
                    .is_some_and(|pending| pending.request_id <= completed.request_id)
                    .then(|| self.deferred_turn_results.remove(&session_id))
                    .flatten();
                if let Some(pending) = pending {
                    self.process_deferred_turn_result(pending).await;
                } else {
                    self.reconcile(session_id).await;
                }
            }
        }
    }

    fn apply_synced_view(&mut self, session_id: Uuid, view: MeetingView) -> SyncApplyResult {
        if let Some(previous) = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
        {
            if previous.relay_pubkey != view.relay_pubkey {
                tracing::warn!(
                    meeting = %session_id,
                    "Meeting V1 full sync attempted to rotate the pinned Relay signer"
                );
                self.schedule_sync_retry(session_id);
                return SyncApplyResult::Failed;
            }
            if !same_frozen_roster(&previous.roster, &view.roster) {
                tracing::warn!(
                    meeting = %session_id,
                    "Meeting V1 full sync changed the frozen participant roster"
                );
                self.schedule_sync_retry(session_id);
                return SyncApplyResult::Failed;
            }
        }
        let Some(participant) = view.roster.get(&self.agent_pubkey) else {
            tracing::warn!(
                meeting = %session_id,
                "Meeting V1 State does not contain this Agent"
            );
            self.schedule_sync_retry(session_id);
            return SyncApplyResult::Failed;
        };
        if participant.participant_type != "agent" {
            tracing::warn!(
                meeting = %session_id,
                participant_type = %participant.participant_type,
                "ACP identity is not frozen as an Agent in Meeting V1"
            );
            self.schedule_sync_retry(session_id);
            return SyncApplyResult::Failed;
        }

        let previous = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .map(|previous| {
                (
                    previous.baton.state_revision,
                    previous.baton.state_event_id.clone(),
                    previous.ended,
                )
            });
        if previous
            .as_ref()
            .is_some_and(|(revision, _, _)| *revision > view.baton.state_revision)
        {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.last_sync = None;
                runtime.retry_at = Instant::now();
            }
            self.emit(
                "meeting_v1_sync_superseded",
                session_id,
                None,
                json!({
                    "fetched_state_revision": view.baton.state_revision,
                    "current_state_revision": previous
                        .as_ref()
                        .map(|(revision, _, _)| *revision)
                        .unwrap_or_default(),
                }),
            );
            return SyncApplyResult::Superseded;
        }
        if previous.as_ref().is_some_and(|(revision, event_id, _)| {
            *revision == view.baton.state_revision && *event_id != view.baton.state_event_id
        }) {
            tracing::warn!(
                meeting = %session_id,
                state_revision = view.baton.state_revision,
                "Meeting V1 full sync conflicts with the live authoritative State"
            );
            self.schedule_sync_retry(session_id);
            return SyncApplyResult::Failed;
        }

        let transitioned_to_ended = view.ended
            && previous
                .as_ref()
                .is_none_or(|(_, _, previously_ended)| !previously_ended);
        self.apply_view_to_ledger(&view);
        let clear_control_retry = view
            .baton
            .offer
            .as_ref()
            .is_none_or(|offer| offer.target_pubkey != self.agent_pubkey);
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.view = Some(view.clone());
            runtime.last_sync = Some(Instant::now());
            runtime.retry_at = Instant::now() + SYNC_RETRY_INTERVAL;
            if clear_control_retry {
                runtime.control_retry_at = None;
            }
        }
        self.emit(
            "meeting_v1_sync_completed",
            session_id,
            None,
            json!({
                "state_revision": view.baton.state_revision,
                "state_event_id": view.baton.state_event_id,
                "intent_revision": view.baton.intent_revision,
                "speech_revision": view.baton.speech_revision,
                "control_epoch": view.baton.control_epoch,
                "decision_epoch": view.baton.decision_epoch,
                "phase": view.baton.phase,
                "source": "background",
            }),
        );
        if transitioned_to_ended {
            self.emit(
                "meeting_v1_ended",
                session_id,
                None,
                json!({ "reason": "relay_state" }),
            );
        }
        SyncApplyResult::Applied
    }

    async fn process_deferred_turn_result(&mut self, pending: DeferredTurnResult) {
        let session_id = pending.request.session_id;
        if self
            .meetings
            .get(&session_id)
            .is_none_or(|runtime| runtime.epoch != pending.session_epoch)
        {
            self.discard_deferred_turn_result(pending);
            return;
        }
        match pending.request.kind {
            MeetingTurnKind::V1Intent => {
                self.handle_intent_result(
                    &pending.turn_id,
                    &pending.request,
                    &pending.raw_output,
                    pending.succeeded,
                )
                .await;
            }
            MeetingTurnKind::V1Granted => {
                self.handle_granted_result(
                    &pending.turn_id,
                    &pending.request,
                    &pending.raw_output,
                    pending.succeeded,
                )
                .await;
            }
            MeetingTurnKind::V1ModeratorControl => {
                self.handle_moderator_control_result(
                    &pending.turn_id,
                    &pending.request,
                    &pending.raw_output,
                    pending.succeeded,
                );
            }
            MeetingTurnKind::V0Intent | MeetingTurnKind::V0Granted => {}
        }
        self.reconcile(session_id).await;
    }

    fn discard_deferred_turn_result(&mut self, pending: DeferredTurnResult) {
        match pending.request.kind {
            MeetingTurnKind::V1Intent => {
                self.mark_trigger_state(
                    pending.request.session_id,
                    &pending.request.basis_id,
                    "pending",
                );
            }
            MeetingTurnKind::V1Granted => {
                if let Some(grant_id) = pending.request.grant_event_id.as_deref() {
                    self.mark_grant_state(pending.request.session_id, grant_id, "received");
                }
            }
            MeetingTurnKind::V1ModeratorControl => {
                if let Some(decision) = self
                    .ledger_for_mut(pending.request.session_id)
                    .and_then(|ledger| ledger.moderator_decision.as_mut())
                {
                    decision.state = "sync_failed".to_string();
                }
            }
            MeetingTurnKind::V0Intent | MeetingTurnKind::V0Granted => {}
        }
        self.emit(
            "meeting_v1_turn_result_deferred",
            pending.request.session_id,
            Some(pending.turn_id),
            json!({
                "reason": "authoritative_state_unavailable",
                "turn_type": match pending.request.kind {
                    MeetingTurnKind::V1Intent => "participant_intent",
                    MeetingTurnKind::V1ModeratorControl => "moderator_control",
                    MeetingTurnKind::V1Granted => "granted_speech",
                    _ => "invalid",
                },
            }),
        );
    }

    fn schedule_sync_retry(&mut self, session_id: Uuid) {
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.last_sync = None;
            runtime.retry_at = Instant::now() + SYNC_RETRY_INTERVAL;
        }
    }

    fn apply_view_to_ledger(&mut self, view: &MeetingView) {
        if view.ended {
            self.teardown_terminal_session(view.session_id);
            return;
        }
        self.ensure_meeting_ledger(view.session_id);
        let key = view.session_id.to_string();
        let agent_pubkey = self.agent_pubkey.clone();
        let active_offer = view
            .baton
            .offer
            .as_ref()
            .filter(|offer| offer.target_pubkey == agent_pubkey)
            .cloned();
        let active_grant = view
            .baton
            .grant
            .as_ref()
            .filter(|grant| grant.holder_pubkey == agent_pubkey)
            .cloned();
        let self_pending_intent = view
            .baton
            .pending_intents
            .iter()
            .find(|intent| intent.author_pubkey == agent_pubkey)
            .cloned();
        let previous_attempt_id = self
            .ledger
            .meetings
            .get(&key)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .map(|decision| decision.attempt.attempt_id.clone());
        let prepared_attempt_transition = self
            .ledger
            .meetings
            .get(&key)
            .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
            .map(|prepared| prepared.action_kind.clone());
        let registered_attempt = view
            .baton
            .active_decision_attempt
            .clone()
            .filter(|attempt| previous_attempt_id.as_deref() != Some(attempt.attempt_id.as_str()));
        let Some(ledger) = self.ledger.meetings.get_mut(&key) else {
            return;
        };
        let was_synced = ledger.meeting_synced;
        if !was_synced {
            for speech in &view.speeches {
                if speech.speech_revision <= view.baton.speech_revision {
                    ledger.seen_speech_ids.insert(speech.event_id.clone());
                }
            }
            let trigger_id = format!("activation:{}", view.session_id);
            ledger
                .triggers
                .entry(trigger_id.clone())
                .or_insert_with(|| {
                    TriggerRecord::new(trigger_id, None, view.baton.speech_revision)
                });
        } else {
            for speech in &view.speeches {
                if speech.speech_revision > view.baton.speech_revision {
                    continue;
                }
                if !ledger.seen_speech_ids.insert(speech.event_id.clone())
                    || speech.author_pubkey == agent_pubkey
                {
                    continue;
                }
                let directed_attempt_is_active = speech.handoff.as_ref().is_some_and(|handoff| {
                    handoff.target_pubkey == agent_pubkey
                        && baton_has_active_handoff_attempt(&view.baton, speech.event_id.as_str())
                });
                if !directed_attempt_is_active {
                    let trigger_id = format!("speech:{}", speech.event_id);
                    ledger
                        .triggers
                        .entry(trigger_id.clone())
                        .or_insert_with(|| {
                            TriggerRecord::new(
                                trigger_id,
                                Some(speech.event_id.clone()),
                                view.baton.speech_revision,
                            )
                        });
                }
            }
        }

        // A direct Handoff normally becomes an Offer atomically with its source
        // speech. If that attempt is blocked or later fails, expose one semantic
        // trigger only after the authoritative State confirms no active attempt.
        for handoff in &view.baton.unresolved_handoffs {
            if handoff.to_pubkey != agent_pubkey
                || baton_has_active_handoff_attempt(&view.baton, &handoff.handoff_id)
            {
                continue;
            }
            let trigger_id = format!("handoff:{}", handoff.handoff_id);
            ledger
                .triggers
                .entry(trigger_id.clone())
                .or_insert_with(|| {
                    TriggerRecord::new(
                        trigger_id,
                        Some(handoff.source_speech_event_id.clone()),
                        view.baton.speech_revision,
                    )
                });
        }

        ledger.meeting_synced = true;
        ledger.state_revision = view.baton.state_revision;
        ledger.speech_revision = view.baton.speech_revision;
        ledger.speech_cursor = view.speech_cursor.clone();

        for trigger in ledger.triggers.values_mut() {
            let prepared_matches = trigger.prepared_event_id.as_ref().is_some_and(|event_id| {
                self_pending_intent.as_ref().is_some_and(|intent| {
                    intent.intent_id == *event_id || intent.current_event_id == *event_id
                })
            });
            if prepared_matches {
                trigger.state = "submitted".to_string();
            } else if matches!(trigger.state.as_str(), "prepared" | "sent_uncertain")
                && view.baton.speech_revision > trigger.basis_speech_revision
            {
                trigger.state = "stale".to_string();
            }
        }

        for reservation in ledger.reservations.values_mut() {
            if active_offer
                .as_ref()
                .is_some_and(|offer| offer.offer_id == reservation.offer_id)
            {
                restore_prepared_offer_response(reservation);
                continue;
            }
            if active_grant
                .as_ref()
                .is_some_and(|grant| grant.source_offer_id == reservation.offer_id)
            {
                reservation.state = "granted".to_string();
                if let Some(grant) = active_grant.as_ref() {
                    reservation.capacity_expires_at_ms = grant.hard_deadline_ms;
                }
            } else if matches!(
                reservation.state.as_str(),
                "ack_prepared" | "ack_sent" | "decline_prepared" | "decline_sent" | "granted"
            ) {
                reservation.state = "released".to_string();
            }
        }

        if let Some(grant) = active_grant.as_ref() {
            // A process may restart after the Relay created the Grant but
            // before the local ACK reservation was written (or after a lost
            // local ledger). Reconstruct the reservation from canonical State
            // before ordinary work can consume the newly started pool.
            ledger
                .reservations
                .entry(grant.source_offer_id.clone())
                .and_modify(|reservation| {
                    reservation.state = "granted".to_string();
                    reservation.capacity_expires_at_ms = grant.hard_deadline_ms;
                })
                .or_insert_with(|| ReservationRecord {
                    offer_id: grant.source_offer_id.clone(),
                    state: "granted".to_string(),
                    ack_event: None,
                    decline_event: None,
                    created_at_ms: now_ms(),
                    capacity_expires_at_ms: grant.hard_deadline_ms,
                });
            let next_progress_at_ms = next_progress_deadline(
                now_ms(),
                grant.soft_lease_expires_at_ms,
                view.baton.baton_config.progress_interval_ms,
            );
            let record = ledger
                .grants
                .entry(grant.grant_id.clone())
                .or_insert_with(|| GrantRecord {
                    grant_id: grant.grant_id.clone(),
                    source_offer_id: grant.source_offer_id.clone(),
                    state: "received".to_string(),
                    basis_speech_revision: grant.basis_speech_revision,
                    soft_lease_expires_at_ms: grant.soft_lease_expires_at_ms,
                    hard_deadline_ms: grant.hard_deadline_ms,
                    progress_seq: grant.progress_seq,
                    next_progress_at_ms,
                    prepared_progress: None,
                    speech_event: None,
                    speech_event_id: None,
                    yield_event: None,
                    format_attempts: 0,
                });
            record.soft_lease_expires_at_ms = grant.soft_lease_expires_at_ms;
            record.hard_deadline_ms = grant.hard_deadline_ms;
            record.progress_seq = grant.progress_seq;
            if record
                .prepared_progress
                .as_ref()
                .is_some_and(|progress| progress.seq <= grant.progress_seq)
            {
                record.prepared_progress = None;
                record.next_progress_at_ms = next_progress_at_ms;
            }
            if matches!(
                record.state.as_str(),
                "terminal" | "released" | "running" | "queued"
            ) {
                restore_active_grant_state(record);
            }
        }

        for speech in &view.speeches {
            if speech.author_pubkey != agent_pubkey {
                continue;
            }
            if let Some(grant) = ledger.grants.get_mut(&speech.grant_id) {
                grant.state = "spoken".to_string();
                grant.speech_event_id = Some(speech.event_id.clone());
            }
        }

        let active_grant_id = active_grant.as_ref().map(|grant| grant.grant_id.as_str());
        for grant in ledger.grants.values_mut() {
            if Some(grant.grant_id.as_str()) != active_grant_id
                && !matches!(grant.state.as_str(), "spoken" | "yielded")
            {
                grant.state = "terminal".to_string();
                grant.prepared_progress = None;
            }
        }

        let active_attempt = view.baton.active_decision_attempt.clone();
        match active_attempt {
            Some(active) => {
                let same_attempt = ledger
                    .moderator_decision
                    .as_ref()
                    .is_some_and(|decision| decision.attempt.attempt_id == active.attempt_id);
                if same_attempt {
                    if let Some(decision) = ledger.moderator_decision.as_mut() {
                        decision.attempt = active;
                    }
                } else {
                    let locally_started =
                        ledger
                            .prepared_moderator_action
                            .as_ref()
                            .is_some_and(|prepared| {
                                matches!(
                                    prepared.action_kind.as_str(),
                                    "decision_attempt_start" | "decision_retry"
                                )
                            });
                    ledger.moderator_decision = Some(ModeratorDecisionRecord {
                        attempt: active,
                        rejections: Vec::new(),
                        handoff_dismissals: Vec::new(),
                        deferrals: Vec::new(),
                        next_action: ModeratorNextAction {
                            action: "idle".to_string(),
                            id: None,
                            reason: "decision has not run".to_string(),
                        },
                        state: if locally_started {
                            "registered"
                        } else {
                            "runtime_lost"
                        }
                        .to_string(),
                        turn_id: None,
                        turn_started_at_ms: None,
                        cas_rebases: 0,
                        fast_rebases: 0,
                        pending_retry: None,
                        pending_finish_reason: None,
                    });
                    if locally_started {
                        ledger.replacement_attempt_id = None;
                    }
                }
                if ledger
                    .prepared_moderator_action
                    .as_ref()
                    .is_some_and(|prepared| {
                        matches!(
                            prepared.action_kind.as_str(),
                            "decision_attempt_start" | "decision_retry"
                        )
                    })
                {
                    ledger.prepared_moderator_action = None;
                }
            }
            None => {
                if let Some(decision) = ledger.moderator_decision.as_mut() {
                    if decision.state == "abandoning" {
                        ledger.replacement_attempt_id = Some(decision.attempt.attempt_id.clone());
                        decision.state = "terminal".to_string();
                    } else if !matches!(
                        decision.state.as_str(),
                        "starting" | "retrying" | "terminal"
                    ) {
                        decision.state = "terminal".to_string();
                    }
                }
                if ledger
                    .prepared_moderator_action
                    .as_ref()
                    .is_some_and(|prepared| prepared.action_kind != "decision_attempt_start")
                {
                    ledger.prepared_moderator_action = None;
                }
            }
        }

        self.persist_ledger_best_effort();
        if let Some(attempt) = registered_attempt {
            self.emit_moderator_decision_event(
                "meeting_v1_moderator_attempt_registered",
                view.session_id,
                None,
                (
                    "registered",
                    if prepared_attempt_transition.as_deref() == Some("decision_retry") {
                        "relay_retry_registered"
                    } else {
                        "relay_attempt_registered"
                    },
                ),
                None,
                json!({
                    "attempt_id": attempt.attempt_id,
                    "deadline_ms": attempt.deadline_ms,
                }),
            );
            if prepared_attempt_transition.as_deref() == Some("decision_retry") {
                self.emit_moderator_decision_event(
                    "meeting_v1_moderator_decision_retry_started",
                    view.session_id,
                    None,
                    ("registered", "retry_ticket_consumed"),
                    None,
                    json!({
                        "attempt_id": attempt.attempt_id,
                        "deadline_ms": attempt.deadline_ms,
                    }),
                );
            }
        }
    }

    async fn reconcile(&mut self, session_id: Uuid) {
        let Some(view) = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.clone())
        else {
            return;
        };
        self.discard_stale_granted_requests(session_id, &view);
        self.discard_stale_queued_moderator_request(session_id, &view);
        if view.ended {
            self.pending
                .retain(|request| request.session_id != session_id);
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.queued = false;
            }
            return;
        }

        if self.retry_prepared_control(session_id, &view).await {
            return;
        }
        if self.handle_offer(session_id, &view).await {
            return;
        }

        if let Some(grant) = view
            .baton
            .grant
            .as_ref()
            .filter(|grant| grant.holder_pubkey == self.agent_pubkey)
        {
            self.preempt_intent_turn(session_id);
            if self
                .retry_prepared_grant_terminal(session_id, &view, grant)
                .await
            {
                return;
            }
            if self.grant_waits_for_canonical_state(session_id, &grant.grant_id) {
                return;
            }
            if !self.semantic_snapshot_ready(session_id) {
                self.request_fast_backfill(session_id);
                return;
            }
            if !speech_projection_complete(&view) || !grant_context_complete(&view, grant) {
                self.request_fast_backfill(session_id);
                return;
            }
            let busy = self.session_turn_busy(session_id);
            if !busy && !self.deferred_turn_results.contains_key(&session_id) {
                self.queue_granted_turn(session_id, &view, grant);
            }
            return;
        }

        if self
            .retry_prepared_moderator_action(session_id, &view)
            .await
        {
            return;
        }
        let local_decision_state = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .map(|decision| decision.state.clone());
        match local_decision_state.as_deref() {
            Some("runtime_lost") => {
                self.prepare_moderator_attempt_abandon(session_id, &view);
                return;
            }
            Some("result_stale") => {
                self.prepare_moderator_attempt_finish(session_id, &view);
                return;
            }
            _ => {}
        }

        if view
            .baton
            .offer
            .as_ref()
            .is_some_and(|offer| offer.target_pubkey == self.agent_pubkey)
        {
            return;
        }

        if !self.semantic_snapshot_ready(session_id) {
            self.request_fast_backfill(session_id);
            return;
        }
        if !speech_projection_complete(&view) {
            self.request_fast_backfill(session_id);
            return;
        }

        if view.baton.moderator_pubkey == self.agent_pubkey {
            self.preempt_participant_turn(session_id);
            if !matches!(
                view.baton.phase.as_str(),
                "moderator_control" | "moderator_idle"
            ) || human_priority_active(&view.baton)
            {
                // Shared state may invalidate an in-flight decision, but it
                // never physically cancels the provider Turn. Its natural
                // terminal is fenced after the already-requested Full Sync.
                return;
            }
            if moderator_deadline_expired(&view.baton, now_ms()) {
                return;
            }

            let decision_state = self
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| decision.state.clone());
            match decision_state.as_deref() {
                Some("retry_pending") => {
                    self.prepare_moderator_decision_retry(session_id, &view);
                }
                Some("ready" | "rebasing") => {
                    self.execute_ready_moderator_control(session_id, &view)
                        .await;
                }
                Some("registered") => {
                    if !self.session_turn_busy(session_id)
                        && !self.deferred_turn_results.contains_key(&session_id)
                    {
                        self.queue_moderator_control(session_id, &view);
                    }
                }
                Some(
                    "queued" | "running" | "starting" | "finishing" | "retrying" | "abandoning",
                ) => {}
                Some("terminal") | None => {
                    let has_replacement = self
                        .ledger_for(session_id)
                        .and_then(|ledger| ledger.replacement_attempt_id.as_ref())
                        .is_some();
                    if view.baton.active_decision_attempt.is_none()
                        && (view.baton.decision_attempt == 0 || has_replacement)
                        && moderator_has_startable_candidate(&view.baton)
                        && !self.session_turn_busy(session_id)
                        && !self.deferred_turn_results.contains_key(&session_id)
                    {
                        self.prepare_moderator_attempt_start(session_id, &view);
                    }
                }
                Some(_) => {}
            }
            return;
        }

        self.replace_stale_queued_intent(session_id, &view);
        if self.session_turn_busy(session_id)
            || self.deferred_turn_results.contains_key(&session_id)
        {
            return;
        }

        if self.retry_prepared_intent(session_id, &view).await {
            return;
        }
        self.queue_latest_intent_trigger(session_id, &view);
    }

    fn queue_moderator_control(&mut self, session_id: Uuid, view: &MeetingView) {
        let now = now_ms();
        let Some(attempt) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .filter(|decision| decision.state == "registered")
            .map(|decision| decision.attempt.clone())
        else {
            return;
        };
        let hard_deadline_unix_ms = attempt
            .deadline_ms
            .saturating_sub(MODERATOR_DEADLINE_SAFETY_MARGIN.as_millis() as i64);
        if now >= hard_deadline_unix_ms {
            return;
        }
        if attempt.candidate_refs.is_empty() {
            self.prepare_moderator_complete_cohort(session_id, view);
            return;
        }
        let prompt = build_moderator_control_prompt(view, &attempt, hard_deadline_unix_ms);
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            if let Some(decision) = ledger.moderator_decision.as_mut() {
                decision.state = "queued".to_string();
            }
        }
        self.persist_ledger_best_effort();
        self.queue_turn(MeetingTurnRequest {
            session_id,
            prompt,
            hard_deadline_unix_ms,
            kind: MeetingTurnKind::V1ModeratorControl,
            format_retry: false,
            basis_id: attempt.attempt_id.clone(),
            round_number: attempt.speech_revision,
            speech_cursor: view.speech_cursor.clone(),
            floor_revision: view.baton.state_revision,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        });
        self.emit(
            "meeting_v1_moderator_control_started",
            session_id,
            None,
            json!({
                "attempt_id": attempt.attempt_id,
                "control_epoch": attempt.control_epoch,
                "decision_epoch": attempt.decision_epoch,
                "attempt_number": attempt.attempt_number,
                "candidate_snapshot_hash": attempt.candidate_snapshot_hash,
                "deadline_ms": attempt.deadline_ms,
            }),
        );
    }

    fn handle_moderator_control_result(
        &mut self,
        turn_id: &str,
        request: &MeetingTurnRequest,
        raw_output: &str,
        succeeded: bool,
    ) {
        let Some(view) = self
            .meetings
            .get(&request.session_id)
            .and_then(|runtime| runtime.view.clone())
        else {
            return;
        };
        let Some(decision) = self
            .ledger_for(request.session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .filter(|decision| {
                decision.attempt.attempt_id == request.basis_id
                    && matches!(decision.state.as_str(), "running" | "queued")
            })
            .cloned()
        else {
            return;
        };
        let guard_failure =
            moderator_attempt_guard_failure(&view, &decision.attempt, &self.agent_pubkey, now_ms());
        if let Some(reason) = guard_failure {
            self.emit_moderator_decision_event(
                "meeting_v1_moderator_decision_validated",
                request.session_id,
                Some(turn_id.to_string()),
                ("invalid", reason),
                None,
                json!({}),
            );
            self.mark_moderator_result_stale(request.session_id, reason);
            self.emit(
                "meeting_v1_moderator_plan_stale",
                request.session_id,
                Some(turn_id.to_string()),
                json!({
                    "attempt_id": decision.attempt.attempt_id,
                    "reason": reason,
                }),
            );
            return;
        }
        let output = if succeeded {
            parse_control_output(raw_output, &view, &decision.attempt, &self.agent_pubkey).ok()
        } else {
            None
        };
        let Some(output) = output else {
            self.emit_moderator_decision_event(
                "meeting_v1_moderator_decision_validated",
                request.session_id,
                Some(turn_id.to_string()),
                ("invalid", "no_action"),
                None,
                json!({}),
            );
            self.mark_moderator_result_stale(request.session_id, "no_action");
            return;
        };
        if let Some(current) = self
            .ledger_for_mut(request.session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
            .filter(|current| current.attempt.attempt_id == request.basis_id)
        {
            current.rejections = output.rejections;
            current.handoff_dismissals = output.handoff_dismissals;
            current.deferrals = output.deferrals;
            current.next_action = output.next_action;
            current.state = "ready".to_string();
            current.turn_id = Some(turn_id.to_string());
        }
        self.persist_ledger_best_effort();
        self.emit_moderator_decision_event(
            "meeting_v1_moderator_decision_validated",
            request.session_id,
            Some(turn_id.to_string()),
            ("valid", "semantic_guard_passed"),
            None,
            json!({}),
        );
        self.emit(
            "meeting_v1_moderator_control_ready",
            request.session_id,
            Some(turn_id.to_string()),
            json!({
                "attempt_id": decision.attempt.attempt_id,
                "control_epoch": decision.attempt.control_epoch,
                "decision_epoch": decision.attempt.decision_epoch,
                "candidate_snapshot_hash": decision.attempt.candidate_snapshot_hash,
                "latency_ms": now_ms().saturating_sub(request.queued_at_unix_ms),
            }),
        );
    }

    async fn execute_ready_moderator_control(
        &mut self,
        session_id: Uuid,
        view: &MeetingView,
    ) -> bool {
        let Some(decision) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .filter(|decision| matches!(decision.state.as_str(), "ready" | "rebasing"))
            .cloned()
        else {
            return false;
        };
        if self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.moderator_rebase_at)
            .is_some_and(|deadline| Instant::now() < deadline)
        {
            return true;
        }
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.moderator_rebase_at = None;
        }
        if let Some(reason) =
            moderator_attempt_guard_failure(view, &decision.attempt, &self.agent_pubkey, now_ms())
        {
            self.mark_moderator_result_stale(session_id, reason);
            return true;
        }

        if let Some(proposal) = decision.rejections.first().cloned() {
            let candidate = decision
                .attempt
                .candidate_refs
                .iter()
                .find(|candidate| {
                    candidate.source_type == "intent" && candidate.source_id == proposal.intent_id
                })
                .cloned();
            if let Some(candidate) = candidate {
                if !intent_candidate_is_current(&candidate, &view.baton) {
                    self.skip_moderator_cleanup(
                        session_id,
                        "reject",
                        &proposal.intent_id,
                        "dependency_stale",
                    );
                    return true;
                }
                return self.prepare_moderator_action(
                    session_id,
                    view,
                    &decision,
                    ModeratorActionSpec::Reject {
                        candidate,
                        proposal,
                    },
                );
            }
            self.skip_moderator_cleanup(
                session_id,
                "reject",
                &proposal.intent_id,
                "not_in_candidate_cohort",
            );
            return true;
        }

        if let Some(proposal) = decision.handoff_dismissals.first().cloned() {
            let candidate = decision
                .attempt
                .candidate_refs
                .iter()
                .find(|candidate| {
                    candidate.source_type == "handoff" && candidate.source_id == proposal.handoff_id
                })
                .cloned();
            if let Some(candidate) = candidate {
                if !handoff_candidate_is_current(&candidate, &view.baton)
                    || baton_has_active_handoff_attempt(&view.baton, &candidate.source_id)
                {
                    self.skip_moderator_cleanup(
                        session_id,
                        "dismiss_handoff",
                        &proposal.handoff_id,
                        "dependency_stale",
                    );
                    return true;
                }
                return self.prepare_moderator_action(
                    session_id,
                    view,
                    &decision,
                    ModeratorActionSpec::Dismiss {
                        candidate,
                        proposal,
                    },
                );
            }
            self.skip_moderator_cleanup(
                session_id,
                "dismiss_handoff",
                &proposal.handoff_id,
                "not_in_candidate_cohort",
            );
            return true;
        }

        let action = match moderator_next_action_spec(&decision, &self.agent_pubkey) {
            Ok(action) => action,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    attempt = %decision.attempt.attempt_id,
                    "could not resolve Meeting V1 moderator plan: {error}"
                );
                self.mark_moderator_result_stale(session_id, "no_action");
                return true;
            }
        };
        if matches!(action, ModeratorActionSpec::Idle) {
            if current_cohort_has_candidates(&view.baton, decision.attempt.decision_epoch) {
                self.set_moderator_finish_reason(session_id, "idle_wait_fallback");
                self.prepare_moderator_attempt_finish(session_id, view);
            } else {
                self.prepare_moderator_complete_cohort(session_id, view);
            }
            return true;
        }
        self.prepare_moderator_action(session_id, view, &decision, action)
    }

    async fn retry_prepared_moderator_action(
        &mut self,
        session_id: Uuid,
        view: &MeetingView,
    ) -> bool {
        let Some(prepared) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.prepared_moderator_action.clone())
        else {
            return false;
        };
        if prepared.state == "sent" {
            self.request_fast_backfill(session_id);
            return true;
        }
        if prepared.state == "rejected" {
            if let Some(ledger) = self.ledger_for_mut(session_id) {
                ledger.prepared_moderator_action = None;
            }
            self.persist_ledger_best_effort();
            return true;
        }
        if !self.semantic_snapshot_ready(session_id) {
            return true;
        }
        if matches!(prepared.action_kind.as_str(), "reject" | "dismiss_handoff") {
            let source_is_current = self
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .and_then(|decision| {
                    decision
                        .attempt
                        .candidate_refs
                        .iter()
                        .find(|candidate| candidate.source_id == prepared.object_id)
                })
                .is_some_and(|candidate| {
                    if prepared.action_kind == "reject" {
                        intent_candidate_is_current(candidate, &view.baton)
                    } else {
                        handoff_candidate_is_current(candidate, &view.baton)
                    }
                });
            if !source_is_current {
                if let Some(ledger) = self.ledger_for_mut(session_id) {
                    ledger.prepared_moderator_action = None;
                }
                self.skip_moderator_cleanup(
                    session_id,
                    &prepared.action_kind,
                    &prepared.object_id,
                    "canonical_state_already_advanced",
                );
                return true;
            }
        }
        let attempt = prepared.attempt_id.as_deref().and_then(|attempt_id| {
            self.ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .filter(|decision| decision.attempt.attempt_id == attempt_id)
                .map(|decision| decision.attempt.clone())
        });
        let replay_allowed = match prepared.action_kind.as_str() {
            "decision_attempt_finish" | "decision_attempt_abandon" => true,
            "decision_attempt_start" => {
                view.baton.active_decision_attempt.is_none()
                    && matches!(
                        view.baton.phase.as_str(),
                        "moderator_control" | "moderator_idle"
                    )
                    && view.baton.moderator_pubkey == self.agent_pubkey
                    && !human_priority_active(&view.baton)
                    && now_ms() < prepared.hard_deadline_unix_ms
            }
            _ => attempt.as_ref().is_some_and(|attempt| {
                moderator_attempt_guard_failure(view, attempt, &self.agent_pubkey, now_ms())
                    .is_none()
            }),
        };
        if !replay_allowed {
            if let Some(ledger) = self.ledger_for_mut(session_id) {
                ledger.prepared_moderator_action = None;
            }
            if !matches!(
                prepared.action_kind.as_str(),
                "decision_attempt_start" | "decision_attempt_finish" | "decision_attempt_abandon"
            ) {
                self.mark_moderator_result_stale(session_id, "control_changed");
            } else {
                self.persist_ledger_best_effort();
            }
            return true;
        }
        let Ok(event) = serde_json::from_value::<Event>(prepared.event.clone()) else {
            if let Some(ledger) = self.ledger_for_mut(session_id) {
                ledger.prepared_moderator_action = None;
            }
            self.mark_moderator_result_stale(session_id, "no_action");
            return true;
        };
        if !self.persist_ledger_required(session_id, "moderator_action_retry") {
            return true;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Moderator {
                session_id,
                event_id: prepared.event_id.clone(),
            },
            ProtocolSubmissionContext::Moderator {
                action_kind: prepared.action_kind,
                object_id: prepared.object_id,
                attempt_id: prepared.attempt_id,
                turn_id: None,
                queued_at_ms: Some(prepared.created_at_ms),
                #[cfg(feature = "meeting-v1-acceptance")]
                barrier: None,
            },
            event,
        );
        true
    }

    fn skip_moderator_cleanup(
        &mut self,
        session_id: Uuid,
        action_kind: &str,
        object_id: &str,
        reason: &str,
    ) {
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            if action_kind == "reject" {
                decision
                    .rejections
                    .retain(|proposal| proposal.intent_id != object_id);
            } else {
                decision
                    .handoff_dismissals
                    .retain(|proposal| proposal.handoff_id != object_id);
            }
        }
        self.persist_ledger_best_effort();
        self.emit(
            "meeting_v1_moderator_cleanup_skipped",
            session_id,
            None,
            json!({
                "action": action_kind,
                "object_id": object_id,
                "reason": reason,
            }),
        );
    }

    fn prepare_moderator_action(
        &mut self,
        session_id: Uuid,
        view: &MeetingView,
        decision: &ModeratorDecisionRecord,
        action: ModeratorActionSpec,
    ) -> bool {
        let (action_kind, object_id, event) =
            match build_moderator_action_event(session_id, view, decision, &action, &self.keys) {
                Ok(action) => action,
                Err(error) => {
                    tracing::warn!(
                        meeting = %session_id,
                        attempt = %decision.attempt.attempt_id,
                        "could not prepare Meeting V1 moderator action: {error}"
                    );
                    self.mark_moderator_result_stale(session_id, "no_action");
                    return true;
                }
            };
        self.prepare_and_submit_moderator_event(
            session_id,
            action_kind,
            object_id,
            Some(decision.attempt.attempt_id.clone()),
            decision.attempt.deadline_ms,
            event,
        )
    }

    fn prepare_moderator_attempt_start(&mut self, session_id: Uuid, view: &MeetingView) -> bool {
        if view.baton.active_decision_attempt.is_some()
            || self
                .ledger_for(session_id)
                .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
                .is_some()
        {
            return false;
        }
        let replacement = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.replacement_attempt_id.clone());
        let event = match buzz_sdk::build_meeting_v1_decision_attempt_start(
            MeetingV1DecisionAttemptStartParams {
                session_id,
                expected_control_epoch: view.baton.control_epoch,
                expected_decision_epoch: view.baton.decision_epoch,
                expected_intent_revision: view.baton.intent_revision,
                expected_speech_revision: view.baton.speech_revision,
                expected_state_event_id: &view.baton.state_event_id,
                replacement_of_attempt_id: replacement.as_deref(),
            },
        )
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    "could not prepare Meeting V1 DecisionAttempt Start: {error}"
                );
                return true;
            }
        };
        let hard_deadline = replacement
            .as_deref()
            .and_then(|attempt_id| {
                self.ledger_for(session_id)
                    .and_then(|ledger| ledger.moderator_decision.as_ref())
                    .filter(|decision| decision.attempt.attempt_id == attempt_id)
                    .map(|decision| decision.attempt.deadline_ms)
            })
            .unwrap_or_else(|| {
                moderator_local_deadline(&view.baton, now_ms())
                    .max(now_ms().saturating_add(PROTOCOL_SUBMIT_TIMEOUT.as_millis() as i64))
            });
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.state = "starting".to_string();
        }
        self.prepare_and_submit_moderator_event(
            session_id,
            "decision_attempt_start".to_string(),
            view.baton.state_event_id.clone(),
            replacement,
            hard_deadline,
            event,
        )
    }

    fn prepare_moderator_attempt_finish(&mut self, session_id: Uuid, _view: &MeetingView) -> bool {
        let Some((attempt, reason)) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .and_then(|decision| {
                decision
                    .pending_finish_reason
                    .as_ref()
                    .map(|reason| (decision.attempt.clone(), reason.clone()))
            })
        else {
            return false;
        };
        let outcome = if matches!(reason.as_str(), "no_action" | "idle_wait_fallback") {
            MeetingV1DecisionAttemptFinishOutcome::Completed
        } else {
            MeetingV1DecisionAttemptFinishOutcome::Discarded
        };
        let event = match buzz_sdk::build_meeting_v1_decision_attempt_finish(
            MeetingV1DecisionAttemptFinishParams {
                session_id,
                attempt_id: &attempt.attempt_id,
                outcome,
                reason_code: &reason,
            },
        )
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    attempt = %attempt.attempt_id,
                    "could not prepare Meeting V1 DecisionAttempt Finish: {error}"
                );
                return true;
            }
        };
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.state = "finishing".to_string();
        }
        self.prepare_and_submit_moderator_event(
            session_id,
            "decision_attempt_finish".to_string(),
            attempt.attempt_id.clone(),
            Some(attempt.attempt_id),
            i64::MAX,
            event,
        )
    }

    fn prepare_moderator_decision_retry(&mut self, session_id: Uuid, _view: &MeetingView) -> bool {
        let Some((attempt, pending)) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .and_then(|decision| {
                decision
                    .pending_retry
                    .as_ref()
                    .map(|pending| (decision.attempt.clone(), pending.clone()))
            })
        else {
            return false;
        };
        let event = match buzz_sdk::build_meeting_v1_decision_retry(MeetingV1DecisionRetryParams {
            session_id,
            attempt_id: &attempt.attempt_id,
            retry_ticket_id: &pending.retry_ticket_id,
            failed_action_event_id: &pending.failed_action_event_id,
            expected_control_epoch: attempt.control_epoch,
            expected_decision_epoch: attempt.decision_epoch,
            expected_attempt_number: attempt.attempt_number,
        })
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    attempt = %attempt.attempt_id,
                    "could not prepare Meeting V1 DecisionRetry: {error}"
                );
                self.mark_moderator_result_stale(session_id, "source_changed");
                return true;
            }
        };
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.state = "retrying".to_string();
        }
        self.prepare_and_submit_moderator_event(
            session_id,
            "decision_retry".to_string(),
            pending.retry_ticket_id,
            Some(attempt.attempt_id),
            attempt.deadline_ms,
            event,
        )
    }

    fn prepare_moderator_complete_cohort(&mut self, session_id: Uuid, _view: &MeetingView) -> bool {
        let Some(attempt) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .map(|decision| decision.attempt.clone())
        else {
            return false;
        };
        let event =
            match buzz_sdk::build_meeting_v1_complete_cohort(MeetingV1CompleteCohortParams {
                session_id,
                attempt_id: &attempt.attempt_id,
                expected_control_epoch: attempt.control_epoch,
                expected_decision_epoch: attempt.decision_epoch,
            })
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|builder| sign_builder(builder, &self.keys))
            {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(
                        meeting = %session_id,
                        attempt = %attempt.attempt_id,
                        "could not prepare Meeting V1 CompleteCohort: {error}"
                    );
                    self.mark_moderator_result_stale(session_id, "no_action");
                    return true;
                }
            };
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.state = "finishing".to_string();
        }
        self.prepare_and_submit_moderator_event(
            session_id,
            "complete_cohort".to_string(),
            attempt.attempt_id.clone(),
            Some(attempt.attempt_id),
            attempt.deadline_ms,
            event,
        )
    }

    fn prepare_moderator_attempt_abandon(&mut self, session_id: Uuid, _view: &MeetingView) -> bool {
        let Some(attempt) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .map(|decision| decision.attempt.clone())
        else {
            return false;
        };
        let event = match buzz_sdk::build_meeting_v1_decision_attempt_abandon(
            MeetingV1DecisionAttemptAbandonParams {
                session_id,
                attempt_id: &attempt.attempt_id,
            },
        )
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    attempt = %attempt.attempt_id,
                    "could not prepare Meeting V1 DecisionAttempt Abandon: {error}"
                );
                return true;
            }
        };
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.state = "abandoning".to_string();
        }
        self.prepare_and_submit_moderator_event(
            session_id,
            "decision_attempt_abandon".to_string(),
            attempt.attempt_id.clone(),
            Some(attempt.attempt_id),
            i64::MAX,
            event,
        )
    }

    fn prepare_and_submit_moderator_event(
        &mut self,
        session_id: Uuid,
        action_kind: String,
        object_id: String,
        attempt_id: Option<String>,
        hard_deadline_unix_ms: i64,
        event: Event,
    ) -> bool {
        let event_id = event.id.to_hex();
        let Some(event_value) = serde_json::to_value(&event).ok() else {
            self.mark_moderator_result_stale(session_id, "no_action");
            return true;
        };
        let turn_id = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .and_then(|decision| decision.turn_id.clone());
        let queued_at_ms = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .map(|decision| decision.attempt.started_at_ms);
        #[cfg(feature = "meeting-v1-acceptance")]
        let barrier = self.acceptance_barrier_for_moderator_action(
            session_id,
            &action_kind,
            &object_id,
            &event_id,
            hard_deadline_unix_ms,
            turn_id.clone(),
        );
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            ledger.prepared_moderator_action = Some(PreparedModeratorAction {
                action_kind: action_kind.clone(),
                object_id: object_id.clone(),
                attempt_id: attempt_id.clone(),
                event: event_value,
                event_id: event_id.clone(),
                state: "prepared".to_string(),
                created_at_ms: now_ms(),
                hard_deadline_unix_ms,
            });
        }
        if !self.persist_ledger_required(session_id, "moderator_action") {
            return true;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Moderator {
                session_id,
                event_id,
            },
            ProtocolSubmissionContext::Moderator {
                action_kind,
                object_id,
                attempt_id,
                turn_id,
                queued_at_ms,
                #[cfg(feature = "meeting-v1-acceptance")]
                barrier,
            },
            event,
        );
        true
    }

    #[cfg(feature = "meeting-v1-acceptance")]
    fn acceptance_barrier_for_moderator_action(
        &mut self,
        session_id: Uuid,
        action_kind: &str,
        object_id: &str,
        signed_event_id: &str,
        hard_deadline_unix_ms: i64,
        turn_id: Option<String>,
    ) -> Option<Box<(PathBuf, PreSubmitBarrierFrame)>> {
        let selected_source_type = match action_kind {
            "select_intent" | "moderator_speak" => "intent",
            "select_handoff" => "handoff",
            _ => return None,
        };
        let decision = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .cloned()?;
        let view = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .cloned()?;
        let selected = decision.attempt.candidate_refs.iter().find(|candidate| {
            candidate.source_type == selected_source_type && candidate.source_id == object_id
        })?;
        let socket_path = self.acceptance_barrier.claim()?;
        let frame = PreSubmitBarrierFrame {
            frame_type: "meeting_v1_pre_submit",
            token: Uuid::new_v4().to_string(),
            harness_pid: std::process::id(),
            session_id: session_id.to_string(),
            turn_id,
            attempt_id: decision.attempt.attempt_id.clone(),
            control_epoch: decision.attempt.control_epoch,
            decision_epoch: decision.attempt.decision_epoch,
            attempt_number: decision.attempt.attempt_number,
            speech_revision: decision.attempt.speech_revision,
            snapshot_intent_revision: decision.attempt.snapshot_intent_revision,
            current_intent_revision: view.baton.intent_revision,
            candidate_snapshot_hash: decision.attempt.candidate_snapshot_hash.clone(),
            candidate_cohort: decision
                .attempt
                .candidate_refs
                .iter()
                .map(|candidate| AcceptanceCandidateRef {
                    source_type: candidate.source_type.clone(),
                    source_id: candidate.source_id.clone(),
                    current_event_id: candidate.current_event_id.clone(),
                    author_pubkey: candidate.author_pubkey.clone(),
                    eligible_decision_epoch: candidate.eligible_decision_epoch,
                })
                .collect(),
            selected_source_type: selected_source_type.to_string(),
            selected_source_id: object_id.to_string(),
            selected_source_event_id: selected.current_event_id.clone(),
            action_kind: action_kind.to_string(),
            signed_event_id: signed_event_id.to_string(),
            hard_deadline_unix_ms,
        };
        Some(Box::new((socket_path, frame)))
    }

    fn set_moderator_finish_reason(&mut self, session_id: Uuid, reason: &str) {
        if let Some(decision) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
        {
            decision.pending_finish_reason = Some(reason.to_string());
            decision.state = "result_stale".to_string();
        }
        self.persist_ledger_best_effort();
    }

    fn mark_moderator_result_stale(&mut self, session_id: Uuid, reason: &str) {
        let reason = match reason {
            "human_priority" => "human_priority",
            "speech_changed" => "speech_changed",
            "meeting_ended" => "meeting_ended",
            "moderator_changed" => "moderator_changed",
            "cas_churn" => "cas_churn",
            "source_changed" => "source_changed",
            "no_action" | "idle_wait_fallback" => reason,
            _ => "control_changed",
        };
        self.set_moderator_finish_reason(session_id, reason);
        self.emit_moderator_decision_event(
            "meeting_v1_moderator_decision_discarded",
            session_id,
            None,
            ("discarded", reason),
            None,
            json!({ "reason": reason }),
        );
    }

    fn semantic_snapshot_ready(&self, session_id: Uuid) -> bool {
        self.meetings
            .get(&session_id)
            .is_some_and(|runtime| runtime.last_sync.is_some())
    }

    fn discard_stale_granted_requests(&mut self, session_id: Uuid, view: &MeetingView) {
        let active_grant_id = view
            .baton
            .grant
            .as_ref()
            .filter(|grant| grant.holder_pubkey == self.agent_pubkey)
            .map(|grant| grant.grant_id.as_str());
        self.pending.retain(|request| {
            request.session_id != session_id
                || request.kind != MeetingTurnKind::V1Granted
                || request.grant_event_id.as_deref() == active_grant_id
        });
        let still_queued = self
            .pending
            .iter()
            .any(|request| request.session_id == session_id);
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.queued = still_queued;
        }
        let stale_in_flight = self.in_flight.values().any(|request| {
            request.session_id == session_id
                && request.kind == MeetingTurnKind::V1Granted
                && request.grant_event_id.as_deref() != active_grant_id
        });
        if stale_in_flight {
            self.preemptions.insert(session_id);
        }
    }

    fn discard_stale_queued_moderator_request(&mut self, session_id: Uuid, view: &MeetingView) {
        let queued_attempt = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .filter(|decision| decision.state == "queued")
            .map(|decision| decision.attempt.clone());
        let valid_attempt_id = queued_attempt
            .as_ref()
            .filter(|attempt| {
                moderator_attempt_guard_failure(view, attempt, &self.agent_pubkey, now_ms())
                    .is_none()
            })
            .map(|attempt| attempt.attempt_id.as_str());
        let mut removed_bases = Vec::new();
        self.pending.retain(|request| {
            let stale = request.session_id == session_id
                && request.kind == MeetingTurnKind::V1ModeratorControl
                && valid_attempt_id != Some(request.basis_id.as_str());
            if stale {
                removed_bases.push(request.basis_id.clone());
            }
            !stale
        });
        if removed_bases.is_empty() {
            return;
        }
        let still_queued = self
            .pending
            .iter()
            .any(|request| request.session_id == session_id);
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.queued = still_queued;
        }
        let stale_reason = queued_attempt
            .as_ref()
            .filter(|attempt| removed_bases.contains(&attempt.attempt_id))
            .and_then(|attempt| {
                moderator_attempt_guard_failure(view, attempt, &self.agent_pubkey, now_ms())
            });
        if let Some(reason) = stale_reason {
            self.mark_moderator_result_stale(session_id, reason);
        } else {
            self.persist_ledger_best_effort();
        }
        self.emit(
            "meeting_v1_moderator_queued_turn_discarded",
            session_id,
            None,
            json!({
                "attempt_ids": removed_bases,
                "reason": stale_reason.unwrap_or("attempt_not_current"),
            }),
        );
    }

    fn grant_waits_for_canonical_state(&self, session_id: Uuid, grant_id: &str) -> bool {
        self.ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(grant_id))
            .is_some_and(|record| {
                matches!(
                    record.state.as_str(),
                    "speech_sent" | "yield_sent" | "spoken" | "yielded"
                )
            })
    }

    fn request_fast_backfill(&mut self, session_id: Uuid) {
        self.request_full_sync(session_id);
    }

    fn replace_stale_queued_intent(&mut self, session_id: Uuid, view: &MeetingView) {
        let newest_pending = self.ledger_for(session_id).and_then(|ledger| {
            ledger
                .triggers
                .values()
                .filter(|trigger| trigger.state == "pending")
                .max_by_key(|trigger| (trigger.basis_speech_revision, trigger.created_at_ms))
                .map(|trigger| trigger.trigger_id.clone())
        });
        let queued = self
            .pending
            .iter()
            .find(|request| {
                request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent
            })
            .map(|request| (request.basis_id.clone(), request.round_number));
        let Some((queued_basis, queued_revision)) = queued else {
            return;
        };
        let should_replace = queued_revision != view.baton.speech_revision
            || newest_pending
                .as_ref()
                .is_some_and(|basis| basis != &queued_basis);
        if !should_replace {
            return;
        }
        self.pending.retain(|request| {
            !(request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent)
        });
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.queued = false;
        }
        self.mark_trigger_state(session_id, &queued_basis, "superseded");
    }

    async fn retry_prepared_control(&mut self, session_id: Uuid, view: &MeetingView) -> bool {
        let Some(offer) = view
            .baton
            .offer
            .as_ref()
            .filter(|offer| offer.target_pubkey == self.agent_pubkey)
        else {
            return false;
        };
        let prepared = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.reservations.get(&offer.offer_id))
            .and_then(|reservation| match reservation.state.as_str() {
                "ack_prepared" | "ack_sent" => reservation
                    .ack_event
                    .clone()
                    .map(|event| (OfferSubmissionAction::Ack, event)),
                "decline_prepared" | "decline_sent" => reservation
                    .decline_event
                    .clone()
                    .map(|event| (OfferSubmissionAction::Decline, event)),
                _ => None,
            });
        let Some((action, value)) = prepared else {
            return false;
        };
        let Ok(event) = serde_json::from_value::<Event>(value) else {
            self.release_reservation(session_id, &offer.offer_id, "invalid_prepared_event");
            return true;
        };
        if !self.persist_ledger_required(session_id, "offer_response_retry") {
            self.schedule_offer_retry_if_active(session_id, &offer.offer_id);
            return true;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Offer {
                session_id,
                offer_id: offer.offer_id.clone(),
            },
            ProtocolSubmissionContext::Offer {
                offer_id: offer.offer_id.clone(),
                action,
                allocation_source: offer.allocation_source.clone(),
                turn_role: offer.turn_role.clone(),
                created_at_ms: offer.created_at_ms,
            },
            event,
        );
        true
    }

    async fn handle_offer(&mut self, session_id: Uuid, view: &MeetingView) -> bool {
        let Some(offer) = view
            .baton
            .offer
            .as_ref()
            .filter(|offer| offer.target_pubkey == self.agent_pubkey)
            .cloned()
        else {
            return false;
        };
        if offer.target_participant_type != "agent" {
            tracing::warn!(
                meeting = %session_id,
                offer = %offer.offer_id,
                "Meeting V1 Offer targets ACP identity with non-Agent frozen type"
            );
            return true;
        }
        if now_ms() >= offer.ack_deadline_ms {
            return true;
        }
        if self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.reservations.get(&offer.offer_id))
            .is_some()
        {
            return true;
        }

        let reserved_elsewhere = self.active_reservation_count_excluding(session_id);
        let assigned_elsewhere = self.assigned_grant_count_excluding(session_id);
        let unassigned_reservations = reserved_elsewhere.saturating_sub(assigned_elsewhere);
        let mut reclaimable_turns: Vec<_> = self
            .in_flight
            .values()
            .filter(|request| match request.kind {
                MeetingTurnKind::V1Intent => self
                    .ledger_for(request.session_id)
                    .and_then(|ledger| ledger.triggers.get(&request.basis_id))
                    .is_some_and(|trigger| trigger.state == "running"),
                MeetingTurnKind::V1ModeratorControl => false,
                MeetingTurnKind::V1Granted
                | MeetingTurnKind::V0Intent
                | MeetingTurnKind::V0Granted => false,
            })
            .map(|request| {
                (
                    match request.kind {
                        MeetingTurnKind::V1Intent => 0,
                        MeetingTurnKind::V1ModeratorControl => 1,
                        _ => 2,
                    },
                    request.session_id,
                )
            })
            .collect();
        reclaimable_turns.extend(
            self.external_reclaimable_turns
                .iter()
                .map(|session_id| (0, *session_id)),
        );
        reclaimable_turns.sort_unstable();
        reclaimable_turns.dedup_by_key(|(_, session_id)| *session_id);
        let reclaimable_slots = reclaimable_turns.len();
        let has_physical_slot =
            self.available_agent_slots.saturating_add(reclaimable_slots) > unassigned_reservations;
        let should_ack = self.auto_accept_offers
            && reserved_elsewhere < self.agent_capacity
            && has_physical_slot;
        let (state, ack_event, decline_event, event) = if should_ack {
            let event = match buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
                session_id,
                offer_id: &offer.offer_id,
            })
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|builder| sign_builder(builder, &self.keys))
            {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(
                        meeting = %session_id,
                        offer = %offer.offer_id,
                        "could not prepare deterministic Meeting V1 ACK: {error}"
                    );
                    return true;
                }
            };
            (
                "ack_prepared",
                serde_json::to_value(&event).ok(),
                None,
                event,
            )
        } else {
            let reason = if !self.auto_accept_offers {
                "local Meeting Offer policy declined this turn"
            } else if reserved_elsewhere >= self.agent_capacity {
                "local Agent turn capacity is fully reserved"
            } else {
                "no physical Agent turn slot is currently available"
            };
            let event =
                match buzz_sdk::build_meeting_v1_offer_decline(MeetingV1OfferDeclineParams {
                    session_id,
                    offer_id: &offer.offer_id,
                    reason: Some(reason),
                })
                .map_err(|error| anyhow!(error.to_string()))
                .and_then(|builder| sign_builder(builder, &self.keys))
                {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(
                            meeting = %session_id,
                            offer = %offer.offer_id,
                            "could not prepare deterministic Meeting V1 Decline: {error}"
                        );
                        return true;
                    }
                };
            (
                "decline_prepared",
                None,
                serde_json::to_value(&event).ok(),
                event,
            )
        };
        let reservation = ReservationRecord {
            offer_id: offer.offer_id.clone(),
            state: state.to_string(),
            ack_event,
            decline_event,
            created_at_ms: now_ms(),
            capacity_expires_at_ms: offer
                .ack_deadline_ms
                .saturating_add(view.baton.baton_config.grant_hard_deadline_ms.max(0)),
        };
        if (should_ack && reservation.ack_event.is_none())
            || (!should_ack && reservation.decline_event.is_none())
        {
            tracing::warn!(
                meeting = %session_id,
                offer = %offer.offer_id,
                "deterministic Meeting V1 Offer response could not be serialized"
            );
            return true;
        }
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            ledger
                .reservations
                .insert(offer.offer_id.clone(), reservation);
        }
        if !self.persist_ledger_required(
            session_id,
            if should_ack {
                "offer_ack"
            } else {
                "offer_decline"
            },
        ) {
            return true;
        }

        if should_ack {
            let required_reclaims = unassigned_reservations
                .saturating_add(1)
                .saturating_sub(self.available_agent_slots);
            for (_, reclaimed_session) in reclaimable_turns.into_iter().take(required_reclaims) {
                self.preempt_intent_turn(reclaimed_session);
                // External V0 Intent turns have no V1 ledger/runtime entry,
                // but the protocol-neutral coordinator still needs the same
                // cancellation request to release their physical Agent slot.
                self.preemptions.insert(reclaimed_session);
            }
            self.preempt_intent_turn(session_id);
        }
        let action = if should_ack {
            OfferSubmissionAction::Ack
        } else {
            OfferSubmissionAction::Decline
        };
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Offer {
                session_id,
                offer_id: offer.offer_id.clone(),
            },
            ProtocolSubmissionContext::Offer {
                offer_id: offer.offer_id.clone(),
                action,
                allocation_source: offer.allocation_source.clone(),
                turn_role: offer.turn_role.clone(),
                created_at_ms: offer.created_at_ms,
            },
            event,
        );
        true
    }

    fn active_reservation_count_excluding(&self, excluded: Uuid) -> usize {
        self.active_reservation_count(Some(excluded))
    }

    fn active_reservation_count(&self, excluded: Option<Uuid>) -> usize {
        let now = now_ms();
        self.meetings
            .keys()
            .filter(|session_id| excluded.is_none_or(|excluded| **session_id != excluded))
            .filter(|session_id| {
                self.ledger_for(**session_id).is_some_and(|ledger| {
                    ledger
                        .reservations
                        .values()
                        .any(|reservation| reservation_is_active_at(reservation, now))
                })
            })
            .count()
    }

    fn granted_request_uses_active_reservation(&self, request: &MeetingTurnRequest) -> bool {
        if request.kind != MeetingTurnKind::V1Granted {
            return false;
        }
        let Some(grant_id) = request.grant_event_id.as_deref() else {
            return false;
        };
        let Some(grant) = self
            .meetings
            .get(&request.session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .and_then(|view| view.baton.grant.as_ref())
            .filter(|grant| grant.grant_id == grant_id && grant.holder_pubkey == self.agent_pubkey)
        else {
            return false;
        };
        self.ledger_for(request.session_id)
            .and_then(|ledger| ledger.grants.get(grant_id).map(|record| (ledger, record)))
            .filter(|(_, record)| record.source_offer_id == grant.source_offer_id)
            .and_then(|(ledger, record)| ledger.reservations.get(&record.source_offer_id))
            .is_some_and(|reservation| reservation_is_active_at(reservation, now_ms()))
    }

    fn assigned_grant_count_excluding(&self, excluded: Uuid) -> usize {
        self.in_flight
            .values()
            .filter(|request| {
                request.session_id != excluded
                    && self.granted_request_uses_active_reservation(request)
            })
            .map(|request| request.session_id)
            .collect::<BTreeSet<_>>()
            .len()
    }

    fn schedule_offer_retry_if_active(&mut self, session_id: Uuid, offer_id: &str) {
        let retry = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .and_then(|view| view.baton.offer.as_ref())
            .is_some_and(|offer| offer.offer_id == offer_id && now_ms() < offer.ack_deadline_ms);
        if retry {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                // The exact same signed response is replayed. Make it eligible
                // in this controller tick so a two-second uncertain transport
                // attempt still leaves room inside the five-second ACK window.
                runtime.control_retry_at = Some(Instant::now());
            }
        }
    }

    fn release_reservation(&mut self, session_id: Uuid, offer_id: &str, reason: &str) {
        if let Some(reservation) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.reservations.get_mut(offer_id))
        {
            reservation.state = "released".to_string();
        }
        self.persist_ledger_best_effort();
        self.emit(
            "meeting_v1_reservation_released",
            session_id,
            None,
            json!({ "offer_id": offer_id, "reason": reason }),
        );
    }

    fn preempt_intent_turn(&mut self, session_id: Uuid) {
        let mut removed_bases = Vec::new();
        self.pending.retain(|request| {
            let remove =
                request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent;
            if remove {
                removed_bases.push(request.basis_id.clone());
            }
            !remove
        });
        if !removed_bases.is_empty() {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.queued = false;
            }
        }
        let in_flight_basis = self
            .in_flight
            .values()
            .find(|request| {
                request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent
            })
            .map(|request| request.basis_id.clone());
        if let Some(basis) = in_flight_basis {
            removed_bases.push(basis);
            self.preemptions.insert(session_id);
        }
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            for basis in removed_bases {
                if let Some(trigger) = ledger.triggers.get_mut(&basis) {
                    if matches!(trigger.state.as_str(), "pending" | "queued" | "running") {
                        trigger.state = "preempted".to_string();
                    }
                }
            }
        }
        self.persist_ledger_best_effort();
    }

    fn preempt_participant_turn(&mut self, session_id: Uuid) {
        let mut removed_bases = Vec::new();
        self.pending.retain(|request| {
            let remove =
                request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent;
            if remove {
                removed_bases.push(request.basis_id.clone());
            }
            !remove
        });
        if !removed_bases.is_empty() {
            if let Some(runtime) = self.meetings.get_mut(&session_id) {
                runtime.queued = false;
            }
        }
        if let Some(request) = self.in_flight.values().find(|request| {
            request.session_id == session_id && request.kind == MeetingTurnKind::V1Intent
        }) {
            removed_bases.push(request.basis_id.clone());
            self.preemptions.insert(session_id);
        }
        if let Some(ledger) = self.ledger_for_mut(session_id) {
            for basis in removed_bases {
                if let Some(trigger) = ledger.triggers.get_mut(&basis) {
                    if matches!(trigger.state.as_str(), "pending" | "queued" | "running") {
                        trigger.state = "preempted".to_string();
                    }
                }
            }
        }
        self.persist_ledger_best_effort();
    }

    fn queue_granted_turn(&mut self, session_id: Uuid, view: &MeetingView, grant: &GrantView) {
        let safety_margin_ms = grant_safety_margin_ms(view);
        let hard_deadline_unix_ms = grant.hard_deadline_ms.saturating_sub(safety_margin_ms);
        if now_ms() >= hard_deadline_unix_ms {
            return;
        }
        let basis_id = grant
            .source_intent_id
            .clone()
            .or_else(|| grant.source_handoff_id.clone())
            .unwrap_or_else(|| format!("grant:{}", grant.grant_id));
        let prompt = build_granted_prompt(view, grant, &basis_id);
        if let Some(record) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.grants.get_mut(&grant.grant_id))
        {
            record.state = "queued".to_string();
        }
        self.persist_ledger_best_effort();
        self.queue_turn(MeetingTurnRequest {
            session_id,
            prompt,
            hard_deadline_unix_ms,
            kind: MeetingTurnKind::V1Granted,
            format_retry: false,
            basis_id,
            round_number: view.baton.speech_revision,
            speech_cursor: view.speech_cursor.clone(),
            floor_revision: view.baton.state_revision,
            grant_event_id: Some(grant.grant_id.clone()),
            queued_at_unix_ms: now_ms(),
        });
        self.emit(
            "meeting_v1_grant_received",
            session_id,
            None,
            json!({
                "grant_id": grant.grant_id,
                "allocation_source": grant.allocation_source,
                "turn_role": grant.turn_role,
                "hard_deadline_ms": grant.hard_deadline_ms,
            }),
        );
    }

    fn queue_latest_intent_trigger(&mut self, session_id: Uuid, view: &MeetingView) {
        let candidate = {
            let Some(ledger) = self.ledger_for_mut(session_id) else {
                return;
            };
            let newest = ledger
                .triggers
                .values()
                .filter(|trigger| trigger.state == "pending")
                .max_by_key(|trigger| (trigger.basis_speech_revision, trigger.created_at_ms))
                .map(|trigger| trigger.trigger_id.clone());
            if let Some(newest) = newest.as_deref() {
                for trigger in ledger.triggers.values_mut() {
                    if trigger.state == "pending" && trigger.trigger_id != newest {
                        trigger.state = "superseded".to_string();
                    }
                }
            }
            newest
        };
        let Some(trigger_id) = candidate else {
            return;
        };
        let hard_deadline_unix_ms = now_ms().saturating_add(INTENT_MAX_DURATION.as_millis() as i64);
        let prompt = build_intent_prompt(view, &trigger_id, hard_deadline_unix_ms);
        if let Some(trigger) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.triggers.get_mut(&trigger_id))
        {
            trigger.state = "queued".to_string();
            trigger.basis_speech_revision = view.baton.speech_revision;
        }
        self.persist_ledger_best_effort();
        self.queue_turn(MeetingTurnRequest {
            session_id,
            prompt,
            hard_deadline_unix_ms,
            kind: MeetingTurnKind::V1Intent,
            format_retry: false,
            basis_id: trigger_id.clone(),
            round_number: view.baton.speech_revision,
            speech_cursor: view.speech_cursor.clone(),
            floor_revision: view.baton.state_revision,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        });
        self.emit(
            "meeting_v1_intent_started",
            session_id,
            None,
            json!({
                "trigger_id": trigger_id,
                "speech_revision": view.baton.speech_revision,
            }),
        );
    }

    fn queue_turn(&mut self, request: MeetingTurnRequest) {
        let session_id = request.session_id;
        if self.deferred_turn_results.contains_key(&session_id)
            || self
                .in_flight
                .values()
                .any(|in_flight| in_flight.session_id == session_id)
        {
            return;
        }
        let Some(runtime) = self.meetings.get_mut(&session_id) else {
            return;
        };
        if runtime.queued || runtime.in_flight_turn.is_some() {
            return;
        }
        runtime.queued = true;
        match request.kind {
            MeetingTurnKind::V1Granted => self.pending.push_front(request),
            MeetingTurnKind::V1ModeratorControl => {
                let position = self
                    .pending
                    .iter()
                    .position(|queued| queued.kind == MeetingTurnKind::V1Intent)
                    .unwrap_or(self.pending.len());
                self.pending.insert(position, request);
            }
            MeetingTurnKind::V1Intent => self.pending.push_back(request),
            MeetingTurnKind::V0Intent | MeetingTurnKind::V0Granted => {}
        }
    }

    fn session_turn_busy(&self, session_id: Uuid) -> bool {
        self.meetings
            .get(&session_id)
            .is_some_and(|runtime| runtime.queued || runtime.in_flight_turn.is_some())
            || self
                .in_flight
                .values()
                .any(|request| request.session_id == session_id)
    }

    async fn retry_prepared_grant_terminal(
        &mut self,
        session_id: Uuid,
        _view: &MeetingView,
        grant: &GrantView,
    ) -> bool {
        let prepared = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .and_then(|record| match record.state.as_str() {
                "speech_prepared" | "speech_sent_uncertain" => record
                    .speech_event
                    .clone()
                    .map(|event| (GrantTerminalAction::Speech, event)),
                "yield_prepared" | "yield_sent_uncertain" => record
                    .yield_event
                    .clone()
                    .map(|event| (GrantTerminalAction::Yield, event)),
                _ => None,
            });
        let Some((action, value)) = prepared else {
            return false;
        };
        let Ok(event) = serde_json::from_value::<Event>(value) else {
            self.mark_grant_state(session_id, &grant.grant_id, "terminal");
            return true;
        };
        if !self.persist_ledger_required(session_id, "grant_terminal_retry") {
            self.request_fast_backfill(session_id);
            return true;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::GrantTerminal {
                session_id,
                grant_id: grant.grant_id.clone(),
            },
            ProtocolSubmissionContext::GrantTerminal {
                grant_id: grant.grant_id.clone(),
                source_offer_id: grant.source_offer_id.clone(),
                action,
                turn_id: None,
                queued_at_ms: None,
                grant_started_at_ms: None,
            },
            event,
        );
        true
    }

    async fn retry_prepared_intent(&mut self, session_id: Uuid, view: &MeetingView) -> bool {
        let expired = self.ledger_for(session_id).and_then(|ledger| {
            ledger.triggers.values().find_map(|trigger| {
                (matches!(trigger.state.as_str(), "prepared" | "sent_uncertain")
                    && trigger
                        .hard_deadline_unix_ms
                        .is_some_and(|deadline| now_ms() >= deadline))
                .then(|| trigger.trigger_id.clone())
            })
        });
        if let Some(trigger_id) = expired {
            self.mark_trigger_state(session_id, &trigger_id, "stale");
            return true;
        }
        let prepared = self.ledger_for(session_id).and_then(|ledger| {
            ledger.triggers.values().find_map(|trigger| {
                (matches!(trigger.state.as_str(), "prepared" | "sent_uncertain")
                    && trigger.basis_speech_revision == view.baton.speech_revision)
                    .then(|| {
                        trigger
                            .prepared_event
                            .clone()
                            .map(|event| (trigger.trigger_id.clone(), event))
                    })
                    .flatten()
            })
        });
        let Some((trigger_id, value)) = prepared else {
            return false;
        };
        let Ok(event) = serde_json::from_value::<Event>(value) else {
            self.mark_trigger_state(session_id, &trigger_id, "stale");
            return true;
        };
        if !self.persist_ledger_required(session_id, "intent_retry") {
            self.request_fast_backfill(session_id);
            return true;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Intent {
                session_id,
                trigger_id: trigger_id.clone(),
            },
            ProtocolSubmissionContext::Intent {
                trigger_id,
                turn_id: None,
                queued_at_ms: None,
            },
            event,
        );
        true
    }

    async fn handle_intent_result(
        &mut self,
        turn_id: &str,
        request: &MeetingTurnRequest,
        raw_output: &str,
        succeeded: bool,
    ) {
        let current = self
            .meetings
            .get(&request.session_id)
            .and_then(|runtime| runtime.view.clone());
        let valid = current.as_ref().is_some_and(|view| {
            !view.ended
                && view.baton.speech_revision == request.round_number
                && view
                    .baton
                    .grant
                    .as_ref()
                    .is_none_or(|grant| grant.holder_pubkey != self.agent_pubkey)
                && view
                    .baton
                    .offer
                    .as_ref()
                    .is_none_or(|offer| offer.target_pubkey != self.agent_pubkey)
                && self
                    .ledger_for(request.session_id)
                    .and_then(|ledger| ledger.triggers.get(&request.basis_id))
                    .is_some_and(|trigger| trigger.state == "running")
        });
        if !valid {
            self.mark_trigger_state(request.session_id, &request.basis_id, "stale");
            self.emit(
                "meeting_v1_intent_stale",
                request.session_id,
                Some(turn_id.to_string()),
                json!({
                    "trigger_id": request.basis_id,
                    "observed_speech_revision": request.round_number,
                }),
            );
            return;
        }

        let output = if succeeded {
            match parse_intent_output(raw_output) {
                Ok(output) => Some(output),
                Err(error) => {
                    if self.queue_intent_format_retry(request, &error) {
                        return;
                    }
                    None
                }
            }
        } else {
            None
        };
        let Some(output) = output else {
            self.mark_trigger_state(request.session_id, &request.basis_id, "failed");
            self.emit(
                "meeting_v1_intent_completed",
                request.session_id,
                Some(turn_id.to_string()),
                json!({
                    "trigger_id": request.basis_id,
                    "decision": "PASS",
                    "outcome": if succeeded { "invalid_output" } else { "turn_failed" },
                    "latency_ms": now_ms().saturating_sub(request.queued_at_unix_ms),
                }),
            );
            return;
        };
        if output.action == "PASS" {
            self.mark_trigger_state(request.session_id, &request.basis_id, "passed");
            self.emit(
                "meeting_v1_intent_completed",
                request.session_id,
                Some(turn_id.to_string()),
                json!({
                    "trigger_id": request.basis_id,
                    "decision": "PASS",
                    "outcome": "private_only",
                    "latency_ms": now_ms().saturating_sub(request.queued_at_unix_ms),
                }),
            );
            return;
        }

        let Some(view) = current else {
            return;
        };
        let Some(summary) = output.summary.as_deref() else {
            self.mark_trigger_state(request.session_id, &request.basis_id, "failed");
            return;
        };
        if output
            .addressed_to
            .as_ref()
            .is_some_and(|pubkey| !view.roster.contains_key(pubkey))
        {
            self.mark_trigger_state(request.session_id, &request.basis_id, "invalid_addressee");
            return;
        }
        let own_pending = view
            .baton
            .pending_intents
            .iter()
            .find(|intent| intent.author_pubkey == self.agent_pubkey);
        let builder = if let Some(intent) = own_pending {
            buzz_sdk::build_meeting_v1_intent_refresh(MeetingV1IntentRefreshParams {
                session_id: request.session_id,
                intent_id: &intent.intent_id,
                previous_event_id: &intent.current_event_id,
                basis_speech_revision: view.baton.speech_revision,
                addressed_to: output.addressed_to.as_deref(),
                summary,
            })
        } else {
            buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
                session_id: request.session_id,
                basis_speech_revision: view.baton.speech_revision,
                addressed_to: output.addressed_to.as_deref(),
                summary,
            })
        };
        let event = match builder
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %request.session_id,
                    trigger = %request.basis_id,
                    "Meeting V1 Intent build failed: {error}"
                );
                self.mark_trigger_state(request.session_id, &request.basis_id, "failed");
                return;
            }
        };
        if let Some(trigger) = self
            .ledger_for_mut(request.session_id)
            .and_then(|ledger| ledger.triggers.get_mut(&request.basis_id))
        {
            trigger.prepared_event = serde_json::to_value(&event).ok();
            trigger.prepared_event_id = Some(event.id.to_hex());
            trigger.basis_speech_revision = view.baton.speech_revision;
            trigger.state = "prepared".to_string();
        }
        let prepared_is_complete = self
            .ledger_for(request.session_id)
            .and_then(|ledger| ledger.triggers.get(&request.basis_id))
            .is_some_and(|trigger| trigger.prepared_event.is_some());
        if !prepared_is_complete {
            self.mark_trigger_state(request.session_id, &request.basis_id, "failed");
            return;
        }
        if !self.persist_ledger_required(request.session_id, "intent") {
            self.request_fast_backfill(request.session_id);
            return;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::Intent {
                session_id: request.session_id,
                trigger_id: request.basis_id.clone(),
            },
            ProtocolSubmissionContext::Intent {
                trigger_id: request.basis_id.clone(),
                turn_id: Some(turn_id.to_string()),
                queued_at_ms: Some(request.queued_at_unix_ms),
            },
            event,
        );
    }

    fn queue_intent_format_retry(
        &mut self,
        request: &MeetingTurnRequest,
        error: &anyhow::Error,
    ) -> bool {
        let Some(trigger) = self
            .ledger_for_mut(request.session_id)
            .and_then(|ledger| ledger.triggers.get_mut(&request.basis_id))
        else {
            return false;
        };
        if !reserve_format_retry(&mut trigger.format_attempts) {
            return false;
        }
        trigger.state = "queued".to_string();
        self.persist_ledger_best_effort();
        let mut retry = request.clone();
        retry.format_retry = true;
        retry.prompt = intent_format_correction_prompt();
        retry.queued_at_unix_ms = now_ms();
        self.queue_turn(retry);
        self.emit(
            "meeting_v1_intent_format_retry",
            request.session_id,
            None,
            json!({ "trigger_id": request.basis_id, "error": error.to_string() }),
        );
        true
    }

    async fn handle_granted_result(
        &mut self,
        turn_id: &str,
        request: &MeetingTurnRequest,
        raw_output: &str,
        succeeded: bool,
    ) {
        let Some(grant_id) = request.grant_event_id.as_deref() else {
            return;
        };
        let current = self
            .meetings
            .get(&request.session_id)
            .and_then(|runtime| runtime.view.clone());
        let grant = current.as_ref().and_then(|view| {
            view.baton.grant.as_ref().filter(|grant| {
                !view.ended
                    && grant.grant_id == grant_id
                    && grant.holder_pubkey == self.agent_pubkey
                    && now_ms()
                        < grant
                            .hard_deadline_ms
                            .saturating_sub(grant_safety_margin_ms(view))
            })
        });
        let Some(grant) = grant.cloned() else {
            self.mark_grant_state(request.session_id, grant_id, "stale");
            self.emit(
                "meeting_v1_grant_result_late",
                request.session_id,
                Some(turn_id.to_string()),
                json!({ "grant_id": grant_id }),
            );
            return;
        };

        let output = if succeeded {
            match parse_granted_output(raw_output) {
                Ok(output) => Some(output),
                Err(error) => {
                    if self.queue_granted_format_retry(request, grant_id, &error) {
                        return;
                    }
                    None
                }
            }
        } else {
            None
        };
        let Some(output) = output else {
            self.prepare_and_submit_yield(
                request.session_id,
                &grant,
                MeetingV1GrantYieldReason::UnableToAnswer,
                "Agent turn failed before producing a valid Meeting response",
            )
            .await;
            return;
        };
        if output.action == "YIELD" {
            self.prepare_and_submit_yield(
                request.session_id,
                &grant,
                MeetingV1GrantYieldReason::NoLongerNeeded,
                output
                    .reason
                    .as_deref()
                    .unwrap_or("No useful contribution remains"),
            )
            .await;
            return;
        }

        let Some(view) = current else {
            return;
        };
        let Some(content) = output.content.as_deref() else {
            return;
        };
        if output.mention_pubkeys.len() > MAX_MENTIONS
            || output
                .mention_pubkeys
                .iter()
                .any(|pubkey| !view.roster.contains_key(pubkey))
        {
            self.prepare_and_submit_yield(
                request.session_id,
                &grant,
                MeetingV1GrantYieldReason::UnableToAnswer,
                "Generated speech referenced a participant outside the frozen roster",
            )
            .await;
            return;
        }
        let handoff_type = output
            .handoff
            .as_ref()
            .map(|handoff| parse_handoff_type(&handoff.handoff_type))
            .transpose();
        let handoff_type = match handoff_type {
            Ok(value) => value,
            Err(_) => {
                self.prepare_and_submit_yield(
                    request.session_id,
                    &grant,
                    MeetingV1GrantYieldReason::UnableToAnswer,
                    "Generated speech contained an invalid Directed Handoff",
                )
                .await;
                return;
            }
        };
        if output.handoff.as_ref().is_some_and(|handoff| {
            handoff.target_pubkey == self.agent_pubkey
                || !view.roster.contains_key(&handoff.target_pubkey)
        }) {
            self.prepare_and_submit_yield(
                request.session_id,
                &grant,
                MeetingV1GrantYieldReason::UnableToAnswer,
                "Generated Directed Handoff target is invalid",
            )
            .await;
            return;
        }
        let mention_refs: Vec<&str> = output.mention_pubkeys.iter().map(String::as_str).collect();
        let handoff = output
            .handoff
            .as_ref()
            .zip(handoff_type)
            .map(|(handoff, handoff_type)| MeetingV1DirectedHandoff {
                target_pubkey: &handoff.target_pubkey,
                handoff_type,
                reason: &handoff.reason,
            });
        let event = match buzz_sdk::build_meeting_v1_speech(MeetingV1SpeechParams {
            session_id: request.session_id,
            grant_id,
            speech_revision: view.baton.speech_revision.saturating_add(1),
            content,
            mentions: &mention_refs,
            handoff,
        })
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, &self.keys))
        {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(
                    meeting = %request.session_id,
                    grant = %grant_id,
                    "Meeting V1 speech build failed: {error}"
                );
                self.prepare_and_submit_yield(
                    request.session_id,
                    &grant,
                    MeetingV1GrantYieldReason::UnableToAnswer,
                    "Generated speech did not satisfy the Meeting protocol",
                )
                .await;
                return;
            }
        };
        if let Some(record) = self
            .ledger_for_mut(request.session_id)
            .and_then(|ledger| ledger.grants.get_mut(grant_id))
        {
            record.speech_event = serde_json::to_value(&event).ok();
            record.speech_event_id = Some(event.id.to_hex());
            record.state = "speech_prepared".to_string();
        }
        let prepared_is_complete = self
            .ledger_for(request.session_id)
            .and_then(|ledger| ledger.grants.get(grant_id))
            .is_some_and(|record| record.speech_event.is_some());
        if !prepared_is_complete {
            self.prepare_and_submit_yield(
                request.session_id,
                &grant,
                MeetingV1GrantYieldReason::UnableToAnswer,
                "Prepared speech could not be serialized durably",
            )
            .await;
            return;
        }
        if !self.persist_ledger_required(request.session_id, "speech") {
            self.request_fast_backfill(request.session_id);
            return;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::GrantTerminal {
                session_id: request.session_id,
                grant_id: grant_id.to_string(),
            },
            ProtocolSubmissionContext::GrantTerminal {
                grant_id: grant_id.to_string(),
                source_offer_id: grant.source_offer_id.clone(),
                action: GrantTerminalAction::Speech,
                turn_id: Some(turn_id.to_string()),
                queued_at_ms: Some(request.queued_at_unix_ms),
                grant_started_at_ms: Some(
                    grant
                        .hard_deadline_ms
                        .saturating_sub(view.baton.baton_config.grant_hard_deadline_ms),
                ),
            },
            event,
        );
    }

    fn queue_granted_format_retry(
        &mut self,
        request: &MeetingTurnRequest,
        grant_id: &str,
        error: &anyhow::Error,
    ) -> bool {
        let Some(grant) = self
            .ledger_for_mut(request.session_id)
            .and_then(|ledger| ledger.grants.get_mut(grant_id))
        else {
            return false;
        };
        if !reserve_format_retry(&mut grant.format_attempts) {
            return false;
        }
        grant.state = "queued".to_string();
        self.persist_ledger_best_effort();
        let mut retry = request.clone();
        retry.format_retry = true;
        retry.prompt = granted_format_correction_prompt();
        retry.queued_at_unix_ms = now_ms();
        self.queue_turn(retry);
        self.emit(
            "meeting_v1_grant_format_retry",
            request.session_id,
            None,
            json!({ "grant_id": grant_id, "error": error.to_string() }),
        );
        true
    }

    async fn prepare_and_submit_yield(
        &mut self,
        session_id: Uuid,
        grant: &GrantView,
        reason_code: MeetingV1GrantYieldReason,
        reason: &str,
    ) {
        let existing = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .and_then(|record| record.yield_event.clone());
        let event = if let Some(value) = existing {
            match serde_json::from_value::<Event>(value) {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(
                        meeting = %session_id,
                        grant = %grant.grant_id,
                        "prepared Meeting V1 Yield is invalid: {error}"
                    );
                    self.mark_grant_state(session_id, &grant.grant_id, "terminal");
                    return;
                }
            }
        } else {
            let bounded_reason: String = reason.chars().take(500).collect();
            let event = match buzz_sdk::build_meeting_v1_grant_yield(MeetingV1GrantYieldParams {
                session_id,
                grant_id: &grant.grant_id,
                reason_code: Some(reason_code),
                reason: Some(&bounded_reason),
            })
            .map_err(|error| anyhow!(error.to_string()))
            .and_then(|builder| sign_builder(builder, &self.keys))
            {
                Ok(event) => event,
                Err(error) => {
                    tracing::warn!(
                        meeting = %session_id,
                        grant = %grant.grant_id,
                        "could not prepare Meeting V1 Yield: {error}"
                    );
                    return;
                }
            };
            if let Some(record) = self
                .ledger_for_mut(session_id)
                .and_then(|ledger| ledger.grants.get_mut(&grant.grant_id))
            {
                record.yield_event = serde_json::to_value(&event).ok();
                record.state = "yield_prepared".to_string();
            }
            event
        };
        let prepared_is_complete = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .is_some_and(|record| record.yield_event.is_some());
        if !prepared_is_complete || !self.persist_ledger_required(session_id, "yield") {
            self.request_fast_backfill(session_id);
            return;
        }
        self.submit_protocol_in_background(
            ProtocolSubmissionKey::GrantTerminal {
                session_id,
                grant_id: grant.grant_id.clone(),
            },
            ProtocolSubmissionContext::GrantTerminal {
                grant_id: grant.grant_id.clone(),
                source_offer_id: grant.source_offer_id.clone(),
                action: GrantTerminalAction::Yield,
                turn_id: None,
                queued_at_ms: None,
                grant_started_at_ms: None,
            },
            event,
        );
    }

    async fn maintain_grant(&mut self, session_id: Uuid) {
        let Some(view) = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.clone())
        else {
            return;
        };
        let Some(grant) = view
            .baton
            .grant
            .as_ref()
            .filter(|grant| grant.holder_pubkey == self.agent_pubkey)
            .cloned()
        else {
            return;
        };
        let now = now_ms();
        if now >= grant.hard_deadline_ms || now >= grant.soft_lease_expires_at_ms {
            self.request_full_sync(session_id);
            return;
        }
        let record_state = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .map(|record| record.state.clone());
        if matches!(
            record_state.as_deref(),
            Some("speech_sent" | "yield_sent" | "spoken" | "yielded")
        ) {
            self.request_fast_backfill(session_id);
            return;
        }
        if matches!(
            record_state.as_deref(),
            Some(
                "speech_prepared"
                    | "speech_sent_uncertain"
                    | "yield_prepared"
                    | "yield_sent_uncertain"
            )
        ) {
            let _ = self
                .retry_prepared_grant_terminal(session_id, &view, &grant)
                .await;
            return;
        }
        let safety_deadline = grant
            .hard_deadline_ms
            .saturating_sub(grant_safety_margin_ms(&view));
        if now >= safety_deadline {
            self.cancel_granted_turn(session_id, &grant.grant_id);
            self.prepare_and_submit_yield(
                session_id,
                &grant,
                MeetingV1GrantYieldReason::Cancelled,
                "Harness safety margin reached before the Grant hard deadline",
            )
            .await;
            return;
        }

        let next_progress_at_ms = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .map(|record| record.next_progress_at_ms)
            .unwrap_or(now);
        if now >= next_progress_at_ms {
            self.submit_progress(session_id, &view, &grant);
            return;
        }

        let busy = self.session_turn_busy(session_id);
        if !busy && !self.deferred_turn_results.contains_key(&session_id) {
            self.reconcile(session_id).await;
        }
    }

    fn cancel_granted_turn(&mut self, session_id: Uuid, grant_id: &str) {
        self.pending.retain(|request| {
            request.session_id != session_id
                || request.kind != MeetingTurnKind::V1Granted
                || request.grant_event_id.as_deref() != Some(grant_id)
        });
        let still_queued = self
            .pending
            .iter()
            .any(|request| request.session_id == session_id);
        if let Some(runtime) = self.meetings.get_mut(&session_id) {
            runtime.queued = still_queued;
        }
        if self.in_flight.values().any(|request| {
            request.session_id == session_id
                && request.kind == MeetingTurnKind::V1Granted
                && request.grant_event_id.as_deref() == Some(grant_id)
        }) {
            self.preemptions.insert(session_id);
        }
    }

    fn submit_progress(&mut self, session_id: Uuid, _view: &MeetingView, grant: &GrantView) {
        let in_flight_key = (session_id, grant.grant_id.clone());
        if self.progress_in_flight.contains_key(&in_flight_key)
            || self.progress_waiting_for_state.contains_key(&in_flight_key)
        {
            return;
        }
        let existing = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant.grant_id))
            .and_then(|record| record.prepared_progress.clone());
        let (seq, event) = if let Some(prepared) = existing {
            let Ok(event) = serde_json::from_value::<Event>(prepared.event) else {
                if let Some(record) = self
                    .ledger_for_mut(session_id)
                    .and_then(|ledger| ledger.grants.get_mut(&grant.grant_id))
                {
                    record.prepared_progress = None;
                }
                self.persist_ledger_best_effort();
                return;
            };
            (prepared.seq, event)
        } else {
            let seq = grant.progress_seq.saturating_add(1);
            let stage = self.progress_stage(session_id, &grant.grant_id);
            let event =
                match buzz_sdk::build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
                    session_id,
                    grant_id: &grant.grant_id,
                    progress_seq: seq,
                    stage,
                })
                .map_err(|error| anyhow!(error.to_string()))
                .and_then(|builder| sign_builder(builder, &self.keys))
                {
                    Ok(event) => event,
                    Err(error) => {
                        tracing::warn!(
                            meeting = %session_id,
                            grant = %grant.grant_id,
                            "could not prepare Meeting V1 Progress: {error}"
                        );
                        return;
                    }
                };
            if let Some(record) = self
                .ledger_for_mut(session_id)
                .and_then(|ledger| ledger.grants.get_mut(&grant.grant_id))
            {
                let Ok(serialized_event) = serde_json::to_value(&event) else {
                    tracing::warn!(
                        meeting = %session_id,
                        grant = %grant.grant_id,
                        "could not serialize prepared Meeting V1 Progress"
                    );
                    return;
                };
                record.prepared_progress = Some(PreparedProgress {
                    seq,
                    event: serialized_event,
                    state: "prepared".to_string(),
                });
            }
            (seq, event)
        };

        if !self.persist_ledger_required(session_id, "progress") {
            return;
        }
        let submitted_stage = tag_value(&event, "stage")
            .and_then(parse_progress_stage)
            .unwrap_or_else(|| self.progress_stage(session_id, &grant.grant_id));
        self.next_progress_submission_id =
            self.next_progress_submission_id.saturating_add(1).max(1);
        let submission_id = self.next_progress_submission_id;
        let session_epoch = self
            .meetings
            .get(&session_id)
            .map_or(0, |runtime| runtime.epoch);
        self.progress_in_flight.insert(
            in_flight_key,
            ProgressInFlight {
                session_epoch,
                submission_id,
                event_id: event.id.to_hex(),
            },
        );
        let rest = self.rest.clone();
        let result_tx = self.progress_result_tx.clone();
        let grant_id = grant.grant_id.clone();
        let event_id = event.id.to_hex();
        let _task = tokio::spawn(async move {
            let attempt = AssertUnwindSafe(submit_protocol_event(&rest, &event))
                .catch_unwind()
                .await;
            let result = match attempt {
                Ok(result) => result,
                Err(_) => Err(ProtocolSubmitFailure::Uncertain(
                    "background Progress submission task panicked".to_string(),
                )),
            };
            if result_tx
                .send(ProgressTaskResult {
                    session_id,
                    session_epoch,
                    grant_id,
                    submission_id,
                    event_id,
                    progress_seq: seq,
                    stage: submitted_stage,
                    result,
                })
                .is_err()
            {
                tracing::debug!(
                    meeting = %session_id,
                    progress_seq = seq,
                    "Meeting V1 coordinator stopped before Progress submission completed"
                );
            }
        });
    }

    fn drain_progress_results(&mut self) {
        let mut completed = Vec::new();
        while let Ok(result) = self.progress_result_rx.try_recv() {
            completed.push(result);
        }
        for result in completed {
            self.handle_progress_result(result);
        }
    }

    fn handle_progress_result(&mut self, completed: ProgressTaskResult) {
        let in_flight_key = (completed.session_id, completed.grant_id.clone());
        let current_epoch = self
            .meetings
            .get(&completed.session_id)
            .map_or(0, |runtime| runtime.epoch);
        if self
            .progress_in_flight
            .get(&in_flight_key)
            .is_none_or(|in_flight| {
                current_epoch != completed.session_epoch
                    || in_flight.session_epoch != completed.session_epoch
                    || in_flight.submission_id != completed.submission_id
                    || in_flight.event_id != completed.event_id
            })
        {
            return;
        }
        self.progress_in_flight.remove(&in_flight_key);
        let next_progress_at_ms = self
            .meetings
            .get(&completed.session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .and_then(|view| {
                view.baton
                    .grant
                    .as_ref()
                    .filter(|grant| grant.grant_id == completed.grant_id)
                    .map(|grant| {
                        next_progress_deadline(
                            now_ms(),
                            grant.soft_lease_expires_at_ms,
                            view.baton.baton_config.progress_interval_ms,
                        )
                    })
            })
            .unwrap_or_else(now_ms);
        let mut wait_for_state = false;
        if let Some(record) = self
            .ledger_for_mut(completed.session_id)
            .and_then(|ledger| ledger.grants.get_mut(&completed.grant_id))
        {
            let prepared_matches = record.prepared_progress.as_ref().is_some_and(|prepared| {
                prepared.seq == completed.progress_seq
                    && serialized_event_id(&prepared.event).as_deref()
                        == Some(completed.event_id.as_str())
            });
            match &completed.result {
                Ok(_) if prepared_matches => {
                    if let Some(prepared) = record
                        .prepared_progress
                        .as_mut()
                        .filter(|prepared| prepared.seq == completed.progress_seq)
                    {
                        prepared.state = "sent".to_string();
                    }
                    record.next_progress_at_ms = next_progress_at_ms;
                }
                Err(ProtocolSubmitFailure::Uncertain(_)) if prepared_matches => {
                    if let Some(prepared) = record
                        .prepared_progress
                        .as_mut()
                        .filter(|prepared| prepared.seq == completed.progress_seq)
                    {
                        prepared.state = "uncertain".to_string();
                    }
                }
                Err(ProtocolSubmitFailure::Rejected(_)) if prepared_matches => {
                    record.prepared_progress = None;
                    record.next_progress_at_ms = now_ms();
                    wait_for_state = true;
                }
                Ok(_)
                | Err(ProtocolSubmitFailure::Uncertain(_))
                | Err(ProtocolSubmitFailure::Rejected(_)) => {}
            }
        }
        if wait_for_state {
            if let Some(request_id) = self.request_full_sync(completed.session_id) {
                self.progress_waiting_for_state
                    .insert(in_flight_key.clone(), request_id);
            }
        }
        self.persist_ledger_best_effort();
        self.emit(
            "meeting_v1_progress",
            completed.session_id,
            None,
            json!({
                "grant_id": completed.grant_id.as_str(),
                "progress_seq": completed.progress_seq,
                "stage": progress_stage_name(completed.stage),
                "outcome": protocol_submission_label(&completed.result),
            }),
        );
        if let Err(error) = &completed.result {
            tracing::warn!(
                meeting = %completed.session_id,
                grant = %completed.grant_id,
                progress_seq = completed.progress_seq,
                "Meeting V1 Progress was not confirmed: {error}"
            );
        }
        if !wait_for_state {
            self.request_fast_backfill(completed.session_id);
        }
    }

    fn progress_stage(&self, session_id: Uuid, grant_id: &str) -> MeetingV1ProgressStage {
        let state = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(grant_id))
            .map(|grant| grant.state.as_str());
        if matches!(
            state,
            Some(
                "speech_prepared"
                    | "speech_sent_uncertain"
                    | "yield_prepared"
                    | "yield_sent_uncertain"
            )
        ) {
            MeetingV1ProgressStage::Submitting
        } else if self.in_flight.values().any(|request| {
            request.session_id == session_id && request.kind == MeetingTurnKind::V1Granted
        }) {
            MeetingV1ProgressStage::Generating
        } else {
            MeetingV1ProgressStage::ContextSync
        }
    }

    fn mark_trigger_state(&mut self, session_id: Uuid, trigger_id: &str, state: &str) {
        if let Some(trigger) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.triggers.get_mut(trigger_id))
        {
            trigger.state = state.to_string();
        }
        self.persist_ledger_best_effort();
    }

    fn mark_grant_state(&mut self, session_id: Uuid, grant_id: &str, state: &str) {
        if let Some(grant) = self
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.grants.get_mut(grant_id))
        {
            grant.state = state.to_string();
        }
        self.persist_ledger_best_effort();
    }

    fn ledger_for(&self, session_id: Uuid) -> Option<&MeetingLedger> {
        self.ledger.meetings.get(&session_id.to_string())
    }

    fn ledger_for_mut(&mut self, session_id: Uuid) -> Option<&mut MeetingLedger> {
        self.ledger.meetings.get_mut(&session_id.to_string())
    }

    fn persist_ledger_best_effort(&self) {
        if let Err(error) = persist_ledger(&self.ledger_path, &self.ledger) {
            tracing::warn!(
                path = %self.ledger_path.display(),
                "Meeting V1 ledger persistence failed: {error}"
            );
        }
    }

    fn persist_terminal_ledger_cleanup(&mut self) {
        match persist_ledger(&self.ledger_path, &self.ledger) {
            Ok(()) => {
                self.terminal_ledger_cleanup_retry_at = None;
            }
            Err(error) => {
                self.terminal_ledger_cleanup_retry_at =
                    Some(Instant::now() + TERMINAL_LEDGER_CLEANUP_RETRY_INTERVAL);
                tracing::warn!(
                    path = %self.ledger_path.display(),
                    "terminal Meeting V1 ledger cleanup persistence failed; retry scheduled: {error}"
                );
            }
        }
    }

    fn retry_terminal_ledger_cleanup_if_due(&mut self) {
        if self
            .terminal_ledger_cleanup_retry_at
            .is_some_and(|retry_at| Instant::now() >= retry_at)
        {
            self.persist_terminal_ledger_cleanup();
        }
    }

    fn persist_ledger_required(&self, session_id: Uuid, prepared_action: &str) -> bool {
        match persist_ledger(&self.ledger_path, &self.ledger) {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    meeting = %session_id,
                    action = prepared_action,
                    path = %self.ledger_path.display(),
                    "prepared Meeting V1 event was not sent because durable persistence failed: {error}"
                );
                self.emit(
                    "meeting_v1_prepared_persistence_failed",
                    session_id,
                    None,
                    json!({ "action": prepared_action }),
                );
                false
            }
        }
    }

    fn emit_moderator_decision_event(
        &self,
        kind: &str,
        session_id: Uuid,
        turn_id: Option<String>,
        disposition: (&str, &str),
        model_latency_ms: Option<i64>,
        extra: Value,
    ) {
        let (outcome, reason) = disposition;
        let Some(decision) = self
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
        else {
            return;
        };
        let current_intent_revision = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .map_or(decision.attempt.snapshot_intent_revision, |view| {
                view.baton.intent_revision
            });
        let phase = self
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .map(|view| view.baton.phase.clone());
        let (selected_source_type, selected_source_id) = match decision.next_action.action.as_str()
        {
            "select_intent" | "moderator_speak" | "withdraw_self" => {
                (Some("intent"), decision.next_action.id.clone())
            }
            "select_handoff" => (Some("handoff"), decision.next_action.id.clone()),
            _ => (None, None),
        };
        let candidate_sources: Vec<_> = decision
            .attempt
            .candidate_refs
            .iter()
            .map(|candidate| {
                json!({
                    "source_type": candidate.source_type,
                    "source_id": candidate.source_id,
                    "current_event_id": candidate.current_event_id,
                    "author_pubkey": candidate.author_pubkey,
                    "eligible_decision_epoch": candidate.eligible_decision_epoch,
                })
            })
            .collect();
        let mut payload = json!({
            "attempt_id": decision.attempt.attempt_id,
            "control_epoch": decision.attempt.control_epoch,
            "decision_epoch": decision.attempt.decision_epoch,
            "attempt_number": decision.attempt.attempt_number,
            "speech_revision": decision.attempt.speech_revision,
            "snapshot_intent_revision": decision.attempt.snapshot_intent_revision,
            "current_intent_revision": current_intent_revision,
            "candidate_count": decision.attempt.candidate_refs.len(),
            "candidate_snapshot_hash": decision.attempt.candidate_snapshot_hash,
            "candidate_sources": candidate_sources,
            "attempt_deadline_ms": decision.attempt.deadline_ms,
            "selected_source_type": selected_source_type,
            "selected_source_id": selected_source_id,
            "phase": phase,
            "outcome": outcome,
            "reason": reason,
            "model_latency_ms": model_latency_ms,
        });
        if let (Some(payload), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                payload.insert(key.clone(), value.clone());
            }
        }
        self.emit(
            kind,
            session_id,
            turn_id.or_else(|| decision.turn_id.clone()),
            payload,
        );
    }

    fn emit(&self, kind: &str, session_id: Uuid, turn_id: Option<String>, payload: Value) {
        if let Some(observer) = &self.observer {
            observer.emit(
                kind,
                None,
                &observer::context_for(Some(session_id), None, turn_id),
                payload,
            );
        }
    }
}

async fn submit_protocol_event(
    rest: &RestClient,
    event: &Event,
) -> std::result::Result<Value, ProtocolSubmitFailure> {
    let response =
        tokio::time::timeout(PROTOCOL_SUBMIT_TIMEOUT, rest.submit_event_outcome(event)).await;
    match response {
        Err(_) => Err(ProtocolSubmitFailure::Uncertain(format!(
            "submission exceeded {}ms",
            PROTOCOL_SUBMIT_TIMEOUT.as_millis()
        ))),
        Ok(ProtocolSubmitOutcome::Accepted(accepted)) => Ok(accepted.response),
        Ok(ProtocolSubmitOutcome::Rejected(rejected)) => {
            Err(ProtocolSubmitFailure::Rejected(rejected))
        }
        Ok(ProtocolSubmitOutcome::Uncertain(uncertain)) => {
            Err(ProtocolSubmitFailure::Uncertain(uncertain.reason))
        }
    }
}

fn protocol_submission_label<T>(
    result: &std::result::Result<T, ProtocolSubmitFailure>,
) -> &'static str {
    match result {
        Ok(_) => "accepted",
        Err(ProtocolSubmitFailure::Rejected(_)) => "rejected",
        Err(ProtocolSubmitFailure::Uncertain(_)) => "uncertain",
    }
}

async fn fetch_meeting_view(rest: &RestClient, session_id: Uuid) -> Result<MeetingView> {
    let d_tag = SingleLetterTag::lowercase(Alphabet::D);
    let h_tag = SingleLetterTag::lowercase(Alphabet::H);
    let session = session_id.to_string();
    let identity_filters = [
        Filter::new()
            .kind(Kind::Custom(KIND_NIP29_GROUP_METADATA as u16))
            .custom_tags(d_tag, [session.as_str()])
            .limit(4),
        Filter::new()
            .kind(Kind::Custom(KIND_NIP29_GROUP_MEMBERS as u16))
            .custom_tags(d_tag, [session.as_str()])
            .limit(4),
    ];
    let value = rest.query(&identity_filters).await?;
    let raw_identity_events = value
        .as_array()
        .ok_or_else(|| anyhow!("Meeting V1 identity query returned a non-array response"))?;
    let mut events = Vec::with_capacity(raw_identity_events.len());
    for value in raw_identity_events {
        let event: Event = serde_json::from_value(value.clone())
            .context("Meeting V1 identity query contained a malformed event")?;
        event
            .verify()
            .map_err(|error| anyhow!("Meeting V1 identity signature is invalid: {error}"))?;
        events.push(event);
    }
    let history_filter = Filter::new()
        .kinds([
            Kind::Custom(KIND_STREAM_MESSAGE as u16),
            Kind::Custom(KIND_MEETING_CREATE as u16),
            Kind::Custom(KIND_MEETING_END as u16),
            Kind::Custom(KIND_MEETING_ROUND_STATE as u16),
            Kind::Custom(KIND_MEETING_SPEECH_INTENT as u16),
            Kind::Custom(KIND_MEETING_MODERATOR_COMMAND as u16),
            Kind::Custom(KIND_MEETING_HUMAN_FLOOR_REQUEST as u16),
            Kind::Custom(KIND_MEETING_OFFER_RESPONSE as u16),
            Kind::Custom(KIND_MEETING_GRANT_SIGNAL as u16),
        ])
        .custom_tags(h_tag, [session.as_str()]);
    events.extend(fetch_meeting_history(rest, history_filter).await?);

    let metadata = latest_kind(&events, KIND_NIP29_GROUP_METADATA)
        .cloned()
        .ok_or_else(|| anyhow!("Meeting V1 metadata is missing"))?;
    if tag_value(&metadata, "d") != Some(session.as_str())
        || tag_value(&metadata, "room_kind") != Some("meeting")
    {
        return Err(anyhow!("channel metadata is not a Meeting room"));
    }
    let relay_pubkey = metadata.pubkey.to_hex();
    let members = latest_kind(&events, KIND_NIP29_GROUP_MEMBERS)
        .cloned()
        .ok_or_else(|| anyhow!("Meeting V1 roster is missing"))?;
    if members.pubkey != metadata.pubkey || tag_value(&members, "d") != Some(session.as_str()) {
        return Err(anyhow!(
            "Meeting V1 metadata and roster have different Relay signers"
        ));
    }

    let mut state_events: Vec<Event> = events
        .iter()
        .filter(|event| {
            event.kind.as_u16() as u32 == KIND_MEETING_ROUND_STATE
                && event.pubkey.to_hex() == relay_pubkey
                && tag_value(event, "h") == Some(session.as_str())
                && tag_value(event, "v") == Some("2")
                && tag_value(event, "policy") == Some("moderated-baton-v1")
        })
        .cloned()
        .collect();
    state_events.sort_by(|left, right| {
        state_revision(left)
            .cmp(&state_revision(right))
            .then_with(|| left.id.to_hex().cmp(&right.id.to_hex()))
    });
    let state_event = state_events
        .last()
        .ok_or_else(|| anyhow!("Meeting V1 authoritative State is missing"))?;
    let highest_revision = state_revision(state_event);
    if state_events.iter().rev().skip(1).any(|candidate| {
        state_revision(candidate) == highest_revision && candidate.id != state_event.id
    }) {
        return Err(anyhow!(
            "conflicting Relay State events share the highest state revision"
        ));
    }
    let raw_state_value: Value = serde_json::from_str(&state_event.content)
        .context("Meeting V1 State content is malformed JSON")?;
    let raw_state: RawBatonState = serde_json::from_value(raw_state_value.clone())
        .context("Meeting V1 State content has an invalid shape")?;
    validate_baton_state_event(state_event, session_id, &raw_state)?;

    let mut roster = BTreeMap::new();
    for tag in members.tags.iter() {
        let values = tag.as_slice();
        if values.first().map(String::as_str) != Some("p") {
            continue;
        }
        let Some(pubkey) = values.get(1) else {
            continue;
        };
        if PublicKey::from_hex(pubkey).is_err() {
            continue;
        }
        let role = values
            .get(3)
            .filter(|value| !value.is_empty())
            .cloned()
            .unwrap_or_else(|| "member".to_string());
        roster.insert(
            pubkey.to_ascii_lowercase(),
            Participant {
                pubkey: pubkey.to_ascii_lowercase(),
                role,
                participant_type: String::new(),
                display_name: short_pubkey(pubkey),
            },
        );
    }
    if roster.is_empty() {
        return Err(anyhow!("Meeting V1 roster is empty"));
    }
    let mut seen_types = BTreeSet::new();
    for participant in &raw_state.participants {
        let pubkey = participant.pubkey.to_ascii_lowercase();
        if PublicKey::from_hex(&pubkey).is_err()
            || !matches!(participant.participant_type.as_str(), "human" | "agent")
            || !seen_types.insert(pubkey.clone())
        {
            return Err(anyhow!("Meeting V1 State has an invalid participant"));
        }
        let roster_participant = roster
            .get_mut(&pubkey)
            .ok_or_else(|| anyhow!("Meeting V1 State participant is absent from frozen roster"))?;
        roster_participant.participant_type = participant.participant_type.clone();
    }
    if seen_types.len() != roster.len()
        || roster
            .values()
            .any(|participant| participant.participant_type.is_empty())
    {
        return Err(anyhow!(
            "Meeting V1 State participant types do not match the frozen roster"
        ));
    }
    hydrate_profile_names(rest, &mut roster).await;

    let baton = baton_from_raw_state(state_event, raw_state, raw_state_value);
    if !roster.contains_key(&baton.moderator_pubkey) {
        return Err(anyhow!("Meeting V1 moderator is absent from the roster"));
    }

    let intents = collect_intent_contexts(&events, session_id, &roster);
    let mut speeches = Vec::new();
    for event in events {
        if event.kind.as_u16() as u32 != KIND_STREAM_MESSAGE
            || tag_value(&event, "h") != Some(session.as_str())
            || tag_value(&event, "v") != Some("2")
        {
            continue;
        }
        let author_pubkey = event.pubkey.to_hex();
        let Some(participant) = roster.get(&author_pubkey) else {
            continue;
        };
        let Some(grant_id) = tag_value(&event, "meeting-grant").map(str::to_string) else {
            continue;
        };
        let Some(speech_revision) = tag_value(&event, "speech-revision")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
        else {
            continue;
        };
        // A speech event and its Relay-signed State may arrive in either order.
        // Never expose a future speech to a model until the authoritative State
        // has advanced through that revision.
        if speech_revision > baton.speech_revision {
            continue;
        }
        let handoff_to = tag_value(&event, "handoff-to");
        let handoff_type = tag_value(&event, "handoff-type");
        let handoff_reason = tag_value(&event, "handoff-reason");
        let handoff = match (handoff_to, handoff_type, handoff_reason) {
            (Some(target), Some(kind), Some(reason))
                if roster.contains_key(target)
                    && parse_handoff_type(kind).is_ok()
                    && !reason.is_empty() =>
            {
                Some(SpeechHandoff {
                    target_pubkey: target.to_string(),
                    handoff_type: kind.to_string(),
                    reason: reason.to_string(),
                })
            }
            (None, None, None) => None,
            _ => continue,
        };
        let mentions = event
            .tags
            .iter()
            .filter_map(|tag| {
                let values = tag.as_slice();
                (values.first().map(String::as_str) == Some("p"))
                    .then(|| values.get(1).cloned())
                    .flatten()
            })
            .collect();
        speeches.push(Speech {
            event_id: event.id.to_hex(),
            author_pubkey,
            author_display_name: participant.display_name.clone(),
            content: event.content,
            created_at: event.created_at.as_secs(),
            speech_revision,
            grant_id,
            mentions,
            handoff,
        });
    }
    speeches.sort_by(|left, right| {
        left.speech_revision
            .cmp(&right.speech_revision)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    let speech_cursor = speeches.last().map(|speech| speech.event_id.clone());
    let ended = tag_value(&metadata, "archived") == Some("true") || baton.phase == "ended";

    Ok(MeetingView {
        session_id,
        title: tag_value(&metadata, "name")
            .unwrap_or("Untitled meeting")
            .to_string(),
        description: tag_value(&metadata, "about").map(str::to_string),
        ended,
        relay_pubkey,
        roster,
        speeches,
        intents,
        speech_cursor,
        baton,
    })
}

fn baton_from_raw_state(
    state_event: &Event,
    raw_state: RawBatonState,
    raw_state_value: Value,
) -> BatonView {
    BatonView {
        raw_state: raw_state_value,
        state_event_id: state_event.id.to_hex(),
        phase: raw_state.phase,
        state_revision: raw_state.state_revision,
        floor_revision: raw_state.floor_revision,
        intent_revision: raw_state.intent_revision,
        speech_revision: raw_state.speech_revision,
        control_epoch: raw_state.control_epoch,
        decision_epoch: raw_state.decision_epoch,
        decision_attempt: raw_state.decision_attempt,
        active_decision_attempt: raw_state.active_decision_attempt,
        moderator_pubkey: raw_state.moderator_pubkey.to_ascii_lowercase(),
        baton_config: raw_state.baton_config,
        pending_intents: raw_state.pending_intents,
        human_queue: raw_state.human_queue,
        unresolved_handoffs: raw_state.unresolved_handoffs,
        handoff_depth: raw_state.handoff_depth,
        consecutive_moderator_speeches: raw_state.consecutive_moderator_speeches,
        forced_return_to_moderator: raw_state.forced_return_to_moderator,
        moderator_decision_deadline_ms: raw_state.moderator_decision_deadline_ms,
        next_action_at_ms: raw_state.next_action_at_ms,
        offer: raw_state.offer,
        grant: raw_state.grant,
    }
}

fn validate_live_state_roster(
    state: &RawBatonState,
    roster: &BTreeMap<String, Participant>,
) -> Result<()> {
    if state.participants.len() != roster.len() {
        return Err(anyhow!(
            "live State participant count differs from the frozen roster"
        ));
    }
    let mut seen = BTreeSet::new();
    for participant in &state.participants {
        let pubkey = participant.pubkey.to_ascii_lowercase();
        let Some(frozen) = roster.get(&pubkey) else {
            return Err(anyhow!(
                "live State contains a participant outside the roster"
            ));
        };
        if !seen.insert(pubkey)
            || participant.participant_type != frozen.participant_type
            || !matches!(participant.participant_type.as_str(), "human" | "agent")
        {
            return Err(anyhow!(
                "live State changed a frozen participant classification"
            ));
        }
    }
    if !roster.contains_key(&state.moderator_pubkey.to_ascii_lowercase()) {
        return Err(anyhow!("live State moderator is absent from the roster"));
    }
    Ok(())
}

fn same_frozen_roster(
    previous: &BTreeMap<String, Participant>,
    current: &BTreeMap<String, Participant>,
) -> bool {
    previous.len() == current.len()
        && previous.iter().all(|(pubkey, participant)| {
            current.get(pubkey).is_some_and(|candidate| {
                participant.pubkey == candidate.pubkey
                    && participant.role == candidate.role
                    && participant.participant_type == candidate.participant_type
            })
        })
}

fn collect_intent_contexts(
    events: &[Event],
    session_id: Uuid,
    roster: &BTreeMap<String, Participant>,
) -> BTreeMap<String, IntentContext> {
    #[derive(Clone)]
    struct Refresh {
        intent_id: String,
        previous_event_id: String,
        context: IntentContext,
    }

    let session = session_id.to_string();
    let mut contexts = BTreeMap::new();
    let mut refreshes = Vec::new();
    for event in events {
        if event.kind.as_u16() as u32 != KIND_MEETING_SPEECH_INTENT
            || tag_value(event, "h") != Some(session.as_str())
            || tag_value(event, "v") != Some("2")
        {
            continue;
        }
        let author_pubkey = event.pubkey.to_hex();
        if !roster.contains_key(&author_pubkey)
            || validate_bounded_text(&event.content, MAX_INTENT_SUMMARY_BYTES, "Intent summary")
                .is_err()
        {
            continue;
        }
        let Some(basis_speech_revision) =
            tag_value(event, "basis-speech-revision").and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        let addressed_to = tag_value(event, "addressed-to")
            .map(str::to_ascii_lowercase)
            .filter(|pubkey| roster.contains_key(pubkey));
        let event_id = event.id.to_hex();
        match tag_value(event, "action") {
            Some("submit") => {
                contexts.insert(
                    event_id.clone(),
                    IntentContext {
                        intent_id: event_id.clone(),
                        current_event_id: event_id,
                        author_pubkey,
                        summary: event.content.clone(),
                        addressed_to,
                        basis_speech_revision,
                    },
                );
            }
            Some("refresh") => {
                let Some(intent_id) = tag_value(event, "intent")
                    .filter(|value| is_hex_id(value))
                    .map(str::to_ascii_lowercase)
                else {
                    continue;
                };
                let Some(previous_event_id) = tag_value(event, "prev")
                    .filter(|value| is_hex_id(value))
                    .map(str::to_ascii_lowercase)
                else {
                    continue;
                };
                refreshes.push(Refresh {
                    intent_id: intent_id.clone(),
                    previous_event_id,
                    context: IntentContext {
                        intent_id,
                        current_event_id: event_id,
                        author_pubkey,
                        summary: event.content.clone(),
                        addressed_to,
                        basis_speech_revision,
                    },
                });
            }
            _ => {}
        }
    }

    // Accepted Refresh commands form a prev-linked chain. Resolve by that
    // chain rather than Nostr timestamp order, whose one-second granularity can
    // place several accepted updates in an arbitrary order.
    loop {
        let mut advanced = false;
        refreshes.retain(|refresh| {
            let matches_current = contexts.get(&refresh.intent_id).is_some_and(|current| {
                current.current_event_id == refresh.previous_event_id
                    && current.author_pubkey == refresh.context.author_pubkey
            });
            if matches_current {
                contexts.insert(refresh.intent_id.clone(), refresh.context.clone());
                advanced = true;
                false
            } else {
                true
            }
        });
        if !advanced {
            break;
        }
    }
    contexts
}

fn is_hex_id(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|character| character.is_ascii_hexdigit())
}

fn candidate_snapshot_value(attempt: &ActiveDecisionAttemptView) -> Value {
    let candidate_refs: Vec<_> = attempt
        .candidate_refs
        .iter()
        .map(|candidate| {
            if candidate.source_type == "intent" {
                json!({
                    "source_type": "intent",
                    "source_id": candidate.source_id,
                    "current_event_id": candidate.current_event_id,
                    "author_pubkey": candidate.author_pubkey,
                    "moderator_self": candidate.moderator_self,
                    "basis_speech_revision": candidate.basis_speech_revision,
                    "summary": candidate.summary,
                    "addressed_to": candidate.addressed_to,
                    "eligible_decision_epoch": candidate.eligible_decision_epoch,
                    "created_at_ms": candidate.created_at_ms,
                })
            } else {
                json!({
                    "source_type": "handoff",
                    "source_id": candidate.source_id,
                    "source_speech_event_id": candidate.source_speech_event_id,
                    "from_pubkey": candidate.from_pubkey,
                    "target_pubkey": candidate.target_pubkey,
                    "reason_type": candidate.reason_type,
                    "reason_text": candidate.reason_text,
                    "attempt_count": candidate.attempt_count,
                    "eligible_decision_epoch": candidate.eligible_decision_epoch,
                    "created_at_ms": candidate.created_at_ms,
                })
            }
        })
        .collect();
    json!({
        "version": 1,
        "control_epoch": attempt.control_epoch,
        "decision_epoch": attempt.decision_epoch,
        "speech_revision": attempt.speech_revision,
        "snapshot_intent_revision": attempt.snapshot_intent_revision,
        "candidate_refs": candidate_refs,
    })
}

fn candidate_snapshot_hash(attempt: &ActiveDecisionAttemptView) -> Result<String> {
    let encoded = serde_json::to_vec(&candidate_snapshot_value(attempt))
        .context("serialize Meeting V1 Candidate Cohort")?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

fn validate_active_decision_attempt(
    state: &RawBatonState,
    attempt: &ActiveDecisionAttemptView,
) -> Result<()> {
    if !is_hex_id(&attempt.attempt_id)
        || !is_hex_id(&attempt.snapshot_state_event_id)
        || !is_hex_id(&attempt.candidate_snapshot_hash)
        || attempt.control_epoch != state.control_epoch
        || attempt.decision_epoch != state.decision_epoch
        || attempt.attempt_number == 0
        || attempt.attempt_number != state.decision_attempt
        || attempt.speech_revision != state.speech_revision
        || attempt.deadline_ms <= attempt.started_at_ms
    {
        return Err(anyhow!(
            "Meeting V1 active DecisionAttempt has invalid authority fields"
        ));
    }
    let mut sources = BTreeSet::new();
    for candidate in &attempt.candidate_refs {
        if !is_hex_id(&candidate.source_id)
            || candidate.eligible_decision_epoch > attempt.decision_epoch
            || !sources.insert((candidate.source_type.clone(), candidate.source_id.clone()))
        {
            return Err(anyhow!(
                "Meeting V1 Candidate Cohort has an invalid or duplicate source"
            ));
        }
        match candidate.source_type.as_str() {
            "intent"
                if candidate
                    .current_event_id
                    .as_deref()
                    .is_none_or(|event_id| !is_hex_id(event_id))
                    || candidate
                        .author_pubkey
                        .as_deref()
                        .is_none_or(|pubkey| PublicKey::from_hex(pubkey).is_err())
                    || candidate.summary.as_deref().is_none_or(str::is_empty)
                    || candidate.attempt_count.is_some() =>
            {
                return Err(anyhow!(
                    "Meeting V1 Candidate Cohort has an invalid Intent source"
                ));
            }
            "handoff"
                if candidate
                    .source_speech_event_id
                    .as_deref()
                    .is_none_or(|event_id| !is_hex_id(event_id))
                    || candidate
                        .from_pubkey
                        .as_deref()
                        .is_none_or(|pubkey| PublicKey::from_hex(pubkey).is_err())
                    || candidate
                        .target_pubkey
                        .as_deref()
                        .is_none_or(|pubkey| PublicKey::from_hex(pubkey).is_err())
                    || candidate.reason_type.as_deref().is_none_or(str::is_empty)
                    || candidate.reason_text.as_deref().is_none_or(str::is_empty)
                    || candidate.attempt_count.is_none()
                    || candidate.current_event_id.is_some() =>
            {
                return Err(anyhow!(
                    "Meeting V1 Candidate Cohort has an invalid Handoff source"
                ));
            }
            "intent" | "handoff" => {}
            _ => {
                return Err(anyhow!(
                    "Meeting V1 Candidate Cohort has an unknown source type"
                ));
            }
        }
    }
    if candidate_snapshot_hash(attempt)? != attempt.candidate_snapshot_hash.to_ascii_lowercase() {
        return Err(anyhow!(
            "Meeting V1 Candidate Cohort hash does not match its Relay State payload"
        ));
    }
    Ok(())
}

fn validate_baton_state_event(
    event: &Event,
    session_id: Uuid,
    state: &RawBatonState,
) -> Result<()> {
    let expected = [
        ("h", session_id.to_string()),
        ("v", "2".to_string()),
        ("policy", "moderated-baton-v1".to_string()),
        ("phase", state.phase.clone()),
        ("floor-revision", state.floor_revision.to_string()),
        ("intent-revision", state.intent_revision.to_string()),
        ("speech-revision", state.speech_revision.to_string()),
        ("state-revision", state.state_revision.to_string()),
        ("moderator", state.moderator_pubkey.to_ascii_lowercase()),
    ];
    for (name, value) in expected {
        if tag_value(event, name).map(str::to_ascii_lowercase) != Some(value) {
            return Err(anyhow!(
                "Meeting V1 State tag {name} does not match content"
            ));
        }
    }
    if state.state_revision == 0
        || state.floor_revision == 0
        || state.control_epoch == 0
        || !matches!(
            state.phase.as_str(),
            "moderator_idle" | "moderator_control" | "offered" | "granted" | "ended"
        )
        || PublicKey::from_hex(&state.moderator_pubkey).is_err()
    {
        return Err(anyhow!("Meeting V1 State has invalid core fields"));
    }
    if let Some(attempt) = state.active_decision_attempt.as_ref() {
        validate_active_decision_attempt(state, attempt)?;
    }
    match state.phase.as_str() {
        "offered" if state.offer.is_none() || state.grant.is_some() => {
            return Err(anyhow!(
                "offered Meeting V1 State has invalid active objects"
            ));
        }
        "granted" if state.grant.is_none() || state.offer.is_some() => {
            return Err(anyhow!(
                "granted Meeting V1 State has invalid active objects"
            ));
        }
        "moderator_idle" | "moderator_control" | "ended"
            if state.offer.is_some() || state.grant.is_some() =>
        {
            return Err(anyhow!(
                "non-floor Meeting V1 State contains an active Offer or Grant"
            ));
        }
        _ => {}
    }
    Ok(())
}

fn state_revision(event: &Event) -> u64 {
    tag_value(event, "state-revision")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

async fn hydrate_profile_names(rest: &RestClient, roster: &mut BTreeMap<String, Participant>) {
    let authors: Vec<PublicKey> = roster
        .keys()
        .filter_map(|pubkey| PublicKey::from_hex(pubkey).ok())
        .collect();
    if authors.is_empty() {
        return;
    }
    let filter = Filter::new()
        .kind(Kind::Metadata)
        .authors(authors)
        .limit(roster.len());
    let Ok(value) = rest.query(&[filter]).await else {
        return;
    };
    let Some(events) = value.as_array() else {
        return;
    };
    for value in events {
        let Ok(event) = serde_json::from_value::<Event>(value.clone()) else {
            continue;
        };
        if event.verify().is_err() {
            continue;
        }
        let Ok(profile) = serde_json::from_str::<Value>(&event.content) else {
            continue;
        };
        let name = profile
            .get("display_name")
            .or_else(|| profile.get("name"))
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty());
        if let (Some(participant), Some(name)) = (roster.get_mut(&event.pubkey.to_hex()), name) {
            participant.display_name = name.chars().take(128).collect();
        }
    }
}

fn latest_kind(events: &[Event], kind: u32) -> Option<&Event> {
    events
        .iter()
        .filter(|event| event.kind.as_u16() as u32 == kind)
        .max_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| right.id.to_hex().cmp(&left.id.to_hex()))
        })
}

fn short_pubkey(pubkey: &str) -> String {
    pubkey.chars().take(12).collect()
}

fn baton_has_active_handoff_attempt(baton: &BatonView, handoff_id: &str) -> bool {
    baton
        .offer
        .as_ref()
        .and_then(|offer| offer.source_handoff_id.as_deref())
        == Some(handoff_id)
        || baton
            .grant
            .as_ref()
            .and_then(|grant| grant.source_handoff_id.as_deref())
            == Some(handoff_id)
}

fn human_priority_active(baton: &BatonView) -> bool {
    !baton.human_queue.is_empty()
        || baton.offer.as_ref().is_some_and(|offer| {
            offer.allocation_source == "human_request" || offer.source_request_id.is_some()
        })
        || baton.grant.as_ref().is_some_and(|grant| {
            grant.allocation_source == "human_request" || grant.source_request_id.is_some()
        })
}

fn moderator_has_startable_candidate(baton: &BatonView) -> bool {
    let eligible_through = if baton.phase == "moderator_idle" {
        baton.decision_epoch.saturating_add(1)
    } else {
        baton.decision_epoch
    };
    baton
        .pending_intents
        .iter()
        .any(|intent| !intent.deferred && intent.eligible_decision_epoch <= eligible_through)
        || baton.unresolved_handoffs.iter().any(|handoff| {
            handoff.question_state == "open"
                && handoff.blocked_by.is_none()
                && !handoff.moderator_retry_blocked
                && handoff.eligible_decision_epoch <= eligible_through
        })
}

fn moderator_local_deadline(baton: &BatonView, now: i64) -> i64 {
    baton.moderator_decision_deadline_ms.map_or_else(
        || now.saturating_add(DEFAULT_MODERATOR_DECISION_DURATION.as_millis() as i64),
        |deadline| deadline.saturating_sub(MODERATOR_DEADLINE_SAFETY_MARGIN.as_millis() as i64),
    )
}

fn moderator_deadline_expired(baton: &BatonView, now: i64) -> bool {
    baton
        .moderator_decision_deadline_ms
        .is_some_and(|_| now >= moderator_local_deadline(baton, now))
}

fn moderator_attempt_guard_failure(
    view: &MeetingView,
    attempt: &ActiveDecisionAttemptView,
    agent_pubkey: &str,
    now: i64,
) -> Option<&'static str> {
    if view.ended || view.baton.phase == "ended" {
        return Some("meeting_ended");
    }
    if view.baton.moderator_pubkey != agent_pubkey
        || view
            .roster
            .get(agent_pubkey)
            .is_none_or(|participant| participant.participant_type != "agent")
    {
        return Some("moderator_changed");
    }
    if human_priority_active(&view.baton) {
        return Some("human_priority");
    }
    if !matches!(
        view.baton.phase.as_str(),
        "moderator_control" | "moderator_idle"
    ) || view.baton.control_epoch != attempt.control_epoch
        || view.baton.decision_epoch != attempt.decision_epoch
    {
        return Some("control_changed");
    }
    if view.baton.speech_revision != attempt.speech_revision {
        return Some("speech_changed");
    }
    if view
        .baton
        .active_decision_attempt
        .as_ref()
        .is_none_or(|active| active.attempt_id != attempt.attempt_id)
    {
        return Some("control_changed");
    }
    if now
        >= attempt
            .deadline_ms
            .saturating_sub(MODERATOR_DEADLINE_SAFETY_MARGIN.as_millis() as i64)
    {
        return Some("control_changed");
    }
    None
}

fn intent_candidate_is_current(candidate: &DecisionCandidateRef, baton: &BatonView) -> bool {
    candidate.source_type == "intent"
        && baton.pending_intents.iter().any(|intent| {
            intent.intent_id == candidate.source_id
                && candidate.current_event_id.as_deref() == Some(intent.current_event_id.as_str())
                && !intent.deferred
                && intent.eligible_decision_epoch <= baton.decision_epoch
        })
}

fn handoff_candidate_is_current(candidate: &DecisionCandidateRef, baton: &BatonView) -> bool {
    candidate.source_type == "handoff"
        && baton.unresolved_handoffs.iter().any(|handoff| {
            handoff.handoff_id == candidate.source_id
                && candidate.attempt_count == Some(handoff.attempt_count)
                && handoff.question_state == "open"
                && handoff.blocked_by.is_none()
                && !handoff.moderator_retry_blocked
                && handoff.eligible_decision_epoch <= baton.decision_epoch
        })
}

fn current_cohort_has_candidates(baton: &BatonView, decision_epoch: u64) -> bool {
    baton
        .pending_intents
        .iter()
        .any(|intent| !intent.deferred && intent.eligible_decision_epoch <= decision_epoch)
        || baton.unresolved_handoffs.iter().any(|handoff| {
            handoff.question_state == "open"
                && handoff.blocked_by.is_none()
                && !handoff.moderator_retry_blocked
                && handoff.eligible_decision_epoch <= decision_epoch
        })
}

fn decision_candidate<'a>(
    decision: &'a ModeratorDecisionRecord,
    source_type: &str,
    source_id: &str,
) -> Result<&'a DecisionCandidateRef> {
    decision
        .attempt
        .candidate_refs
        .iter()
        .find(|candidate| candidate.source_type == source_type && candidate.source_id == source_id)
        .ok_or_else(|| anyhow!("moderator output references a source outside Candidate Cohort"))
}

fn moderator_next_action_spec(
    decision: &ModeratorDecisionRecord,
    moderator_pubkey: &str,
) -> Result<ModeratorActionSpec> {
    let self_intent = decision.attempt.candidate_refs.iter().find(|candidate| {
        candidate.source_type == "intent"
            && candidate.moderator_self
            && candidate.author_pubkey.as_deref() == Some(moderator_pubkey)
    });
    match decision.next_action.action.as_str() {
        "idle" => Ok(ModeratorActionSpec::Idle),
        "select_intent" | "moderator_speak" => {
            let intent_id = decision
                .next_action
                .id
                .as_deref()
                .ok_or_else(|| anyhow!("moderator Intent selection has no ID"))?;
            let candidate = decision_candidate(decision, "intent", intent_id)?;
            if self_intent.is_some_and(|own| own.source_id != candidate.source_id) {
                return Err(anyhow!(
                    "moderator self Intent prevents selecting another candidate"
                ));
            }
            let moderator_self = candidate.moderator_self;
            if decision.next_action.action == "moderator_speak" && !moderator_self {
                return Err(anyhow!("moderator_speak must select a self Intent"));
            }
            Ok(ModeratorActionSpec::SelectIntent {
                candidate: candidate.clone(),
                reason: decision.next_action.reason.clone(),
                moderator_self,
            })
        }
        "select_handoff" => {
            if self_intent.is_some() {
                return Err(anyhow!(
                    "moderator self Intent prevents selecting a Handoff"
                ));
            }
            let handoff_id = decision
                .next_action
                .id
                .as_deref()
                .ok_or_else(|| anyhow!("moderator Handoff selection has no ID"))?;
            let candidate = decision_candidate(decision, "handoff", handoff_id)?;
            Ok(ModeratorActionSpec::SelectHandoff {
                candidate: candidate.clone(),
                reason: decision.next_action.reason.clone(),
            })
        }
        "withdraw_self" => {
            let own = self_intent
                .ok_or_else(|| anyhow!("withdraw_self requires a pending self Intent"))?;
            if decision.next_action.id.as_deref() != Some(own.source_id.as_str()) {
                return Err(anyhow!(
                    "withdraw_self must identify the pending self Intent"
                ));
            }
            Ok(ModeratorActionSpec::WithdrawSelf {
                candidate: own.clone(),
            })
        }
        _ => Err(anyhow!("unknown moderator next action")),
    }
}

fn build_moderator_action_event(
    session_id: Uuid,
    view: &MeetingView,
    decision: &ModeratorDecisionRecord,
    action: &ModeratorActionSpec,
    keys: &Keys,
) -> Result<(String, String, Event)> {
    let attempt_id = decision.attempt.attempt_id.as_str();
    let (action_kind, object_id, builder) = match action {
        ModeratorActionSpec::Reject {
            candidate,
            proposal,
        } => {
            let previous_event_id = candidate
                .current_event_id
                .as_deref()
                .ok_or_else(|| anyhow!("Intent candidate has no event version"))?;
            let author_pubkey = candidate
                .author_pubkey
                .as_deref()
                .ok_or_else(|| anyhow!("Intent candidate has no author"))?;
            if candidate.moderator_self {
                return Err(anyhow!(
                    "a moderator must withdraw rather than reject its self Intent"
                ));
            }
            (
                "reject".to_string(),
                candidate.source_id.clone(),
                buzz_sdk::build_meeting_v1_moderator_reject(MeetingV1ModeratorRejectParams {
                    session_id,
                    intent_id: &candidate.source_id,
                    previous_event_id,
                    intent_author_pubkey: author_pubkey,
                    reason_code: parse_rejection_reason(&proposal.reason_code)?,
                    reason_text: &proposal.reason_text,
                    attempt_id: Some(attempt_id),
                }),
            )
        }
        ModeratorActionSpec::Dismiss {
            candidate,
            proposal,
        } => {
            let expected_attempt_count = candidate
                .attempt_count
                .ok_or_else(|| anyhow!("Handoff candidate has no attempt count"))?;
            (
                "dismiss_handoff".to_string(),
                candidate.source_id.clone(),
                buzz_sdk::build_meeting_v1_moderator_dismiss_handoff(
                    MeetingV1ModeratorDismissHandoffParams {
                        session_id,
                        handoff_id: &candidate.source_id,
                        expected_speech_revision: decision.attempt.speech_revision,
                        expected_attempt_count,
                        reason_code: parse_handoff_dismiss_reason(&proposal.reason_code)?,
                        reason_text: &proposal.reason_text,
                        attempt_id: Some(attempt_id),
                    },
                ),
            )
        }
        ModeratorActionSpec::SelectIntent {
            candidate,
            reason,
            moderator_self,
        } => {
            let expected_source_event_id = candidate
                .current_event_id
                .as_deref()
                .ok_or_else(|| anyhow!("Intent candidate has no event version"))?;
            let deferral_sources: Vec<_> = if *moderator_self {
                decision
                    .deferrals
                    .iter()
                    .filter_map(|deferral| {
                        decision
                            .attempt
                            .candidate_refs
                            .iter()
                            .find(|candidate| {
                                candidate.source_type == "intent"
                                    && candidate.source_id == deferral.intent_id
                                    && !candidate.moderator_self
                                    && intent_candidate_is_current(candidate, &view.baton)
                            })
                            .and_then(|candidate| {
                                candidate
                                    .current_event_id
                                    .as_deref()
                                    .map(|event_id| (deferral, candidate, event_id))
                            })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if *moderator_self && view.baton.consecutive_moderator_speeches >= 1 {
                let required: BTreeSet<_> = decision
                    .attempt
                    .candidate_refs
                    .iter()
                    .filter(|candidate| {
                        candidate.source_type == "intent"
                            && !candidate.moderator_self
                            && intent_candidate_is_current(candidate, &view.baton)
                    })
                    .map(|candidate| candidate.source_id.as_str())
                    .collect();
                let provided: BTreeSet<_> = deferral_sources
                    .iter()
                    .map(|(_, candidate, _)| candidate.source_id.as_str())
                    .collect();
                if !required.is_subset(&provided) {
                    return Err(anyhow!(
                        "consecutive moderator speech is missing required Deferrals"
                    ));
                }
            }
            let deferrals: Vec<_> = deferral_sources
                .iter()
                .map(
                    |(deferral, candidate, previous_event_id)| MeetingV1IntentDeferral {
                        intent_id: &candidate.source_id,
                        previous_event_id,
                        reason: &deferral.reason,
                    },
                )
                .collect();
            (
                if *moderator_self {
                    "moderator_speak".to_string()
                } else {
                    "select_intent".to_string()
                },
                candidate.source_id.clone(),
                buzz_sdk::build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
                    session_id,
                    selection: MeetingV1Selection::Intent {
                        intent_id: &candidate.source_id,
                    },
                    expected_control_epoch: decision.attempt.control_epoch,
                    expected_decision_epoch: decision.attempt.decision_epoch,
                    expected_intent_revision: view.baton.intent_revision,
                    expected_speech_revision: decision.attempt.speech_revision,
                    selection_reason: Some(reason),
                    deferrals: &deferrals,
                    attempt_id: Some(attempt_id),
                    expected_source_event_id: Some(expected_source_event_id),
                }),
            )
        }
        ModeratorActionSpec::SelectHandoff { candidate, reason } => {
            let expected_attempt_count = candidate
                .attempt_count
                .ok_or_else(|| anyhow!("Handoff candidate has no attempt count"))?;
            (
                "select_handoff".to_string(),
                candidate.source_id.clone(),
                buzz_sdk::build_meeting_v1_moderator_select(MeetingV1ModeratorSelectParams {
                    session_id,
                    selection: MeetingV1Selection::Handoff {
                        handoff_id: &candidate.source_id,
                        expected_attempt_count,
                    },
                    expected_control_epoch: decision.attempt.control_epoch,
                    expected_decision_epoch: decision.attempt.decision_epoch,
                    expected_intent_revision: view.baton.intent_revision,
                    expected_speech_revision: decision.attempt.speech_revision,
                    selection_reason: Some(reason),
                    deferrals: &[],
                    attempt_id: Some(attempt_id),
                    expected_source_event_id: None,
                }),
            )
        }
        ModeratorActionSpec::WithdrawSelf { candidate } => {
            let previous_event_id = candidate
                .current_event_id
                .as_deref()
                .ok_or_else(|| anyhow!("Intent candidate has no event version"))?;
            (
                "withdraw_self".to_string(),
                candidate.source_id.clone(),
                buzz_sdk::build_meeting_v1_moderator_withdraw_self(
                    MeetingV1ModeratorWithdrawSelfParams {
                        session_id,
                        attempt_id,
                        intent_id: &candidate.source_id,
                        previous_event_id,
                    },
                ),
            )
        }
        ModeratorActionSpec::Idle => return Err(anyhow!("idle has no protocol event")),
    };
    let event = builder
        .map_err(|error| anyhow!(error.to_string()))
        .and_then(|builder| sign_builder(builder, keys))?;
    Ok((action_kind, object_id, event))
}

fn speech_projection_complete(view: &MeetingView) -> bool {
    if view.baton.speech_revision == 0 {
        return true;
    }
    let revisions: BTreeSet<_> = view
        .speeches
        .iter()
        .filter_map(|speech| {
            (speech.speech_revision <= view.baton.speech_revision).then_some(speech.speech_revision)
        })
        .collect();
    u64::try_from(revisions.len()).ok() == Some(view.baton.speech_revision)
        && revisions.first() == Some(&1)
        && revisions.last() == Some(&view.baton.speech_revision)
}

fn grant_context_complete(view: &MeetingView, grant: &GrantView) -> bool {
    grant
        .source_intent_id
        .as_ref()
        .is_none_or(|intent_id| view.intents.contains_key(intent_id))
        && grant
            .source_speech_event_id
            .as_ref()
            .is_none_or(|event_id| {
                view.speeches
                    .iter()
                    .any(|speech| speech.event_id == *event_id)
            })
}

fn next_progress_deadline(now: i64, soft_lease_expires_at_ms: i64, interval_ms: i64) -> i64 {
    let regular = now.saturating_add(interval_ms.max(1_000));
    let before_expiry = soft_lease_expires_at_ms.saturating_sub(1_000);
    regular.min(before_expiry).max(now)
}

fn reservation_is_active_at(reservation: &ReservationRecord, now: i64) -> bool {
    matches!(
        reservation.state.as_str(),
        "ack_prepared" | "ack_sent" | "granted"
    ) && (reservation.capacity_expires_at_ms == 0 || now < reservation.capacity_expires_at_ms)
}

fn restore_prepared_offer_response(reservation: &mut ReservationRecord) {
    if matches!(
        reservation.state.as_str(),
        "ack_prepared" | "ack_sent" | "decline_prepared" | "decline_sent" | "granted"
    ) {
        return;
    }
    if reservation.ack_event.is_some() {
        reservation.state = "ack_prepared".to_string();
    } else if reservation.decline_event.is_some() {
        reservation.state = "decline_prepared".to_string();
    }
}

fn restore_active_grant_state(grant: &mut GrantRecord) {
    if grant.yield_event.is_some() {
        grant.state = "yield_prepared".to_string();
    } else if grant.speech_event.is_some() {
        grant.state = "speech_prepared".to_string();
    } else {
        grant.state = "received".to_string();
    }
}

fn grant_safety_margin_ms(view: &MeetingView) -> i64 {
    let configured = view.baton.baton_config.agent_safety_margin_ms;
    if configured > 0 {
        configured
    } else {
        DEFAULT_GRANT_SAFETY_MARGIN.as_millis() as i64
    }
}

fn build_intent_prompt(view: &MeetingView, trigger_id: &str, hard_deadline_unix_ms: i64) -> String {
    let recent_shared_conversation = prompt_speeches(&view.speeches, view.baton.speech_revision);
    let recent_shared_conversation_window = prompt_speech_window_metadata(
        &view.speeches,
        &recent_shared_conversation,
        view.baton.speech_revision,
    );
    let envelope = json!({
        "turn_type": "participant_intent",
        "session": {
            "id": view.session_id,
            "title": view.title,
            "description": view.description,
            "relay_pubkey": view.relay_pubkey,
        },
        "roster": view.roster.values().collect::<Vec<_>>(),
        "baton_state_event_id": view.baton.state_event_id,
        "baton": view.baton.raw_state,
        "trigger": trigger_context(view, trigger_id),
        "speech_cursor": view.speech_cursor,
        "recent_shared_conversation_window": recent_shared_conversation_window,
        "recent_shared_conversation": recent_shared_conversation,
        "hard_deadline_unix_ms": hard_deadline_unix_ms,
        "tool_policy": "advisory-v1",
        "allowed_tools": "normally exposed Harness tools for gathering context or evidence; no persistent writes or Meeting-event publishing",
        "output_schema": {
            "submit": {
                "action": "SUBMIT",
                "summary": "one sentence, at most 512 UTF-8 bytes",
                "addressed_to": "roster pubkey or null"
            },
            "pass": {
                "action": "PASS",
                "summary": null,
                "addressed_to": null
            }
        }
    });
    format!(
        "{PARTICIPANT_INTENT_PROMPT}\n\nUNTRUSTED MEETING CONTEXT:\n{}",
        serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
    )
}

fn build_granted_prompt(view: &MeetingView, grant: &GrantView, basis_id: &str) -> String {
    let recent_shared_conversation = prompt_speeches(&view.speeches, view.baton.speech_revision);
    let recent_shared_conversation_window = prompt_speech_window_metadata(
        &view.speeches,
        &recent_shared_conversation,
        view.baton.speech_revision,
    );
    let envelope = json!({
        "turn_type": "granted_speech",
        "session": {
            "id": view.session_id,
            "title": view.title,
            "description": view.description,
            "relay_pubkey": view.relay_pubkey,
        },
        "roster": view.roster.values().collect::<Vec<_>>(),
        "baton_state_event_id": view.baton.state_event_id,
        "baton": view.baton.raw_state,
        "grant": grant,
        "source_intent": grant
            .source_intent_id
            .as_ref()
            .and_then(|intent_id| view.intents.get(intent_id)),
        "basis": trigger_context(view, basis_id),
        "speech_cursor": view.speech_cursor,
        "recent_shared_conversation_window": recent_shared_conversation_window,
        "recent_shared_conversation": recent_shared_conversation,
        "harness_hard_deadline_unix_ms": grant
            .hard_deadline_ms
            .saturating_sub(grant_safety_margin_ms(view)),
        "tool_policy": "advisory-v1",
        "allowed_tools": "normally exposed Harness tools for gathering context or evidence; no persistent writes or Meeting-event publishing",
        "output_schema": {
            "say": {
                "action": "SAY",
                "content": "one complete public contribution",
                "mention_pubkeys": ["zero or more roster pubkeys"],
                "handoff": {
                    "target_pubkey": "another roster pubkey",
                    "handoff_type": "question | information_request | clarification | review | response_requested",
                    "reason": "why the target should receive the next Offer"
                },
                "reason": null
            },
            "yield": {
                "action": "YIELD",
                "content": null,
                "mention_pubkeys": [],
                "handoff": null,
                "reason": "why no useful contribution remains"
            }
        }
    });
    format!(
        "{GRANTED_SPEECH_PROMPT}\n\nUNTRUSTED MEETING CONTEXT:\n{}",
        serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
    )
}

fn build_moderator_control_prompt(
    view: &MeetingView,
    attempt: &ActiveDecisionAttemptView,
    hard_deadline_unix_ms: i64,
) -> String {
    let recent_shared_conversation = prompt_speeches(&view.speeches, attempt.speech_revision);
    let recent_shared_conversation_window = prompt_speech_window_metadata(
        &view.speeches,
        &recent_shared_conversation,
        attempt.speech_revision,
    );
    let envelope = json!({
        "turn_kind": "control_decision",
        "session": {
            "id": view.session_id,
            "title": view.title,
            "description": view.description,
        },
        "roster": view.roster.values().collect::<Vec<_>>(),
        "moderator_state": {
            "moderator_pubkey": view.baton.moderator_pubkey,
            "handoff_depth": view.baton.handoff_depth,
            "consecutive_moderator_speeches": view.baton.consecutive_moderator_speeches,
            "forced_return_to_moderator": view.baton.forced_return_to_moderator,
        },
        "decision_attempt": {
            "attempt_id": attempt.attempt_id,
            "control_epoch": attempt.control_epoch,
            "decision_epoch": attempt.decision_epoch,
            "attempt_number": attempt.attempt_number,
            "speech_revision": attempt.speech_revision,
            "snapshot_intent_revision": attempt.snapshot_intent_revision,
            "snapshot_state_event_id": attempt.snapshot_state_event_id,
            "candidate_snapshot_hash": attempt.candidate_snapshot_hash,
            "started_at_ms": attempt.started_at_ms,
            "deadline_ms": attempt.deadline_ms,
        },
        "candidate_cohort": attempt.candidate_refs,
        "recent_shared_conversation_window": recent_shared_conversation_window,
        "recent_shared_conversation": recent_shared_conversation,
        "harness_hard_deadline_unix_ms": hard_deadline_unix_ms,
        "tool_policy": "advisory-v1",
        "output_schema": {
            "rejections": [{
                "intent_id": "pending Intent ID",
                "reason_code": "off_topic | duplicate | superseded | unsupported | agenda_mismatch",
                "reason_text": "required explanation"
            }],
            "handoff_dismissals": [{
                "handoff_id": "open Handoff ID with no active attempt",
                "reason_code": "superseded | answered_elsewhere | out_of_scope | no_longer_needed",
                "reason_text": "required explanation"
            }],
            "deferrals": [{
                "intent_id": "other pending Intent ID; moderator-self selection only",
                "reason": "required explanation"
            }],
            "next_action": {
                "action": "select_intent | select_handoff | moderator_speak | withdraw_self | idle",
                "id": "selected supplied ID, or null for idle",
                "reason": "short decision explanation"
            }
        }
    });
    format!(
        "{MODERATOR_PROMPT}\n\nUNTRUSTED MEETING CONTEXT:\n{}",
        serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
    )
}

fn trigger_context(view: &MeetingView, trigger_id: &str) -> Value {
    if let Some(intent) = view.intents.get(trigger_id) {
        return json!({ "id": trigger_id, "intent": intent });
    }
    if let Some(event_id) = trigger_id.strip_prefix("speech:") {
        if let Some(speech) = view
            .speeches
            .iter()
            .find(|speech| speech.event_id == event_id)
        {
            return json!({ "id": trigger_id, "speech": speech });
        }
    }
    if let Some(handoff_id) = trigger_id.strip_prefix("handoff:") {
        if let Some(handoff) = view
            .baton
            .unresolved_handoffs
            .iter()
            .find(|handoff| handoff.handoff_id == handoff_id)
        {
            return json!({ "id": trigger_id, "handoff": handoff });
        }
    }
    json!({ "id": trigger_id })
}

fn prompt_speeches(speeches: &[Speech], authoritative_revision: u64) -> Vec<&Speech> {
    let mut bytes = 0usize;
    let mut selected = Vec::new();
    for speech in speeches
        .iter()
        .rev()
        .filter(|speech| speech.speech_revision <= authoritative_revision)
        .take(PROMPT_SPEECH_LIMIT)
    {
        let next = speech.content.len().saturating_add(384);
        if bytes.saturating_add(next) > PROMPT_CONTENT_LIMIT {
            break;
        }
        bytes = bytes.saturating_add(next);
        selected.push(speech);
    }
    selected.reverse();
    selected
}

fn prompt_speech_window_metadata(
    speeches: &[Speech],
    selected: &[&Speech],
    authoritative_revision: u64,
) -> Value {
    let authoritative_speech_count = speeches
        .iter()
        .filter(|speech| speech.speech_revision <= authoritative_revision)
        .count();
    json!({
        "authoritative_revision": authoritative_revision,
        "authoritative_speech_count": authoritative_speech_count,
        "included_speech_count": selected.len(),
        "first_included_revision": selected.first().map(|speech| speech.speech_revision),
        "last_included_revision": selected.last().map(|speech| speech.speech_revision),
        "omitted_earlier_speech_count": authoritative_speech_count.saturating_sub(selected.len()),
        "is_truncated": selected.len() < authoritative_speech_count,
        "older_history_lookup": {
            "tool": "meeting_read",
            "operation": "history",
            "limit_default": 100,
            "limit_maximum": 500,
        }
    })
}

fn parse_intent_output(raw: &str) -> Result<IntentOutput> {
    let output: IntentOutput =
        serde_json::from_str(raw.trim()).context("V1 Intent output is not exact JSON")?;
    match output.action.as_str() {
        "SUBMIT" => {
            let summary = output
                .summary
                .as_deref()
                .ok_or_else(|| anyhow!("SUBMIT requires summary"))?;
            validate_bounded_text(summary, MAX_INTENT_SUMMARY_BYTES, "Intent summary")?;
            if let Some(pubkey) = output.addressed_to.as_deref() {
                PublicKey::from_hex(pubkey)
                    .map_err(|_| anyhow!("addressed_to is not a public key"))?;
            }
        }
        "PASS" => {
            if output.summary.is_some() || output.addressed_to.is_some() {
                return Err(anyhow!("PASS requires null summary and addressed_to"));
            }
        }
        _ => return Err(anyhow!("Intent action must be SUBMIT or PASS")),
    }
    Ok(output)
}

fn parse_granted_output(raw: &str) -> Result<GrantedOutput> {
    let output: GrantedOutput =
        serde_json::from_str(raw.trim()).context("V1 Granted output is not exact JSON")?;
    match output.action.as_str() {
        "SAY" => {
            let content = output
                .content
                .as_deref()
                .ok_or_else(|| anyhow!("SAY requires content"))?;
            validate_bounded_text(content, MAX_SPEECH_BYTES, "speech content")?;
            if output.reason.is_some() || output.mention_pubkeys.len() > MAX_MENTIONS {
                return Err(anyhow!("SAY has an invalid reason or mention count"));
            }
            for pubkey in &output.mention_pubkeys {
                PublicKey::from_hex(pubkey)
                    .map_err(|_| anyhow!("speech mention is not a public key"))?;
            }
            if let Some(handoff) = &output.handoff {
                PublicKey::from_hex(&handoff.target_pubkey)
                    .map_err(|_| anyhow!("Handoff target is not a public key"))?;
                parse_handoff_type(&handoff.handoff_type)?;
                validate_bounded_text(&handoff.reason, MAX_REASON_BYTES, "Handoff reason")?;
            }
        }
        "YIELD" => {
            if output.content.is_some()
                || !output.mention_pubkeys.is_empty()
                || output.handoff.is_some()
            {
                return Err(anyhow!(
                    "YIELD cannot include content, mentions, or Handoff"
                ));
            }
            let reason = output
                .reason
                .as_deref()
                .ok_or_else(|| anyhow!("YIELD requires reason"))?;
            validate_bounded_text(reason, 512, "Yield reason")?;
        }
        _ => return Err(anyhow!("Granted action must be SAY or YIELD")),
    }
    Ok(output)
}

fn parse_control_output(
    raw: &str,
    view: &MeetingView,
    attempt: &ActiveDecisionAttemptView,
    moderator_pubkey: &str,
) -> Result<ControlOutput> {
    let output: ControlOutput = serde_json::from_str(raw.trim())
        .context("V1 moderator control output is not exact JSON")?;
    if output.rejections.len() > MAX_MODERATOR_CLEANUPS
        || output.handoff_dismissals.len() > MAX_MODERATOR_CLEANUPS
        || output.deferrals.len() > 12
    {
        return Err(anyhow!("moderator control exceeds proposal limits"));
    }
    validate_bounded_text(
        &output.next_action.reason,
        512,
        "moderator next-action reason",
    )?;
    let mut rejected = BTreeSet::new();
    for rejection in &output.rejections {
        parse_rejection_reason(&rejection.reason_code)?;
        validate_bounded_text(
            &rejection.reason_text,
            MAX_REASON_BYTES,
            "moderator rejection reason",
        )?;
        let candidate = attempt.candidate_refs.iter().find(|candidate| {
            candidate.source_type == "intent" && candidate.source_id == rejection.intent_id
        });
        if candidate.is_none_or(|candidate| candidate.moderator_self)
            || !rejected.insert(rejection.intent_id.clone())
        {
            return Err(anyhow!(
                "moderator rejection references an unknown, self, or duplicate Cohort Intent"
            ));
        }
    }
    let mut dismissed = BTreeSet::new();
    for dismissal in &output.handoff_dismissals {
        parse_handoff_dismiss_reason(&dismissal.reason_code)?;
        validate_bounded_text(
            &dismissal.reason_text,
            MAX_REASON_BYTES,
            "moderator Handoff dismissal reason",
        )?;
        if !attempt.candidate_refs.iter().any(|candidate| {
            candidate.source_type == "handoff" && candidate.source_id == dismissal.handoff_id
        }) || !dismissed.insert(dismissal.handoff_id.clone())
        {
            return Err(anyhow!(
                "moderator dismissal references an unknown or duplicate Cohort Handoff"
            ));
        }
    }

    let self_intent = attempt.candidate_refs.iter().find(|candidate| {
        candidate.source_type == "intent"
            && candidate.moderator_self
            && candidate.author_pubkey.as_deref() == Some(moderator_pubkey)
    });
    let selected_id = output.next_action.id.as_deref();
    match output.next_action.action.as_str() {
        "idle" => {
            if selected_id.is_some() || !output.deferrals.is_empty() {
                return Err(anyhow!("moderator idle cannot carry an ID or Deferrals"));
            }
        }
        "select_intent" => {
            let selected = selected_id
                .and_then(|id| {
                    attempt.candidate_refs.iter().find(|candidate| {
                        candidate.source_type == "intent" && candidate.source_id == id
                    })
                })
                .ok_or_else(|| anyhow!("moderator selected an Intent outside Candidate Cohort"))?;
            if selected.moderator_self
                || self_intent.is_some()
                || !output.deferrals.is_empty()
                || rejected.contains(&selected.source_id)
            {
                return Err(anyhow!(
                    "select_intent cannot bypass a moderator self Intent or carry Deferrals"
                ));
            }
            let author = selected
                .author_pubkey
                .as_deref()
                .and_then(|pubkey| view.roster.get(pubkey));
            if author.is_none() {
                return Err(anyhow!(
                    "selected Intent author is not in the frozen roster"
                ));
            }
        }
        "select_handoff" => {
            let handoff_id = selected_id.ok_or_else(|| anyhow!("select_handoff requires an ID"))?;
            if self_intent.is_some()
                || !output.deferrals.is_empty()
                || !attempt.candidate_refs.iter().any(|candidate| {
                    candidate.source_type == "handoff"
                        && candidate.source_id == handoff_id
                        && candidate
                            .target_pubkey
                            .as_deref()
                            .is_some_and(|pubkey| view.roster.contains_key(pubkey))
                })
                || dismissed.contains(handoff_id)
            {
                return Err(anyhow!(
                    "select_handoff is outside Candidate Cohort or blocked by self Intent"
                ));
            }
        }
        "moderator_speak" => {
            let own = self_intent.ok_or_else(|| {
                anyhow!("moderator_speak requires a self Intent in Candidate Cohort")
            })?;
            if selected_id != Some(own.source_id.as_str()) || rejected.contains(&own.source_id) {
                return Err(anyhow!(
                    "moderator_speak must identify the Cohort self Intent"
                ));
            }
        }
        "withdraw_self" => {
            let own = self_intent
                .ok_or_else(|| anyhow!("withdraw_self requires a Cohort self Intent"))?;
            if selected_id != Some(own.source_id.as_str())
                || !output.deferrals.is_empty()
                || rejected.contains(&own.source_id)
            {
                return Err(anyhow!(
                    "withdraw_self must identify only the Cohort self Intent"
                ));
            }
        }
        _ => return Err(anyhow!("unknown moderator next action")),
    }

    let mut deferrals = BTreeSet::new();
    for deferral in &output.deferrals {
        validate_bounded_text(
            &deferral.reason,
            MAX_REASON_BYTES,
            "moderator Deferral reason",
        )?;
        let intent = attempt
            .candidate_refs
            .iter()
            .find(|candidate| {
                candidate.source_type == "intent" && candidate.source_id == deferral.intent_id
            })
            .ok_or_else(|| anyhow!("Deferral references an Intent outside Candidate Cohort"))?;
        if intent.moderator_self
            || selected_id == Some(intent.source_id.as_str())
            || rejected.contains(&intent.source_id)
            || !deferrals.insert(intent.source_id.clone())
        {
            return Err(anyhow!(
                "moderator Deferral is self, selected, or duplicate"
            ));
        }
    }
    if output.next_action.action == "moderator_speak"
        && view.baton.consecutive_moderator_speeches >= 1
    {
        let missing = attempt.candidate_refs.iter().find(|candidate| {
            candidate.source_type == "intent"
                && !candidate.moderator_self
                && !rejected.contains(&candidate.source_id)
                && !deferrals.contains(&candidate.source_id)
        });
        if let Some(candidate) = missing {
            return Err(anyhow!(
                "consecutive moderator speech must defer Cohort Intent {}",
                candidate.source_id
            ));
        }
    }
    Ok(output)
}

fn parse_rejection_reason(value: &str) -> Result<MeetingV1IntentRejectionReason> {
    match value {
        "off_topic" => Ok(MeetingV1IntentRejectionReason::OffTopic),
        "duplicate" => Ok(MeetingV1IntentRejectionReason::Duplicate),
        "superseded" => Ok(MeetingV1IntentRejectionReason::Superseded),
        "unsupported" => Ok(MeetingV1IntentRejectionReason::Unsupported),
        "agenda_mismatch" => Ok(MeetingV1IntentRejectionReason::AgendaMismatch),
        _ => Err(anyhow!("unknown moderator Intent rejection reason")),
    }
}

fn parse_handoff_dismiss_reason(value: &str) -> Result<MeetingV1HandoffDismissReason> {
    match value {
        "superseded" => Ok(MeetingV1HandoffDismissReason::Superseded),
        "answered_elsewhere" => Ok(MeetingV1HandoffDismissReason::AnsweredElsewhere),
        "out_of_scope" => Ok(MeetingV1HandoffDismissReason::OutOfScope),
        "no_longer_needed" => Ok(MeetingV1HandoffDismissReason::NoLongerNeeded),
        _ => Err(anyhow!("unknown moderator Handoff dismissal reason")),
    }
}

fn parse_handoff_type(value: &str) -> Result<MeetingV1HandoffType> {
    match value {
        "question" => Ok(MeetingV1HandoffType::Question),
        "information_request" => Ok(MeetingV1HandoffType::InformationRequest),
        "clarification" => Ok(MeetingV1HandoffType::Clarification),
        "review" => Ok(MeetingV1HandoffType::Review),
        "response_requested" => Ok(MeetingV1HandoffType::ResponseRequested),
        _ => Err(anyhow!("unknown Directed Handoff type")),
    }
}

fn intent_format_correction_prompt() -> String {
    "FORMAT CORRECTION ONLY. Return exactly one raw JSON object, with no Markdown: \
     {\"action\":\"SUBMIT\",\"summary\":\"one sentence\",\"addressed_to\":null} or \
     {\"action\":\"PASS\",\"summary\":null,\"addressed_to\":null}. Preserve the prior \
     semantic decision and do not inspect more evidence."
        .to_string()
}

fn granted_format_correction_prompt() -> String {
    "FORMAT CORRECTION ONLY. Return exactly one raw JSON object, with no Markdown: \
     {\"action\":\"SAY\",\"content\":\"...\",\"mention_pubkeys\":[],\"handoff\":null,\"reason\":null} \
     or {\"action\":\"YIELD\",\"content\":null,\"mention_pubkeys\":[],\"handoff\":null,\
     \"reason\":\"...\"}. Preserve the prior semantic decision and do not inspect more evidence."
        .to_string()
}

fn progress_stage_name(stage: MeetingV1ProgressStage) -> &'static str {
    match stage {
        MeetingV1ProgressStage::ContextSync => "context_sync",
        MeetingV1ProgressStage::ToolUse => "tool_use",
        MeetingV1ProgressStage::Generating => "generating",
        MeetingV1ProgressStage::Composing => "composing",
        MeetingV1ProgressStage::Submitting => "submitting",
    }
}

fn parse_progress_stage(value: &str) -> Option<MeetingV1ProgressStage> {
    match value {
        "context_sync" => Some(MeetingV1ProgressStage::ContextSync),
        "tool_use" => Some(MeetingV1ProgressStage::ToolUse),
        "generating" => Some(MeetingV1ProgressStage::Generating),
        "composing" => Some(MeetingV1ProgressStage::Composing),
        "submitting" => Some(MeetingV1ProgressStage::Submitting),
        _ => None,
    }
}

fn serialized_event_id(value: &Value) -> Option<String> {
    serde_json::from_value::<Event>(value.clone())
        .ok()
        .map(|event| event.id.to_hex())
}

fn reserve_format_retry(attempts: &mut u8) -> bool {
    if *attempts >= 1 {
        return false;
    }
    *attempts += 1;
    true
}

fn ledger_path_for(agent_pubkey: &str) -> PathBuf {
    if let Some(path) = std::env::var_os("BUZZ_ACP_MEETING_V1_LEDGER_PATH") {
        return PathBuf::from(path);
    }
    let root = dirs::state_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(std::env::temp_dir);
    root.join("buzz")
        .join(format!("meeting-v1-agent-{}.json", &agent_pubkey[..16]))
}

fn load_ledger(path: &Path) -> Result<AgentLedger> {
    if !path.exists() {
        return Ok(AgentLedger::default());
    }
    let bytes = std::fs::read(path)
        .with_context(|| format!("read Meeting V1 ledger {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse Meeting V1 ledger {}", path.display()))
}

fn migrate_loaded_ledger(ledger: &mut AgentLedger, agent_pubkey: &str, path: &Path) -> bool {
    if ledger.agent_pubkey == agent_pubkey && ledger.version == PREVIOUS_LEDGER_VERSION {
        // V3 already contains durable ACK/Intent/Progress/SAY/YIELD events.
        // V4 optimistic-decision fields deserialize with defaults, so only
        // advance the version and preserve every previously signed event.
        ledger.version = LEDGER_VERSION;
        return true;
    }
    if ledger.version == LEDGER_VERSION && ledger.agent_pubkey == agent_pubkey {
        return false;
    }
    if ledger.version != 0 {
        tracing::warn!(
            path = %path.display(),
            found_version = ledger.version,
            "Meeting V1 ledger version/identity changed; rebuilding from Relay State"
        );
    }
    *ledger = AgentLedger {
        version: LEDGER_VERSION,
        agent_pubkey: agent_pubkey.to_string(),
        meetings: BTreeMap::new(),
    };
    true
}

fn persist_ledger(path: &Path, ledger: &AgentLedger) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Meeting V1 ledger path has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("create Meeting V1 ledger directory {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(ledger)?;
    let tmp = parent.join(format!(
        ".meeting-v1-ledger-{}-{}.tmp",
        std::process::id(),
        Uuid::new_v4()
    ));
    let write_result = (|| -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .with_context(|| format!("create temporary Meeting V1 ledger {}", tmp.display()))?;
        file.write_all(&bytes)
            .with_context(|| format!("write temporary Meeting V1 ledger {}", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("sync temporary Meeting V1 ledger {}", tmp.display()))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error).with_context(|| format!("replace Meeting V1 ledger {}", path.display()));
    }
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .with_context(|| format!("sync Meeting V1 ledger directory {}", parent.display()))?;
    Ok(())
}

fn recover_interrupted_turns(ledger: &mut AgentLedger) -> (usize, usize, bool) {
    let mut recovered_intents = 0;
    let mut recovered_grants = 0;
    let mut changed = false;
    for meeting in ledger.meetings.values_mut() {
        let (intents, grants, meeting_changed) = recover_interrupted_meeting_turns(meeting);
        recovered_intents += intents;
        recovered_grants += grants;
        changed |= meeting_changed;
    }
    (recovered_intents, recovered_grants, changed)
}

fn recover_interrupted_meeting_turns(meeting: &mut MeetingLedger) -> (usize, usize, bool) {
    let mut recovered_intents = 0;
    let mut recovered_grants = 0;
    let mut changed = false;
    for trigger in meeting.triggers.values_mut() {
        if trigger.state == "running" || trigger.state == "queued" {
            trigger.state =
                if trigger.prepared_event.is_some() && trigger.prepared_event_id.is_some() {
                    "prepared".to_string()
                } else {
                    "pending".to_string()
                };
            recovered_intents += 1;
            changed = true;
        }
    }
    for grant in meeting.grants.values_mut() {
        if grant.state == "running" || grant.state == "queued" {
            restore_active_grant_state(grant);
            recovered_grants += 1;
            changed = true;
        }
    }
    if let Some(decision) = meeting.moderator_decision.as_mut() {
        if matches!(decision.state.as_str(), "queued" | "running") {
            // The provider process cannot be proven to have survived this
            // Runtime. Relay State reconciliation will abandon the registered
            // attempt before a bounded replacement is started.
            decision.state = "runtime_lost".to_string();
            decision.turn_id = None;
            decision.turn_started_at_ms = None;
            changed = true;
        }
    }
    if let Some(prepared) = meeting.prepared_moderator_action.as_mut() {
        if prepared.state == "sent" {
            // The response may have been lost after Relay acceptance. Replay
            // the exact same signed event and reconcile its canonical result.
            prepared.state = "prepared".to_string();
            changed = true;
        }
    }
    (recovered_intents, recovered_grants, changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Tag, Timestamp};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn pubkey(byte: u8) -> String {
        hex::encode([byte; 32])
    }

    fn base_state() -> RawBatonState {
        RawBatonState {
            phase: "moderator_idle".to_string(),
            state_revision: 1,
            floor_revision: 1,
            intent_revision: 0,
            speech_revision: 0,
            control_epoch: 1,
            decision_epoch: 0,
            decision_attempt: 0,
            active_decision_attempt: None,
            moderator_pubkey: pubkey(1),
            baton_config: BatonConfigView {
                progress_interval_ms: 10_000,
                grant_hard_deadline_ms: 300_000,
                agent_safety_margin_ms: 30_000,
                moderator_max_rejudgments: default_moderator_max_rejudgments(),
                moderator_max_cas_rebases_per_attempt: default_moderator_max_cas_rebases(),
            },
            participants: vec![RawParticipant {
                pubkey: pubkey(1),
                participant_type: "agent".to_string(),
            }],
            pending_intents: Vec::new(),
            human_queue: Vec::new(),
            unresolved_handoffs: Vec::new(),
            handoff_depth: 0,
            consecutive_moderator_speeches: 0,
            forced_return_to_moderator: false,
            moderator_decision_deadline_ms: None,
            next_action_at_ms: None,
            offer: None,
            grant: None,
        }
    }

    fn baton_view() -> BatonView {
        BatonView {
            raw_state: json!({
                "phase": "moderator_idle",
                "state_revision": 1,
                "floor_revision": 1,
                "intent_revision": 0,
                "speech_revision": 0,
                "control_epoch": 1,
                "decision_epoch": 0,
                "moderator_pubkey": pubkey(1),
            }),
            state_event_id: pubkey(9),
            phase: "moderator_idle".to_string(),
            state_revision: 1,
            floor_revision: 1,
            intent_revision: 0,
            speech_revision: 0,
            control_epoch: 1,
            decision_epoch: 0,
            decision_attempt: 0,
            active_decision_attempt: None,
            moderator_pubkey: pubkey(1),
            baton_config: BatonConfigView {
                progress_interval_ms: 10_000,
                grant_hard_deadline_ms: 300_000,
                agent_safety_margin_ms: 30_000,
                moderator_max_rejudgments: default_moderator_max_rejudgments(),
                moderator_max_cas_rebases_per_attempt: default_moderator_max_cas_rebases(),
            },
            pending_intents: Vec::new(),
            human_queue: Vec::new(),
            unresolved_handoffs: Vec::new(),
            handoff_depth: 0,
            consecutive_moderator_speeches: 0,
            forced_return_to_moderator: false,
            moderator_decision_deadline_ms: None,
            next_action_at_ms: None,
            offer: None,
            grant: None,
        }
    }

    fn meeting_view(session_id: Uuid, agent_pubkey: &str, other_pubkey: &str) -> MeetingView {
        let roster = BTreeMap::from([
            (
                agent_pubkey.to_string(),
                Participant {
                    pubkey: agent_pubkey.to_string(),
                    role: "member".to_string(),
                    participant_type: "agent".to_string(),
                    display_name: "Agent".to_string(),
                },
            ),
            (
                other_pubkey.to_string(),
                Participant {
                    pubkey: other_pubkey.to_string(),
                    role: "member".to_string(),
                    participant_type: "human".to_string(),
                    display_name: "Human".to_string(),
                },
            ),
        ]);
        let mut baton = baton_view();
        baton.moderator_pubkey = other_pubkey.to_string();
        baton.raw_state["moderator_pubkey"] = json!(other_pubkey);
        MeetingView {
            session_id,
            title: "Test meeting".to_string(),
            description: Some("Test-only Meeting V1 context".to_string()),
            ended: false,
            relay_pubkey: pubkey(10),
            roster,
            speeches: Vec::new(),
            intents: BTreeMap::new(),
            speech_cursor: None,
            baton,
        }
    }

    fn ended_meeting_view(
        session_id: Uuid,
        agent_pubkey: &str,
        other_pubkey: &str,
        state_revision: u64,
    ) -> MeetingView {
        let mut view = meeting_view(session_id, agent_pubkey, other_pubkey);
        view.ended = true;
        view.baton.phase = "ended".to_string();
        view.baton.state_revision = state_revision;
        view.baton.state_event_id = pubkey((state_revision as u8).saturating_add(20));
        view.baton.raw_state["phase"] = json!("ended");
        view.baton.raw_state["state_revision"] = json!(state_revision);
        view
    }

    fn test_coordinator(
        keys: Keys,
        ledger_path: PathBuf,
        observer: Option<ObserverHandle>,
    ) -> MeetingV1Coordinator {
        let agent_pubkey = keys.public_key().to_hex();
        let (sync_result_tx, sync_result_rx) = tokio::sync::mpsc::unbounded_channel();
        let (protocol_result_tx, protocol_result_rx) = tokio::sync::mpsc::unbounded_channel();
        let (progress_result_tx, progress_result_rx) = tokio::sync::mpsc::unbounded_channel();
        MeetingV1Coordinator {
            rest: RestClient {
                http: reqwest::Client::new(),
                base_url: "http://127.0.0.1:9".to_string(),
                keys: keys.clone(),
                auth_tag_json: None,
            },
            keys,
            agent_pubkey: agent_pubkey.clone(),
            observer,
            agent_capacity: 1,
            available_agent_slots: 1,
            auto_accept_offers: true,
            ledger_path,
            ledger: AgentLedger {
                version: LEDGER_VERSION,
                agent_pubkey,
                meetings: BTreeMap::new(),
            },
            terminal_ledger_cleanup_retry_at: None,
            meetings: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: HashMap::new(),
            in_flight_epochs: HashMap::new(),
            external_reclaimable_turns: BTreeSet::new(),
            preemptions: BTreeSet::new(),
            next_session_epoch: 0,
            next_sync_request_id: 0,
            sync_result_tx,
            sync_result_rx,
            deferred_turn_results: HashMap::new(),
            next_protocol_submission_id: 0,
            protocol_in_flight: HashMap::new(),
            protocol_result_tx,
            protocol_result_rx,
            next_progress_submission_id: 0,
            progress_in_flight: HashMap::new(),
            progress_waiting_for_state: HashMap::new(),
            progress_result_tx,
            progress_result_rx,
            #[cfg(feature = "meeting-v1-acceptance")]
            acceptance_barrier: PreSubmitAcceptanceBarrier::from_env(),
        }
    }

    fn test_grant(agent_pubkey: &str, grant_id: &str, offer_id: &str) -> GrantView {
        GrantView {
            grant_id: grant_id.to_string(),
            holder_pubkey: agent_pubkey.to_string(),
            allocation_source: "moderator_selection".to_string(),
            turn_role: "participant".to_string(),
            source_offer_id: offer_id.to_string(),
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            basis_speech_revision: 0,
            soft_lease_expires_at_ms: now_ms() + 30_000,
            hard_deadline_ms: now_ms() + 300_000,
            progress_seq: 0,
        }
    }

    fn test_grant_record(grant: &GrantView) -> GrantRecord {
        GrantRecord {
            grant_id: grant.grant_id.clone(),
            source_offer_id: grant.source_offer_id.clone(),
            state: "received".to_string(),
            basis_speech_revision: grant.basis_speech_revision,
            soft_lease_expires_at_ms: grant.soft_lease_expires_at_ms,
            hard_deadline_ms: grant.hard_deadline_ms,
            progress_seq: grant.progress_seq,
            next_progress_at_ms: now_ms() + 10_000,
            prepared_progress: None,
            speech_event: None,
            speech_event_id: None,
            yield_event: None,
            format_attempts: 0,
        }
    }

    fn granted_turn_request(session_id: Uuid, grant_id: &str) -> MeetingTurnRequest {
        MeetingTurnRequest {
            session_id,
            prompt: "test granted turn".to_string(),
            hard_deadline_unix_ms: now_ms() + 270_000,
            kind: MeetingTurnKind::V1Granted,
            format_retry: false,
            basis_id: format!("grant:{grant_id}"),
            round_number: 0,
            speech_cursor: None,
            floor_revision: 1,
            grant_event_id: Some(grant_id.to_string()),
            queued_at_unix_ms: now_ms(),
        }
    }

    fn runtime_with_view(epoch: u64, view: MeetingView) -> MeetingRuntime {
        let mut runtime = MeetingRuntime::new(epoch);
        runtime.view = Some(view);
        runtime.last_sync = Some(Instant::now());
        runtime
    }

    fn intent_candidate(
        source_id: &str,
        event_id: &str,
        author_pubkey: &str,
        moderator_self: bool,
        eligible_decision_epoch: u64,
    ) -> DecisionCandidateRef {
        DecisionCandidateRef {
            source_type: "intent".to_string(),
            source_id: source_id.to_string(),
            current_event_id: Some(event_id.to_string()),
            author_pubkey: Some(author_pubkey.to_string()),
            moderator_self,
            basis_speech_revision: Some(0),
            summary: Some("candidate summary".to_string()),
            addressed_to: None,
            source_speech_event_id: None,
            from_pubkey: None,
            target_pubkey: None,
            reason_type: None,
            reason_text: None,
            attempt_count: None,
            eligible_decision_epoch,
            created_at_ms: 1,
        }
    }

    fn handoff_candidate(
        source_id: &str,
        from_pubkey: &str,
        target_pubkey: &str,
        attempt_count: u64,
        eligible_decision_epoch: u64,
    ) -> DecisionCandidateRef {
        DecisionCandidateRef {
            source_type: "handoff".to_string(),
            source_id: source_id.to_string(),
            current_event_id: None,
            author_pubkey: None,
            moderator_self: false,
            basis_speech_revision: None,
            summary: None,
            addressed_to: None,
            source_speech_event_id: Some(pubkey(211)),
            from_pubkey: Some(from_pubkey.to_string()),
            target_pubkey: Some(target_pubkey.to_string()),
            reason_type: Some("question".to_string()),
            reason_text: Some("Please answer".to_string()),
            attempt_count: Some(attempt_count),
            eligible_decision_epoch,
            created_at_ms: 2,
        }
    }

    fn decision_attempt(
        view: &MeetingView,
        candidate_refs: Vec<DecisionCandidateRef>,
    ) -> ActiveDecisionAttemptView {
        let mut attempt = ActiveDecisionAttemptView {
            attempt_id: pubkey(200),
            control_epoch: view.baton.control_epoch,
            decision_epoch: view.baton.decision_epoch,
            attempt_number: 1,
            speech_revision: view.baton.speech_revision,
            snapshot_intent_revision: view.baton.intent_revision,
            snapshot_state_event_id: view.baton.state_event_id.clone(),
            candidate_refs,
            candidate_snapshot_hash: String::new(),
            started_at_ms: now_ms(),
            deadline_ms: now_ms() + DEFAULT_MODERATOR_DECISION_DURATION.as_millis() as i64,
        };
        attempt.candidate_snapshot_hash =
            candidate_snapshot_hash(&attempt).expect("hash candidate snapshot");
        attempt
    }

    fn install_decision(
        coordinator: &mut MeetingV1Coordinator,
        view: &mut MeetingView,
        attempt: ActiveDecisionAttemptView,
        state: &str,
        next_action: ModeratorNextAction,
    ) {
        view.baton.decision_attempt = attempt.attempt_number;
        view.baton.active_decision_attempt = Some(attempt.clone());
        coordinator.apply_view_to_ledger(view);
        coordinator
            .ledger_for_mut(view.session_id)
            .expect("Meeting ledger")
            .moderator_decision = Some(ModeratorDecisionRecord {
            attempt,
            rejections: Vec::new(),
            handoff_dismissals: Vec::new(),
            deferrals: Vec::new(),
            next_action,
            state: state.to_string(),
            turn_id: None,
            turn_started_at_ms: None,
            cas_rebases: 0,
            fast_rebases: 0,
            pending_retry: None,
            pending_finish_reason: None,
        });
    }

    fn protocol_rejection(
        event_id: &str,
        code: &str,
        retry_ticket_id: Option<String>,
    ) -> ProtocolSubmitFailure {
        ProtocolSubmitFailure::Rejected(ProtocolSubmitRejected {
            http_status: 409,
            event_id: event_id.to_string(),
            code: code.to_string(),
            canonical_object_id: None,
            retry_ticket_id,
            message: format!("test rejection: {code}"),
            response: json!({ "accepted": false, "code": code }),
        })
    }

    async fn rest_responding_once(
        status: &str,
        body: &str,
    ) -> (RestClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test HTTP bridge");
        let address = listener.local_addr().expect("read test HTTP address");
        let status = status.to_string();
        let body = body.to_string();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept test HTTP request");
            let mut request = vec![0_u8; 16 * 1024];
            let _ = socket.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write test HTTP response");
        });
        let keys = Keys::generate();
        (
            RestClient {
                http: reqwest::Client::new(),
                base_url: format!("http://{address}"),
                keys,
                auth_tag_json: None,
            },
            server,
        )
    }

    async fn gated_rest_responding_to(
        keys: Keys,
        expected_requests: usize,
    ) -> (
        RestClient,
        tokio::sync::mpsc::UnboundedReceiver<()>,
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind gated test HTTP bridge");
        let address = listener.local_addr().expect("read gated HTTP address");
        let (request_started_tx, request_started_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = tokio::sync::watch::channel(false);
        let server = tokio::spawn(async move {
            let mut handlers = Vec::with_capacity(expected_requests);
            for _ in 0..expected_requests {
                let (mut socket, _) = listener
                    .accept()
                    .await
                    .expect("accept gated test HTTP request");
                let request_started_tx = request_started_tx.clone();
                let mut release_rx = release_rx.clone();
                handlers.push(tokio::spawn(async move {
                    let mut request = vec![0_u8; 16 * 1024];
                    let bytes_read = socket
                        .read(&mut request)
                        .await
                        .expect("read gated test HTTP request");
                    assert!(bytes_read > 0, "gated HTTP request must not be empty");
                    request_started_tx
                        .send(())
                        .expect("report gated HTTP request");
                    while !*release_rx.borrow() {
                        release_rx
                            .changed()
                            .await
                            .expect("gated HTTP response release sender");
                    }
                    const BODY: &str = r#"{"accepted":true}"#;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{BODY}",
                        BODY.len()
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("write gated test HTTP response");
                }));
            }
            for handler in handlers {
                handler.await.expect("join gated HTTP request handler");
            }
        });
        (
            RestClient {
                http: reqwest::Client::new(),
                base_url: format!("http://{address}"),
                keys,
                auth_tag_json: None,
            },
            request_started_rx,
            release_tx,
            server,
        )
    }

    fn agent_offer_view(
        session_id: Uuid,
        agent_pubkey: &str,
        other_pubkey: &str,
        offer_id: &str,
    ) -> MeetingView {
        let mut view = meeting_view(session_id, agent_pubkey, other_pubkey);
        view.baton.phase = "offered".to_string();
        view.baton.offer = Some(OfferView {
            offer_id: offer_id.to_string(),
            target_pubkey: agent_pubkey.to_string(),
            target_participant_type: "agent".to_string(),
            allocation_source: "moderator_selection".to_string(),
            turn_role: "participant".to_string(),
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            created_at_ms: now_ms(),
            ack_deadline_ms: now_ms() + 30_000,
        });
        view
    }

    fn reservation_state<'a>(
        coordinator: &'a MeetingV1Coordinator,
        session_id: Uuid,
        offer_id: &str,
    ) -> Option<&'a str> {
        coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.reservations.get(offer_id))
            .map(|reservation| reservation.state.as_str())
    }

    #[tokio::test]
    async fn offers_in_different_sessions_start_ack_submissions_without_serial_http_waits() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let first_session = Uuid::new_v4();
        let second_session = Uuid::new_v4();
        let first_offer = pubkey(51);
        let second_offer = pubkey(52);
        let first_view =
            agent_offer_view(first_session, &agent_pubkey, &other_pubkey, &first_offer);
        let second_view =
            agent_offer_view(second_session, &agent_pubkey, &other_pubkey, &second_offer);
        let (rest, mut request_started, release_responses, server) =
            gated_rest_responding_to(keys.clone(), 2).await;
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.rest = rest;
        coordinator.agent_capacity = 2;
        coordinator.available_agent_slots = 2;
        coordinator.ensure_meeting_ledger(first_session);
        coordinator.ensure_meeting_ledger(second_session);

        tokio::time::timeout(Duration::from_millis(500), async {
            assert!(coordinator.handle_offer(first_session, &first_view).await);
            assert!(coordinator.handle_offer(second_session, &second_view).await);
        })
        .await
        .expect("both Offer handlers must return without awaiting the two-second HTTP timeout");

        assert_eq!(
            reservation_state(&coordinator, first_session, &first_offer),
            Some("ack_prepared")
        );
        assert_eq!(
            reservation_state(&coordinator, second_session, &second_offer),
            Some("ack_prepared")
        );
        assert_eq!(
            coordinator.protocol_in_flight.len(),
            2,
            "each Session must own an independent background ACK"
        );
        tokio::time::timeout(Duration::from_secs(1), async {
            request_started
                .recv()
                .await
                .expect("first ACK reached the gated HTTP server");
            request_started
                .recv()
                .await
                .expect("second ACK reached the gated HTTP server");
        })
        .await
        .expect("both ACK requests must start while responses remain gated");

        coordinator.drain_protocol_results().await;
        assert_eq!(
            reservation_state(&coordinator, first_session, &first_offer),
            Some("ack_prepared")
        );
        assert_eq!(
            reservation_state(&coordinator, second_session, &second_offer),
            Some("ack_prepared")
        );

        release_responses
            .send(true)
            .expect("release both gated HTTP responses");
        server.await.expect("join gated HTTP server");
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                coordinator.drain_protocol_results().await;
                let both_sent = reservation_state(&coordinator, first_session, &first_offer)
                    == Some("ack_sent")
                    && reservation_state(&coordinator, second_session, &second_offer)
                        == Some("ack_sent");
                if both_sent {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("drain_protocol_results must apply both matching ACK completions");
        assert!(coordinator.protocol_in_flight.is_empty());
    }

    #[tokio::test]
    async fn sixteen_sessions_start_and_settle_independent_ack_submissions_concurrently() {
        const SESSION_COUNT: usize = 16;

        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let offers: Vec<_> = (0..SESSION_COUNT)
            .map(|index| {
                let session_id = Uuid::new_v4();
                let offer_id = pubkey(120 + index as u8);
                let view = agent_offer_view(session_id, &agent_pubkey, &other_pubkey, &offer_id);
                (session_id, offer_id, view)
            })
            .collect();
        let (rest, mut request_started, release_responses, server) =
            gated_rest_responding_to(keys.clone(), SESSION_COUNT).await;
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.rest = rest;
        coordinator.agent_capacity = SESSION_COUNT;
        coordinator.available_agent_slots = SESSION_COUNT;
        for (session_id, _, _) in &offers {
            coordinator.ensure_meeting_ledger(*session_id);
        }

        tokio::time::timeout(Duration::from_secs(3), async {
            for (session_id, _, view) in &offers {
                assert!(coordinator.handle_offer(*session_id, view).await);
            }
        })
        .await
        .expect("all Offer handlers must return without awaiting gated HTTP responses");

        assert_eq!(
            coordinator.protocol_in_flight.len(),
            SESSION_COUNT,
            "every Session must own an independent background ACK"
        );
        for (session_id, offer_id, _) in &offers {
            assert_eq!(
                reservation_state(&coordinator, *session_id, offer_id),
                Some("ack_prepared")
            );
        }

        tokio::time::timeout(Duration::from_secs(3), async {
            for _ in 0..SESSION_COUNT {
                request_started
                    .recv()
                    .await
                    .expect("every ACK must reach the gated HTTP server");
            }
        })
        .await
        .expect("all ACK requests must start before any response is released");

        coordinator.drain_protocol_results().await;
        for (session_id, offer_id, _) in &offers {
            assert_eq!(
                reservation_state(&coordinator, *session_id, offer_id),
                Some("ack_prepared"),
                "a gated response must leave its reservation prepared"
            );
        }

        release_responses
            .send(true)
            .expect("release all gated HTTP responses");
        server.await.expect("join gated HTTP server");
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                coordinator.drain_protocol_results().await;
                if offers.iter().all(|(session_id, offer_id, _)| {
                    reservation_state(&coordinator, *session_id, offer_id) == Some("ack_sent")
                }) {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("every independent ACK completion must settle");
        assert!(coordinator.protocol_in_flight.is_empty());
    }

    #[tokio::test]
    async fn eight_agent_slots_complete_five_granted_rounds_without_duplicate_or_cross_session_speech(
    ) {
        const SESSION_COUNT: usize = 8;
        const ROUND_COUNT: usize = 5;

        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let mut coordinator = test_coordinator(
            keys.clone(),
            dir.path().join("meeting-v1-ledger.json"),
            None,
        );
        coordinator.agent_capacity = SESSION_COUNT;
        coordinator.available_agent_slots = SESSION_COUNT;

        let mut sessions = Vec::with_capacity(SESSION_COUNT);
        for index in 0..SESSION_COUNT {
            let session_id = Uuid::new_v4();
            let view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
            coordinator.apply_view_to_ledger(&view);
            let activation_id = format!("activation:{session_id}");
            coordinator
                .ledger_for_mut(session_id)
                .and_then(|ledger| ledger.triggers.get_mut(&activation_id))
                .expect("initial activation trigger")
                .state = "passed".to_string();
            coordinator.meetings.insert(
                session_id,
                runtime_with_view(index as u64 + 1, view.clone()),
            );
            sessions.push((session_id, view));
        }

        let mut all_speech_event_ids = BTreeSet::new();
        for round_index in 0..ROUND_COUNT {
            let (rest, mut request_started, release_responses, server) =
                gated_rest_responding_to(keys.clone(), SESSION_COUNT).await;
            coordinator.rest = rest;
            let mut expected = BTreeMap::new();

            for (session_index, (session_id, view)) in sessions.iter_mut().enumerate() {
                let grant_id = pubkey(20 + (round_index * SESSION_COUNT + session_index) as u8);
                let offer_id = pubkey(80 + (round_index * SESSION_COUNT + session_index) as u8);
                let mut grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
                grant.basis_speech_revision = view.baton.speech_revision;
                view.baton.phase = "granted".to_string();
                view.baton.state_revision = view.baton.state_revision.saturating_add(1);
                view.baton.state_event_id =
                    pubkey(140 + (round_index * SESSION_COUNT + session_index) as u8);
                view.baton.grant = Some(grant);
                view.baton.raw_state["phase"] = json!("granted");
                view.baton.raw_state["state_revision"] = json!(view.baton.state_revision);
                view.baton.raw_state["speech_revision"] = json!(view.baton.speech_revision);

                coordinator.apply_view_to_ledger(view);
                let runtime = coordinator
                    .meetings
                    .get_mut(session_id)
                    .expect("active Meeting runtime");
                runtime.view = Some(view.clone());
                runtime.last_sync = Some(Instant::now());

                coordinator.reconcile(*session_id).await;
                coordinator.reconcile(*session_id).await;
                let content = format!(
                    "Session {session_index} round {} has one canonical answer.",
                    round_index + 1
                );
                expected.insert(*session_id, (grant_id, offer_id, content));
            }

            assert_eq!(
                coordinator.pending.len(),
                SESSION_COUNT,
                "each active Grant must queue exactly one Agent turn"
            );
            assert_eq!(
                coordinator
                    .pending
                    .iter()
                    .map(|request| request.session_id)
                    .collect::<BTreeSet<_>>()
                    .len(),
                SESSION_COUNT,
                "repeated reconciliation must not duplicate a Session turn"
            );

            for dispatch_index in 0..SESSION_COUNT {
                let request = coordinator.pop_pending().expect("queued Granted turn");
                assert_eq!(request.kind, MeetingTurnKind::V1Granted);
                let (grant_id, _, content) = expected
                    .get(&request.session_id)
                    .expect("request belongs to this stress round");
                assert_eq!(request.grant_event_id.as_deref(), Some(grant_id.as_str()));
                let turn_id = format!("stress-round-{round_index}-turn-{dispatch_index}");
                coordinator.mark_dispatched(turn_id.clone(), request.clone());

                // The production wrapper removes ownership before applying the
                // already-synchronized semantic result. Keep that invariant in
                // this focused controller test without starting another Relay
                // history query.
                coordinator.in_flight.remove(&turn_id);
                coordinator.in_flight_epochs.remove(&turn_id);
                coordinator
                    .meetings
                    .get_mut(&request.session_id)
                    .expect("dispatched Meeting runtime")
                    .in_flight_turn = None;
                let output = json!({
                    "action": "SAY",
                    "content": content,
                    "mention_pubkeys": [],
                    "handoff": null,
                    "reason": null,
                })
                .to_string();
                coordinator
                    .handle_granted_result(&turn_id, &request, &output, true)
                    .await;
            }

            assert!(coordinator.pending.is_empty());
            assert_eq!(
                coordinator.protocol_in_flight.len(),
                SESSION_COUNT,
                "all Speech submissions must be independently in flight"
            );
            tokio::time::timeout(Duration::from_secs(3), async {
                for _ in 0..SESSION_COUNT {
                    request_started
                        .recv()
                        .await
                        .expect("every Speech must reach the gated HTTP server");
                }
            })
            .await
            .expect("all Speech requests must start before any response is released");

            let mut prepared = BTreeMap::new();
            for (session_id, (grant_id, _, expected_content)) in &expected {
                let record = coordinator
                    .ledger_for(*session_id)
                    .and_then(|ledger| ledger.grants.get(grant_id))
                    .expect("durable Grant record");
                assert_eq!(record.state, "speech_prepared");
                let event: Event = serde_json::from_value(
                    record
                        .speech_event
                        .clone()
                        .expect("durable prepared Speech"),
                )
                .expect("deserialize prepared Speech");
                assert_eq!(event.kind.as_u16() as u32, KIND_STREAM_MESSAGE);
                let session_tag = session_id.to_string();
                assert_eq!(tag_value(&event, "h"), Some(session_tag.as_str()));
                assert_eq!(tag_value(&event, "meeting-grant"), Some(grant_id.as_str()));
                assert_eq!(
                    tag_value(&event, "speech-revision")
                        .and_then(|value| value.parse::<u64>().ok()),
                    Some(round_index as u64 + 1)
                );
                assert_eq!(&event.content, expected_content);
                assert!(
                    all_speech_event_ids.insert(event.id.to_hex()),
                    "every Session/round must own a distinct signed Speech"
                );
                prepared.insert(*session_id, event);
            }

            release_responses
                .send(true)
                .expect("release all Speech responses");
            server.await.expect("join gated Speech server");
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    coordinator.drain_protocol_results().await;
                    if expected.iter().all(|(session_id, (grant_id, _, _))| {
                        coordinator
                            .ledger_for(*session_id)
                            .and_then(|ledger| ledger.grants.get(grant_id))
                            .is_some_and(|record| record.state == "speech_sent")
                    }) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("every Speech completion must settle independently");
            assert!(coordinator.protocol_in_flight.is_empty());

            // A successful POST is not yet authoritative. Until Relay State
            // advances, every Session must wait instead of publishing a second
            // Speech for the same Grant.
            for (session_id, _) in &sessions {
                coordinator
                    .meetings
                    .get_mut(session_id)
                    .expect("active Meeting runtime")
                    .last_sync = Some(Instant::now());
                coordinator.reconcile(*session_id).await;
            }
            assert!(coordinator.pending.is_empty());
            assert!(coordinator.protocol_in_flight.is_empty());

            for (session_id, view) in &mut sessions {
                let (grant_id, offer_id, expected_content) = expected
                    .get(session_id)
                    .expect("canonical Speech belongs to this Session");
                let event = prepared
                    .remove(session_id)
                    .expect("prepared Speech for canonical projection");
                view.speeches.push(Speech {
                    event_id: event.id.to_hex(),
                    author_pubkey: agent_pubkey.clone(),
                    author_display_name: "Agent".to_string(),
                    content: expected_content.clone(),
                    created_at: event.created_at.as_secs(),
                    speech_revision: round_index as u64 + 1,
                    grant_id: grant_id.clone(),
                    mentions: Vec::new(),
                    handoff: None,
                });
                view.speech_cursor = Some(event.id.to_hex());
                view.baton.phase = "moderator_control".to_string();
                view.baton.state_revision = view.baton.state_revision.saturating_add(1);
                view.baton.speech_revision = round_index as u64 + 1;
                view.baton.state_event_id =
                    pubkey(200 + (round_index * SESSION_COUNT + expected.len()) as u8);
                view.baton.grant = None;
                view.baton.raw_state["phase"] = json!("moderator_control");
                view.baton.raw_state["state_revision"] = json!(view.baton.state_revision);
                view.baton.raw_state["speech_revision"] = json!(view.baton.speech_revision);

                coordinator.apply_view_to_ledger(view);
                let runtime = coordinator
                    .meetings
                    .get_mut(session_id)
                    .expect("active Meeting runtime");
                runtime.view = Some(view.clone());
                runtime.last_sync = Some(Instant::now());
                coordinator.reconcile(*session_id).await;

                let ledger = coordinator
                    .ledger_for(*session_id)
                    .expect("Session-scoped Meeting ledger");
                assert_eq!(ledger.grants[grant_id].state, "spoken");
                assert_eq!(ledger.reservations[offer_id].state, "released");
                assert!(
                    !ledger
                        .triggers
                        .contains_key(&format!("speech:{}", event.id.to_hex())),
                    "an Agent's own Speech must not trigger another Intent turn"
                );
            }
            assert!(coordinator.pending.is_empty());
        }

        assert_eq!(all_speech_event_ids.len(), SESSION_COUNT * ROUND_COUNT);
        for (session_id, _) in sessions {
            let ledger = coordinator
                .ledger_for(session_id)
                .expect("final Session ledger");
            assert_eq!(ledger.grants.len(), ROUND_COUNT);
            assert!(ledger
                .grants
                .values()
                .all(|record| record.state == "spoken"));
        }
    }

    #[tokio::test]
    async fn ten_agent_identities_observe_twenty_shared_rounds_without_duplicate_intent_turns() {
        const AGENT_COUNT: usize = 10;
        const ROUND_COUNT: usize = 20;

        let dir = tempfile::tempdir().expect("temp ledger directory");
        let session_id = Uuid::new_v4();
        let agent_keys: Vec<_> = (0..AGENT_COUNT).map(|_| Keys::generate()).collect();
        let agent_pubkeys: Vec<_> = agent_keys
            .iter()
            .map(|keys| keys.public_key().to_hex())
            .collect();
        let moderator_pubkey = Keys::generate().public_key().to_hex();
        let mut roster = BTreeMap::new();
        for (index, pubkey) in agent_pubkeys.iter().enumerate() {
            roster.insert(
                pubkey.clone(),
                Participant {
                    pubkey: pubkey.clone(),
                    role: "member".to_string(),
                    participant_type: "agent".to_string(),
                    display_name: format!("Agent {index}"),
                },
            );
        }
        roster.insert(
            moderator_pubkey.clone(),
            Participant {
                pubkey: moderator_pubkey.clone(),
                role: "moderator".to_string(),
                participant_type: "human".to_string(),
                display_name: "Human moderator".to_string(),
            },
        );
        let mut baton = baton_view();
        baton.moderator_pubkey = moderator_pubkey.clone();
        baton.raw_state["moderator_pubkey"] = json!(moderator_pubkey);
        let mut shared_view = MeetingView {
            session_id,
            title: "Ten-Agent stress meeting".to_string(),
            description: Some("Twenty canonical speech rounds".to_string()),
            ended: false,
            relay_pubkey: pubkey(10),
            roster,
            speeches: Vec::new(),
            intents: BTreeMap::new(),
            speech_cursor: None,
            baton,
        };

        let mut coordinators = Vec::with_capacity(AGENT_COUNT);
        for (index, keys) in agent_keys.into_iter().enumerate() {
            let mut coordinator = test_coordinator(
                keys,
                dir.path().join(format!("meeting-v1-ledger-{index}.json")),
                None,
            );
            coordinator.apply_view_to_ledger(&shared_view);
            let activation_id = format!("activation:{session_id}");
            coordinator
                .ledger_for_mut(session_id)
                .and_then(|ledger| ledger.triggers.get_mut(&activation_id))
                .expect("initial activation trigger")
                .state = "passed".to_string();
            coordinator
                .meetings
                .insert(session_id, runtime_with_view(1, shared_view.clone()));
            coordinators.push(coordinator);
        }

        for round_index in 0..ROUND_COUNT {
            let speaker_index = round_index % AGENT_COUNT;
            let event_id = pubkey(40 + round_index as u8);
            let grant_id = pubkey(80 + round_index as u8);
            let content = format!(
                "Round {} contribution from Agent {speaker_index}.",
                round_index + 1
            );
            shared_view.speeches.push(Speech {
                event_id: event_id.clone(),
                author_pubkey: agent_pubkeys[speaker_index].clone(),
                author_display_name: format!("Agent {speaker_index}"),
                content: content.clone(),
                created_at: round_index as u64 + 1,
                speech_revision: round_index as u64 + 1,
                grant_id,
                mentions: Vec::new(),
                handoff: None,
            });
            shared_view.speech_cursor = Some(event_id.clone());
            shared_view.baton.phase = "moderator_control".to_string();
            shared_view.baton.state_revision = shared_view.baton.state_revision.saturating_add(1);
            shared_view.baton.speech_revision = round_index as u64 + 1;
            shared_view.baton.state_event_id = pubkey(120 + round_index as u8);
            shared_view.baton.raw_state["phase"] = json!("moderator_control");
            shared_view.baton.raw_state["state_revision"] = json!(shared_view.baton.state_revision);
            shared_view.baton.raw_state["speech_revision"] =
                json!(shared_view.baton.speech_revision);

            for (agent_index, coordinator) in coordinators.iter_mut().enumerate() {
                coordinator.apply_view_to_ledger(&shared_view);
                let runtime = coordinator
                    .meetings
                    .get_mut(&session_id)
                    .expect("active shared Meeting runtime");
                runtime.view = Some(shared_view.clone());
                runtime.last_sync = Some(Instant::now());

                coordinator.reconcile(session_id).await;
                coordinator.reconcile(session_id).await;
                let trigger_id = format!("speech:{event_id}");
                if agent_index == speaker_index {
                    assert!(
                        coordinator.pending.is_empty(),
                        "the speaker must not react to its own Speech"
                    );
                    assert!(
                        !coordinator
                            .ledger_for(session_id)
                            .expect("speaker Meeting ledger")
                            .triggers
                            .contains_key(&trigger_id),
                        "the speaker must not create an Intent trigger for itself"
                    );
                    continue;
                }

                assert_eq!(
                    coordinator.pending.len(),
                    1,
                    "each observing Agent must own one semantic turn per Speech"
                );
                let request = coordinator.pop_pending().expect("observer Intent turn");
                assert_eq!(request.kind, MeetingTurnKind::V1Intent);
                assert_eq!(request.session_id, session_id);
                assert_eq!(request.basis_id, trigger_id);
                assert!(
                    request.prompt.contains(&content),
                    "every observer must receive the latest shared Speech"
                );

                let turn_id = format!("agent-{agent_index}-round-{round_index}");
                coordinator.mark_dispatched(turn_id.clone(), request.clone());
                coordinator.in_flight.remove(&turn_id);
                coordinator.in_flight_epochs.remove(&turn_id);
                coordinator
                    .meetings
                    .get_mut(&session_id)
                    .expect("observer Meeting runtime")
                    .in_flight_turn = None;
                coordinator
                    .handle_intent_result(
                        &turn_id,
                        &request,
                        r#"{"action":"PASS","summary":null,"addressed_to":null}"#,
                        true,
                    )
                    .await;
                coordinator.reconcile(session_id).await;
                coordinator.reconcile(session_id).await;
                assert!(
                    coordinator.pending.is_empty(),
                    "a completed semantic trigger must not queue twice"
                );
                assert_eq!(
                    coordinator
                        .ledger_for(session_id)
                        .and_then(|ledger| ledger.triggers.get(&trigger_id))
                        .map(|trigger| trigger.state.as_str()),
                    Some("passed")
                );
                assert!(coordinator.protocol_in_flight.is_empty());
            }
        }

        for (agent_index, coordinator) in coordinators.iter().enumerate() {
            let ledger = coordinator
                .ledger_for(session_id)
                .expect("final identity-scoped Meeting ledger");
            let authored_speeches = (0..ROUND_COUNT)
                .filter(|round_index| round_index % AGENT_COUNT == agent_index)
                .count();
            assert_eq!(ledger.seen_speech_ids.len(), ROUND_COUNT);
            assert_eq!(
                ledger.triggers.len(),
                1 + ROUND_COUNT - authored_speeches,
                "the ledger must contain one activation plus one trigger per foreign Speech"
            );
            assert!(ledger.triggers.values().all(|trigger| {
                trigger.state == "passed" || trigger.trigger_id.starts_with("activation:")
            }));
        }
    }

    #[tokio::test]
    async fn completed_participant_turn_prepares_and_submits_intent_asynchronously() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let trigger_id = "activation:intent-submission".to_string();
        let (rest, mut request_started, release, server) =
            gated_rest_responding_to(keys.clone(), 1).await;
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.rest = rest;
        let view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view));
        coordinator.ensure_meeting_ledger(session_id);
        let mut trigger = TriggerRecord::new(trigger_id.clone(), None, 0);
        trigger.state = "running".to_string();
        coordinator
            .ledger_for_mut(session_id)
            .expect("participant Meeting ledger")
            .triggers
            .insert(trigger_id.clone(), trigger);
        let mut request = granted_turn_request(session_id, &pubkey(30));
        request.kind = MeetingTurnKind::V1Intent;
        request.basis_id = trigger_id.clone();
        request.round_number = 0;
        request.grant_event_id = None;

        coordinator
            .handle_intent_result(
                "participant-intent-turn",
                &request,
                r#"{"action":"SUBMIT","summary":"Surface the dependency risk.","addressed_to":null}"#,
                true,
            )
            .await;

        tokio::time::timeout(Duration::from_secs(1), request_started.recv())
            .await
            .expect("Intent submission must start without blocking the coordinator")
            .expect("Intent submission observer remains open");
        let trigger = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.triggers.get(&trigger_id))
            .expect("durable prepared Intent");
        assert_eq!(trigger.state, "prepared");
        assert!(trigger.prepared_event.is_some());
        assert!(trigger.prepared_event_id.is_some());
        assert!(coordinator
            .protocol_in_flight
            .contains_key(&ProtocolSubmissionKey::Intent {
                session_id,
                trigger_id,
            }));

        release.send(true).expect("release Intent response");
        server.await.expect("join Intent responder");
    }

    #[test]
    fn remove_preserves_prepared_ack_decline_speech_and_yield_events() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let ack_offer_id = pubkey(61);
        let decline_offer_id = pubkey(62);
        let speech_grant_id = pubkey(63);
        let yield_grant_id = pubkey(64);

        let ack_event = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
            session_id,
            offer_id: &ack_offer_id,
        })
        .expect("build prepared ACK")
        .sign_with_keys(&keys)
        .expect("sign prepared ACK");
        let decline_event = buzz_sdk::build_meeting_v1_offer_decline(MeetingV1OfferDeclineParams {
            session_id,
            offer_id: &decline_offer_id,
            reason: Some("No local capacity"),
        })
        .expect("build prepared Decline")
        .sign_with_keys(&keys)
        .expect("sign prepared Decline");
        let speech_event = buzz_sdk::build_meeting_v1_speech(MeetingV1SpeechParams {
            session_id,
            grant_id: &speech_grant_id,
            speech_revision: 1,
            content: "Prepared speech must survive a transient unregister.",
            mentions: &[],
            handoff: None,
        })
        .expect("build prepared Speech")
        .sign_with_keys(&keys)
        .expect("sign prepared Speech");
        let yield_event = buzz_sdk::build_meeting_v1_grant_yield(MeetingV1GrantYieldParams {
            session_id,
            grant_id: &yield_grant_id,
            reason_code: Some(MeetingV1GrantYieldReason::NoLongerNeeded),
            reason: Some("Prepared Yield must survive a transient unregister."),
        })
        .expect("build prepared Yield")
        .sign_with_keys(&keys)
        .expect("sign prepared Yield");

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.meetings.insert(
            session_id,
            runtime_with_view(1, meeting_view(session_id, &agent_pubkey, &other_pubkey)),
        );
        coordinator.ensure_meeting_ledger(session_id);
        let ledger = coordinator
            .ledger_for_mut(session_id)
            .expect("Meeting ledger");
        ledger.reservations.insert(
            ack_offer_id.clone(),
            ReservationRecord {
                offer_id: ack_offer_id.clone(),
                state: "ack_prepared".to_string(),
                ack_event: Some(serde_json::to_value(&ack_event).expect("serialize prepared ACK")),
                decline_event: None,
                created_at_ms: now_ms(),
                capacity_expires_at_ms: now_ms() + 300_000,
            },
        );
        ledger.reservations.insert(
            decline_offer_id.clone(),
            ReservationRecord {
                offer_id: decline_offer_id.clone(),
                state: "decline_prepared".to_string(),
                ack_event: None,
                decline_event: Some(
                    serde_json::to_value(&decline_event).expect("serialize prepared Decline"),
                ),
                created_at_ms: now_ms(),
                capacity_expires_at_ms: now_ms() + 300_000,
            },
        );
        let mut speech_record =
            test_grant_record(&test_grant(&agent_pubkey, &speech_grant_id, &ack_offer_id));
        speech_record.state = "speech_prepared".to_string();
        speech_record.speech_event =
            Some(serde_json::to_value(&speech_event).expect("serialize prepared Speech"));
        speech_record.speech_event_id = Some(speech_event.id.to_hex());
        ledger.grants.insert(speech_grant_id.clone(), speech_record);
        let mut yield_record = test_grant_record(&test_grant(
            &agent_pubkey,
            &yield_grant_id,
            &decline_offer_id,
        ));
        yield_record.state = "yield_prepared".to_string();
        yield_record.yield_event =
            Some(serde_json::to_value(&yield_event).expect("serialize prepared Yield"));
        ledger.grants.insert(yield_grant_id.clone(), yield_record);

        coordinator.remove(session_id);

        assert!(!coordinator.meetings.contains_key(&session_id));
        assert_eq!(
            coordinator.active_reservation_count(None),
            0,
            "an unregistered Session must not hold live pool capacity"
        );
        let ledger = coordinator.ledger_for(session_id).expect("durable ledger");
        assert_eq!(ledger.reservations[&ack_offer_id].state, "ack_prepared");
        assert_eq!(
            ledger.reservations[&decline_offer_id].state,
            "decline_prepared"
        );
        assert_eq!(ledger.grants[&speech_grant_id].state, "speech_prepared");
        assert_eq!(ledger.grants[&yield_grant_id].state, "yield_prepared");
        assert_eq!(
            ledger.reservations[&ack_offer_id]
                .ack_event
                .as_ref()
                .and_then(serialized_event_id),
            Some(ack_event.id.to_hex())
        );
        assert_eq!(
            ledger.reservations[&decline_offer_id]
                .decline_event
                .as_ref()
                .and_then(serialized_event_id),
            Some(decline_event.id.to_hex())
        );
        assert_eq!(
            ledger.grants[&speech_grant_id]
                .speech_event
                .as_ref()
                .and_then(serialized_event_id),
            Some(speech_event.id.to_hex())
        );
        assert_eq!(
            ledger.grants[&yield_grant_id]
                .yield_event
                .as_ref()
                .and_then(serialized_event_id),
            Some(yield_event.id.to_hex())
        );
    }

    #[tokio::test]
    async fn authoritative_end_tears_down_runtime_ledger_and_all_session_work() {
        const PRIVATE_CONTENT: &str = "PRIVATE_TERMINAL_MEETING_CONTENT";
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let ledger_path = dir.path().join("meeting-v1-ledger.json");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let grant_id = pubkey(70);
        let offer_id = pubkey(71);
        let trigger_id = "speech:terminal".to_string();
        let turn_id = "terminal-granted-turn".to_string();
        let mut coordinator = test_coordinator(keys, ledger_path.clone(), None);

        let mut active_view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        active_view.speeches.push(Speech {
            event_id: pubkey(72),
            author_pubkey: other_pubkey.clone(),
            author_display_name: "Human".to_string(),
            content: PRIVATE_CONTENT.to_string(),
            created_at: 1,
            speech_revision: 1,
            grant_id: pubkey(73),
            mentions: Vec::new(),
            handoff: None,
        });
        let mut runtime = runtime_with_view(7, active_view);
        runtime.last_sync = Some(Instant::now() - SYNC_INTERVAL);
        runtime.queued = true;
        runtime.in_flight_turn = Some(turn_id.clone());
        runtime.control_retry_at = Some(Instant::now());
        coordinator.meetings.insert(session_id, runtime);
        coordinator.ensure_meeting_ledger(session_id);

        let mut trigger = TriggerRecord::new(trigger_id.clone(), Some(pubkey(72)), 1);
        trigger.state = "prepared".to_string();
        trigger.prepared_event = Some(json!({
            "content": PRIVATE_CONTENT,
            "summary": PRIVATE_CONTENT,
        }));
        trigger.prepared_event_id = Some(pubkey(74));
        let grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
        let mut grant_record = test_grant_record(&grant);
        grant_record.state = "speech_prepared".to_string();
        grant_record.speech_event = Some(json!({ "content": PRIVATE_CONTENT }));
        grant_record.speech_event_id = Some(pubkey(75));
        grant_record.yield_event = Some(json!({ "reason": PRIVATE_CONTENT }));
        let ledger = coordinator
            .ledger_for_mut(session_id)
            .expect("active Meeting ledger");
        ledger.meeting_synced = true;
        ledger.seen_speech_ids.insert(pubkey(72));
        ledger.triggers.insert(trigger_id.clone(), trigger);
        ledger.reservations.insert(
            offer_id.clone(),
            ReservationRecord {
                offer_id: offer_id.clone(),
                state: "ack_prepared".to_string(),
                ack_event: Some(json!({ "reason": PRIVATE_CONTENT })),
                decline_event: None,
                created_at_ms: now_ms(),
                capacity_expires_at_ms: now_ms() + 300_000,
            },
        );
        ledger.grants.insert(grant_id.clone(), grant_record);
        coordinator.persist_ledger_best_effort();
        assert!(
            std::fs::read_to_string(&ledger_path)
                .expect("read active ledger")
                .contains(PRIVATE_CONTENT),
            "the fixture must prove terminal compaction removes persisted private content"
        );

        let mut queued_request = granted_turn_request(session_id, &grant_id);
        queued_request.kind = MeetingTurnKind::V1Intent;
        queued_request.prompt = PRIVATE_CONTENT.to_string();
        queued_request.basis_id = trigger_id.clone();
        queued_request.grant_event_id = None;
        coordinator.pending.push_back(queued_request.clone());
        coordinator.deferred_turn_results.insert(
            session_id,
            DeferredTurnResult {
                request_id: 40,
                session_epoch: 7,
                turn_id: "deferred-terminal-turn".to_string(),
                request: queued_request,
                raw_output: PRIVATE_CONTENT.to_string(),
                succeeded: true,
            },
        );
        coordinator.protocol_in_flight.insert(
            ProtocolSubmissionKey::Intent {
                session_id,
                trigger_id,
            },
            ProtocolInFlight {
                session_epoch: 7,
                submission_id: 41,
                event_id: pubkey(76),
            },
        );
        coordinator.progress_in_flight.insert(
            (session_id, grant_id.clone()),
            ProgressInFlight {
                session_epoch: 7,
                submission_id: 42,
                event_id: pubkey(77),
            },
        );
        coordinator
            .progress_waiting_for_state
            .insert((session_id, grant_id.clone()), 43);
        coordinator
            .in_flight
            .insert(turn_id.clone(), granted_turn_request(session_id, &grant_id));
        coordinator.in_flight_epochs.insert(turn_id.clone(), 7);

        let ended_view = ended_meeting_view(session_id, &agent_pubkey, &other_pubkey, 2);
        assert_eq!(
            coordinator.apply_synced_view(session_id, ended_view),
            SyncApplyResult::Applied
        );

        assert!(!coordinator.meetings.contains_key(&session_id));
        assert!(coordinator.ledger_for(session_id).is_none());
        assert!(!coordinator
            .pending
            .iter()
            .any(|request| request.session_id == session_id));
        assert!(!coordinator.deferred_turn_results.contains_key(&session_id));
        assert!(!coordinator
            .protocol_in_flight
            .keys()
            .any(|key| key.session_id() == session_id));
        assert!(!coordinator
            .progress_in_flight
            .keys()
            .any(|(meeting_id, _)| *meeting_id == session_id));
        assert!(!coordinator
            .progress_waiting_for_state
            .keys()
            .any(|(meeting_id, _)| *meeting_id == session_id));
        assert_eq!(
            coordinator.take_preemptions(),
            vec![session_id],
            "terminal State must cancel even an in-flight Granted turn"
        );
        assert!(
            coordinator.in_flight.contains_key(&turn_id),
            "turn ownership remains until the cancellation completion is delivered"
        );

        let persisted = std::fs::read_to_string(&ledger_path).expect("read compacted ledger");
        assert!(!persisted.contains(PRIVATE_CONTENT));
        assert!(!load_ledger(&ledger_path)
            .expect("load compacted ledger")
            .meetings
            .contains_key(&session_id.to_string()));

        let sync_requests_before_tick = coordinator.next_sync_request_id;
        coordinator.tick().await;
        assert_eq!(
            coordinator.next_sync_request_id, sync_requests_before_tick,
            "a terminal runtime must not be scheduled for periodic sync"
        );

        coordinator
            .handle_turn_result(&turn_id, PRIVATE_CONTENT.to_string(), true)
            .await;
        assert!(!coordinator.in_flight.contains_key(&turn_id));
        assert!(!coordinator.in_flight_epochs.contains_key(&turn_id));
        assert!(!coordinator.meetings.contains_key(&session_id));
        assert!(coordinator.ledger_for(session_id).is_none());
        assert!(coordinator.pending.is_empty());
    }

    #[tokio::test]
    async fn terminal_ledger_cleanup_retries_after_failure_without_active_meetings() {
        const PRIVATE_CONTENT: &str = "PRIVATE_TERMINAL_RETRY_CONTENT";
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let ledger_path = dir.path().join("meeting-v1-ledger.json");
        let preserved_ledger_path = dir.path().join("meeting-v1-ledger.private");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut coordinator = test_coordinator(keys, ledger_path.clone(), None);

        assert!(coordinator.register_local(session_id));
        let mut trigger = TriggerRecord::new("terminal-retry".to_string(), Some(pubkey(78)), 1);
        trigger.state = "prepared".to_string();
        trigger.prepared_event = Some(json!({ "content": PRIVATE_CONTENT }));
        coordinator
            .ledger_for_mut(session_id)
            .expect("active Meeting ledger")
            .triggers
            .insert(trigger.trigger_id.clone(), trigger);
        persist_ledger(&ledger_path, &coordinator.ledger).expect("persist private active ledger");
        assert!(std::fs::read_to_string(&ledger_path)
            .expect("read private active ledger")
            .contains(PRIVATE_CONTENT));

        // Preserve the old durable file and make its configured destination a
        // directory so the terminal cleanup's atomic rename fails.
        std::fs::rename(&ledger_path, &preserved_ledger_path)
            .expect("preserve private ledger before injected failure");
        std::fs::create_dir(&ledger_path).expect("block atomic ledger replacement");

        let ended = ended_meeting_view(session_id, &agent_pubkey, &other_pubkey, 2);
        assert_eq!(
            coordinator.apply_synced_view(session_id, ended),
            SyncApplyResult::Applied
        );
        assert!(coordinator.meetings.is_empty());
        assert!(coordinator.ledger.meetings.is_empty());
        assert!(
            coordinator.terminal_ledger_cleanup_retry_at.is_some(),
            "a failed terminal cleanup must remain independently retryable"
        );
        assert!(
            std::fs::read_to_string(&preserved_ledger_path)
                .expect("read preserved private ledger")
                .contains(PRIVATE_CONTENT),
            "the injected failure must leave the old durable private content intact"
        );

        std::fs::remove_dir(&ledger_path).expect("remove atomic-replace blocker");
        std::fs::rename(&preserved_ledger_path, &ledger_path)
            .expect("restore old private ledger before retry");
        coordinator.terminal_ledger_cleanup_retry_at = Some(Instant::now());

        coordinator.tick().await;

        assert!(
            coordinator.terminal_ledger_cleanup_retry_at.is_none(),
            "a successful periodic retry must clear the cleanup marker"
        );
        assert!(coordinator.meetings.is_empty());
        let persisted =
            std::fs::read_to_string(&ledger_path).expect("read retried terminal ledger");
        assert!(!persisted.contains(PRIVATE_CONTENT));
        assert!(load_ledger(&ledger_path)
            .expect("load retried terminal ledger")
            .meetings
            .is_empty());
    }

    #[tokio::test]
    async fn terminal_teardown_does_not_cancel_a_running_moderator_decision() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        let kinds = [
            MeetingTurnKind::V1Intent,
            MeetingTurnKind::V1ModeratorControl,
            MeetingTurnKind::V1Granted,
        ];
        let mut cancelled_sessions = BTreeSet::new();
        let mut turns = Vec::new();

        for (index, kind) in kinds.into_iter().enumerate() {
            let session_id = Uuid::new_v4();
            let epoch = index as u64 + 1;
            let turn_id = format!("terminal-turn-{index}");
            let grant_id = pubkey(80 + index as u8);
            if kind != MeetingTurnKind::V1ModeratorControl {
                cancelled_sessions.insert(session_id);
            }
            turns.push(turn_id.clone());
            coordinator
                .meetings
                .insert(session_id, MeetingRuntime::new(epoch));
            coordinator.ensure_meeting_ledger(session_id);
            let mut request = granted_turn_request(session_id, &grant_id);
            request.kind = kind;
            if kind != MeetingTurnKind::V1Granted {
                request.grant_event_id = None;
            }
            coordinator.in_flight.insert(turn_id.clone(), request);
            coordinator.in_flight_epochs.insert(turn_id, epoch);

            coordinator.teardown_terminal_session(session_id);
            assert!(!coordinator.meetings.contains_key(&session_id));
            assert!(coordinator.ledger_for(session_id).is_none());
        }

        assert_eq!(
            coordinator
                .take_preemptions()
                .into_iter()
                .collect::<BTreeSet<_>>(),
            cancelled_sessions,
            "Meeting End cancels participant/granted work but not a running Moderator Decision"
        );
        for turn_id in turns {
            coordinator
                .handle_turn_result(&turn_id, "late terminal result".to_string(), true)
                .await;
        }
        assert!(coordinator.in_flight.is_empty());
        assert!(coordinator.in_flight_epochs.is_empty());
        assert!(coordinator.pending.is_empty());
        assert!(coordinator.ledger.meetings.is_empty());
    }

    #[tokio::test]
    async fn restarted_and_repeated_ended_registration_is_inert_and_tears_down_again() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let ledger_path = dir.path().join("meeting-v1-ledger.json");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();

        let mut original = test_coordinator(keys.clone(), ledger_path.clone(), None);
        assert!(original.register_local(session_id));
        let first_ended = ended_meeting_view(session_id, &agent_pubkey, &other_pubkey, 2);
        assert_eq!(
            original.apply_synced_view(session_id, first_ended),
            SyncApplyResult::Applied
        );
        assert!(!original.meetings.contains_key(&session_id));
        assert!(original.ledger_for(session_id).is_none());
        drop(original);

        let durable = load_ledger(&ledger_path).expect("load terminally compacted ledger");
        assert!(!durable.meetings.contains_key(&session_id.to_string()));
        let mut restarted = test_coordinator(keys, ledger_path, None);
        restarted.ledger = durable;

        for state_revision in [2, 3] {
            assert!(
                restarted.register_local(session_id),
                "terminal teardown must allow an idempotent detector re-registration"
            );
            assert!(restarted.meetings.contains_key(&session_id));
            assert!(restarted.ledger_for(session_id).is_some());
            let protocol_submissions_before = restarted.next_protocol_submission_id;
            let progress_submissions_before = restarted.next_progress_submission_id;
            let ended =
                ended_meeting_view(session_id, &agent_pubkey, &other_pubkey, state_revision);
            assert_eq!(
                restarted.apply_synced_view(session_id, ended),
                SyncApplyResult::Applied
            );
            assert!(!restarted.meetings.contains_key(&session_id));
            assert!(restarted.ledger_for(session_id).is_none());
            assert!(restarted.pending.is_empty());
            assert!(restarted.in_flight.is_empty());
            assert!(restarted.protocol_in_flight.is_empty());
            assert!(restarted.progress_in_flight.is_empty());
            assert!(restarted.preemptions.is_empty());
            assert_eq!(
                restarted.next_protocol_submission_id, protocol_submissions_before,
                "an ended registration must not emit a protocol action"
            );
            assert_eq!(
                restarted.next_progress_submission_id, progress_submissions_before,
                "an ended registration must not emit Progress"
            );

            let sync_requests_before_tick = restarted.next_sync_request_id;
            restarted.tick().await;
            assert_eq!(
                restarted.next_sync_request_id, sync_requests_before_tick,
                "repeated terminal teardown must remain absent from periodic sync"
            );
        }
    }

    #[tokio::test]
    async fn reregister_restores_exact_replay_and_rejects_old_epoch_completion() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let offer_session = Uuid::new_v4();
        let offer_id = pubkey(65);
        let grant_session = Uuid::new_v4();
        let grant_id = pubkey(66);
        let grant_offer_id = pubkey(67);

        let ack_event = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
            session_id: offer_session,
            offer_id: &offer_id,
        })
        .expect("build replayable ACK")
        .sign_with_keys(&keys)
        .expect("sign replayable ACK");
        let speech_event = buzz_sdk::build_meeting_v1_speech(MeetingV1SpeechParams {
            session_id: grant_session,
            grant_id: &grant_id,
            speech_revision: 1,
            content: "Replay this exact prepared speech.",
            mentions: &[],
            handoff: None,
        })
        .expect("build replayable Speech")
        .sign_with_keys(&keys)
        .expect("sign replayable Speech");

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.ensure_meeting_ledger(offer_session);
        coordinator
            .ledger_for_mut(offer_session)
            .expect("Offer ledger")
            .reservations
            .insert(
                offer_id.clone(),
                ReservationRecord {
                    offer_id: offer_id.clone(),
                    // Models a ledger written by the old remove behavior.
                    state: "released".to_string(),
                    ack_event: Some(
                        serde_json::to_value(&ack_event).expect("serialize replayable ACK"),
                    ),
                    decline_event: None,
                    created_at_ms: now_ms(),
                    capacity_expires_at_ms: now_ms() + 300_000,
                },
            );
        let offer_view = agent_offer_view(offer_session, &agent_pubkey, &other_pubkey, &offer_id);
        coordinator
            .meetings
            .insert(offer_session, MeetingRuntime::new(2));
        assert_eq!(
            coordinator.apply_synced_view(offer_session, offer_view.clone()),
            SyncApplyResult::Applied
        );
        assert_eq!(
            reservation_state(&coordinator, offer_session, &offer_id),
            Some("ack_prepared")
        );
        assert!(
            coordinator
                .retry_prepared_control(offer_session, &offer_view)
                .await
        );
        let offer_key = ProtocolSubmissionKey::Offer {
            session_id: offer_session,
            offer_id: offer_id.clone(),
        };
        let current = coordinator
            .protocol_in_flight
            .get(&offer_key)
            .expect("replayed ACK in flight")
            .clone();
        assert_eq!(current.session_epoch, 2);
        assert_eq!(current.event_id, ack_event.id.to_hex());

        coordinator
            .handle_protocol_result(ProtocolTaskResult {
                key: offer_key.clone(),
                session_epoch: 1,
                submission_id: current.submission_id,
                event_id: current.event_id.clone(),
                context: ProtocolSubmissionContext::Offer {
                    offer_id: offer_id.clone(),
                    action: OfferSubmissionAction::Ack,
                    allocation_source: "moderator_selection".to_string(),
                    turn_role: "participant".to_string(),
                    created_at_ms: now_ms(),
                },
                result: Ok(json!({ "accepted": true })),
            })
            .await;
        assert_eq!(
            coordinator.protocol_in_flight.get(&offer_key),
            Some(&current),
            "a completion from the removed epoch must not complete the replay"
        );
        assert_eq!(
            reservation_state(&coordinator, offer_session, &offer_id),
            Some("ack_prepared")
        );

        coordinator.ensure_meeting_ledger(grant_session);
        let grant = test_grant(&agent_pubkey, &grant_id, &grant_offer_id);
        let mut grant_record = test_grant_record(&grant);
        grant_record.state = "terminal".to_string();
        grant_record.speech_event =
            Some(serde_json::to_value(&speech_event).expect("serialize replayable Speech"));
        grant_record.speech_event_id = Some(speech_event.id.to_hex());
        coordinator
            .ledger_for_mut(grant_session)
            .expect("Grant ledger")
            .grants
            .insert(grant_id.clone(), grant_record);
        let mut grant_view = meeting_view(grant_session, &agent_pubkey, &other_pubkey);
        grant_view.baton.phase = "granted".to_string();
        grant_view.baton.grant = Some(grant.clone());
        coordinator
            .meetings
            .insert(grant_session, MeetingRuntime::new(3));
        assert_eq!(
            coordinator.apply_synced_view(grant_session, grant_view.clone()),
            SyncApplyResult::Applied
        );
        assert_eq!(
            coordinator
                .ledger_for(grant_session)
                .and_then(|ledger| ledger.grants.get(&grant_id))
                .map(|record| record.state.as_str()),
            Some("speech_prepared")
        );
        assert!(
            coordinator
                .retry_prepared_grant_terminal(grant_session, &grant_view, &grant)
                .await
        );
        let grant_key = ProtocolSubmissionKey::GrantTerminal {
            session_id: grant_session,
            grant_id: grant_id.clone(),
        };
        let replay = coordinator
            .protocol_in_flight
            .get(&grant_key)
            .expect("replayed Speech in flight");
        assert_eq!(replay.session_epoch, 3);
        assert_eq!(replay.event_id, speech_event.id.to_hex());
    }

    #[test]
    fn participant_intent_output_never_accepts_a_candidate_speech() {
        let submit = parse_intent_output(&format!(
            r#"{{"action":"SUBMIT","summary":"Surface the stale dependency risk.","addressed_to":"{}"}}"#,
            pubkey(2)
        ))
        .expect("valid V1 Intent");
        assert_eq!(submit.action, "SUBMIT");

        assert!(
            parse_intent_output(r#"{"action":"PASS","summary":null,"addressed_to":null}"#).is_ok()
        );
        assert!(parse_intent_output(
            r#"{"action":"SUBMIT","summary":"risk","addressed_to":null,"content":"full candidate speech"}"#
        )
        .is_err());
        assert!(parse_intent_output(
            r#"{"action":"PASS","summary":"I still want to speak","addressed_to":null}"#
        )
        .is_err());
    }

    #[test]
    fn granted_output_requires_exact_say_yield_and_handoff_shapes() {
        let say = format!(
            r#"{{"action":"SAY","content":"The dependency is stale.","mention_pubkeys":[],"handoff":{{"target_pubkey":"{}","handoff_type":"review","reason":"Please verify the mitigation."}},"reason":null}}"#,
            pubkey(2)
        );
        assert!(parse_granted_output(&say).is_ok());
        assert!(parse_granted_output(
            r#"{"action":"YIELD","content":null,"mention_pubkeys":[],"handoff":null,"reason":"Already covered."}"#
        )
        .is_ok());
        assert!(parse_granted_output(
            r#"{"action":"YIELD","content":"must not publish","mention_pubkeys":[],"handoff":null,"reason":"stale"}"#
        )
        .is_err());
        let invalid_handoff = format!(
            r#"{{"action":"SAY","content":"x","mention_pubkeys":[],"handoff":{{"target_pubkey":"{}","handoff_type":"delegate","reason":"x"}},"reason":null}}"#,
            pubkey(2)
        );
        assert!(parse_granted_output(&invalid_handoff).is_err());
    }

    #[test]
    fn relay_state_validation_pins_v2_tags_and_active_object_invariants() {
        let meeting_id = Uuid::new_v4();
        let state = base_state();
        let content = serde_json::to_string(&json!({
            "phase": state.phase,
            "state_revision": state.state_revision,
            "floor_revision": state.floor_revision,
            "intent_revision": state.intent_revision,
            "speech_revision": state.speech_revision,
            "control_epoch": state.control_epoch,
            "decision_epoch": state.decision_epoch,
            "moderator_pubkey": state.moderator_pubkey,
            "baton_config": state.baton_config,
            "participants": [{
                "pubkey": pubkey(1),
                "participant_type": "agent"
            }],
            "pending_intents": [],
            "unresolved_handoffs": [],
            "offer": null,
            "grant": null
        }))
        .expect("serialize State");
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::Custom(KIND_MEETING_ROUND_STATE as u16), content)
            .tags([
                Tag::parse(["h", &meeting_id.to_string()]).expect("h"),
                Tag::parse(["v", "2"]).expect("v"),
                Tag::parse(["policy", "moderated-baton-v1"]).expect("policy"),
                Tag::parse(["phase", "moderator_idle"]).expect("phase"),
                Tag::parse(["floor-revision", "1"]).expect("floor"),
                Tag::parse(["intent-revision", "0"]).expect("intent"),
                Tag::parse(["speech-revision", "0"]).expect("speech"),
                Tag::parse(["state-revision", "1"]).expect("state"),
                Tag::parse(["moderator", &pubkey(1)]).expect("moderator"),
            ])
            .sign_with_keys(&keys)
            .expect("sign State");
        assert!(validate_baton_state_event(&event, meeting_id, &state).is_ok());

        let mut invalid = base_state();
        invalid.phase = "offered".to_string();
        assert!(validate_baton_state_event(&event, meeting_id, &invalid).is_err());
    }

    #[test]
    fn directed_handoff_trigger_waits_while_offer_or_grant_is_active() {
        let mut baton = baton_view();
        let handoff_id = pubkey(7);
        baton.offer = Some(OfferView {
            offer_id: pubkey(8),
            target_pubkey: pubkey(2),
            target_participant_type: "agent".to_string(),
            allocation_source: "directed_handoff".to_string(),
            turn_role: "participant".to_string(),
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: Some(handoff_id.clone()),
            source_speech_event_id: Some(handoff_id.clone()),
            handoff_context: None,
            created_at_ms: 1,
            ack_deadline_ms: 2,
        });
        assert!(baton_has_active_handoff_attempt(&baton, &handoff_id));
        baton.offer = None;
        assert!(!baton_has_active_handoff_attempt(&baton, &handoff_id));
    }

    #[test]
    fn stale_grant_does_not_reduce_another_meeting_reservation_but_matching_grant_does() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let reserved_session = Uuid::new_v4();
        let stale_session = Uuid::new_v4();
        let offer_id = pubkey(11);
        let grant_id = pubkey(12);
        let stale_grant_id = pubkey(13);
        let grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);

        coordinator.ensure_meeting_ledger(reserved_session);
        let ledger = coordinator
            .ledger_for_mut(reserved_session)
            .expect("reserved Meeting ledger");
        ledger.reservations.insert(
            offer_id.clone(),
            ReservationRecord {
                offer_id: offer_id.clone(),
                state: "ack_sent".to_string(),
                ack_event: None,
                decline_event: None,
                created_at_ms: now_ms(),
                capacity_expires_at_ms: now_ms() + 300_000,
            },
        );
        ledger
            .grants
            .insert(grant_id.clone(), test_grant_record(&grant));

        let mut reserved_view = meeting_view(reserved_session, &agent_pubkey, &other_pubkey);
        reserved_view.baton.phase = "granted".to_string();
        reserved_view.baton.grant = Some(grant.clone());
        coordinator
            .meetings
            .insert(reserved_session, runtime_with_view(1, reserved_view));
        coordinator.meetings.insert(
            stale_session,
            runtime_with_view(2, meeting_view(stale_session, &agent_pubkey, &other_pubkey)),
        );

        let stale_request = granted_turn_request(stale_session, &stale_grant_id);
        assert!(!coordinator.granted_request_uses_active_reservation(&stale_request));
        coordinator
            .in_flight
            .insert("stale-grant-turn".to_string(), stale_request);
        assert_eq!(
            coordinator.unassigned_reserved_slots(),
            1,
            "a canonically stale Grant must not consume another Meeting's reservation"
        );

        coordinator.in_flight.clear();
        let matching_request = granted_turn_request(reserved_session, &grant_id);
        assert!(coordinator.granted_request_uses_active_reservation(&matching_request));
        coordinator
            .in_flight
            .insert("matching-grant-turn".to_string(), matching_request);
        assert_eq!(
            coordinator.unassigned_reserved_slots(),
            0,
            "a running Grant may consume only its own active reservation"
        );
    }

    #[test]
    fn background_sync_rejects_relay_signer_rotation_without_mutating_the_view() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let initial = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        let pinned_signer = initial.relay_pubkey.clone();
        let pinned_revision = initial.baton.state_revision;
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, initial));

        let mut rotated = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        rotated.relay_pubkey = pubkey(14);
        rotated.baton.state_revision = pinned_revision + 1;
        rotated.baton.state_event_id = pubkey(15);

        assert!(matches!(
            coordinator.apply_synced_view(session_id, rotated),
            SyncApplyResult::Failed
        ));
        let current = coordinator
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.view.as_ref())
            .expect("pinned Meeting view");
        assert_eq!(current.relay_pubkey, pinned_signer);
        assert_eq!(current.baton.state_revision, pinned_revision);
    }

    #[tokio::test]
    async fn deferred_turn_result_blocks_a_second_semantic_turn() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let grant_id = pubkey(16);
        let offer_id = pubkey(17);
        let grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
        let mut view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        view.baton.phase = "granted".to_string();
        view.baton.grant = Some(grant);
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.apply_view_to_ledger(&view);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view));

        let request = granted_turn_request(session_id, &grant_id);
        coordinator.deferred_turn_results.insert(
            session_id,
            DeferredTurnResult {
                request_id: 1,
                session_epoch: 1,
                turn_id: "completed-model-turn".to_string(),
                request,
                raw_output: r#"{"action":"YIELD","content":null,"mention_pubkeys":[],"handoff":null,"reason":"covered"}"#.to_string(),
                succeeded: true,
            },
        );

        coordinator.reconcile(session_id).await;
        assert!(coordinator.pending.is_empty());
        assert!(
            !coordinator
                .meetings
                .get(&session_id)
                .expect("Meeting runtime")
                .queued
        );

        coordinator.deferred_turn_results.remove(&session_id);
        coordinator.reconcile(session_id).await;
        assert_eq!(coordinator.pending.len(), 1);
        assert_eq!(coordinator.front_kind(), Some(MeetingTurnKind::V1Granted));
    }

    #[tokio::test]
    async fn old_epoch_turn_blocks_duplicate_after_immediate_reregister() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let grant_id = pubkey(18);
        let offer_id = pubkey(19);
        let grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
        let mut view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        view.baton.phase = "granted".to_string();
        view.baton.grant = Some(grant);
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.apply_view_to_ledger(&view);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(2, view));

        coordinator.in_flight.insert(
            "old-epoch-granted-turn".to_string(),
            granted_turn_request(session_id, &grant_id),
        );
        coordinator.reconcile(session_id).await;
        assert!(
            coordinator.pending.is_empty(),
            "the replacement runtime must wait for the cancelled old-epoch model turn"
        );

        coordinator.in_flight.remove("old-epoch-granted-turn");
        coordinator.reconcile(session_id).await;
        assert_eq!(coordinator.pending.len(), 1);
        assert_eq!(coordinator.front_kind(), Some(MeetingTurnKind::V1Granted));
    }

    #[test]
    fn progress_completion_requires_current_submission_and_prepared_event() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let session_id = Uuid::new_v4();
        let grant_id = pubkey(18);
        let offer_id = pubkey(19);
        let current_event =
            buzz_sdk::build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
                session_id,
                grant_id: &grant_id,
                progress_seq: 2,
                stage: MeetingV1ProgressStage::Generating,
            })
            .expect("build current Progress")
            .sign_with_keys(&keys)
            .expect("sign current Progress");
        let old_event = buzz_sdk::build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
            session_id,
            grant_id: &grant_id,
            progress_seq: 1,
            stage: MeetingV1ProgressStage::ContextSync,
        })
        .expect("build old Progress")
        .sign_with_keys(&keys)
        .expect("sign old Progress");
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.ensure_meeting_ledger(session_id);
        let grant = test_grant(&agent_pubkey, &grant_id, &offer_id);
        let mut record = test_grant_record(&grant);
        let original_deadline = record.next_progress_at_ms;
        record.prepared_progress = Some(PreparedProgress {
            seq: 2,
            event: serde_json::to_value(&current_event).expect("serialize current Progress"),
            state: "prepared".to_string(),
        });
        coordinator
            .ledger_for_mut(session_id)
            .expect("Progress Meeting ledger")
            .grants
            .insert(grant_id.clone(), record);

        let in_flight_key = (session_id, grant_id.clone());
        let current_event_id = current_event.id.to_hex();
        coordinator.progress_in_flight.insert(
            in_flight_key.clone(),
            ProgressInFlight {
                session_epoch: 0,
                submission_id: 22,
                event_id: current_event_id.clone(),
            },
        );

        coordinator.handle_progress_result(ProgressTaskResult {
            session_id,
            session_epoch: 0,
            grant_id: grant_id.clone(),
            submission_id: 21,
            event_id: current_event_id.clone(),
            progress_seq: 2,
            stage: MeetingV1ProgressStage::Generating,
            result: Ok(json!({ "accepted": true })),
        });
        let in_flight = coordinator
            .progress_in_flight
            .get(&in_flight_key)
            .expect("current Progress remains in flight");
        assert_eq!(in_flight.submission_id, 22);
        assert_eq!(in_flight.event_id, current_event_id);

        coordinator.handle_progress_result(ProgressTaskResult {
            session_id,
            session_epoch: 0,
            grant_id: grant_id.clone(),
            submission_id: 22,
            event_id: old_event.id.to_hex(),
            progress_seq: 2,
            stage: MeetingV1ProgressStage::Generating,
            result: Ok(json!({ "accepted": true })),
        });
        let in_flight = coordinator
            .progress_in_flight
            .get(&in_flight_key)
            .expect("different prepared event must not complete Progress");
        assert_eq!(in_flight.submission_id, 22);
        assert_eq!(in_flight.event_id, current_event_id);
        let prepared = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.grants.get(&grant_id))
            .and_then(|record| record.prepared_progress.as_ref())
            .expect("current prepared Progress");
        assert_eq!(prepared.state, "prepared");
        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.grants.get(&grant_id))
                .map(|record| record.next_progress_at_ms),
            Some(original_deadline)
        );

        coordinator.handle_progress_result(ProgressTaskResult {
            session_id,
            session_epoch: 0,
            grant_id: grant_id.clone(),
            submission_id: 22,
            event_id: current_event_id,
            progress_seq: 2,
            stage: MeetingV1ProgressStage::Generating,
            result: Ok(json!({ "accepted": true })),
        });
        assert!(!coordinator.progress_in_flight.contains_key(&in_flight_key));
        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.grants.get(&grant_id))
                .and_then(|record| record.prepared_progress.as_ref())
                .map(|prepared| prepared.state.as_str()),
            Some("sent")
        );
    }

    #[test]
    fn speech_is_not_exposed_until_relay_state_covers_its_revision() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.ensure_meeting_ledger(session_id);
        let ledger = coordinator
            .ledger_for_mut(session_id)
            .expect("test Meeting ledger");
        ledger.meeting_synced = true;
        ledger.triggers.clear();

        let first_speech_id = pubkey(20);
        let mut view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        view.speeches.push(Speech {
            event_id: first_speech_id.clone(),
            author_pubkey: other_pubkey.clone(),
            author_display_name: "Human".to_string(),
            content: "A speech whose State transition has not arrived yet.".to_string(),
            created_at: 1,
            speech_revision: 1,
            grant_id: pubkey(21),
            mentions: Vec::new(),
            handoff: None,
        });

        // Live speech can arrive before its Relay State. It must not be marked
        // seen or create a semantic trigger while authoritative revision is 0.
        coordinator.apply_view_to_ledger(&view);
        let ledger = coordinator
            .ledger_for(session_id)
            .expect("test Meeting ledger");
        assert!(!ledger.seen_speech_ids.contains(&first_speech_id));
        assert!(!ledger
            .triggers
            .contains_key(&format!("speech:{first_speech_id}")));

        // Once State covers revision 1, the exact same speech becomes visible.
        view.baton.speech_revision = 1;
        view.baton.state_revision = 2;
        view.speech_cursor = Some(first_speech_id.clone());
        coordinator.apply_view_to_ledger(&view);
        let ledger = coordinator
            .ledger_for(session_id)
            .expect("test Meeting ledger");
        assert!(ledger.seen_speech_ids.contains(&first_speech_id));
        assert!(ledger
            .triggers
            .contains_key(&format!("speech:{first_speech_id}")));
        assert!(speech_projection_complete(&view));

        // State may also arrive before all canonical speech history. A missing
        // revision keeps the semantic controller behind the projection barrier.
        view.baton.speech_revision = 2;
        view.baton.state_revision = 3;
        assert!(!speech_projection_complete(&view));
        view.speeches.push(Speech {
            event_id: pubkey(22),
            author_pubkey: other_pubkey,
            author_display_name: "Human".to_string(),
            content: "The missing second canonical speech.".to_string(),
            created_at: 2,
            speech_revision: 2,
            grant_id: pubkey(23),
            mentions: Vec::new(),
            handoff: None,
        });
        assert!(speech_projection_complete(&view));
    }

    #[tokio::test]
    async fn directed_speech_before_state_acks_offer_without_duplicate_voluntary_intent() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let speech_id = pubkey(24);
        let offer_id = pubkey(25);
        let (rest, mut request_started, release, server) =
            gated_rest_responding_to(keys.clone(), 1).await;
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.rest = rest;

        let mut initial = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, initial.clone()));
        coordinator.ensure_meeting_ledger(session_id);
        let ledger = coordinator
            .ledger_for_mut(session_id)
            .expect("test Meeting ledger");
        ledger.meeting_synced = true;
        ledger.triggers.clear();

        let directed_speech = Speech {
            event_id: speech_id.clone(),
            author_pubkey: other_pubkey.clone(),
            author_display_name: "Human".to_string(),
            content: "Please review this result.".to_string(),
            created_at: 1,
            speech_revision: 1,
            grant_id: pubkey(26),
            mentions: vec![agent_pubkey.clone()],
            handoff: Some(SpeechHandoff {
                target_pubkey: agent_pubkey.clone(),
                handoff_type: "review".to_string(),
                reason: "Confirm the evidence.".to_string(),
            }),
        };

        // The canonical speech can fan out before the Relay-signed State that
        // atomically creates its directed-Handoff Offer.
        initial.speeches.push(directed_speech);
        initial.speech_cursor = Some(speech_id.clone());
        coordinator.apply_view_to_ledger(&initial);
        let ledger = coordinator
            .ledger_for(session_id)
            .expect("ledger before authoritative State");
        assert!(!ledger.seen_speech_ids.contains(&speech_id));
        assert!(!ledger.triggers.contains_key(&format!("speech:{speech_id}")));

        let mut authoritative = initial;
        authoritative.baton.phase = "offered".to_string();
        authoritative.baton.state_revision = 2;
        authoritative.baton.speech_revision = 1;
        authoritative.baton.offer = Some(OfferView {
            offer_id: offer_id.clone(),
            target_pubkey: agent_pubkey.clone(),
            target_participant_type: "agent".to_string(),
            allocation_source: "directed_handoff".to_string(),
            turn_role: "participant".to_string(),
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: Some(speech_id.clone()),
            source_speech_event_id: Some(speech_id.clone()),
            handoff_context: Some(HandoffContextView {
                from_pubkey: other_pubkey.clone(),
                reason_type: "review".to_string(),
                reason_text: "Confirm the evidence.".to_string(),
            }),
            created_at_ms: now_ms(),
            ack_deadline_ms: now_ms() + 30_000,
        });
        authoritative.baton.unresolved_handoffs = vec![OpenHandoffView {
            handoff_id: speech_id.clone(),
            source_speech_event_id: speech_id.clone(),
            from_pubkey: other_pubkey,
            to_pubkey: agent_pubkey,
            reason_type: "review".to_string(),
            reason_text: "Confirm the evidence.".to_string(),
            question_state: "open".to_string(),
            attempt_count: 1,
            last_offer_id: Some(offer_id.clone()),
            last_grant_id: None,
            last_attempt_outcome: Some("offered".to_string()),
            blocked_by: None,
            moderator_retry_blocked: false,
            eligible_decision_epoch: 0,
        }];
        coordinator
            .meetings
            .get_mut(&session_id)
            .expect("registered Meeting runtime")
            .view = Some(authoritative.clone());
        coordinator.apply_view_to_ledger(&authoritative);
        coordinator.reconcile(session_id).await;

        tokio::time::timeout(Duration::from_secs(1), request_started.recv())
            .await
            .expect("deterministic ACK submission must start")
            .expect("ACK submission observer remains open");
        let ledger = coordinator
            .ledger_for(session_id)
            .expect("ledger after authoritative State");
        assert!(ledger.seen_speech_ids.contains(&speech_id));
        assert!(!ledger.triggers.contains_key(&format!("speech:{speech_id}")));
        assert!(!ledger
            .triggers
            .contains_key(&format!("handoff:{speech_id}")));
        assert_eq!(
            reservation_state(&coordinator, session_id, &offer_id),
            Some("ack_prepared")
        );
        assert!(
            coordinator
                .pending
                .iter()
                .all(|request| request.kind != MeetingTurnKind::V1Intent),
            "the directed speech must produce an ACK, never a duplicate voluntary Intent"
        );

        release.send(true).expect("release ACK response");
        server.await.expect("join ACK responder");
    }

    #[test]
    fn first_progress_deadline_moves_before_a_near_soft_expiry() {
        let now = 1_000_000_i64;

        assert_eq!(
            next_progress_deadline(now, now + 60_000, 10_000),
            now + 10_000,
            "ordinary Grants use the configured Progress interval"
        );
        assert_eq!(
            next_progress_deadline(now, now + 2_500, 10_000),
            now + 1_500,
            "the first Progress must be due one second before a near soft expiry"
        );
        assert_eq!(
            next_progress_deadline(now, now + 750, 10_000),
            now,
            "less than one second of lease headroom requires immediate Progress"
        );
        assert_eq!(
            next_progress_deadline(now, now + 60_000, 100),
            now + 1_000,
            "malformed sub-second intervals are clamped"
        );
    }

    #[test]
    fn intent_context_follows_prev_chain_instead_of_event_order() {
        let session_id = Uuid::new_v4();
        let author = Keys::generate();
        let other = Keys::generate();
        let author_pubkey = author.public_key().to_hex();
        let other_pubkey = other.public_key().to_hex();
        let timestamp = Timestamp::from(1_700_000_000_u64);

        let submit = buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
            session_id,
            basis_speech_revision: 0,
            addressed_to: None,
            summary: "Original summary that must be replaced.",
        })
        .expect("build initial Intent")
        .custom_created_at(timestamp)
        .sign_with_keys(&author)
        .expect("sign initial Intent");
        let refresh_one = buzz_sdk::build_meeting_v1_intent_refresh(MeetingV1IntentRefreshParams {
            session_id,
            intent_id: &submit.id.to_hex(),
            previous_event_id: &submit.id.to_hex(),
            basis_speech_revision: 1,
            addressed_to: None,
            summary: "Intermediate summary that must also be replaced.",
        })
        .expect("build first Intent Refresh")
        .custom_created_at(timestamp)
        .sign_with_keys(&author)
        .expect("sign first Intent Refresh");
        let refresh_two = buzz_sdk::build_meeting_v1_intent_refresh(MeetingV1IntentRefreshParams {
            session_id,
            intent_id: &submit.id.to_hex(),
            previous_event_id: &refresh_one.id.to_hex(),
            basis_speech_revision: 2,
            addressed_to: Some(&other_pubkey),
            summary: "Latest summary selected through the prev chain.",
        })
        .expect("build second Intent Refresh")
        .custom_created_at(timestamp)
        .sign_with_keys(&author)
        .expect("sign second Intent Refresh");

        let roster = BTreeMap::from([
            (
                author_pubkey.clone(),
                Participant {
                    pubkey: author_pubkey.clone(),
                    role: "member".to_string(),
                    participant_type: "agent".to_string(),
                    display_name: "Agent".to_string(),
                },
            ),
            (
                other_pubkey.clone(),
                Participant {
                    pubkey: other_pubkey.clone(),
                    role: "member".to_string(),
                    participant_type: "human".to_string(),
                    display_name: "Human".to_string(),
                },
            ),
        ]);
        // Deliberately reverse the accepted Refresh chain. All events also have
        // the same Nostr timestamp, so timestamp/order cannot select the winner.
        let contexts = collect_intent_contexts(
            &[refresh_two.clone(), submit.clone(), refresh_one],
            session_id,
            &roster,
        );
        let intent_id = submit.id.to_hex();
        let current = contexts.get(&intent_id).expect("resolved Intent context");
        assert_eq!(current.current_event_id, refresh_two.id.to_hex());
        assert_eq!(
            current.summary,
            "Latest summary selected through the prev chain."
        );
        assert_eq!(current.addressed_to.as_deref(), Some(other_pubkey.as_str()));
        assert_eq!(current.basis_speech_revision, 2);

        let mut view = meeting_view(session_id, &author_pubkey, &other_pubkey);
        view.roster = roster;
        view.intents = contexts;
        let grant = GrantView {
            grant_id: pubkey(30),
            holder_pubkey: author_pubkey,
            allocation_source: "moderator_selection".to_string(),
            turn_role: "participant".to_string(),
            source_offer_id: pubkey(31),
            source_intent_id: Some(intent_id.clone()),
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            basis_speech_revision: 2,
            soft_lease_expires_at_ms: now_ms() + 30_000,
            hard_deadline_ms: now_ms() + 300_000,
            progress_seq: 0,
        };
        assert!(grant_context_complete(&view, &grant));
        let prompt = build_granted_prompt(&view, &grant, &intent_id);
        assert!(prompt.contains("Latest summary selected through the prev chain."));
        assert!(prompt.contains(&refresh_two.id.to_hex()));
        assert!(!prompt.contains("Original summary that must be replaced."));
        assert!(!prompt.contains("Intermediate summary that must also be replaced."));
    }

    #[test]
    fn ledger_file_round_trip_and_recovery_preserve_signed_event_ids() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let path = dir.path().join("meeting-v1-ledger.json");
        let session_id = Uuid::new_v4();
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let offer_id = pubkey(40);
        let speech_grant_id = pubkey(41);
        let yield_grant_id = pubkey(42);

        let intent_event = buzz_sdk::build_meeting_v1_intent_submit(MeetingV1IntentSubmitParams {
            session_id,
            basis_speech_revision: 3,
            addressed_to: None,
            summary: "Persist this prepared Intent.",
        })
        .expect("build prepared Intent")
        .sign_with_keys(&keys)
        .expect("sign prepared Intent");
        let ack_event = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
            session_id,
            offer_id: &offer_id,
        })
        .expect("build prepared ACK")
        .sign_with_keys(&keys)
        .expect("sign prepared ACK");
        let progress_event =
            buzz_sdk::build_meeting_v1_grant_progress(MeetingV1GrantProgressParams {
                session_id,
                grant_id: &speech_grant_id,
                progress_seq: 1,
                stage: MeetingV1ProgressStage::Generating,
            })
            .expect("build prepared Progress")
            .sign_with_keys(&keys)
            .expect("sign prepared Progress");
        let speech_event = buzz_sdk::build_meeting_v1_speech(MeetingV1SpeechParams {
            session_id,
            grant_id: &speech_grant_id,
            speech_revision: 4,
            content: "Persist this prepared canonical speech.",
            mentions: &[],
            handoff: None,
        })
        .expect("build prepared speech")
        .sign_with_keys(&keys)
        .expect("sign prepared speech");
        let yield_event = buzz_sdk::build_meeting_v1_grant_yield(MeetingV1GrantYieldParams {
            session_id,
            grant_id: &yield_grant_id,
            reason_code: Some(MeetingV1GrantYieldReason::NoLongerNeeded),
            reason: Some("Persist this prepared Yield."),
        })
        .expect("build prepared Yield")
        .sign_with_keys(&keys)
        .expect("sign prepared Yield");

        let trigger_id = "speech:prepared".to_string();
        let mut trigger = TriggerRecord::new(trigger_id.clone(), Some("prepared".into()), 3);
        trigger.state = "running".to_string();
        trigger.prepared_event =
            Some(serde_json::to_value(&intent_event).expect("serialize prepared Intent"));
        trigger.prepared_event_id = Some(intent_event.id.to_hex());
        let speech_grant = GrantRecord {
            grant_id: speech_grant_id.clone(),
            source_offer_id: offer_id.clone(),
            state: "running".to_string(),
            basis_speech_revision: 3,
            soft_lease_expires_at_ms: now_ms() + 30_000,
            hard_deadline_ms: now_ms() + 300_000,
            progress_seq: 0,
            next_progress_at_ms: now_ms(),
            prepared_progress: Some(PreparedProgress {
                seq: 1,
                event: serde_json::to_value(&progress_event).expect("serialize prepared Progress"),
                state: "uncertain".to_string(),
            }),
            speech_event: Some(
                serde_json::to_value(&speech_event).expect("serialize prepared speech"),
            ),
            speech_event_id: Some(speech_event.id.to_hex()),
            yield_event: None,
            format_attempts: 1,
        };
        let yield_grant = GrantRecord {
            grant_id: yield_grant_id.clone(),
            source_offer_id: pubkey(43),
            state: "queued".to_string(),
            basis_speech_revision: 3,
            soft_lease_expires_at_ms: now_ms() + 30_000,
            hard_deadline_ms: now_ms() + 300_000,
            progress_seq: 0,
            next_progress_at_ms: now_ms(),
            prepared_progress: None,
            speech_event: None,
            speech_event_id: None,
            yield_event: Some(
                serde_json::to_value(&yield_event).expect("serialize prepared Yield"),
            ),
            format_attempts: 0,
        };
        let meeting_key = session_id.to_string();
        let ledger = AgentLedger {
            version: LEDGER_VERSION,
            agent_pubkey: agent_pubkey.clone(),
            meetings: BTreeMap::from([(
                meeting_key.clone(),
                MeetingLedger {
                    session_id: meeting_key,
                    agent_pubkey: agent_pubkey.clone(),
                    meeting_synced: true,
                    triggers: BTreeMap::from([(trigger_id.clone(), trigger)]),
                    reservations: BTreeMap::from([(
                        offer_id.clone(),
                        ReservationRecord {
                            offer_id: offer_id.clone(),
                            state: "ack_prepared".to_string(),
                            ack_event: Some(
                                serde_json::to_value(&ack_event).expect("serialize prepared ACK"),
                            ),
                            decline_event: None,
                            created_at_ms: now_ms(),
                            capacity_expires_at_ms: now_ms() + 300_000,
                        },
                    )]),
                    grants: BTreeMap::from([
                        (speech_grant_id.clone(), speech_grant),
                        (yield_grant_id.clone(), yield_grant),
                    ]),
                    ..MeetingLedger::default()
                },
            )]),
        };

        persist_ledger(&path, &ledger).expect("persist real Meeting V1 ledger");
        let mut loaded = load_ledger(&path).expect("load real Meeting V1 ledger");
        assert_eq!(recover_interrupted_turns(&mut loaded), (1, 2, true));
        let recovered = loaded
            .meetings
            .get(&session_id.to_string())
            .expect("recovered Meeting ledger");
        assert_eq!(recovered.triggers[&trigger_id].state, "prepared");
        assert_eq!(recovered.grants[&speech_grant_id].state, "speech_prepared");
        assert_eq!(recovered.grants[&yield_grant_id].state, "yield_prepared");

        let decode = |value: &Value| {
            let event: Event =
                serde_json::from_value(value.clone()).expect("decode persisted signed event");
            event.verify().expect("verify persisted signed event");
            event
        };
        let recovered_intent = decode(
            recovered.triggers[&trigger_id]
                .prepared_event
                .as_ref()
                .expect("prepared Intent"),
        );
        let recovered_ack = decode(
            recovered.reservations[&offer_id]
                .ack_event
                .as_ref()
                .expect("prepared ACK"),
        );
        let recovered_progress = decode(
            &recovered.grants[&speech_grant_id]
                .prepared_progress
                .as_ref()
                .expect("prepared Progress")
                .event,
        );
        let recovered_speech = decode(
            recovered.grants[&speech_grant_id]
                .speech_event
                .as_ref()
                .expect("prepared speech"),
        );
        let recovered_yield = decode(
            recovered.grants[&yield_grant_id]
                .yield_event
                .as_ref()
                .expect("prepared Yield"),
        );
        assert_eq!(recovered_intent.id, intent_event.id);
        assert_eq!(recovered_ack.id, ack_event.id);
        assert_eq!(recovered_progress.id, progress_event.id);
        assert_eq!(recovered_speech.id, speech_event.id);
        assert_eq!(recovered_yield.id, yield_event.id);

        persist_ledger(&path, &loaded).expect("persist recovered Meeting V1 ledger");
        let mut reloaded = load_ledger(&path).expect("reload recovered Meeting V1 ledger");
        assert_eq!(
            recover_interrupted_turns(&mut reloaded),
            (0, 0, false),
            "recovery must be idempotent after durable rewrite"
        );
    }

    #[tokio::test]
    async fn protocol_submission_classification_keeps_private_errors_out_of_telemetry() {
        let event = buzz_sdk::build_meeting_v1_offer_ack(MeetingV1OfferAckParams {
            session_id: Uuid::new_v4(),
            offer_id: &pubkey(50),
        })
        .expect("build test ACK")
        .sign_with_keys(&Keys::generate())
        .expect("sign test ACK");

        let (accepted_rest, accepted_server) =
            rest_responding_once("200 OK", r#"{"accepted":true}"#).await;
        let accepted = submit_protocol_event(&accepted_rest, &event).await;
        accepted_server.await.expect("join accepted HTTP server");
        assert_eq!(protocol_submission_label(&accepted), "accepted");

        const PRIVATE_REJECTION: &str = "PRIVATE_REJECTION_REASON_MUST_NOT_REACH_TELEMETRY";
        let rejected_body = json!({ "accepted": false, "message": PRIVATE_REJECTION }).to_string();
        let (rejected_rest, rejected_server) = rest_responding_once("200 OK", &rejected_body).await;
        let rejected = submit_protocol_event(&rejected_rest, &event).await;
        rejected_server.await.expect("join rejected HTTP server");
        assert_eq!(protocol_submission_label(&rejected), "rejected");
        let rejected_error = rejected.as_ref().expect_err("Relay rejection");
        assert!(!rejected_error.is_uncertain());

        let (bad_request_rest, bad_request_server) =
            rest_responding_once("400 Bad Request", r#"{"error":"private protocol body"}"#).await;
        let bad_request = submit_protocol_event(&bad_request_rest, &event).await;
        bad_request_server
            .await
            .expect("join bad-request HTTP server");
        assert_eq!(protocol_submission_label(&bad_request), "rejected");
        assert!(!bad_request
            .as_ref()
            .expect_err("deterministic HTTP rejection")
            .is_uncertain());

        let (uncertain_rest, uncertain_server) =
            rest_responding_once("200 OK", r#"{"error":"private transport body"}"#).await;
        let uncertain = submit_protocol_event(&uncertain_rest, &event).await;
        uncertain_server.await.expect("join uncertain HTTP server");
        assert_eq!(protocol_submission_label(&uncertain), "uncertain");
        assert!(uncertain
            .as_ref()
            .expect_err("uncertain submission")
            .is_uncertain());

        // Observer payloads use only the closed label, never Relay error text.
        let telemetry = json!({
            "event_id": event.id.to_hex(),
            "outcome": protocol_submission_label(&rejected),
        })
        .to_string();
        assert!(!telemetry.contains(PRIVATE_REJECTION));
        assert!(!telemetry.contains("private protocol body"));
        assert!(!telemetry.contains("private transport body"));
    }

    #[test]
    fn ledger_restart_resumes_model_turns_but_preserves_prepared_protocol_events() {
        let meeting_id = Uuid::new_v4().to_string();
        let trigger_id = "speech:abc".to_string();
        let mut trigger = TriggerRecord::new(trigger_id.clone(), Some("abc".into()), 4);
        trigger.state = "running".to_string();
        trigger.prepared_event = Some(json!({"id": pubkey(3)}));
        let grant_id = pubkey(4);
        let grant = GrantRecord {
            grant_id: grant_id.clone(),
            source_offer_id: pubkey(5),
            state: "running".to_string(),
            basis_speech_revision: 4,
            soft_lease_expires_at_ms: 100,
            hard_deadline_ms: 200,
            progress_seq: 1,
            next_progress_at_ms: 50,
            prepared_progress: Some(PreparedProgress {
                seq: 2,
                event: json!({"id": pubkey(6)}),
                state: "uncertain".to_string(),
            }),
            speech_event: Some(json!({"id": pubkey(7)})),
            speech_event_id: Some(pubkey(7)),
            yield_event: None,
            format_attempts: 1,
        };
        let meeting = MeetingLedger {
            session_id: meeting_id.clone(),
            agent_pubkey: pubkey(1),
            triggers: BTreeMap::from([(trigger_id.clone(), trigger)]),
            grants: BTreeMap::from([(grant_id.clone(), grant)]),
            ..MeetingLedger::default()
        };
        let mut ledger = AgentLedger {
            version: LEDGER_VERSION,
            agent_pubkey: pubkey(1),
            meetings: BTreeMap::from([(meeting_id, meeting)]),
        };

        assert_eq!(recover_interrupted_turns(&mut ledger), (1, 1, true));
        let recovered = ledger.meetings.values().next().expect("meeting");
        assert_eq!(recovered.triggers[&trigger_id].state, "pending");
        assert!(recovered.triggers[&trigger_id].prepared_event.is_some());
        assert_eq!(recovered.grants[&grant_id].state, "speech_prepared");
        assert!(recovered.grants[&grant_id].prepared_progress.is_some());
        assert!(recovered.grants[&grant_id].speech_event.is_some());
    }

    #[test]
    fn participant_prompt_requests_only_a_summary() {
        let intent_prompt = PARTICIPANT_INTENT_PROMPT
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let granted_prompt = GRANTED_SPEECH_PROMPT
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(intent_prompt.contains("Do not draft"));
        assert!(intent_prompt.contains("one concise"));
        assert!(intent_prompt.contains("meeting_read"));
        assert!(intent_prompt.contains("advisory-v1"));
        assert!(intent_prompt.contains("persistent write operations"));
        assert!(intent_prompt.contains("publish a Meeting event"));
        assert!(!intent_prompt.contains("read-only"));
        assert!(granted_prompt.contains("advisory-v1"));
        assert!(granted_prompt.contains("persistent write operations"));
        assert!(granted_prompt.contains("only as a recommendation"));
        assert!(granted_prompt.contains("publish a Meeting event"));
        assert!(!granted_prompt.contains("read-only"));
        assert!(granted_prompt.contains("meeting_read"));
        assert!(granted_prompt.contains("SAY"));

        let metadata = prompt_speech_window_metadata(&[], &[], 7);
        assert_eq!(metadata["authoritative_revision"], 7);
        assert_eq!(metadata["included_speech_count"], 0);
        assert_eq!(metadata["is_truncated"], false);
        assert_eq!(metadata["older_history_lookup"]["operation"], "history");

        let oversized = Speech {
            event_id: pubkey(60),
            author_pubkey: pubkey(61),
            author_display_name: "Participant".to_string(),
            content: "x".repeat(PROMPT_CONTENT_LIMIT),
            created_at: 1,
            speech_revision: 1,
            grant_id: pubkey(62),
            mentions: Vec::new(),
            handoff: None,
        };
        let speeches = [oversized];
        let selected = prompt_speeches(&speeches, 1);
        assert!(
            selected.is_empty(),
            "one oversized speech must not bypass the recent-window byte cap"
        );
    }

    #[test]
    fn generated_v1_prompts_expose_advisory_tool_policy() {
        let session_id = Uuid::new_v4();
        let agent_pubkey = pubkey(70);
        let other_pubkey = pubkey(71);
        let view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        let intent_prompt =
            build_intent_prompt(&view, "meeting:create", now_ms().saturating_add(60_000));
        let grant = GrantView {
            grant_id: pubkey(72),
            holder_pubkey: agent_pubkey,
            allocation_source: "moderator_selection".to_string(),
            turn_role: "participant".to_string(),
            source_offer_id: pubkey(73),
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            basis_speech_revision: 0,
            soft_lease_expires_at_ms: now_ms().saturating_add(30_000),
            hard_deadline_ms: now_ms().saturating_add(300_000),
            progress_seq: 0,
        };
        let granted_prompt = build_granted_prompt(&view, &grant, "grant:test");

        for prompt in [intent_prompt, granted_prompt] {
            assert!(prompt.contains(r#""tool_policy": "advisory-v1""#));
            assert!(prompt.contains("normally exposed Harness tools"));
            assert!(prompt.contains("no persistent writes or Meeting-event publishing"));
            assert!(!prompt.contains("read-only inspection tools"));
        }
    }

    #[test]
    fn moderator_prompt_exposes_identity_and_fairness_state() {
        let moderator = pubkey(79);
        let other = pubkey(78);
        let mut view = meeting_view(Uuid::new_v4(), &moderator, &other);
        view.baton.moderator_pubkey = moderator.clone();
        view.baton.consecutive_moderator_speeches = 1;
        view.baton.handoff_depth = 4;
        view.baton.speech_revision = 1;
        view.speeches.push(Speech {
            event_id: pubkey(77),
            author_pubkey: other,
            author_display_name: "Participant".to_string(),
            content: "x".repeat(PROMPT_CONTENT_LIMIT),
            created_at: 1,
            speech_revision: 1,
            grant_id: pubkey(76),
            mentions: Vec::new(),
            handoff: None,
        });
        view.baton.decision_epoch = 1;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(75),
                &pubkey(74),
                &moderator,
                true,
                1,
            )],
        );
        let control_prompt = build_moderator_control_prompt(&view, &attempt, now_ms() + 60_000);

        assert!(control_prompt.contains("withdraw_self"));
        assert!(control_prompt.contains(&format!(r#""moderator_pubkey": "{moderator}""#)));
        assert!(control_prompt.contains(r#""consecutive_moderator_speeches": 1"#));
        assert!(control_prompt.contains(r#""handoff_depth": 4"#));
        assert!(control_prompt.contains(r#""candidate_cohort""#));
        assert!(control_prompt.contains(&attempt.candidate_snapshot_hash));
        assert!(control_prompt.contains(r#""recent_shared_conversation_window""#));
        assert!(control_prompt.contains(r#""is_truncated": true"#));
        assert!(control_prompt.contains(r#""omitted_earlier_speech_count": 1"#));
        assert!(control_prompt.contains(r#""operation": "history""#));
        assert!(!control_prompt.contains("agenda_ranking"));
        assert!(!control_prompt.contains("cached_agenda_ranking"));
        assert!(!control_prompt.contains("moderator_summary"));
    }

    #[test]
    fn candidate_snapshot_hash_matches_relay_variant_shape() {
        let moderator = pubkey(79);
        let other = pubkey(78);
        let mut view = meeting_view(Uuid::new_v4(), &moderator, &other);
        view.baton.moderator_pubkey = moderator.clone();
        view.baton.decision_epoch = 1;
        let attempt = decision_attempt(
            &view,
            vec![
                intent_candidate(&pubkey(80), &pubkey(81), &other, false, 1),
                handoff_candidate(&pubkey(82), &moderator, &other, 2, 1),
            ],
        );

        assert_eq!(
            candidate_snapshot_hash(&attempt).expect("rehash snapshot"),
            attempt.candidate_snapshot_hash
        );
        let encoded = candidate_snapshot_value(&attempt).to_string();
        assert!(
            !encoded.contains(r#""current_event_id":null"#),
            "variant-only Relay JSON must not gain null fields"
        );
        assert!(
            !encoded.contains(r#""attempt_count":null"#),
            "Intent candidates must not gain Handoff-only fields"
        );
    }

    #[test]
    fn human_priority_and_deadline_budget_suppress_moderator_actions() {
        let mut baton = baton_view();
        assert!(!human_priority_active(&baton));
        baton.human_queue.push(HumanQueueView {
            request_id: pubkey(90),
            requester_pubkey: pubkey(2),
            queue_position: 1,
            state: "queued".to_string(),
        });
        assert!(human_priority_active(&baton));

        let now = 1_000_000;
        baton.human_queue.clear();
        baton.moderator_decision_deadline_ms = None;
        assert!(!moderator_deadline_expired(&baton, now));
        assert_eq!(
            moderator_local_deadline(&baton, now),
            now + DEFAULT_MODERATOR_DECISION_DURATION.as_millis() as i64
        );
        baton.moderator_decision_deadline_ms =
            Some(now + MODERATOR_DEADLINE_SAFETY_MARGIN.as_millis() as i64);
        assert!(moderator_deadline_expired(&baton, now));
        baton.moderator_decision_deadline_ms =
            Some(now + MODERATOR_DEADLINE_SAFETY_MARGIN.as_millis() as i64 + 1);
        assert!(!moderator_deadline_expired(&baton, now));
    }

    #[test]
    fn moderator_parser_enforces_self_priority_and_active_handoff_safety() {
        let session_id = Uuid::new_v4();
        let moderator = pubkey(91);
        let other = pubkey(92);
        let mut view = meeting_view(session_id, &moderator, &other);
        view.baton.moderator_pubkey = moderator.clone();
        view.baton.pending_intents = vec![
            PendingIntentView {
                intent_id: pubkey(93),
                current_event_id: pubkey(94),
                author_pubkey: moderator.clone(),
                basis_speech_revision: 0,
                summary: "moderator point".to_string(),
                addressed_to: None,
                created_at_ms: 1,
                deferred: false,
                selection_attempt_count: 0,
                last_offer_id: None,
                last_attempt_outcome: None,
                eligible_decision_epoch: 0,
            },
            PendingIntentView {
                intent_id: pubkey(95),
                current_event_id: pubkey(96),
                author_pubkey: other.clone(),
                basis_speech_revision: 0,
                summary: "participant point".to_string(),
                addressed_to: None,
                created_at_ms: 2,
                deferred: false,
                selection_attempt_count: 0,
                last_offer_id: None,
                last_attempt_outcome: None,
                eligible_decision_epoch: 0,
            },
        ];
        let attempt = decision_attempt(
            &view,
            vec![
                intent_candidate(&pubkey(93), &pubkey(94), &moderator, true, 0),
                intent_candidate(&pubkey(95), &pubkey(96), &other, false, 0),
            ],
        );
        let valid = json!({
            "rejections": [],
            "handoff_dismissals": [],
            "deferrals": [{
                "intent_id": pubkey(95),
                "reason": "The moderator must first frame the decision."
            }],
            "next_action": {
                "action": "moderator_speak",
                "id": pubkey(93),
                "reason": "Frame the next decision."
            }
        });
        assert!(parse_control_output(&valid.to_string(), &view, &attempt, &moderator).is_ok());

        let bypass = json!({
            "rejections": [],
            "handoff_dismissals": [],
            "deferrals": [],
            "next_action": {
                "action": "select_intent",
                "id": pubkey(95),
                "reason": "Skip the moderator."
            }
        });
        assert!(parse_control_output(&bypass.to_string(), &view, &attempt, &moderator).is_err());

        view.baton.consecutive_moderator_speeches = 1;
        let missing_deferral = json!({
            "rejections": [],
            "handoff_dismissals": [],
            "deferrals": [],
            "next_action": {
                "action": "moderator_speak",
                "id": pubkey(93),
                "reason": "Speak again without accounting for the waiting participant."
            }
        });
        assert!(
            parse_control_output(&missing_deferral.to_string(), &view, &attempt, &moderator)
                .is_err()
        );
        assert!(parse_control_output(&valid.to_string(), &view, &attempt, &moderator).is_ok());

        let withdraw = json!({
            "rejections": [],
            "handoff_dismissals": [],
            "deferrals": [],
            "next_action": {
                "action": "withdraw_self",
                "id": pubkey(93),
                "reason": "The moderator point is no longer useful."
            }
        });
        assert!(parse_control_output(&withdraw.to_string(), &view, &attempt, &moderator).is_ok());

        let outside_cohort = json!({
            "rejections": [],
            "handoff_dismissals": [],
            "deferrals": [],
            "next_action": {
                "action": "select_handoff",
                "id": pubkey(97),
                "reason": "Invent an unavailable source."
            }
        });
        assert!(
            parse_control_output(&outside_cohort.to_string(), &view, &attempt, &moderator).is_err()
        );
    }

    #[test]
    fn late_intent_does_not_change_the_authoritative_candidate_cohort() {
        let moderator = pubkey(101);
        let other = pubkey(102);
        let mut view = meeting_view(Uuid::new_v4(), &moderator, &other);
        view.baton.moderator_pubkey = moderator;
        view.baton.decision_epoch = 3;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(103),
                &pubkey(104),
                &other,
                false,
                3,
            )],
        );
        let snapshot_hash = attempt.candidate_snapshot_hash.clone();
        view.baton.active_decision_attempt = Some(attempt.clone());

        view.baton.intent_revision += 1;
        view.baton.pending_intents.push(PendingIntentView {
            intent_id: pubkey(105),
            current_event_id: pubkey(106),
            author_pubkey: other,
            basis_speech_revision: view.baton.speech_revision,
            summary: "late candidate".to_string(),
            addressed_to: None,
            created_at_ms: 2,
            deferred: false,
            selection_attempt_count: 0,
            last_offer_id: None,
            last_attempt_outcome: None,
            eligible_decision_epoch: 4,
        });

        assert_eq!(attempt.candidate_refs.len(), 1);
        assert_eq!(attempt.candidate_snapshot_hash, snapshot_hash);
        assert_eq!(
            moderator_attempt_guard_failure(
                &view,
                &attempt,
                &view.baton.moderator_pubkey,
                now_ms()
            ),
            None,
            "a late ordinary Intent must not invalidate the running Attempt"
        );
    }

    #[tokio::test]
    async fn agent_moderator_does_not_dispatch_while_another_participant_has_the_grant() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let speaker_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &speaker_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.raw_state["moderator_pubkey"] = json!(moderator_pubkey);
        view.baton.phase = "granted".to_string();
        view.baton.intent_revision = 1;
        view.baton.pending_intents = vec![PendingIntentView {
            intent_id: pubkey(105),
            current_event_id: pubkey(106),
            author_pubkey: speaker_pubkey.clone(),
            basis_speech_revision: 0,
            summary: "Review this contribution next.".to_string(),
            addressed_to: None,
            created_at_ms: now_ms(),
            deferred: false,
            selection_attempt_count: 0,
            last_offer_id: None,
            last_attempt_outcome: None,
            eligible_decision_epoch: 1,
        }];
        view.baton.grant = Some(GrantView {
            grant_id: pubkey(107),
            holder_pubkey: speaker_pubkey,
            allocation_source: "moderator_select".to_string(),
            turn_role: "participant".to_string(),
            source_offer_id: pubkey(108),
            source_intent_id: Some(pubkey(109)),
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            basis_speech_revision: 0,
            soft_lease_expires_at_ms: now_ms() + 60_000,
            hard_deadline_ms: now_ms() + 300_000,
            progress_seq: 0,
        });

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));
        coordinator.apply_view_to_ledger(&view);
        coordinator.reconcile(session_id).await;

        assert!(
            coordinator.pending.is_empty(),
            "offered/granted phases must not dispatch a moderator model Turn"
        );
        assert!(coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .is_none());
        assert!(coordinator.preemptions.is_empty());
    }

    #[tokio::test]
    async fn empty_moderator_idle_does_not_call_the_model() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.raw_state["moderator_pubkey"] = json!(moderator_pubkey);
        view.baton.phase = "moderator_idle".to_string();

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.apply_view_to_ledger(&view);
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view));

        coordinator.reconcile(session_id).await;

        assert!(coordinator.pending.is_empty());
        assert!(coordinator.protocol_in_flight.is_empty());
        assert!(coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .is_none());
    }

    #[tokio::test]
    async fn registered_attempt_queues_exactly_one_moderator_model_turn() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.raw_state["moderator_pubkey"] = json!(moderator_pubkey);
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        view.baton.intent_revision = 1;
        let candidate = intent_candidate(&pubkey(110), &pubkey(111), &other_pubkey, false, 1);
        let attempt = decision_attempt(&view, vec![candidate]);
        let observer = ObserverHandle::in_process();

        let mut coordinator = test_coordinator(
            keys,
            dir.path().join("meeting-v1-ledger.json"),
            Some(observer.clone()),
        );
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "registered",
            ModeratorNextAction {
                action: "select_intent".to_string(),
                id: Some(pubkey(110)),
                reason: "advance the meeting".to_string(),
            },
        );
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));

        coordinator.reconcile(session_id).await;
        coordinator.reconcile(session_id).await;
        assert_eq!(coordinator.pending.len(), 1);
        let request = coordinator.pop_pending().expect("moderator Control Turn");
        assert_eq!(request.kind, MeetingTurnKind::V1ModeratorControl);
        assert_eq!(request.basis_id, attempt.attempt_id);
        assert!(request.prompt.contains(&attempt.candidate_snapshot_hash));
        assert!(coordinator.pending.is_empty());

        coordinator.mark_dispatched("moderator-turn-1".to_string(), request);
        let started = observer
            .snapshot()
            .into_iter()
            .find(|event| event.kind == "meeting_v1_moderator_decision_started")
            .expect("structured Moderator Decision start evidence");
        assert_eq!(started.turn_id.as_deref(), Some("moderator-turn-1"));
        assert_eq!(started.payload["attempt_id"], attempt.attempt_id);
        assert_eq!(
            started.payload["candidate_snapshot_hash"],
            attempt.candidate_snapshot_hash
        );
        assert_eq!(started.payload["candidate_count"], 1);
        assert_eq!(started.payload["phase"], "moderator_control");
        let registered = observer
            .snapshot()
            .into_iter()
            .find(|event| event.kind == "meeting_v1_moderator_attempt_registered")
            .expect("structured Relay-registered attempt evidence");
        assert_eq!(registered.payload["attempt_id"], attempt.attempt_id);
        assert_eq!(
            registered.payload["candidate_snapshot_hash"],
            attempt.candidate_snapshot_hash
        );
    }

    #[tokio::test]
    async fn human_priority_discards_a_queued_moderator_turn_before_dispatch_without_cancel() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let human_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &human_pubkey);
        view.roster
            .get_mut(&human_pubkey)
            .expect("human participant")
            .participant_type = "human".to_string();
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(112),
                &pubkey(113),
                &human_pubkey,
                false,
                1,
            )],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "registered",
            ModeratorNextAction {
                action: "idle".to_string(),
                id: None,
                reason: "not decided".to_string(),
            },
        );
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));
        coordinator.reconcile(session_id).await;
        assert_eq!(coordinator.pending.len(), 1);

        view.baton.human_queue.push(HumanQueueView {
            request_id: pubkey(114),
            requester_pubkey: human_pubkey,
            queue_position: 1,
            state: "queued".to_string(),
        });
        coordinator
            .meetings
            .get_mut(&session_id)
            .expect("Meeting runtime")
            .view = Some(view.clone());
        coordinator.apply_view_to_ledger(&view);
        coordinator.reconcile(session_id).await;

        assert!(coordinator.pending.is_empty());
        assert!(coordinator.in_flight.is_empty());
        assert!(coordinator.preemptions.is_empty());
        let prepared = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
            .expect("attempt finish after queued result became ineligible");
        assert_eq!(prepared.action_kind, "decision_attempt_finish");
        assert_eq!(
            prepared.attempt_id.as_deref(),
            Some(attempt.attempt_id.as_str())
        );
    }

    #[tokio::test]
    async fn malformed_moderator_output_closes_the_attempt_without_a_second_model_call() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(112),
                &pubkey(113),
                &other_pubkey,
                false,
                1,
            )],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "running",
            ModeratorNextAction {
                action: "idle".to_string(),
                id: None,
                reason: "not decided".to_string(),
            },
        );
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));
        let request = MeetingTurnRequest {
            session_id,
            prompt: "moderate".to_string(),
            hard_deadline_unix_ms: attempt.deadline_ms,
            kind: MeetingTurnKind::V1ModeratorControl,
            format_retry: false,
            basis_id: attempt.attempt_id,
            round_number: view.baton.speech_revision,
            speech_cursor: None,
            floor_revision: view.baton.state_revision,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        };
        coordinator.handle_moderator_control_result("control-1", &request, "{malformed", true);

        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| (
                    decision.state.as_str(),
                    decision.pending_finish_reason.as_deref()
                )),
            Some(("result_stale", Some("no_action")))
        );
        assert!(coordinator.pending.is_empty());
    }

    #[tokio::test]
    async fn late_state_does_not_preempt_a_running_moderator_decision() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(114),
                &pubkey(115),
                &other_pubkey,
                false,
                1,
            )],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "running",
            ModeratorNextAction {
                action: "idle".to_string(),
                id: None,
                reason: "pending".to_string(),
            },
        );
        let mut runtime = runtime_with_view(1, view.clone());
        runtime.in_flight_turn = Some("moderator-turn".to_string());
        coordinator.meetings.insert(session_id, runtime);
        coordinator.in_flight.insert(
            "moderator-turn".to_string(),
            MeetingTurnRequest {
                session_id,
                prompt: "moderate".to_string(),
                hard_deadline_unix_ms: attempt.deadline_ms,
                kind: MeetingTurnKind::V1ModeratorControl,
                format_retry: false,
                basis_id: attempt.attempt_id.clone(),
                round_number: 0,
                speech_cursor: None,
                floor_revision: 1,
                grant_event_id: None,
                queued_at_unix_ms: now_ms(),
            },
        );

        view.baton.intent_revision += 1;
        view.baton.pending_intents.push(PendingIntentView {
            intent_id: pubkey(116),
            current_event_id: pubkey(117),
            author_pubkey: other_pubkey,
            basis_speech_revision: view.baton.speech_revision,
            summary: "late intent".to_string(),
            addressed_to: None,
            created_at_ms: now_ms(),
            deferred: false,
            selection_attempt_count: 0,
            last_offer_id: None,
            last_attempt_outcome: None,
            eligible_decision_epoch: 2,
        });
        coordinator
            .meetings
            .get_mut(&session_id)
            .expect("Meeting runtime")
            .view = Some(view.clone());
        coordinator.apply_view_to_ledger(&view);
        coordinator.reconcile(session_id).await;
        assert!(coordinator.preemptions.is_empty());
        assert!(coordinator.in_flight.contains_key("moderator-turn"));
        assert!(coordinator.pending.is_empty());
        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| decision.state.as_str()),
            Some("running")
        );
    }

    #[tokio::test]
    async fn human_priority_waits_for_natural_moderator_terminal_then_fences_the_result() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let human_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &human_pubkey);
        view.roster
            .get_mut(&human_pubkey)
            .expect("human participant")
            .participant_type = "human".to_string();
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        let intent_id = pubkey(118);
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &intent_id,
                &pubkey(119),
                &human_pubkey,
                false,
                1,
            )],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "running",
            ModeratorNextAction {
                action: "idle".to_string(),
                id: None,
                reason: "pending".to_string(),
            },
        );
        let request = MeetingTurnRequest {
            session_id,
            prompt: "moderate".to_string(),
            hard_deadline_unix_ms: attempt.deadline_ms,
            kind: MeetingTurnKind::V1ModeratorControl,
            format_retry: false,
            basis_id: attempt.attempt_id.clone(),
            round_number: 0,
            speech_cursor: None,
            floor_revision: 1,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        };
        let mut runtime = runtime_with_view(1, view.clone());
        runtime.in_flight_turn = Some("moderator-turn".to_string());
        coordinator.meetings.insert(session_id, runtime);
        coordinator
            .in_flight
            .insert("moderator-turn".to_string(), request.clone());

        view.baton.human_queue.push(HumanQueueView {
            request_id: pubkey(120),
            requester_pubkey: human_pubkey,
            queue_position: 1,
            state: "queued".to_string(),
        });
        coordinator
            .meetings
            .get_mut(&session_id)
            .expect("Meeting runtime")
            .view = Some(view.clone());
        coordinator.apply_view_to_ledger(&view);
        coordinator.reconcile(session_id).await;

        assert!(coordinator.preemptions.is_empty());
        assert!(coordinator.in_flight.contains_key("moderator-turn"));
        coordinator.handle_moderator_control_result(
            "moderator-turn",
            &request,
            &json!({
                "rejections": [],
                "handoff_dismissals": [],
                "deferrals": [],
                "next_action": {
                    "action": "select_intent",
                    "id": intent_id,
                    "reason": "would otherwise select"
                }
            })
            .to_string(),
            true,
        );

        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| (
                    decision.state.as_str(),
                    decision.pending_finish_reason.as_deref()
                )),
            Some(("result_stale", Some("human_priority")))
        );
        assert!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
                .is_none(),
            "a Human-priority result must not prepare a protocol action"
        );
    }

    #[tokio::test]
    async fn selected_source_rejection_schedules_attempt_retry_without_another_model_turn() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        let intent_id = pubkey(121);
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &intent_id,
                &pubkey(122),
                &other_pubkey,
                false,
                1,
            )],
        );
        let failed_event_id = pubkey(123);
        let retry_ticket_id = pubkey(124);

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt.clone(),
            "ready",
            ModeratorNextAction {
                action: "select_intent".to_string(),
                id: Some(intent_id),
                reason: "select the candidate".to_string(),
            },
        );
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));
        let outcome = Err(protocol_rejection(
            &failed_event_id,
            "selected_source_changed",
            Some(retry_ticket_id.clone()),
        ));
        coordinator.handle_moderator_protocol_outcome(
            session_id,
            "select_intent",
            &pubkey(121),
            &failed_event_id,
            &outcome,
        );

        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| (
                    decision.state.as_str(),
                    decision.pending_retry.as_ref().map(|retry| (
                        retry.retry_ticket_id.as_str(),
                        retry.failed_action_event_id.as_str()
                    ))
                )),
            Some((
                "retry_pending",
                Some((retry_ticket_id.as_str(), failed_event_id.as_str()))
            ))
        );
        assert!(coordinator.pending.is_empty());

        assert!(coordinator.prepare_moderator_decision_retry(session_id, &view));
        let prepared = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
            .expect("attempt-bound DecisionRetry");
        assert_eq!(prepared.action_kind, "decision_retry");
        assert_eq!(
            prepared.attempt_id.as_deref(),
            Some(attempt.attempt_id.as_str())
        );
        assert_eq!(coordinator.protocol_in_flight.len(), 1);
        assert!(
            coordinator.pending.is_empty(),
            "selected-source retry is a Relay Attempt retry, not an immediate second LLM call"
        );
    }

    #[tokio::test]
    async fn stale_auxiliary_cleanup_does_not_invalidate_the_main_selection() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey.clone();
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        view.baton.intent_revision = 2;
        let stale_intent_id = pubkey(125);
        let selected_intent_id = pubkey(126);
        let selected_event_id = pubkey(127);
        view.baton.pending_intents.push(PendingIntentView {
            intent_id: selected_intent_id.clone(),
            current_event_id: selected_event_id.clone(),
            author_pubkey: other_pubkey.clone(),
            basis_speech_revision: 0,
            summary: "current selection".to_string(),
            addressed_to: None,
            created_at_ms: 2,
            deferred: false,
            selection_attempt_count: 0,
            last_offer_id: None,
            last_attempt_outcome: None,
            eligible_decision_epoch: 1,
        });
        let attempt = decision_attempt(
            &view,
            vec![
                intent_candidate(&stale_intent_id, &pubkey(128), &other_pubkey, false, 1),
                intent_candidate(
                    &selected_intent_id,
                    &selected_event_id,
                    &other_pubkey,
                    false,
                    1,
                ),
            ],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt,
            "ready",
            ModeratorNextAction {
                action: "select_intent".to_string(),
                id: Some(selected_intent_id.clone()),
                reason: "advance with the current candidate".to_string(),
            },
        );
        coordinator
            .ledger_for_mut(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_mut())
            .expect("moderator decision")
            .rejections
            .push(ModeratorRejection {
                intent_id: stale_intent_id,
                reason_code: "superseded".to_string(),
                reason_text: "the old candidate disappeared".to_string(),
            });
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view.clone()));

        assert!(
            coordinator
                .execute_ready_moderator_control(session_id, &view)
                .await
        );
        let decision = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.moderator_decision.as_ref())
            .expect("moderator decision");
        assert!(decision.rejections.is_empty());
        assert_eq!(decision.state, "ready");
        assert_eq!(
            decision.next_action.id.as_deref(),
            Some(selected_intent_id.as_str())
        );
        assert!(coordinator.pending.is_empty());

        assert!(
            coordinator
                .execute_ready_moderator_control(session_id, &view)
                .await
        );
        let prepared = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.prepared_moderator_action.as_ref())
            .expect("main selection remains executable");
        assert_eq!(prepared.action_kind, "select_intent");
        assert_eq!(prepared.object_id, selected_intent_id);
    }

    #[test]
    fn cas_rebase_is_bounded_and_coalesces_after_three_fast_conflicts() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let moderator_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let mut view = meeting_view(session_id, &moderator_pubkey, &other_pubkey);
        view.baton.moderator_pubkey = moderator_pubkey;
        view.baton.phase = "moderator_control".to_string();
        view.baton.decision_epoch = 1;
        view.baton
            .baton_config
            .moderator_max_cas_rebases_per_attempt = 4;
        let attempt = decision_attempt(
            &view,
            vec![intent_candidate(
                &pubkey(129),
                &pubkey(130),
                &other_pubkey,
                false,
                1,
            )],
        );

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        install_decision(
            &mut coordinator,
            &mut view,
            attempt,
            "ready",
            ModeratorNextAction {
                action: "select_intent".to_string(),
                id: Some(pubkey(129)),
                reason: "select".to_string(),
            },
        );
        coordinator
            .meetings
            .insert(session_id, runtime_with_view(1, view));

        coordinator.schedule_moderator_rebase(session_id);
        coordinator.schedule_moderator_rebase(session_id);
        assert!(coordinator
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.moderator_rebase_at)
            .is_none());
        coordinator.schedule_moderator_rebase(session_id);
        assert!(coordinator
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.moderator_rebase_at)
            .is_some());
        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| (decision.state.as_str(), decision.cas_rebases)),
            Some(("rebasing", 3))
        );
        assert!(coordinator.pending.is_empty());

        coordinator.schedule_moderator_rebase(session_id);
        assert_eq!(
            coordinator
                .ledger_for(session_id)
                .and_then(|ledger| ledger.moderator_decision.as_ref())
                .map(|decision| (
                    decision.state.as_str(),
                    decision.cas_rebases,
                    decision.pending_finish_reason.as_deref()
                )),
            Some(("result_stale", 4, Some("cas_churn")))
        );
        assert!(coordinator
            .meetings
            .get(&session_id)
            .and_then(|runtime| runtime.moderator_rebase_at)
            .is_none());
        assert!(coordinator.pending.is_empty());
    }

    #[test]
    fn blocked_or_retry_blocked_handoff_cannot_start_or_reenter_a_cohort() {
        let moderator = pubkey(131);
        let other = pubkey(132);
        let mut baton = baton_view();
        baton.decision_epoch = 1;
        baton.unresolved_handoffs.push(OpenHandoffView {
            handoff_id: pubkey(133),
            source_speech_event_id: pubkey(134),
            from_pubkey: moderator.clone(),
            to_pubkey: other.clone(),
            reason_type: "question".to_string(),
            reason_text: "Need a direct answer".to_string(),
            question_state: "open".to_string(),
            attempt_count: 1,
            last_offer_id: None,
            last_grant_id: None,
            last_attempt_outcome: Some("timeout".to_string()),
            blocked_by: Some("human_request".to_string()),
            moderator_retry_blocked: false,
            eligible_decision_epoch: 1,
        });
        let candidate = handoff_candidate(&pubkey(133), &moderator, &other, 1, 1);

        assert!(!moderator_has_startable_candidate(&baton));
        assert!(!current_cohort_has_candidates(&baton, 1));
        assert!(!handoff_candidate_is_current(&candidate, &baton));

        let handoff = baton.unresolved_handoffs.first_mut().expect("open Handoff");
        handoff.blocked_by = None;
        handoff.moderator_retry_blocked = true;
        assert!(!moderator_has_startable_candidate(&baton));
        assert!(!current_cohort_has_candidates(&baton, 1));
        assert!(!handoff_candidate_is_current(&candidate, &baton));
    }

    #[tokio::test]
    async fn offer_does_not_cancel_a_running_moderator_decision_to_reclaim_capacity() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let offer_id = pubkey(107);
        let mut view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        view.baton.phase = "offered".to_string();
        view.baton.offer = Some(OfferView {
            offer_id: offer_id.clone(),
            target_pubkey: agent_pubkey,
            target_participant_type: "agent".to_string(),
            allocation_source: "moderator_select".to_string(),
            turn_role: "participant".to_string(),
            source_intent_id: Some(pubkey(108)),
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            created_at_ms: now_ms(),
            ack_deadline_ms: now_ms() + 60_000,
        });

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.available_agent_slots = 0;
        coordinator.apply_view_to_ledger(&view);
        let mut runtime = runtime_with_view(1, view.clone());
        runtime.in_flight_turn = Some("moderator-turn".to_string());
        coordinator.meetings.insert(session_id, runtime);
        coordinator.in_flight.insert(
            "moderator-turn".to_string(),
            MeetingTurnRequest {
                session_id,
                prompt: "moderate".to_string(),
                hard_deadline_unix_ms: now_ms() + 60_000,
                kind: MeetingTurnKind::V1ModeratorControl,
                format_retry: false,
                basis_id: pubkey(109),
                round_number: 0,
                speech_cursor: None,
                floor_revision: 1,
                grant_event_id: None,
                queued_at_unix_ms: now_ms(),
            },
        );

        assert!(coordinator.handle_offer(session_id, &view).await);
        let reservation = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.reservations.get(&offer_id))
            .expect("prepared Offer response");
        assert_eq!(reservation.state, "decline_prepared");
        assert!(reservation.ack_event.is_none());
        assert!(reservation.decline_event.is_some());
        assert!(coordinator.preemptions.is_empty());
        assert!(coordinator.in_flight.contains_key("moderator-turn"));
    }

    #[tokio::test]
    async fn offer_ack_reclaims_a_slot_from_external_v0_intent() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let session_id = Uuid::new_v4();
        let v0_session_id = Uuid::new_v4();
        let offer_id = pubkey(110);
        let mut view = meeting_view(session_id, &agent_pubkey, &other_pubkey);
        view.baton.phase = "offered".to_string();
        view.baton.offer = Some(OfferView {
            offer_id: offer_id.clone(),
            target_pubkey: agent_pubkey,
            target_participant_type: "agent".to_string(),
            allocation_source: "moderator_select".to_string(),
            turn_role: "participant".to_string(),
            source_intent_id: Some(pubkey(111)),
            source_request_id: None,
            source_handoff_id: None,
            source_speech_event_id: None,
            handoff_context: None,
            created_at_ms: now_ms(),
            ack_deadline_ms: now_ms() + 60_000,
        });

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.available_agent_slots = 0;
        coordinator.set_external_reclaimable_turns(BTreeSet::from([v0_session_id]));
        coordinator.apply_view_to_ledger(&view);

        assert!(coordinator.handle_offer(session_id, &view).await);
        let reservation = coordinator
            .ledger_for(session_id)
            .and_then(|ledger| ledger.reservations.get(&offer_id))
            .expect("prepared Offer response");
        assert_eq!(reservation.state, "ack_prepared");
        assert!(reservation.ack_event.is_some());
        assert_eq!(coordinator.take_preemptions(), vec![v0_session_id]);
    }

    #[tokio::test]
    async fn offer_declines_when_ack_would_require_preempting_moderator_control() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let agent_pubkey = keys.public_key().to_hex();
        let other_pubkey = Keys::generate().public_key().to_hex();
        let offered_session = Uuid::new_v4();
        let participant_session = Uuid::new_v4();
        let moderator_session = Uuid::new_v4();
        let offered_id = pubkey(113);
        let reserved_offer_id = pubkey(114);
        let trigger_id = "speech:multi-slot".to_string();
        let view = agent_offer_view(offered_session, &agent_pubkey, &other_pubkey, &offered_id);

        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        coordinator.agent_capacity = 3;
        coordinator.available_agent_slots = 0;
        for session_id in [offered_session, participant_session, moderator_session] {
            coordinator.ensure_meeting_ledger(session_id);
            coordinator.meetings.insert(
                session_id,
                runtime_with_view(1, meeting_view(session_id, &agent_pubkey, &other_pubkey)),
            );
        }
        coordinator
            .meetings
            .get_mut(&offered_session)
            .expect("offered Meeting runtime")
            .view = Some(view.clone());

        let participant_ledger = coordinator
            .ledger_for_mut(participant_session)
            .expect("participant Meeting ledger");
        participant_ledger.reservations.insert(
            reserved_offer_id.clone(),
            ReservationRecord {
                offer_id: reserved_offer_id,
                state: "ack_sent".to_string(),
                ack_event: None,
                decline_event: None,
                created_at_ms: now_ms(),
                capacity_expires_at_ms: now_ms() + 300_000,
            },
        );
        let mut trigger = TriggerRecord::new(trigger_id.clone(), None, 0);
        trigger.state = "running".to_string();
        participant_ledger
            .triggers
            .insert(trigger_id.clone(), trigger);

        let request = |session_id, kind, basis_id: String| MeetingTurnRequest {
            session_id,
            prompt: "reclaimable".to_string(),
            hard_deadline_unix_ms: now_ms() + 60_000,
            kind,
            format_retry: false,
            basis_id,
            round_number: 0,
            speech_cursor: None,
            floor_revision: 1,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        };
        coordinator.in_flight.insert(
            "participant-turn".to_string(),
            request(
                participant_session,
                MeetingTurnKind::V1Intent,
                trigger_id.clone(),
            ),
        );
        coordinator.in_flight.insert(
            "moderator-turn".to_string(),
            request(
                moderator_session,
                MeetingTurnKind::V1ModeratorControl,
                pubkey(115),
            ),
        );

        assert!(coordinator.handle_offer(offered_session, &view).await);
        assert_eq!(
            reservation_state(&coordinator, offered_session, &offered_id),
            Some("decline_prepared")
        );
        assert!(
            coordinator.preemptions.is_empty(),
            "declining must not preempt either the participant or moderator"
        );
        assert_eq!(
            coordinator
                .ledger_for(participant_session)
                .and_then(|ledger| ledger.triggers.get(&trigger_id))
                .map(|trigger| trigger.state.as_str()),
            Some("running")
        );
        assert!(coordinator.in_flight.contains_key("moderator-turn"));
    }

    #[test]
    fn local_queue_orders_granted_then_moderator_then_participant() {
        let dir = tempfile::tempdir().expect("temp ledger directory");
        let keys = Keys::generate();
        let mut coordinator =
            test_coordinator(keys, dir.path().join("meeting-v1-ledger.json"), None);
        let participant_session = Uuid::new_v4();
        let moderator_session = Uuid::new_v4();
        let granted_session = Uuid::new_v4();
        for session_id in [participant_session, moderator_session, granted_session] {
            coordinator
                .meetings
                .insert(session_id, MeetingRuntime::new(1));
        }
        let request = |session_id, kind| MeetingTurnRequest {
            session_id,
            prompt: "test".to_string(),
            hard_deadline_unix_ms: now_ms() + 60_000,
            kind,
            format_retry: false,
            basis_id: pubkey(100),
            round_number: 0,
            speech_cursor: None,
            floor_revision: 1,
            grant_event_id: None,
            queued_at_unix_ms: now_ms(),
        };
        coordinator.queue_turn(request(participant_session, MeetingTurnKind::V1Intent));
        coordinator.queue_turn(request(
            moderator_session,
            MeetingTurnKind::V1ModeratorControl,
        ));
        coordinator.queue_turn(request(granted_session, MeetingTurnKind::V1Granted));

        assert_eq!(
            coordinator
                .pending
                .iter()
                .map(|request| request.kind)
                .collect::<Vec<_>>(),
            vec![
                MeetingTurnKind::V1Granted,
                MeetingTurnKind::V1ModeratorControl,
                MeetingTurnKind::V1Intent,
            ]
        );
    }

    #[test]
    fn v3_ledger_migration_preserves_every_prepared_protocol_event() {
        let agent_pubkey = pubkey(110);
        let session_id = Uuid::new_v4();
        let session_key = session_id.to_string();
        let trigger_id = "speech:prepared".to_string();
        let offer_id = pubkey(111);
        let grant_id = pubkey(112);
        let intent_event = json!({ "id": pubkey(113), "kind": "intent" });
        let ack_event = json!({ "id": pubkey(114), "kind": "ack" });
        let progress_event = json!({ "id": pubkey(115), "kind": "progress" });
        let speech_event = json!({ "id": pubkey(116), "kind": "speech" });
        let yield_event = json!({ "id": pubkey(117), "kind": "yield" });
        let ledger = AgentLedger {
            version: PREVIOUS_LEDGER_VERSION,
            agent_pubkey: agent_pubkey.clone(),
            meetings: BTreeMap::from([(
                session_key.clone(),
                MeetingLedger {
                    session_id: session_key.clone(),
                    agent_pubkey: agent_pubkey.clone(),
                    triggers: BTreeMap::from([(
                        trigger_id.clone(),
                        TriggerRecord {
                            trigger_id: trigger_id.clone(),
                            source_event_id: Some(pubkey(118)),
                            basis_speech_revision: 1,
                            created_at_ms: 1,
                            state: "prepared".to_string(),
                            prepared_event: Some(intent_event.clone()),
                            prepared_event_id: Some(pubkey(113)),
                            format_attempts: 0,
                            hard_deadline_unix_ms: None,
                        },
                    )]),
                    reservations: BTreeMap::from([(
                        offer_id.clone(),
                        ReservationRecord {
                            offer_id: offer_id.clone(),
                            state: "ack_prepared".to_string(),
                            ack_event: Some(ack_event.clone()),
                            decline_event: None,
                            created_at_ms: 1,
                            capacity_expires_at_ms: now_ms() + 300_000,
                        },
                    )]),
                    grants: BTreeMap::from([(
                        grant_id.clone(),
                        GrantRecord {
                            grant_id: grant_id.clone(),
                            source_offer_id: offer_id,
                            state: "speech_prepared".to_string(),
                            basis_speech_revision: 1,
                            soft_lease_expires_at_ms: now_ms() + 30_000,
                            hard_deadline_ms: now_ms() + 300_000,
                            progress_seq: 1,
                            next_progress_at_ms: now_ms() + 10_000,
                            prepared_progress: Some(PreparedProgress {
                                seq: 2,
                                event: progress_event.clone(),
                                state: "prepared".to_string(),
                            }),
                            speech_event: Some(speech_event.clone()),
                            speech_event_id: Some(pubkey(116)),
                            yield_event: Some(yield_event.clone()),
                            format_attempts: 0,
                        },
                    )]),
                    ..MeetingLedger::default()
                },
            )]),
        };

        let mut raw = serde_json::to_value(ledger).expect("serialize V3-shaped ledger");
        let meeting = raw["meetings"][session_key.as_str()]
            .as_object_mut()
            .expect("serialized Meeting ledger");
        meeting.remove("moderator_decision");
        meeting.remove("prepared_moderator_action");
        meeting.remove("replacement_attempt_id");
        meeting["triggers"][trigger_id.as_str()]
            .as_object_mut()
            .expect("serialized V3 Trigger")
            .remove("hard_deadline_unix_ms");
        let mut loaded: AgentLedger =
            serde_json::from_value(raw).expect("load a V3 ledger without optimistic fields");

        assert!(migrate_loaded_ledger(
            &mut loaded,
            &agent_pubkey,
            Path::new("/tmp/meeting-v1-ledger-test.json")
        ));
        assert_eq!(loaded.version, LEDGER_VERSION);
        let meeting = loaded
            .meetings
            .get(&session_key)
            .expect("migrated Meeting ledger");
        assert_eq!(
            meeting
                .triggers
                .get(&trigger_id)
                .and_then(|trigger| trigger.prepared_event.as_ref()),
            Some(&intent_event)
        );
        assert_eq!(
            meeting
                .reservations
                .values()
                .next()
                .and_then(|reservation| reservation.ack_event.as_ref()),
            Some(&ack_event)
        );
        let grant = meeting.grants.get(&grant_id).expect("migrated Grant");
        assert_eq!(
            grant
                .prepared_progress
                .as_ref()
                .map(|progress| &progress.event),
            Some(&progress_event)
        );
        assert_eq!(grant.speech_event.as_ref(), Some(&speech_event));
        assert_eq!(grant.yield_event.as_ref(), Some(&yield_event));
        assert!(meeting.moderator_decision.is_none());
        assert!(meeting.prepared_moderator_action.is_none());
        assert!(meeting.replacement_attempt_id.is_none());
    }

    #[test]
    fn moderator_recovery_keeps_prepared_event_but_rewinds_model_work() {
        let agent_pubkey = pubkey(101);
        let other_pubkey = pubkey(102);
        let session_uuid = Uuid::new_v4();
        let session_id = session_uuid.to_string();
        let view = meeting_view(session_uuid, &agent_pubkey, &other_pubkey);
        let attempt = decision_attempt(&view, Vec::new());
        let event_id = pubkey(101);
        let mut meeting = MeetingLedger {
            session_id,
            moderator_decision: Some(ModeratorDecisionRecord {
                attempt: attempt.clone(),
                rejections: Vec::new(),
                handoff_dismissals: Vec::new(),
                deferrals: Vec::new(),
                next_action: ModeratorNextAction {
                    action: "idle".to_string(),
                    id: None,
                    reason: "test".to_string(),
                },
                state: "running".to_string(),
                turn_id: Some("lost-provider-turn".to_string()),
                turn_started_at_ms: Some(now_ms()),
                cas_rebases: 0,
                fast_rebases: 0,
                pending_retry: None,
                pending_finish_reason: None,
            }),
            prepared_moderator_action: Some(PreparedModeratorAction {
                action_kind: "select_intent".to_string(),
                object_id: pubkey(102),
                attempt_id: Some(attempt.attempt_id),
                event: json!({ "id": event_id }),
                event_id: event_id.clone(),
                state: "sent".to_string(),
                created_at_ms: 1,
                hard_deadline_unix_ms: now_ms() + 60_000,
            }),
            ..MeetingLedger::default()
        };
        assert_eq!(
            recover_interrupted_meeting_turns(&mut meeting),
            (0, 0, true)
        );
        assert_eq!(
            meeting
                .moderator_decision
                .as_ref()
                .map(|decision| (decision.state.as_str(), decision.turn_id.as_deref())),
            Some(("runtime_lost", None))
        );
        let prepared = meeting
            .prepared_moderator_action
            .expect("prepared moderator action");
        assert_eq!(prepared.event_id, event_id);
        assert_eq!(prepared.state, "prepared");
    }

    #[test]
    fn moderator_only_restart_recovery_is_durable_and_keeps_attempt_bound() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("meeting-v1-ledger.json");
        let agent_pubkey = pubkey(110);
        let session_id = Uuid::new_v4().to_string();
        let session_uuid = Uuid::parse_str(&session_id).expect("valid Session UUID");
        let view = meeting_view(session_uuid, &agent_pubkey, &pubkey(109));
        let attempt = decision_attempt(&view, Vec::new());
        let event_id = pubkey(111);
        let ledger = AgentLedger {
            version: LEDGER_VERSION,
            agent_pubkey: agent_pubkey.clone(),
            meetings: BTreeMap::from([(
                session_id.clone(),
                MeetingLedger {
                    session_id: session_id.clone(),
                    agent_pubkey: agent_pubkey.clone(),
                    moderator_decision: Some(ModeratorDecisionRecord {
                        attempt: attempt.clone(),
                        rejections: Vec::new(),
                        handoff_dismissals: Vec::new(),
                        deferrals: Vec::new(),
                        next_action: ModeratorNextAction {
                            action: "idle".to_string(),
                            id: None,
                            reason: "test".to_string(),
                        },
                        state: "queued".to_string(),
                        turn_id: Some("lost-provider-turn".to_string()),
                        turn_started_at_ms: None,
                        cas_rebases: 0,
                        fast_rebases: 0,
                        pending_retry: None,
                        pending_finish_reason: None,
                    }),
                    prepared_moderator_action: Some(PreparedModeratorAction {
                        action_kind: "select_intent".to_string(),
                        object_id: pubkey(112),
                        attempt_id: Some(attempt.attempt_id),
                        event: json!({ "id": event_id }),
                        event_id,
                        state: "sent".to_string(),
                        created_at_ms: 1,
                        hard_deadline_unix_ms: now_ms() + 60_000,
                    }),
                    ..MeetingLedger::default()
                },
            )]),
        };
        persist_ledger(&path, &ledger).expect("persist interrupted moderator-only ledger");

        let mut recovered = load_ledger(&path).expect("load interrupted moderator-only ledger");
        let (_, _, changed) = recover_interrupted_turns(&mut recovered);
        assert!(
            changed,
            "moderator-only recovery must request a durable rewrite"
        );
        persist_ledger(&path, &recovered).expect("persist recovered moderator-only ledger");

        let mut reloaded = load_ledger(&path).expect("reload recovered moderator-only ledger");
        let meeting = reloaded
            .meetings
            .get(&session_id)
            .expect("recovered Meeting ledger");
        assert_eq!(
            meeting
                .moderator_decision
                .as_ref()
                .map(|decision| (decision.state.as_str(), decision.turn_id.as_deref())),
            Some(("runtime_lost", None))
        );
        assert_eq!(
            meeting
                .prepared_moderator_action
                .as_ref()
                .map(|action| action.state.as_str()),
            Some("prepared")
        );
        assert_eq!(
            recover_interrupted_turns(&mut reloaded),
            (0, 0, false),
            "the persisted recovery rewrite must be idempotent"
        );
    }
}
