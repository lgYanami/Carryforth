//! Meeting V1 participant controller for ACP-managed Agents.
//!
//! This module deliberately excludes moderator planning. It owns one shared
//! synchronizer per V1 Session, deterministic Offer handling, durable prepared
//! events, Progress heartbeats, and the two LLM turns allowed for an ordinary
//! participant: lightweight Intent and Grant-bound Speech.

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
    MeetingV1DirectedHandoff, MeetingV1GrantProgressParams, MeetingV1GrantYieldParams,
    MeetingV1GrantYieldReason, MeetingV1HandoffType, MeetingV1IntentRefreshParams,
    MeetingV1IntentSubmitParams, MeetingV1OfferAckParams, MeetingV1OfferDeclineParams,
    MeetingV1ProgressStage, MeetingV1SpeechParams,
};
use futures_util::FutureExt;
use nostr::{Alphabet, Event, Filter, Keys, Kind, PublicKey, SingleLetterTag};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::meeting::{
    fetch_meeting_history, now_ms, sign_builder, tag_value, validate_bounded_text, MeetingTurnKind,
    MeetingTurnRequest,
};
use crate::observer::{self, ObserverHandle};
use crate::relay::{BuzzEvent, RestClient};

const LEDGER_VERSION: u32 = 2;
const SYNC_INTERVAL: Duration = Duration::from_secs(30);
const SYNC_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const SYNC_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(5);
const PROTOCOL_SUBMIT_TIMEOUT: Duration = Duration::from_secs(2);
const INTENT_MAX_DURATION: Duration = Duration::from_secs(5 * 60);
const DEFAULT_GRANT_SAFETY_MARGIN: Duration = Duration::from_secs(30);
const PROMPT_SPEECH_LIMIT: usize = 100;
const PROMPT_CONTENT_LIMIT: usize = 128 * 1024;
const MAX_INTENT_SUMMARY_BYTES: usize = 512;
const MAX_REASON_BYTES: usize = 1024;
const MAX_SPEECH_BYTES: usize = 256 * 1024;
const MAX_MENTIONS: usize = 12;

const PARTICIPANT_INTENT_PROMPT: &str = include_str!("meeting_participant_intent_prompt.md");
const GRANTED_SPEECH_PROMPT: &str = include_str!("meeting_granted_speech_prompt.md");

#[derive(Debug, Clone)]
struct MeetingRuntime {
    epoch: u64,
    view: Option<MeetingView>,
    last_sync: Option<Instant>,
    retry_at: Instant,
    control_retry_at: Option<Instant>,
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
    moderator_pubkey: String,
    baton_config: BatonConfigView,
    pending_intents: Vec<PendingIntentView>,
    unresolved_handoffs: Vec<OpenHandoffView>,
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
    moderator_pubkey: String,
    baton_config: BatonConfigView,
    participants: Vec<RawParticipant>,
    #[serde(default)]
    pending_intents: Vec<PendingIntentView>,
    #[serde(default)]
    unresolved_handoffs: Vec<OpenHandoffView>,
    offer: Option<OfferView>,
    grant: Option<GrantView>,
}

#[derive(Debug, Deserialize)]
struct RawParticipant {
    pubkey: String,
    participant_type: String,
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
    Rejected(String),
    Uncertain(String),
}

impl std::fmt::Display for ProtocolSubmitFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) => write!(formatter, "Relay rejected the event: {message}"),
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
}

impl ProtocolSubmissionKey {
    fn session_id(&self) -> Uuid {
        match self {
            Self::Offer { session_id, .. }
            | Self::Intent { session_id, .. }
            | Self::GrantTerminal { session_id, .. } => *session_id,
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

/// Ordinary-participant V1 controller.
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
    meetings: HashMap<Uuid, MeetingRuntime>,
    pending: VecDeque<MeetingTurnRequest>,
    in_flight: HashMap<String, MeetingTurnRequest>,
    in_flight_epochs: HashMap<String, u64>,
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
        if ledger.version != LEDGER_VERSION || ledger.agent_pubkey != agent_pubkey {
            if ledger.version != 0 {
                tracing::warn!(
                    path = %ledger_path.display(),
                    found_version = ledger.version,
                    "Meeting V1 ledger version/identity changed; rebuilding from Relay State"
                );
            }
            ledger = AgentLedger {
                version: LEDGER_VERSION,
                agent_pubkey: agent_pubkey.clone(),
                meetings: BTreeMap::new(),
            };
        }
        let (recovered_intents, recovered_grants) = recover_interrupted_turns(&mut ledger);
        if recovered_intents > 0 || recovered_grants > 0 {
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
            meetings: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: HashMap::new(),
            in_flight_epochs: HashMap::new(),
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
        }
    }

    pub(super) fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub(super) fn set_available_agent_slots(&mut self, available: usize) {
        self.available_agent_slots = available.min(self.agent_capacity);
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
            MeetingTurnKind::V0Intent | MeetingTurnKind::V0Granted => {}
        }
        self.persist_ledger_best_effort();
        self.emit(
            "meeting_v1_turn_started",
            request.session_id,
            Some(turn_id.clone()),
            json!({
                "turn_id": turn_id,
                "turn_type": match request.kind {
                    MeetingTurnKind::V1Intent => "participant_intent",
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
        if self.meetings.contains_key(&session_id) {
            return;
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
        self.request_full_sync(session_id);
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
                "speech_revision": updated.baton.speech_revision,
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
            let attempt = AssertUnwindSafe(submit_protocol_event(&rest, &event))
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
                if let Err(error) = &completed.result {
                    tracing::warn!(
                        meeting = %session_id,
                        offer = %offer_id,
                        action = action.as_str(),
                        "Meeting V1 Offer response was not confirmed: {error}"
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
                if let Err(error) = &completed.result {
                    tracing::warn!(
                        meeting = %session_id,
                        trigger = %trigger_id,
                        "Meeting V1 Intent submission was not confirmed: {error}"
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
                if let Err(error) = &completed.result {
                    tracing::warn!(
                        meeting = %session_id,
                        grant = %grant_id,
                        action = action.as_str(),
                        "Meeting V1 Grant terminal action was not confirmed: {error}"
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
        }
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
                if self
                    .deferred_turn_results
                    .get(&session_id)
                    .is_some_and(|pending| pending.request_id <= completed.request_id)
                {
                    if let Some(pending) = self.deferred_turn_results.remove(&session_id) {
                        self.discard_deferred_turn_result(pending);
                    }
                }
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
                "speech_revision": view.baton.speech_revision,
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

        if view.ended {
            for trigger in ledger.triggers.values_mut() {
                if !matches!(trigger.state.as_str(), "passed" | "submitted" | "stale") {
                    trigger.state = "stale".to_string();
                }
            }
            for reservation in ledger.reservations.values_mut() {
                reservation.state = "released".to_string();
            }
            for grant in ledger.grants.values_mut() {
                if grant.state != "spoken" {
                    grant.state = "terminal".to_string();
                }
            }
        }
        self.persist_ledger_best_effort();
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

        self.replace_stale_queued_intent(session_id, &view);
        if self.session_turn_busy(session_id)
            || self.deferred_turn_results.contains_key(&session_id)
        {
            return;
        }

        // Stage 3 intentionally does not let an Agent moderator use the
        // ordinary voluntary controller as a substitute for ModeratorPlan.
        if view.baton.moderator_pubkey == self.agent_pubkey {
            return;
        }

        if self.retry_prepared_intent(session_id, &view).await {
            return;
        }
        self.queue_latest_intent_trigger(session_id, &view);
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
        let reclaimable_intent = self
            .in_flight
            .values()
            .find(|request| {
                request.kind == MeetingTurnKind::V1Intent
                    && self
                        .ledger_for(request.session_id)
                        .and_then(|ledger| ledger.triggers.get(&request.basis_id))
                        .is_some_and(|trigger| trigger.state == "running")
            })
            .map(|request| request.session_id);
        let reclaimable_slots = usize::from(reclaimable_intent.is_some());
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
            if let Some(reclaimed_session) = reclaimable_intent {
                self.preempt_intent_turn(reclaimed_session);
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
    let response = tokio::time::timeout(PROTOCOL_SUBMIT_TIMEOUT, rest.submit_event(event)).await;
    match response {
        Err(_) => Err(ProtocolSubmitFailure::Uncertain(format!(
            "submission exceeded {}ms",
            PROTOCOL_SUBMIT_TIMEOUT.as_millis()
        ))),
        Ok(Ok(response)) if response.get("accepted").and_then(Value::as_bool) == Some(false) => {
            Err(ProtocolSubmitFailure::Rejected(
                response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Relay rejected Meeting command")
                    .to_string(),
            ))
        }
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(ProtocolSubmitFailure::Uncertain(error.to_string())),
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
        moderator_pubkey: raw_state.moderator_pubkey.to_ascii_lowercase(),
        baton_config: raw_state.baton_config,
        pending_intents: raw_state.pending_intents,
        unresolved_handoffs: raw_state.unresolved_handoffs,
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

fn recover_interrupted_turns(ledger: &mut AgentLedger) -> (usize, usize) {
    let mut recovered_intents = 0;
    let mut recovered_grants = 0;
    for meeting in ledger.meetings.values_mut() {
        let (intents, grants) = recover_interrupted_meeting_turns(meeting);
        recovered_intents += intents;
        recovered_grants += grants;
    }
    (recovered_intents, recovered_grants)
}

fn recover_interrupted_meeting_turns(meeting: &mut MeetingLedger) -> (usize, usize) {
    let mut recovered_intents = 0;
    let mut recovered_grants = 0;
    for trigger in meeting.triggers.values_mut() {
        if trigger.state == "running" || trigger.state == "queued" {
            trigger.state =
                if trigger.prepared_event.is_some() && trigger.prepared_event_id.is_some() {
                    "prepared".to_string()
                } else {
                    "pending".to_string()
                };
            recovered_intents += 1;
        }
    }
    for grant in meeting.grants.values_mut() {
        if grant.state == "running" || grant.state == "queued" {
            restore_active_grant_state(grant);
            recovered_grants += 1;
        }
    }
    (recovered_intents, recovered_grants)
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
            moderator_pubkey: pubkey(1),
            baton_config: BatonConfigView {
                progress_interval_ms: 10_000,
                grant_hard_deadline_ms: 300_000,
                agent_safety_margin_ms: 30_000,
            },
            participants: vec![RawParticipant {
                pubkey: pubkey(1),
                participant_type: "agent".to_string(),
            }],
            pending_intents: Vec::new(),
            unresolved_handoffs: Vec::new(),
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
            moderator_pubkey: pubkey(1),
            baton_config: BatonConfigView {
                progress_interval_ms: 10_000,
                grant_hard_deadline_ms: 300_000,
                agent_safety_margin_ms: 30_000,
            },
            pending_intents: Vec::new(),
            unresolved_handoffs: Vec::new(),
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
            meetings: HashMap::new(),
            pending: VecDeque::new(),
            in_flight: HashMap::new(),
            in_flight_epochs: HashMap::new(),
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
        assert_eq!(recover_interrupted_turns(&mut loaded), (1, 2));
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
            (0, 0),
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

        let (uncertain_rest, uncertain_server) =
            rest_responding_once("400 Bad Request", r#"{"error":"private transport body"}"#).await;
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

        assert_eq!(recover_interrupted_turns(&mut ledger), (1, 1));
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
}
