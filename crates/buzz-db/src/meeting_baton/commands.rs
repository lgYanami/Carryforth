//! Meeting V1 moderated-baton command state machine.
//!
//! Relay handlers validate the signed wire vocabulary and translate it into
//! the typed commands in this module. Every public write entry point repeats
//! the security and logical-state checks while holding the Meeting Session row
//! lock, so an in-memory Relay state can never become authoritative.

use super::*;

use chrono::Duration;
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

/// Parsed Meeting V1 command executed atomically with its State and outbox rows.
pub struct BatonCommandTxParams<'a> {
    /// Community that owns the Meeting Session.
    pub community_id: CommunityId,
    /// Meeting Session and private channel UUID.
    pub session_id: Uuid,
    /// Strictly validated participant-signed command or speech event.
    pub event: &'a Event,
    /// Relay identity used to sign authoritative State events.
    pub relay_keys: &'a Keys,
    /// Typed command payload derived from the signed event.
    pub command: BatonCommand,
}

/// Source selected by a moderator command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatonSelectionSource {
    /// Select one pending Speech Intent.
    Intent {
        /// Stable Submit event ID.
        intent_id: Vec<u8>,
    },
    /// Retry one unresolved Directed Handoff.
    Handoff {
        /// Stable source-speech/handoff ID.
        handoff_id: Vec<u8>,
        /// Compare-and-swap attempt count.
        expected_attempt_count: i32,
    },
}

/// One Intent deferred atomically by a moderator-self Select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatonIntentDeferral {
    /// Stable Intent ID.
    pub intent_id: Vec<u8>,
    /// Current Intent event ID used as a compare-and-swap token.
    pub previous_event_id: Vec<u8>,
    /// Required moderator explanation.
    pub reason: String,
}

/// Terminal outcome reported for a registered moderator decision attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatonDecisionAttemptFinishOutcome {
    /// The model completed and no further protocol action is required.
    Completed,
    /// The result became irrelevant because a shared protocol prerequisite changed.
    Discarded,
}

impl BatonDecisionAttemptFinishOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Discarded => "discarded",
        }
    }
}

/// Deterministic, non-LLM Grant work stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatonProgressStage {
    /// Synchronizing the latest Meeting/project context.
    ContextSync,
    /// Executing a context-gathering tool.
    ToolUse,
    /// Generating a candidate response.
    Generating,
    /// Composing or editing the final response.
    Composing,
    /// Submitting the canonical speech.
    Submitting,
}

impl BatonProgressStage {
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

/// Optional Directed Handoff carried atomically by a canonical speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatonHandoffInput {
    /// Another frozen participant who should answer.
    pub to_pubkey: Vec<u8>,
    /// Closed Handoff reason category.
    pub reason_type: String,
    /// Required explanation visible to the target.
    pub reason_text: String,
}

/// Typed Meeting V1 participant or moderator command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatonCommand {
    /// Submit a new Speech Intent.
    IntentSubmit {
        /// Speech revision on which the Intent was formed.
        basis_speech_revision: i64,
        /// One-sentence proposed contribution.
        summary: String,
        /// Optional participant addressed by the contribution.
        addressed_to: Option<Vec<u8>>,
    },
    /// Refresh the current version of a pending Intent.
    IntentRefresh {
        /// Stable Submit event ID.
        intent_id: Vec<u8>,
        /// Current event ID used as a compare-and-swap token.
        previous_event_id: Vec<u8>,
        /// New speech revision basis.
        basis_speech_revision: i64,
        /// Replacement one-sentence summary.
        summary: String,
        /// Replacement optional addressee.
        addressed_to: Option<Vec<u8>>,
    },
    /// Withdraw a pending Intent.
    IntentWithdraw {
        /// Stable Submit event ID.
        intent_id: Vec<u8>,
        /// Current event ID used as a compare-and-swap token.
        previous_event_id: Vec<u8>,
    },
    /// Select a pending Intent or unresolved Handoff.
    ModeratorSelect {
        /// Authoritative source object.
        source: BatonSelectionSource,
        /// Control-token compare-and-swap epoch.
        expected_control_epoch: i64,
        /// Moderator-window compare-and-swap epoch.
        expected_decision_epoch: i64,
        /// Intent-pool compare-and-swap revision.
        expected_intent_revision: i64,
        /// Speech-timeline compare-and-swap revision.
        expected_speech_revision: i64,
        /// Optional explanation copied into Offer and Grant.
        selection_reason: Option<String>,
        /// Atomic deferrals permitted only for moderator-self selection.
        deferrals: Vec<BatonIntentDeferral>,
        /// Registered model attempt; required for an Agent moderator.
        attempt_id: Option<Vec<u8>>,
        /// Intent version observed by the model; required for attempt-bound Intent selection.
        expected_source_event_id: Option<Vec<u8>>,
    },
    /// Reject a pending Speech Intent.
    ModeratorReject {
        /// Stable Intent ID.
        intent_id: Vec<u8>,
        /// Current Intent event ID used as a compare-and-swap token.
        previous_event_id: Vec<u8>,
        /// Notification pubkey that must equal the persisted Intent author.
        author_pubkey: Vec<u8>,
        /// Closed rejection reason category.
        reason_code: String,
        /// Required moderator explanation.
        reason_text: String,
        /// Registered model attempt; required for an Agent moderator.
        attempt_id: Option<Vec<u8>>,
    },
    /// Close an unresolved Handoff that has no active attempt.
    ModeratorDismissHandoff {
        /// Stable source-speech/handoff ID.
        handoff_id: Vec<u8>,
        /// Speech-timeline compare-and-swap revision.
        expected_speech_revision: i64,
        /// Handoff-attempt compare-and-swap counter.
        expected_attempt_count: i32,
        /// Closed dismissal reason category.
        reason_code: String,
        /// Required moderator explanation.
        reason_text: String,
        /// Registered model attempt; required for an Agent moderator.
        attempt_id: Option<Vec<u8>>,
    },
    /// Withdraw the Agent moderator's own Intent as an attempt-bound management action.
    ModeratorWithdrawSelf {
        /// Registered model attempt.
        attempt_id: Vec<u8>,
        /// Stable self Intent ID.
        intent_id: Vec<u8>,
        /// Intent version observed by the model.
        previous_event_id: Vec<u8>,
    },
    /// Register the authoritative candidate snapshot immediately before model dispatch.
    ModeratorDecisionAttemptStart {
        /// Control-token compare-and-swap epoch.
        expected_control_epoch: i64,
        /// Moderator-window compare-and-swap epoch.
        expected_decision_epoch: i64,
        /// Intent-pool compare-and-swap revision.
        expected_intent_revision: i64,
        /// Speech-timeline compare-and-swap revision.
        expected_speech_revision: i64,
        /// Relay State event observed by the Controller before starting.
        expected_state_event_id: Vec<u8>,
        /// Abandoned attempt replaced after Runtime recovery, when applicable.
        replacement_of_attempt_id: Option<Vec<u8>>,
    },
    /// Terminalize a registered model attempt that has no primary protocol action.
    ModeratorDecisionAttemptFinish {
        /// Registered attempt.
        attempt_id: Vec<u8>,
        /// Completed or discarded terminal class.
        outcome: BatonDecisionAttemptFinishOutcome,
        /// Closed, machine-readable terminal explanation.
        reason_code: String,
    },
    /// Consume Relay-issued selected-source evidence and atomically register a replacement attempt.
    ModeratorDecisionRetry {
        /// Failed attempt.
        attempt_id: Vec<u8>,
        /// One-use Relay retry ticket.
        retry_ticket_id: Vec<u8>,
        /// Signed primary action whose rejection created the ticket.
        failed_action_event_id: Vec<u8>,
        /// Current control-token compare-and-swap epoch.
        expected_control_epoch: i64,
        /// Current moderator-window compare-and-swap epoch.
        expected_decision_epoch: i64,
        /// Failed attempt number.
        expected_attempt_number: i32,
    },
    /// Close an exhausted current Cohort and atomically expose the next eligible batch.
    ModeratorCompleteCohort {
        /// Registered attempt responsible for completing the Cohort.
        attempt_id: Vec<u8>,
        /// Current control-token compare-and-swap epoch.
        expected_control_epoch: i64,
        /// Current moderator-window compare-and-swap epoch.
        expected_decision_epoch: i64,
    },
    /// Mark a running attempt abandoned after its owning Runtime was lost.
    ModeratorDecisionAttemptAbandon {
        /// Registered attempt.
        attempt_id: Vec<u8>,
    },
    /// Recall control now or latch a forced moderator return.
    ModeratorRecall {
        /// Control epoch to which the Recall applies.
        control_epoch: i64,
        /// Optional explanation.
        reason: Option<String>,
    },
    /// Queue a Human Floor Request.
    HumanRequest,
    /// Withdraw a queued or currently offered Human Floor Request.
    HumanWithdraw {
        /// Stable Request event ID.
        request_id: Vec<u8>,
    },
    /// Accept the currently active Offer.
    OfferAck {
        /// Stable Relay-generated Offer ID.
        offer_id: Vec<u8>,
    },
    /// Decline the currently active Offer.
    OfferDecline {
        /// Stable Relay-generated Offer ID.
        offer_id: Vec<u8>,
        /// Optional short explanation.
        reason: Option<String>,
    },
    /// Advance an active Grant's deterministic progress lease.
    GrantProgress {
        /// Stable Relay-generated Grant ID.
        grant_id: Vec<u8>,
        /// Strictly next monotonic sequence.
        progress_seq: i64,
        /// Observable local execution stage.
        stage: BatonProgressStage,
    },
    /// End an active Grant without speaking.
    GrantYield {
        /// Stable Relay-generated Grant ID.
        grant_id: Vec<u8>,
        /// Optional closed Yield reason category.
        reason_code: Option<String>,
        /// Optional short explanation.
        reason: Option<String>,
    },
    /// Publish the one canonical speech consuming an active Grant.
    Speech {
        /// Stable Relay-generated Grant ID.
        grant_id: Vec<u8>,
        /// Exactly the next Session speech revision.
        speech_revision: i64,
        /// Optional atomic Directed Handoff.
        handoff: Option<BatonHandoffInput>,
    },
}

impl BatonCommand {
    fn expected_kind(&self) -> u32 {
        match self {
            Self::IntentSubmit { .. }
            | Self::IntentRefresh { .. }
            | Self::IntentWithdraw { .. } => buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            Self::ModeratorSelect { .. }
            | Self::ModeratorReject { .. }
            | Self::ModeratorDismissHandoff { .. }
            | Self::ModeratorWithdrawSelf { .. }
            | Self::ModeratorDecisionAttemptStart { .. }
            | Self::ModeratorDecisionAttemptFinish { .. }
            | Self::ModeratorDecisionRetry { .. }
            | Self::ModeratorCompleteCohort { .. }
            | Self::ModeratorDecisionAttemptAbandon { .. }
            | Self::ModeratorRecall { .. } => buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            Self::HumanRequest | Self::HumanWithdraw { .. } => {
                buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST
            }
            Self::OfferAck { .. } | Self::OfferDecline { .. } => {
                buzz_core::kind::KIND_MEETING_OFFER_RESPONSE
            }
            Self::GrantProgress { .. } | Self::GrantYield { .. } => {
                buzz_core::kind::KIND_MEETING_GRANT_SIGNAL
            }
            Self::Speech { .. } => 9,
        }
    }

    fn action(&self) -> &'static str {
        match self {
            Self::IntentSubmit { .. } => "intent_submit",
            Self::IntentRefresh { .. } => "intent_refresh",
            Self::IntentWithdraw { .. } => "intent_withdraw",
            Self::ModeratorSelect { .. } => "moderator_select",
            Self::ModeratorReject { .. } => "moderator_reject",
            Self::ModeratorDismissHandoff { .. } => "moderator_dismiss_handoff",
            Self::ModeratorWithdrawSelf { .. } => "moderator_withdraw_self",
            Self::ModeratorDecisionAttemptStart { .. } => "decision_attempt_start",
            Self::ModeratorDecisionAttemptFinish { .. } => "decision_attempt_finish",
            Self::ModeratorDecisionRetry { .. } => "decision_retry",
            Self::ModeratorCompleteCohort { .. } => "complete_cohort",
            Self::ModeratorDecisionAttemptAbandon { .. } => "decision_attempt_abandon",
            Self::ModeratorRecall { .. } => "moderator_recall",
            Self::HumanRequest => "human_request",
            Self::HumanWithdraw { .. } => "human_withdraw",
            Self::OfferAck { .. } => "offer_ack",
            Self::OfferDecline { .. } => "offer_decline",
            Self::GrantProgress { .. } => "grant_progress",
            Self::GrantYield { .. } => "grant_yield",
            Self::Speech { .. } => "speech",
        }
    }
}

/// Result of a terminal or accepted command inside a committed transaction.
#[derive(Debug, Clone, PartialEq)]
pub enum BatonCommandOutcome {
    /// The signed command became part of the canonical Meeting log.
    Accepted {
        /// Stable Intent/Request/Offer/Grant/Speech object affected.
        canonical_object_id: Option<Vec<u8>>,
        /// State revision produced by the command transition.
        state_revision: i64,
    },
    /// The identical signed command was already processed.
    Duplicate {
        /// Whether the first execution accepted and published the command.
        accepted: bool,
        /// `accepted`, `rejected_terminal`, or `rejected_after_recovery`.
        outcome_class: String,
        /// Stable canonical object recovered from the receipt.
        canonical_object_id: Option<Vec<u8>>,
        /// State revision recorded by the first execution.
        state_revision: Option<i64>,
        /// First execution's outcome code.
        outcome_code: String,
        /// One-use retry evidence returned by the first execution, when any.
        retry_ticket_id: Option<Vec<u8>>,
    },
    /// A semantic conflict was terminal without a deadline transition.
    RejectedTerminal {
        /// Stable machine-readable rejection code.
        code: String,
        /// Canonical object that won the race, when known.
        canonical_object_id: Option<Vec<u8>>,
        /// One-use selected-source conflict evidence.
        retry_ticket_id: Option<Vec<u8>>,
    },
    /// Lazy recovery committed first and made this command terminally late.
    RejectedAfterRecovery {
        /// Stable machine-readable rejection code.
        code: String,
        /// Canonical expired/timed-out object, when known.
        canonical_object_id: Option<Vec<u8>>,
        /// One-use selected-source conflict evidence.
        retry_ticket_id: Option<Vec<u8>>,
    },
}

/// One Relay State produced by deadline or security recovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatonTransitionResult {
    /// Transition primary type.
    pub primary_type: String,
    /// New Relay State revision.
    pub state_revision: i64,
    /// Relay-signed State event ID.
    pub state_event_id: Vec<u8>,
}

/// Fully committed lazy-recovery and command result.
#[derive(Debug, Clone)]
pub struct BatonCommitResult {
    /// Recovery transitions committed before command validation.
    pub recovery_transitions: Vec<BatonTransitionResult>,
    /// Accepted, duplicate, or terminal semantic outcome.
    pub command_outcome: BatonCommandOutcome,
    /// Current authoritative projection after commit.
    pub snapshot: BatonSnapshot,
}

/// One Session whose current deadline may require recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatonDueSession {
    /// Owning Community.
    pub community_id: CommunityId,
    /// Meeting Session UUID.
    pub session_id: Uuid,
    /// Database deadline that made this Session eligible for recovery.
    pub next_action_at: DateTime<Utc>,
}

#[derive(Debug)]
struct Actor {
    pubkey: Vec<u8>,
    participant_type: ParticipantType,
    is_moderator: bool,
}

#[derive(Debug, Clone, Copy)]
struct RevisionDelta {
    floor: bool,
    intent: bool,
    speech: bool,
}

impl RevisionDelta {
    const FLOOR: Self = Self {
        floor: true,
        intent: false,
        speech: false,
    };
    const FLOOR_INTENT: Self = Self {
        floor: true,
        intent: true,
        speech: false,
    };
    const FLOOR_SPEECH: Self = Self {
        floor: true,
        intent: false,
        speech: true,
    };
    const ALL: Self = Self {
        floor: true,
        intent: true,
        speech: true,
    };
}

#[derive(Debug, Clone)]
struct StateTarget {
    phase: BatonPhase,
    active_offer_id: Option<Vec<u8>>,
    active_grant_id: Option<Vec<u8>>,
    handoff_depth: i32,
    consecutive_moderator_speeches: i32,
    forced_return_to_moderator: bool,
    recall_event_id: Option<Vec<u8>>,
    moderator_decision_started_at: Option<DateTime<Utc>>,
    moderator_decision_deadline: Option<DateTime<Utc>>,
    next_action_at: Option<DateTime<Utc>>,
    control_epoch: i64,
    decision_epoch: i64,
    decision_attempt: i32,
    active_decision_attempt_id: Option<Vec<u8>>,
}

impl StateTarget {
    fn from_state(state: &StateRow) -> Self {
        Self {
            phase: state.phase,
            active_offer_id: state.active_offer_id.clone(),
            active_grant_id: state.active_grant_id.clone(),
            handoff_depth: state.handoff_depth,
            consecutive_moderator_speeches: state.consecutive_moderator_speeches,
            forced_return_to_moderator: state.forced_return_to_moderator,
            recall_event_id: state.recall_event_id.clone(),
            moderator_decision_started_at: state.moderator_decision_started_at,
            moderator_decision_deadline: state.moderator_decision_deadline,
            next_action_at: state.next_action_at,
            control_epoch: state.control_epoch,
            decision_epoch: state.decision_epoch,
            decision_attempt: state.decision_attempt,
            active_decision_attempt_id: state.active_decision_attempt_id.clone(),
        }
    }

    fn offered(state: &StateRow, offer_id: Vec<u8>, deadline: DateTime<Utc>) -> Self {
        let mut target = Self::from_state(state);
        target.phase = BatonPhase::Offered;
        target.active_offer_id = Some(offer_id);
        target.active_grant_id = None;
        target.moderator_decision_started_at = None;
        target.moderator_decision_deadline = None;
        target.next_action_at = Some(deadline);
        target
    }

    fn granted(
        state: &StateRow,
        grant_id: Vec<u8>,
        next_action_at: DateTime<Utc>,
        handoff_depth: i32,
    ) -> Self {
        let mut target = Self::from_state(state);
        target.phase = BatonPhase::Granted;
        target.active_offer_id = None;
        target.active_grant_id = Some(grant_id);
        target.handoff_depth = handoff_depth;
        target.moderator_decision_started_at = None;
        target.moderator_decision_deadline = None;
        target.next_action_at = Some(next_action_at);
        target
    }
}

#[derive(Debug)]
struct TransitionSpec {
    primary_type: &'static str,
    primary_object_id: Option<Vec<u8>>,
    caused_by_event_id: Option<Vec<u8>>,
    deadline_type: Option<&'static str>,
    outcome: &'static str,
    blocked_by: Option<&'static str>,
    effects: Vec<Value>,
}

impl TransitionSpec {
    fn command(
        primary_type: &'static str,
        primary_object_id: Option<Vec<u8>>,
        event_id: &[u8],
        effects: Vec<Value>,
    ) -> Self {
        Self {
            primary_type,
            primary_object_id,
            caused_by_event_id: Some(event_id.to_vec()),
            deadline_type: None,
            outcome: "accepted",
            blocked_by: None,
            effects,
        }
    }

    fn deadline(
        primary_type: &'static str,
        primary_object_id: Option<Vec<u8>>,
        deadline_type: &'static str,
        effects: Vec<Value>,
    ) -> Self {
        Self {
            primary_type,
            primary_object_id,
            caused_by_event_id: None,
            deadline_type: Some(deadline_type),
            outcome: "accepted",
            blocked_by: None,
            effects,
        }
    }

    fn json(&self, now: DateTime<Utc>) -> Value {
        json!({
            "primary_type": self.primary_type,
            "outcome": self.outcome,
            "primary_object_id": self.primary_object_id.as_deref().map(hex::encode),
            "caused_by_event_id": self.caused_by_event_id.as_deref().map(hex::encode),
            "deadline_type": self.deadline_type,
            "blocked_by": self.blocked_by,
            "at_ms": now.timestamp_millis(),
            "effects": self.effects,
        })
    }
}

fn effect(
    effect_type: &str,
    object_type: &str,
    object_id: &[u8],
    from: Option<&str>,
    to: Option<&str>,
) -> Value {
    json!({
        "type": effect_type,
        "object_type": object_type,
        "object_id": hex::encode(object_id),
        "from": from,
        "to": to,
    })
}

fn phase_effect(session_id: Uuid, from: BatonPhase, to: BatonPhase) -> Value {
    json!({
        "type": "phase_changed",
        "object_type": "phase",
        "object_id": session_id.to_string(),
        "from": from,
        "to": to,
    })
}

fn control_effect(session_id: Uuid, effect_type: &str) -> Value {
    json!({
        "type": effect_type,
        "object_type": "control",
        "object_id": session_id.to_string(),
        "from": null,
        "to": null,
    })
}

fn append_control_return_effects(effects: &mut Vec<Value>, state: &StateRow, session_id: Uuid) {
    if state.forced_return_to_moderator {
        if let Some(recall_event_id) = state.recall_event_id.as_deref() {
            effects.push(effect(
                "recall_cleared",
                "recall",
                recall_event_id,
                Some("latched"),
                Some("cleared"),
            ));
        }
        effects.push(control_effect(session_id, "forced_return_completed"));
    } else {
        effects.push(control_effect(session_id, "control_returned"));
    }
}

fn sort_effects_by_object_id(effects: &mut [Value]) {
    effects.sort_by(|left, right| {
        left.get("object_id")
            .and_then(Value::as_str)
            .cmp(&right.get("object_id").and_then(Value::as_str))
    });
}

fn random_object_id() -> Vec<u8> {
    let bytes: [u8; 32] = rand::random();
    bytes.to_vec()
}

fn timestamp_ms(value: Option<DateTime<Utc>>) -> Option<i64> {
    value.map(|timestamp| timestamp.timestamp_millis())
}

fn validate_text(value: &str, field: &str, min_bytes: usize, max_bytes: usize) -> Result<()> {
    let length = value.len();
    if length < min_bytes || length > max_bytes {
        return Err(DbError::InvalidData(format!(
            "{field} must contain {min_bytes}..={max_bytes} UTF-8 bytes"
        )));
    }
    if value.trim() != value {
        return Err(DbError::InvalidData(format!(
            "{field} must not have surrounding whitespace"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(DbError::InvalidData(format!(
            "{field} must not contain control characters"
        )));
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>, field: &str, max_bytes: usize) -> Result<()> {
    if let Some(value) = value {
        validate_text(value, field, 1, max_bytes)?;
    }
    Ok(())
}

fn validate_id(value: &[u8], field: &str) -> Result<()> {
    validate_32_bytes(value, field)
}

fn expected_event_kind(command: &BatonCommand) -> Result<Kind> {
    let value = u16::try_from(command.expected_kind()).map_err(|_| {
        DbError::InvalidData(format!(
            "Meeting V1 command kind {} is outside the Nostr range",
            command.expected_kind()
        ))
    })?;
    Ok(Kind::Custom(value))
}

fn preflight_command(params: &BatonCommandTxParams<'_>) -> Result<()> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting session id must not be nil".to_string(),
        ));
    }
    params
        .event
        .verify()
        .map_err(|error| DbError::InvalidData(format!("invalid Meeting V1 event: {error}")))?;
    let expected = expected_event_kind(&params.command)?;
    if params.event.kind != expected {
        return Err(DbError::InvalidData(format!(
            "Meeting V1 {} uses kind {}, expected {}",
            params.command.action(),
            params.event.kind.as_u16(),
            expected.as_u16()
        )));
    }
    validate_command_input(&params.command)
}

fn validate_command_input(command: &BatonCommand) -> Result<()> {
    match command {
        BatonCommand::IntentSubmit {
            basis_speech_revision,
            summary,
            addressed_to,
        } => {
            if *basis_speech_revision < 0 {
                return Err(DbError::InvalidData(
                    "Intent basis speech revision must be non-negative".to_string(),
                ));
            }
            validate_text(summary, "Intent summary", 1, 512)?;
            if let Some(addressed_to) = addressed_to {
                validate_id(addressed_to, "Intent addressee")?;
            }
        }
        BatonCommand::IntentRefresh {
            intent_id,
            previous_event_id,
            basis_speech_revision,
            summary,
            addressed_to,
        } => {
            validate_id(intent_id, "Intent id")?;
            validate_id(previous_event_id, "previous Intent event id")?;
            if *basis_speech_revision < 0 {
                return Err(DbError::InvalidData(
                    "Intent basis speech revision must be non-negative".to_string(),
                ));
            }
            validate_text(summary, "Intent summary", 1, 512)?;
            if let Some(addressed_to) = addressed_to {
                validate_id(addressed_to, "Intent addressee")?;
            }
        }
        BatonCommand::IntentWithdraw {
            intent_id,
            previous_event_id,
        } => {
            validate_id(intent_id, "Intent id")?;
            validate_id(previous_event_id, "previous Intent event id")?;
        }
        BatonCommand::ModeratorSelect {
            source,
            expected_control_epoch,
            expected_decision_epoch,
            expected_intent_revision,
            expected_speech_revision,
            selection_reason,
            deferrals,
            attempt_id,
            expected_source_event_id,
        } => {
            for (field, value) in [
                ("expected control epoch", *expected_control_epoch),
                ("expected decision epoch", *expected_decision_epoch),
                ("expected Intent revision", *expected_intent_revision),
                ("expected speech revision", *expected_speech_revision),
            ] {
                if value < 0 {
                    return Err(DbError::InvalidData(format!(
                        "{field} must be non-negative"
                    )));
                }
            }
            if *expected_control_epoch == 0 {
                return Err(DbError::InvalidData(
                    "expected control epoch must be positive".to_string(),
                ));
            }
            match source {
                BatonSelectionSource::Intent { intent_id } => {
                    validate_id(intent_id, "selected Intent id")?;
                    if let Some(expected_source_event_id) = expected_source_event_id {
                        validate_id(
                            expected_source_event_id,
                            "selected Intent snapshot event id",
                        )?;
                    }
                }
                BatonSelectionSource::Handoff {
                    handoff_id,
                    expected_attempt_count,
                } => {
                    validate_id(handoff_id, "selected Handoff id")?;
                    if *expected_attempt_count < 0 {
                        return Err(DbError::InvalidData(
                            "expected Handoff attempt count must be non-negative".to_string(),
                        ));
                    }
                    if expected_source_event_id.is_some() {
                        return Err(DbError::InvalidData(
                            "Handoff Select cannot carry an Intent source event id".to_string(),
                        ));
                    }
                }
            }
            if let Some(attempt_id) = attempt_id {
                validate_id(attempt_id, "moderator decision attempt id")?;
            }
            validate_optional_text(selection_reason.as_deref(), "selection reason", 512)?;
            if deferrals.len() > MAX_MEETING_PARTICIPANTS {
                return Err(DbError::InvalidData(format!(
                    "Select deferrals exceed the {MAX_MEETING_PARTICIPANTS}-participant limit"
                )));
            }
            let mut ids = HashSet::with_capacity(deferrals.len());
            for deferral in deferrals {
                validate_id(&deferral.intent_id, "deferred Intent id")?;
                validate_id(
                    &deferral.previous_event_id,
                    "deferred Intent previous event id",
                )?;
                validate_text(&deferral.reason, "Intent deferral reason", 1, 1024)?;
                if !ids.insert(deferral.intent_id.as_slice()) {
                    return Err(DbError::InvalidData(
                        "a Select cannot defer the same Intent twice".to_string(),
                    ));
                }
            }
        }
        BatonCommand::ModeratorReject {
            intent_id,
            previous_event_id,
            author_pubkey,
            reason_code,
            reason_text,
            attempt_id,
        } => {
            validate_id(intent_id, "rejected Intent id")?;
            validate_id(previous_event_id, "previous Intent event id")?;
            validate_id(author_pubkey, "rejected Intent author")?;
            if !matches!(
                reason_code.as_str(),
                "off_topic" | "duplicate" | "superseded" | "unsupported" | "agenda_mismatch"
            ) {
                return Err(DbError::InvalidData(
                    "unsupported moderator rejection reason code".to_string(),
                ));
            }
            validate_text(reason_text, "Intent rejection reason", 1, 1024)?;
            if let Some(attempt_id) = attempt_id {
                validate_id(attempt_id, "moderator decision attempt id")?;
            }
        }
        BatonCommand::ModeratorDismissHandoff {
            handoff_id,
            expected_speech_revision,
            expected_attempt_count,
            reason_code,
            reason_text,
            attempt_id,
        } => {
            validate_id(handoff_id, "dismissed Handoff id")?;
            if *expected_speech_revision < 0 || *expected_attempt_count < 0 {
                return Err(DbError::InvalidData(
                    "Handoff dismissal expectations must be non-negative".to_string(),
                ));
            }
            if !matches!(
                reason_code.as_str(),
                "superseded" | "answered_elsewhere" | "out_of_scope" | "no_longer_needed"
            ) {
                return Err(DbError::InvalidData(
                    "unsupported Handoff dismissal reason code".to_string(),
                ));
            }
            validate_text(reason_text, "Handoff dismissal reason", 1, 1024)?;
            if let Some(attempt_id) = attempt_id {
                validate_id(attempt_id, "moderator decision attempt id")?;
            }
        }
        BatonCommand::ModeratorWithdrawSelf {
            attempt_id,
            intent_id,
            previous_event_id,
        } => {
            validate_id(attempt_id, "moderator decision attempt id")?;
            validate_id(intent_id, "withdrawn moderator Intent id")?;
            validate_id(previous_event_id, "previous moderator Intent event id")?;
        }
        BatonCommand::ModeratorDecisionAttemptStart {
            expected_control_epoch,
            expected_decision_epoch,
            expected_intent_revision,
            expected_speech_revision,
            expected_state_event_id,
            replacement_of_attempt_id,
        } => {
            if *expected_control_epoch <= 0
                || *expected_decision_epoch < 0
                || *expected_intent_revision < 0
                || *expected_speech_revision < 0
            {
                return Err(DbError::InvalidData(
                    "moderator DecisionAttempt expectations are out of range".to_string(),
                ));
            }
            validate_id(expected_state_event_id, "expected Relay State event id")?;
            if let Some(replacement_of_attempt_id) = replacement_of_attempt_id {
                validate_id(
                    replacement_of_attempt_id,
                    "replaced moderator decision attempt id",
                )?;
            }
        }
        BatonCommand::ModeratorDecisionAttemptFinish {
            attempt_id,
            outcome,
            reason_code,
        } => {
            validate_id(attempt_id, "moderator decision attempt id")?;
            let supported = match outcome {
                BatonDecisionAttemptFinishOutcome::Completed => {
                    matches!(reason_code.as_str(), "no_action" | "idle_wait_fallback")
                }
                BatonDecisionAttemptFinishOutcome::Discarded => matches!(
                    reason_code.as_str(),
                    "human_priority"
                        | "control_changed"
                        | "speech_changed"
                        | "meeting_ended"
                        | "moderator_changed"
                        | "cas_churn"
                        | "source_changed"
                        | "runtime_replaced"
                ),
            };
            if !supported {
                return Err(DbError::InvalidData(
                    "unsupported moderator DecisionAttempt finish reason".to_string(),
                ));
            }
        }
        BatonCommand::ModeratorDecisionRetry {
            attempt_id,
            retry_ticket_id,
            failed_action_event_id,
            expected_control_epoch,
            expected_decision_epoch,
            expected_attempt_number,
        } => {
            validate_id(attempt_id, "moderator decision attempt id")?;
            validate_id(retry_ticket_id, "moderator retry ticket id")?;
            validate_id(failed_action_event_id, "failed moderator action event id")?;
            if *expected_control_epoch <= 0
                || *expected_decision_epoch <= 0
                || *expected_attempt_number <= 0
            {
                return Err(DbError::InvalidData(
                    "moderator DecisionRetry expectations must be positive".to_string(),
                ));
            }
        }
        BatonCommand::ModeratorCompleteCohort {
            attempt_id,
            expected_control_epoch,
            expected_decision_epoch,
        } => {
            validate_id(attempt_id, "moderator decision attempt id")?;
            if *expected_control_epoch <= 0 || *expected_decision_epoch <= 0 {
                return Err(DbError::InvalidData(
                    "Cohort completion epochs must be positive".to_string(),
                ));
            }
        }
        BatonCommand::ModeratorDecisionAttemptAbandon { attempt_id } => {
            validate_id(attempt_id, "moderator decision attempt id")?;
        }
        BatonCommand::ModeratorRecall {
            control_epoch,
            reason,
        } => {
            if *control_epoch <= 0 {
                return Err(DbError::InvalidData(
                    "Recall control epoch must be positive".to_string(),
                ));
            }
            validate_optional_text(reason.as_deref(), "Recall reason", 1024)?;
        }
        BatonCommand::HumanRequest => {}
        BatonCommand::HumanWithdraw { request_id } => validate_id(request_id, "Human Request id")?,
        BatonCommand::OfferAck { offer_id } => validate_id(offer_id, "Offer id")?,
        BatonCommand::OfferDecline { offer_id, reason } => {
            validate_id(offer_id, "Offer id")?;
            validate_optional_text(reason.as_deref(), "Offer decline reason", 512)?;
        }
        BatonCommand::GrantProgress {
            grant_id,
            progress_seq,
            ..
        } => {
            validate_id(grant_id, "Grant id")?;
            if *progress_seq <= 0 {
                return Err(DbError::InvalidData(
                    "Grant Progress sequence must be positive".to_string(),
                ));
            }
        }
        BatonCommand::GrantYield {
            grant_id,
            reason_code,
            reason,
        } => {
            validate_id(grant_id, "Grant id")?;
            if let Some(reason_code) = reason_code {
                if !matches!(
                    reason_code.as_str(),
                    "no_longer_needed"
                        | "unable_to_answer"
                        | "insufficient_context"
                        | "tool_failure"
                        | "cancelled"
                ) {
                    return Err(DbError::InvalidData(
                        "unsupported Grant Yield reason code".to_string(),
                    ));
                }
            }
            validate_optional_text(reason.as_deref(), "Grant Yield reason", 512)?;
        }
        BatonCommand::Speech {
            grant_id,
            speech_revision,
            handoff,
        } => {
            validate_id(grant_id, "Grant id")?;
            if *speech_revision <= 0 {
                return Err(DbError::InvalidData(
                    "speech revision must be positive".to_string(),
                ));
            }
            if let Some(handoff) = handoff {
                validate_id(&handoff.to_pubkey, "Handoff target")?;
                if !matches!(
                    handoff.reason_type.as_str(),
                    "question"
                        | "information_request"
                        | "clarification"
                        | "review"
                        | "response_requested"
                ) {
                    return Err(DbError::InvalidData(
                        "unsupported Handoff reason type".to_string(),
                    ));
                }
                validate_text(&handoff.reason_text, "Handoff reason", 1, 1024)?;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct IntentRow {
    intent_id: Vec<u8>,
    author_pubkey: Vec<u8>,
    current_event_id: Vec<u8>,
    state: String,
    deferred_by_offer_id: Option<Vec<u8>>,
    eligible_decision_epoch: i64,
}

#[derive(Debug, Clone)]
struct HumanRequestRow {
    request_id: Vec<u8>,
    requester_pubkey: Vec<u8>,
    state: String,
    offer_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct OfferRow {
    offer_id: Vec<u8>,
    target_pubkey: Vec<u8>,
    allocation_source: String,
    turn_role: String,
    allocation_event_id: Option<Vec<u8>>,
    selection_reason: Option<String>,
    source_intent_id: Option<Vec<u8>>,
    source_request_id: Option<Vec<u8>>,
    source_handoff_id: Option<Vec<u8>>,
    source_speech_event_id: Option<Vec<u8>>,
    reason_type: Option<String>,
    reason_text: Option<String>,
    basis_speech_revision: i64,
    depth_mode: String,
    previous_handoff_depth: i32,
    requested_handoff_depth: i32,
    ack_deadline: DateTime<Utc>,
    state: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct GrantRow {
    grant_id: Vec<u8>,
    holder_pubkey: Vec<u8>,
    allocation_source: String,
    turn_role: String,
    source_offer_id: Vec<u8>,
    allocation_event_id: Option<Vec<u8>>,
    selection_reason: Option<String>,
    source_intent_id: Option<Vec<u8>>,
    source_request_id: Option<Vec<u8>>,
    source_handoff_id: Option<Vec<u8>>,
    source_speech_event_id: Option<Vec<u8>>,
    basis_speech_revision: i64,
    depth_mode: String,
    previous_handoff_depth: i32,
    handoff_depth: i32,
    soft_lease_expires_at: DateTime<Utc>,
    hard_deadline: DateTime<Utc>,
    progress_seq: i64,
    state: String,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
struct HandoffRow {
    handoff_id: Vec<u8>,
    source_speech_event_id: Vec<u8>,
    from_pubkey: Vec<u8>,
    to_pubkey: Vec<u8>,
    reason_type: String,
    reason_text: String,
    question_state: String,
    blocked_by: Option<String>,
    last_offer_id: Option<Vec<u8>>,
    last_grant_id: Option<Vec<u8>>,
    attempt_count: i32,
    eligible_decision_epoch: i64,
}

#[derive(Debug)]
struct ReceiptRow {
    author_pubkey: Vec<u8>,
    accepted: bool,
    outcome_class: String,
    outcome_code: String,
    canonical_object_id: Option<Vec<u8>>,
    state_revision: Option<i64>,
    retry_ticket_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct RetryTicketDraft {
    retry_ticket_id: Vec<u8>,
    attempt_id: Vec<u8>,
    failed_action_event_id: Vec<u8>,
    source_type: &'static str,
    source_id: Vec<u8>,
    snapshot_source_event_id: Option<Vec<u8>>,
    snapshot_handoff_attempt_count: Option<i32>,
    conflict_code: &'static str,
    control_epoch: i64,
    decision_epoch: i64,
    deadline_at: DateTime<Utc>,
}

#[derive(Debug)]
struct RetryTicketRow {
    retry_ticket_id: Vec<u8>,
    attempt_id: Vec<u8>,
    failed_action_event_id: Vec<u8>,
    source_type: String,
    source_id: Vec<u8>,
    snapshot_source_event_id: Option<Vec<u8>>,
    snapshot_handoff_attempt_count: Option<i32>,
    control_epoch: i64,
    decision_epoch: i64,
    deadline_at: DateTime<Utc>,
    consumed_at: Option<DateTime<Utc>>,
}

async fn database_now(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
    Ok(sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?)
}

async fn load_actor_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    author_pubkey: &[u8],
) -> Result<Actor> {
    validate_id(author_pubkey, "command author")?;
    let row = sqlx::query(
        "SELECT participant_type, \
                pubkey = (SELECT moderator_pubkey FROM meeting_sessions \
                          WHERE community_id = $1 AND session_id = $2) AS is_moderator \
         FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2 AND pubkey = $3 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(author_pubkey)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(row) = row else {
        return Err(DbError::AccessDenied(
            "not authorized for this private meeting".to_string(),
        ));
    };
    let participant_type: String = row.try_get("participant_type")?;
    Ok(Actor {
        pubkey: author_pubkey.to_vec(),
        participant_type: ParticipantType::parse(&participant_type)?,
        is_moderator: row.try_get("is_moderator")?,
    })
}

async fn ensure_participant_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    pubkey: &[u8],
) -> Result<ParticipantType> {
    validate_id(pubkey, "participant pubkey")?;
    let participant_type: Option<String> = sqlx::query_scalar(
        "SELECT participant_type FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2 AND pubkey = $3 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(pubkey)
    .fetch_optional(tx.as_mut())
    .await?;
    let participant_type = participant_type.ok_or_else(|| {
        DbError::InvalidData("referenced identity is not a frozen participant".to_string())
    })?;
    ParticipantType::parse(&participant_type)
}

async fn actor_security_active_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<bool> {
    crate::meeting::actor_security_active_tx(tx, community_id, pubkey).await
}

async fn preauthorize_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    actor: &Actor,
    command: &BatonCommand,
) -> Result<()> {
    match command {
        BatonCommand::IntentSubmit { addressed_to, .. } => {
            if let Some(addressed_to) = addressed_to {
                if addressed_to == &actor.pubkey {
                    return Err(DbError::InvalidData(
                        "Intent addressee must be another participant".to_string(),
                    ));
                }
                ensure_participant_tx(tx, community_id, session_id, addressed_to).await?;
            }
        }
        BatonCommand::IntentRefresh {
            intent_id,
            addressed_to,
            ..
        } => {
            if let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? {
                if intent.author_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "only the Intent author can refresh it".to_string(),
                    ));
                }
            }
            if let Some(addressed_to) = addressed_to {
                if addressed_to == &actor.pubkey {
                    return Err(DbError::InvalidData(
                        "Intent addressee must be another participant".to_string(),
                    ));
                }
                ensure_participant_tx(tx, community_id, session_id, addressed_to).await?;
            }
        }
        BatonCommand::IntentWithdraw { intent_id, .. } => {
            if actor.is_moderator && actor.participant_type == ParticipantType::Agent {
                return Err(DbError::AccessDenied(
                    "an Agent moderator must use attempt-bound withdraw-self".to_string(),
                ));
            }
            if let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? {
                if intent.author_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "only the Intent author can withdraw it".to_string(),
                    ));
                }
            }
        }
        BatonCommand::ModeratorSelect {
            source, deferrals, ..
        } => {
            if !actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only the frozen Meeting moderator can issue this command".to_string(),
                ));
            }
            if !deferrals.is_empty() {
                match source {
                    BatonSelectionSource::Intent { intent_id } => {
                        if let Some(intent) =
                            load_intent_tx(tx, community_id, session_id, intent_id).await?
                        {
                            if intent.author_pubkey != actor.pubkey {
                                return Err(DbError::InvalidData(
                                    "only moderator-self Select can carry deferrals".to_string(),
                                ));
                            }
                        }
                    }
                    BatonSelectionSource::Handoff { .. } => {
                        return Err(DbError::InvalidData(
                            "Handoff Select cannot carry Intent deferrals".to_string(),
                        ));
                    }
                }
            }
        }
        BatonCommand::ModeratorDismissHandoff { .. }
        | BatonCommand::ModeratorDecisionAttemptStart { .. }
        | BatonCommand::ModeratorDecisionAttemptFinish { .. }
        | BatonCommand::ModeratorDecisionRetry { .. }
        | BatonCommand::ModeratorCompleteCohort { .. }
        | BatonCommand::ModeratorDecisionAttemptAbandon { .. }
        | BatonCommand::ModeratorRecall { .. } => {
            if !actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only the frozen Meeting moderator can issue this command".to_string(),
                ));
            }
        }
        BatonCommand::ModeratorWithdrawSelf { intent_id, .. } => {
            if !actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only the frozen Meeting moderator can issue this command".to_string(),
                ));
            }
            if let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? {
                if intent.author_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "the moderator can only withdraw its own Intent".to_string(),
                    ));
                }
            }
        }
        BatonCommand::ModeratorReject {
            intent_id,
            author_pubkey,
            ..
        } => {
            if !actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only the frozen Meeting moderator can reject an Intent".to_string(),
                ));
            }
            if let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? {
                if intent.author_pubkey != *author_pubkey {
                    return Err(DbError::InvalidData(
                        "rejection notification pubkey does not match the Intent author"
                            .to_string(),
                    ));
                }
            }
        }
        BatonCommand::HumanRequest => {
            if actor.participant_type != ParticipantType::Human || actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only a non-moderator Human participant can request the floor directly"
                        .to_string(),
                ));
            }
        }
        BatonCommand::HumanWithdraw { request_id } => {
            if actor.participant_type != ParticipantType::Human || actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only a non-moderator Human participant can withdraw a Human Request"
                        .to_string(),
                ));
            }
            if let Some(request) = load_request_tx(tx, community_id, session_id, request_id).await?
            {
                if request.requester_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "only the Human Request author can withdraw it".to_string(),
                    ));
                }
            }
        }
        BatonCommand::OfferAck { offer_id } | BatonCommand::OfferDecline { offer_id, .. } => {
            if let Some(offer) = load_offer_tx(tx, community_id, session_id, offer_id).await? {
                if offer.target_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "only the referenced Offer target can respond".to_string(),
                    ));
                }
            }
        }
        BatonCommand::GrantProgress { grant_id, .. }
        | BatonCommand::GrantYield { grant_id, .. }
        | BatonCommand::Speech { grant_id, .. } => {
            if let Some(grant) = load_grant_tx(tx, community_id, session_id, grant_id).await? {
                if grant.holder_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "only the referenced Grant holder can act on it".to_string(),
                    ));
                }
            }
            if let BatonCommand::Speech { handoff, .. } = command {
                if event.content.is_empty() || event.content.len() > 256 * 1024 {
                    return Err(DbError::InvalidData(
                        "Meeting speech content must contain 1..=262144 UTF-8 bytes".to_string(),
                    ));
                }
                if let Some(handoff) = handoff {
                    if handoff.to_pubkey == actor.pubkey {
                        return Err(DbError::InvalidData(
                            "Directed Handoff target must be another participant".to_string(),
                        ));
                    }
                    ensure_participant_tx(tx, community_id, session_id, &handoff.to_pubkey).await?;
                }
                let mut mention_pubkeys = HashSet::new();
                for tag in event.tags.iter() {
                    let parts = tag.as_slice();
                    if parts.first().map(String::as_str) != Some("p") {
                        continue;
                    }
                    let value = parts.get(1).ok_or_else(|| {
                        DbError::InvalidData(
                            "Meeting speech p tag is missing its pubkey".to_string(),
                        )
                    })?;
                    let pubkey = hex::decode(value).map_err(|_| {
                        DbError::InvalidData("Meeting speech p tag is not hex".to_string())
                    })?;
                    validate_id(&pubkey, "Meeting speech mention pubkey")?;
                    if !mention_pubkeys.insert(pubkey.clone()) {
                        return Err(DbError::InvalidData(
                            "Meeting speech cannot mention the same participant twice".to_string(),
                        ));
                    }
                    if mention_pubkeys.len() > MAX_MEETING_PARTICIPANTS {
                        return Err(DbError::InvalidData(format!(
                            "Meeting speech supports at most {MAX_MEETING_PARTICIPANTS} participant mentions"
                        )));
                    }
                    ensure_participant_tx(tx, community_id, session_id, &pubkey).await?;
                }
            }
        }
    }
    Ok(())
}

async fn load_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8],
) -> Result<Option<ReceiptRow>> {
    let row = sqlx::query(
        "SELECT author_pubkey, accepted, outcome_code, canonical_object_id, state_revision, \
                retry_ticket_id, response_json \
         FROM meeting_v1_command_receipts \
         WHERE community_id = $1 AND command_event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(|row| {
        let response_json: Value = row.try_get("response_json")?;
        Ok(ReceiptRow {
            author_pubkey: row.try_get("author_pubkey")?,
            accepted: row.try_get("accepted")?,
            outcome_class: response_json
                .get("outcome_class")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    if response_json
                        .get("accepted")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                    {
                        "accepted"
                    } else {
                        "rejected_terminal"
                    }
                })
                .to_string(),
            outcome_code: row.try_get("outcome_code")?,
            canonical_object_id: row.try_get("canonical_object_id")?,
            state_revision: row.try_get("state_revision")?,
            retry_ticket_id: row.try_get("retry_ticket_id")?,
        })
    })
    .transpose()
}

#[allow(clippy::too_many_arguments)]
async fn insert_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    action: &str,
    accepted: bool,
    outcome_class: &str,
    outcome_code: &str,
    canonical_object_id: Option<&[u8]>,
    state_revision: Option<i64>,
    retry_ticket_id: Option<&[u8]>,
) -> Result<()> {
    let response_json = json!({
        "version": 1,
        "accepted": accepted,
        "outcome_class": outcome_class,
        "outcome_code": outcome_code,
        "canonical_object_id": canonical_object_id.map(hex::encode),
        "state_revision": state_revision,
        "retry_ticket_id": retry_ticket_id.map(hex::encode),
    });
    sqlx::query(
        "INSERT INTO meeting_v1_command_receipts \
             (community_id, session_id, command_event_id, author_pubkey, kind, action, \
              accepted, outcome_code, canonical_object_id, state_revision, retry_ticket_id, \
              response_json) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(event.pubkey.as_bytes())
    .bind(event.kind.as_u16() as i32)
    .bind(action)
    .bind(accepted)
    .bind(outcome_code)
    .bind(canonical_object_id)
    .bind(state_revision)
    .bind(retry_ticket_id)
    .bind(response_json)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn insert_retry_ticket_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    ticket: &RetryTicketDraft,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_moderator_retry_tickets \
             (community_id, session_id, retry_ticket_id, attempt_id, \
              failed_action_event_id, source_type, source_id, \
              snapshot_source_event_id, snapshot_handoff_attempt_count, conflict_code, \
              control_epoch, decision_epoch, deadline_at, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&ticket.retry_ticket_id)
    .bind(&ticket.attempt_id)
    .bind(&ticket.failed_action_event_id)
    .bind(ticket.source_type)
    .bind(&ticket.source_id)
    .bind(&ticket.snapshot_source_event_id)
    .bind(ticket.snapshot_handoff_attempt_count)
    .bind(ticket.conflict_code)
    .bind(ticket.control_epoch)
    .bind(ticket.decision_epoch)
    .bind(ticket.deadline_at)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn load_retry_ticket_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    retry_ticket_id: &[u8],
) -> Result<Option<RetryTicketRow>> {
    let row = sqlx::query(
        "SELECT retry_ticket_id, attempt_id, failed_action_event_id, source_type, source_id, \
                snapshot_source_event_id, snapshot_handoff_attempt_count, \
                control_epoch, decision_epoch, deadline_at, consumed_at \
         FROM meeting_moderator_retry_tickets \
         WHERE community_id = $1 AND session_id = $2 AND retry_ticket_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(retry_ticket_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(|row| {
        Ok(RetryTicketRow {
            retry_ticket_id: row.try_get("retry_ticket_id")?,
            attempt_id: row.try_get("attempt_id")?,
            failed_action_event_id: row.try_get("failed_action_event_id")?,
            source_type: row.try_get("source_type")?,
            source_id: row.try_get("source_id")?,
            snapshot_source_event_id: row.try_get("snapshot_source_event_id")?,
            snapshot_handoff_attempt_count: row.try_get("snapshot_handoff_attempt_count")?,
            control_epoch: row.try_get("control_epoch")?,
            decision_epoch: row.try_get("decision_epoch")?,
            deadline_at: row.try_get("deadline_at")?,
            consumed_at: row.try_get("consumed_at")?,
        })
    })
    .transpose()
}

async fn persist_command_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    received_at: DateTime<Utc>,
) -> Result<()> {
    let created_at_secs = i64::try_from(event.created_at.as_secs())
        .map_err(|_| DbError::InvalidTimestamp(i64::MAX))?;
    let created_at = DateTime::from_timestamp(created_at_secs, 0)
        .ok_or(DbError::InvalidTimestamp(created_at_secs))?;
    let result = sqlx::query(
        "INSERT INTO events \
             (community_id, id, pubkey, created_at, kind, tags, content, sig, \
              received_at, channel_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
         ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .bind(event.pubkey.as_bytes())
    .bind(created_at)
    .bind(event.kind.as_u16() as i32)
    .bind(serde_json::to_value(&event.tags)?)
    .bind(&event.content)
    .bind(event.sig.serialize().as_slice())
    .bind(received_at)
    .bind(session_id)
    .execute(tx.as_mut())
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::InvalidData(format!(
            "Meeting V1 command {} exists without a receipt",
            event.id
        )));
    }
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        community_id,
        session_id,
        event.id.as_bytes().as_slice(),
    )
    .await
}

async fn load_intent_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    intent_id: &[u8],
) -> Result<Option<IntentRow>> {
    let row = sqlx::query(
        "SELECT intent_id, author_pubkey, current_event_id, state, \
                deferred_by_offer_id, eligible_decision_epoch \
         FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(intent_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(intent_from_row).transpose()
}

fn intent_from_row(row: sqlx::postgres::PgRow) -> Result<IntentRow> {
    Ok(IntentRow {
        intent_id: row.try_get("intent_id")?,
        author_pubkey: row.try_get("author_pubkey")?,
        current_event_id: row.try_get("current_event_id")?,
        state: row.try_get("state")?,
        deferred_by_offer_id: row.try_get("deferred_by_offer_id")?,
        eligible_decision_epoch: row.try_get("eligible_decision_epoch")?,
    })
}

async fn load_offer_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    offer_id: &[u8],
) -> Result<Option<OfferRow>> {
    let row = sqlx::query(
        "SELECT offer_id, target_pubkey, allocation_source, turn_role, \
                allocation_event_id, selection_reason, source_intent_id, \
                source_request_id, source_handoff_id, source_speech_event_id, \
                reason_type, reason_text, basis_speech_revision, depth_mode, \
                previous_handoff_depth, requested_handoff_depth, ack_deadline, \
                state, created_at \
         FROM meeting_baton_offers \
         WHERE community_id = $1 AND session_id = $2 AND offer_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(offer_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(offer_from_row).transpose()
}

fn offer_from_row(row: sqlx::postgres::PgRow) -> Result<OfferRow> {
    Ok(OfferRow {
        offer_id: row.try_get("offer_id")?,
        target_pubkey: row.try_get("target_pubkey")?,
        allocation_source: row.try_get("allocation_source")?,
        turn_role: row.try_get("turn_role")?,
        allocation_event_id: row.try_get("allocation_event_id")?,
        selection_reason: row.try_get("selection_reason")?,
        source_intent_id: row.try_get("source_intent_id")?,
        source_request_id: row.try_get("source_request_id")?,
        source_handoff_id: row.try_get("source_handoff_id")?,
        source_speech_event_id: row.try_get("source_speech_event_id")?,
        reason_type: row.try_get("reason_type")?,
        reason_text: row.try_get("reason_text")?,
        basis_speech_revision: row.try_get("basis_speech_revision")?,
        depth_mode: row.try_get("depth_mode")?,
        previous_handoff_depth: row.try_get("previous_handoff_depth")?,
        requested_handoff_depth: row.try_get("requested_handoff_depth")?,
        ack_deadline: row.try_get("ack_deadline")?,
        state: row.try_get("state")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_grant_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    grant_id: &[u8],
) -> Result<Option<GrantRow>> {
    let row = sqlx::query(
        "SELECT grant_id, holder_pubkey, allocation_source, turn_role, \
                source_offer_id, allocation_event_id, selection_reason, \
                source_intent_id, source_request_id, source_handoff_id, \
                source_speech_event_id, basis_speech_revision, depth_mode, \
                previous_handoff_depth, handoff_depth, soft_lease_expires_at, \
                hard_deadline, progress_seq, state, created_at \
         FROM meeting_baton_grants \
         WHERE community_id = $1 AND session_id = $2 AND grant_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(grant_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(grant_from_row).transpose()
}

fn grant_from_row(row: sqlx::postgres::PgRow) -> Result<GrantRow> {
    Ok(GrantRow {
        grant_id: row.try_get("grant_id")?,
        holder_pubkey: row.try_get("holder_pubkey")?,
        allocation_source: row.try_get("allocation_source")?,
        turn_role: row.try_get("turn_role")?,
        source_offer_id: row.try_get("source_offer_id")?,
        allocation_event_id: row.try_get("allocation_event_id")?,
        selection_reason: row.try_get("selection_reason")?,
        source_intent_id: row.try_get("source_intent_id")?,
        source_request_id: row.try_get("source_request_id")?,
        source_handoff_id: row.try_get("source_handoff_id")?,
        source_speech_event_id: row.try_get("source_speech_event_id")?,
        basis_speech_revision: row.try_get("basis_speech_revision")?,
        depth_mode: row.try_get("depth_mode")?,
        previous_handoff_depth: row.try_get("previous_handoff_depth")?,
        handoff_depth: row.try_get("handoff_depth")?,
        soft_lease_expires_at: row.try_get("soft_lease_expires_at")?,
        hard_deadline: row.try_get("hard_deadline")?,
        progress_seq: row.try_get("progress_seq")?,
        state: row.try_get("state")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn load_request_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    request_id: &[u8],
) -> Result<Option<HumanRequestRow>> {
    let row = sqlx::query(
        "SELECT request_id, requester_pubkey, queue_position, state, offer_id \
         FROM meeting_human_floor_requests \
         WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(request_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(request_from_row).transpose()
}

fn request_from_row(row: sqlx::postgres::PgRow) -> Result<HumanRequestRow> {
    Ok(HumanRequestRow {
        request_id: row.try_get("request_id")?,
        requester_pubkey: row.try_get("requester_pubkey")?,
        state: row.try_get("state")?,
        offer_id: row.try_get("offer_id")?,
    })
}

async fn load_handoff_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    handoff_id: &[u8],
) -> Result<Option<HandoffRow>> {
    let row = sqlx::query(
        "SELECT handoff_id, source_speech_event_id, from_pubkey, to_pubkey, \
                reason_type, reason_text, question_state, blocked_by, \
                last_offer_id, last_grant_id, attempt_count, eligible_decision_epoch \
         FROM meeting_directed_handoffs \
         WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(handoff_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(handoff_from_row).transpose()
}

fn handoff_from_row(row: sqlx::postgres::PgRow) -> Result<HandoffRow> {
    Ok(HandoffRow {
        handoff_id: row.try_get("handoff_id")?,
        source_speech_event_id: row.try_get("source_speech_event_id")?,
        from_pubkey: row.try_get("from_pubkey")?,
        to_pubkey: row.try_get("to_pubkey")?,
        reason_type: row.try_get("reason_type")?,
        reason_text: row.try_get("reason_text")?,
        question_state: row.try_get("question_state")?,
        blocked_by: row.try_get("blocked_by")?,
        last_offer_id: row.try_get("last_offer_id")?,
        last_grant_id: row.try_get("last_grant_id")?,
        attempt_count: row.try_get("attempt_count")?,
        eligible_decision_epoch: row.try_get("eligible_decision_epoch")?,
    })
}

#[derive(Debug, Clone)]
struct ModeratorAttemptRow {
    attempt_id: Vec<u8>,
    moderator_pubkey: Vec<u8>,
    control_epoch: i64,
    decision_epoch: i64,
    attempt_number: i32,
    speech_revision: i64,
    candidate_snapshot_json: Value,
    state: String,
    started_at: DateTime<Utc>,
    deadline_at: DateTime<Utc>,
}

fn moderator_attempt_from_row(row: sqlx::postgres::PgRow) -> Result<ModeratorAttemptRow> {
    Ok(ModeratorAttemptRow {
        attempt_id: row.try_get("attempt_id")?,
        moderator_pubkey: row.try_get("moderator_pubkey")?,
        control_epoch: row.try_get("control_epoch")?,
        decision_epoch: row.try_get("decision_epoch")?,
        attempt_number: row.try_get("attempt_number")?,
        speech_revision: row.try_get("speech_revision")?,
        candidate_snapshot_json: row.try_get("candidate_snapshot_json")?,
        state: row.try_get("state")?,
        started_at: row.try_get("started_at")?,
        deadline_at: row.try_get("deadline_at")?,
    })
}

async fn load_moderator_attempt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    attempt_id: &[u8],
) -> Result<Option<ModeratorAttemptRow>> {
    let row = sqlx::query(
        "SELECT attempt_id, moderator_pubkey, control_epoch, decision_epoch, \
                attempt_number, speech_revision, candidate_snapshot_json, state, \
                started_at, deadline_at \
         FROM meeting_moderator_decision_attempts \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(attempt_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(moderator_attempt_from_row).transpose()
}

async fn build_candidate_snapshot_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    moderator_pubkey: &[u8],
    eligible_through_epoch: i64,
) -> Result<(Value, Vec<u8>, usize)> {
    let intent_rows = sqlx::query(
        "SELECT intent_id, current_event_id, author_pubkey, basis_speech_revision, \
                summary, addressed_to, eligible_decision_epoch, created_at \
         FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
           AND deferred_by_offer_id IS NULL \
           AND eligible_decision_epoch <= $3 \
         ORDER BY created_at, intent_id \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(eligible_through_epoch)
    .fetch_all(tx.as_mut())
    .await?;
    let mut candidate_refs = Vec::with_capacity(intent_rows.len());
    for row in intent_rows {
        let intent_id: Vec<u8> = row.try_get("intent_id")?;
        let current_event_id: Vec<u8> = row.try_get("current_event_id")?;
        let author_pubkey: Vec<u8> = row.try_get("author_pubkey")?;
        let addressed_to: Option<Vec<u8>> = row.try_get("addressed_to")?;
        let created_at: DateTime<Utc> = row.try_get("created_at")?;
        candidate_refs.push(json!({
            "source_type": "intent",
            "source_id": hex::encode(intent_id),
            "current_event_id": hex::encode(current_event_id),
            "author_pubkey": hex::encode(&author_pubkey),
            "moderator_self": author_pubkey == moderator_pubkey,
            "basis_speech_revision": row.try_get::<i64, _>("basis_speech_revision")?,
            "summary": row.try_get::<String, _>("summary")?,
            "addressed_to": addressed_to.as_deref().map(hex::encode),
            "eligible_decision_epoch": row.try_get::<i64, _>("eligible_decision_epoch")?,
            "created_at_ms": created_at.timestamp_millis(),
        }));
    }

    let handoff_rows = sqlx::query(
        "SELECT handoff_id, source_speech_event_id, from_pubkey, to_pubkey, \
                reason_type, reason_text, attempt_count, eligible_decision_epoch, created_at \
         FROM meeting_directed_handoffs \
         WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
           AND blocked_by IS NULL AND moderator_retry_blocked_fingerprint IS NULL \
           AND eligible_decision_epoch <= $3 \
         ORDER BY created_at, handoff_id \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(eligible_through_epoch)
    .fetch_all(tx.as_mut())
    .await?;
    candidate_refs.reserve(handoff_rows.len());
    for row in handoff_rows {
        let handoff_id: Vec<u8> = row.try_get("handoff_id")?;
        let source_speech_event_id: Vec<u8> = row.try_get("source_speech_event_id")?;
        let from_pubkey: Vec<u8> = row.try_get("from_pubkey")?;
        let to_pubkey: Vec<u8> = row.try_get("to_pubkey")?;
        let created_at: DateTime<Utc> = row.try_get("created_at")?;
        candidate_refs.push(json!({
            "source_type": "handoff",
            "source_id": hex::encode(handoff_id),
            "source_speech_event_id": hex::encode(source_speech_event_id),
            "from_pubkey": hex::encode(from_pubkey),
            "target_pubkey": hex::encode(to_pubkey),
            "reason_type": row.try_get::<String, _>("reason_type")?,
            "reason_text": row.try_get::<String, _>("reason_text")?,
            "attempt_count": row.try_get::<i32, _>("attempt_count")?,
            "eligible_decision_epoch": row.try_get::<i64, _>("eligible_decision_epoch")?,
            "created_at_ms": created_at.timestamp_millis(),
        }));
    }
    candidate_refs.sort_by(|left, right| {
        left.get("created_at_ms")
            .and_then(Value::as_i64)
            .cmp(&right.get("created_at_ms").and_then(Value::as_i64))
            .then_with(|| {
                left.get("source_id")
                    .and_then(Value::as_str)
                    .cmp(&right.get("source_id").and_then(Value::as_str))
            })
    });
    let candidate_count = candidate_refs.len();
    let snapshot = json!({
        "version": 1,
        "control_epoch": state.control_epoch,
        "decision_epoch": eligible_through_epoch,
        "speech_revision": state.speech_revision,
        "snapshot_intent_revision": state.intent_revision,
        "candidate_refs": candidate_refs,
    });
    let encoded = serde_json::to_vec(&snapshot)?;
    let snapshot_hash = Sha256::digest(encoded).to_vec();
    Ok((snapshot, snapshot_hash, candidate_count))
}

fn attempt_candidate_ref<'a>(
    attempt: &'a ModeratorAttemptRow,
    source_type: &str,
    source_id: &[u8],
) -> Option<&'a Value> {
    let source_id = hex::encode(source_id);
    attempt
        .candidate_snapshot_json
        .get("candidate_refs")
        .and_then(Value::as_array)?
        .iter()
        .find(|candidate| {
            candidate.get("source_type").and_then(Value::as_str) == Some(source_type)
                && candidate.get("source_id").and_then(Value::as_str) == Some(source_id.as_str())
        })
}

enum ModeratorActionAuthority {
    Manual,
    Attempt(ModeratorAttemptRow),
    Rejected {
        code: &'static str,
        canonical_object_id: Option<Vec<u8>>,
    },
}

async fn moderator_action_authority_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    actor: &Actor,
    attempt_id: Option<&[u8]>,
    now: DateTime<Utc>,
) -> Result<ModeratorActionAuthority> {
    if actor.participant_type == ParticipantType::Human {
        return if attempt_id.is_none() {
            Ok(ModeratorActionAuthority::Manual)
        } else {
            Ok(ModeratorActionAuthority::Rejected {
                code: "human_moderator_does_not_use_attempt",
                canonical_object_id: state.active_decision_attempt_id.clone(),
            })
        };
    }
    let Some(attempt_id) = attempt_id else {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_required",
            canonical_object_id: state.active_decision_attempt_id.clone(),
        });
    };
    let Some(attempt) = load_moderator_attempt_tx(tx, community_id, session_id, attempt_id).await?
    else {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_not_found",
            canonical_object_id: None,
        });
    };
    if attempt.moderator_pubkey != actor.pubkey {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_actor_mismatch",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }
    if attempt.state != "running" || state.active_decision_attempt_id.as_deref() != Some(attempt_id)
    {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_not_active",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }
    if attempt.control_epoch != state.control_epoch
        || attempt.decision_epoch != state.decision_epoch
        || attempt.speech_revision != state.speech_revision
    {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_prerequisite_changed",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }
    if now >= attempt.deadline_at {
        return Ok(ModeratorActionAuthority::Rejected {
            code: "moderator_attempt_expired",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }
    Ok(ModeratorActionAuthority::Attempt(attempt))
}

fn candidate_hex(candidate: &Value, field: &str) -> Result<Vec<u8>> {
    let value = candidate
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            DbError::InvalidData(format!(
                "moderator DecisionAttempt candidate is missing {field}"
            ))
        })?;
    let decoded = hex::decode(value).map_err(|_| {
        DbError::InvalidData(format!(
            "moderator DecisionAttempt candidate {field} is not hex"
        ))
    })?;
    validate_id(&decoded, field)?;
    Ok(decoded)
}

fn selected_source_retry_ticket(
    attempt: &ModeratorAttemptRow,
    failed_action_event_id: &[u8],
    source_type: &'static str,
    source_id: &[u8],
    snapshot_source_event_id: Option<Vec<u8>>,
    snapshot_handoff_attempt_count: Option<i32>,
) -> RetryTicketDraft {
    RetryTicketDraft {
        retry_ticket_id: random_object_id(),
        attempt_id: attempt.attempt_id.clone(),
        failed_action_event_id: failed_action_event_id.to_vec(),
        source_type,
        source_id: source_id.to_vec(),
        snapshot_source_event_id,
        snapshot_handoff_attempt_count,
        conflict_code: "selected_source_changed",
        control_epoch: attempt.control_epoch,
        decision_epoch: attempt.decision_epoch,
        deadline_at: attempt.deadline_at,
    }
}

async fn current_cohort_has_candidates_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    decision_epoch: i64,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
               AND deferred_by_offer_id IS NULL AND eligible_decision_epoch <= $3 \
             UNION ALL \
             SELECT 1 FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
               AND blocked_by IS NULL AND moderator_retry_blocked_fingerprint IS NULL \
               AND eligible_decision_epoch <= $3 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(decision_epoch)
    .fetch_one(tx.as_mut())
    .await?)
}

async fn current_cohort_has_handoffs_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    decision_epoch: i64,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
               AND blocked_by IS NULL AND moderator_retry_blocked_fingerprint IS NULL \
               AND eligible_decision_epoch <= $3 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(decision_epoch)
    .fetch_one(tx.as_mut())
    .await?)
}

async fn pending_intents_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT intent_id, current_event_id, author_pubkey, basis_speech_revision, \
                summary, addressed_to, created_at, deferred_by_offer_id IS NOT NULL AS deferred, \
                selection_attempt_count, last_offer_id, last_attempt_outcome, \
                eligible_decision_epoch \
         FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
         ORDER BY created_at, intent_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            let intent_id: Vec<u8> = row.try_get("intent_id")?;
            let current_event_id: Vec<u8> = row.try_get("current_event_id")?;
            let author_pubkey: Vec<u8> = row.try_get("author_pubkey")?;
            let addressed_to: Option<Vec<u8>> = row.try_get("addressed_to")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let last_offer_id: Option<Vec<u8>> = row.try_get("last_offer_id")?;
            Ok(json!({
                "intent_id": hex::encode(intent_id),
                "current_event_id": hex::encode(current_event_id),
                "author_pubkey": hex::encode(author_pubkey),
                "basis_speech_revision": row.try_get::<i64, _>("basis_speech_revision")?,
                "summary": row.try_get::<String, _>("summary")?,
                "addressed_to": addressed_to.as_deref().map(hex::encode),
                "created_at_ms": created_at.timestamp_millis(),
                "deferred": row.try_get::<bool, _>("deferred")?,
                "selection_attempt_count": row.try_get::<i32, _>("selection_attempt_count")?,
                "last_offer_id": last_offer_id.as_deref().map(hex::encode),
                "last_attempt_outcome": row.try_get::<Option<String>, _>("last_attempt_outcome")?,
                "eligible_decision_epoch": row.try_get::<i64, _>("eligible_decision_epoch")?,
            }))
        })
        .collect()
}

async fn human_queue_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT request_id, requester_pubkey, queue_position, state \
         FROM meeting_human_floor_requests \
         WHERE community_id = $1 AND session_id = $2 AND state IN ('queued', 'offered') \
         ORDER BY queue_position",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            let request_id: Vec<u8> = row.try_get("request_id")?;
            let requester_pubkey: Vec<u8> = row.try_get("requester_pubkey")?;
            Ok(json!({
                "request_id": hex::encode(request_id),
                "requester_pubkey": hex::encode(requester_pubkey),
                "queue_position": row.try_get::<i64, _>("queue_position")?,
                "state": row.try_get::<String, _>("state")?,
            }))
        })
        .collect()
}

async fn unresolved_handoffs_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT handoff_id, source_speech_event_id, from_pubkey, to_pubkey, \
                reason_type, reason_text, created_at, question_state, attempt_count, \
                last_offer_id, last_grant_id, last_attempt_outcome, blocked_by, \
                moderator_retry_blocked_fingerprint IS NOT NULL AS moderator_retry_blocked, \
                eligible_decision_epoch \
         FROM meeting_directed_handoffs \
         WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
         ORDER BY created_at, handoff_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            let handoff_id: Vec<u8> = row.try_get("handoff_id")?;
            let source_speech_event_id: Vec<u8> = row.try_get("source_speech_event_id")?;
            let from_pubkey: Vec<u8> = row.try_get("from_pubkey")?;
            let to_pubkey: Vec<u8> = row.try_get("to_pubkey")?;
            let created_at: DateTime<Utc> = row.try_get("created_at")?;
            let last_offer_id: Option<Vec<u8>> = row.try_get("last_offer_id")?;
            let last_grant_id: Option<Vec<u8>> = row.try_get("last_grant_id")?;
            Ok(json!({
                "handoff_id": hex::encode(handoff_id),
                "source_speech_event_id": hex::encode(source_speech_event_id),
                "from_pubkey": hex::encode(from_pubkey),
                "to_pubkey": hex::encode(to_pubkey),
                "reason_type": row.try_get::<String, _>("reason_type")?,
                "reason_text": row.try_get::<String, _>("reason_text")?,
                "created_at_ms": created_at.timestamp_millis(),
                "question_state": row.try_get::<String, _>("question_state")?,
                "attempt_count": row.try_get::<i32, _>("attempt_count")?,
                "last_offer_id": last_offer_id.as_deref().map(hex::encode),
                "last_grant_id": last_grant_id.as_deref().map(hex::encode),
                "last_attempt_outcome": row.try_get::<Option<String>, _>("last_attempt_outcome")?,
                "blocked_by": row.try_get::<Option<String>, _>("blocked_by")?,
                "moderator_retry_blocked": row.try_get::<bool, _>("moderator_retry_blocked")?,
                "eligible_decision_epoch": row.try_get::<i64, _>("eligible_decision_epoch")?,
            }))
        })
        .collect()
}

fn handoff_context_json(
    from_pubkey: Option<&[u8]>,
    reason_type: Option<&str>,
    reason_text: Option<&str>,
) -> Option<Value> {
    match (from_pubkey, reason_type, reason_text) {
        (Some(from), Some(reason_type), Some(reason_text)) => Some(json!({
            "from_pubkey": hex::encode(from),
            "reason_type": reason_type,
            "reason_text": reason_text,
        })),
        _ => None,
    }
}

async fn offer_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    offer_id: Option<&[u8]>,
) -> Result<Option<Value>> {
    let Some(offer_id) = offer_id else {
        return Ok(None);
    };
    let offer = load_offer_tx(tx, community_id, session_id, offer_id)
        .await?
        .ok_or_else(|| DbError::InvalidData("active Offer projection is missing".to_string()))?;
    let target_type =
        ensure_participant_tx(tx, community_id, session_id, &offer.target_pubkey).await?;
    let from_pubkey = if let Some(handoff_id) = offer.source_handoff_id.as_deref() {
        load_handoff_tx(tx, community_id, session_id, handoff_id)
            .await?
            .map(|handoff| handoff.from_pubkey)
    } else {
        None
    };
    Ok(Some(json!({
        "offer_id": hex::encode(&offer.offer_id),
        "target_pubkey": hex::encode(&offer.target_pubkey),
        "target_participant_type": target_type,
        "allocation_source": offer.allocation_source,
        "turn_role": offer.turn_role,
        "allocation_event_id": offer.allocation_event_id.as_deref().map(hex::encode),
        "selection_reason": offer.selection_reason,
        "source_intent_id": offer.source_intent_id.as_deref().map(hex::encode),
        "source_request_id": offer.source_request_id.as_deref().map(hex::encode),
        "source_handoff_id": offer.source_handoff_id.as_deref().map(hex::encode),
        "source_speech_event_id": offer.source_speech_event_id.as_deref().map(hex::encode),
        "basis_speech_revision": offer.basis_speech_revision,
        "depth_mode": offer.depth_mode,
        "previous_handoff_depth": offer.previous_handoff_depth,
        "requested_handoff_depth": offer.requested_handoff_depth,
        "handoff_context": handoff_context_json(
            from_pubkey.as_deref(),
            offer.reason_type.as_deref(),
            offer.reason_text.as_deref(),
        ),
        "created_at_ms": offer.created_at.timestamp_millis(),
        "ack_deadline_ms": offer.ack_deadline.timestamp_millis(),
    })))
}

async fn grant_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    grant_id: Option<&[u8]>,
) -> Result<Option<Value>> {
    let Some(grant_id) = grant_id else {
        return Ok(None);
    };
    let grant = load_grant_tx(tx, community_id, session_id, grant_id)
        .await?
        .ok_or_else(|| DbError::InvalidData("active Grant projection is missing".to_string()))?;
    let handoff = if let Some(handoff_id) = grant.source_handoff_id.as_deref() {
        load_handoff_tx(tx, community_id, session_id, handoff_id).await?
    } else {
        None
    };
    Ok(Some(json!({
        "grant_id": hex::encode(&grant.grant_id),
        "holder_pubkey": hex::encode(&grant.holder_pubkey),
        "allocation_source": grant.allocation_source,
        "turn_role": grant.turn_role,
        "source_offer_id": hex::encode(&grant.source_offer_id),
        "allocation_event_id": grant.allocation_event_id.as_deref().map(hex::encode),
        "selection_reason": grant.selection_reason,
        "source_intent_id": grant.source_intent_id.as_deref().map(hex::encode),
        "source_request_id": grant.source_request_id.as_deref().map(hex::encode),
        "source_handoff_id": grant.source_handoff_id.as_deref().map(hex::encode),
        "source_speech_event_id": grant.source_speech_event_id.as_deref().map(hex::encode),
        "depth_mode": grant.depth_mode,
        "previous_handoff_depth": grant.previous_handoff_depth,
        "handoff_depth": grant.handoff_depth,
        "handoff_context": handoff.as_ref().and_then(|handoff| handoff_context_json(
            Some(&handoff.from_pubkey),
            Some(&handoff.reason_type),
            Some(&handoff.reason_text),
        )),
        "basis_speech_revision": grant.basis_speech_revision,
        "created_at_ms": grant.created_at.timestamp_millis(),
        "soft_lease_expires_at_ms": grant.soft_lease_expires_at.timestamp_millis(),
        "hard_deadline_ms": grant.hard_deadline.timestamp_millis(),
        "progress_seq": grant.progress_seq,
    })))
}

async fn active_decision_attempt_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    attempt_id: Option<&[u8]>,
) -> Result<Option<Value>> {
    let Some(attempt_id) = attempt_id else {
        return Ok(None);
    };
    let row = sqlx::query(
        "SELECT attempt_id, control_epoch, decision_epoch, attempt_number, \
                speech_revision, snapshot_intent_revision, snapshot_state_event_id, \
                candidate_snapshot_json, candidate_snapshot_hash, started_at, deadline_at \
         FROM meeting_moderator_decision_attempts \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
           AND state = 'running'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(attempt_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        DbError::InvalidData("active moderator DecisionAttempt projection is missing".to_string())
    })?;
    let attempt_id: Vec<u8> = row.try_get("attempt_id")?;
    let snapshot_state_event_id: Vec<u8> = row.try_get("snapshot_state_event_id")?;
    let candidate_snapshot_hash: Vec<u8> = row.try_get("candidate_snapshot_hash")?;
    let started_at: DateTime<Utc> = row.try_get("started_at")?;
    let deadline_at: DateTime<Utc> = row.try_get("deadline_at")?;
    let candidate_snapshot: Value = row.try_get("candidate_snapshot_json")?;
    let candidate_refs = candidate_snapshot
        .get("candidate_refs")
        .cloned()
        .ok_or_else(|| {
            DbError::InvalidData(
                "moderator DecisionAttempt snapshot has no candidate refs".to_string(),
            )
        })?;
    Ok(Some(json!({
        "attempt_id": hex::encode(attempt_id),
        "control_epoch": row.try_get::<i64, _>("control_epoch")?,
        "decision_epoch": row.try_get::<i64, _>("decision_epoch")?,
        "attempt_number": row.try_get::<i32, _>("attempt_number")?,
        "speech_revision": row.try_get::<i64, _>("speech_revision")?,
        "snapshot_intent_revision": row.try_get::<i64, _>("snapshot_intent_revision")?,
        "snapshot_state_event_id": hex::encode(snapshot_state_event_id),
        "candidate_refs": candidate_refs,
        "candidate_snapshot_hash": hex::encode(candidate_snapshot_hash),
        "started_at_ms": started_at.timestamp_millis(),
        "deadline_ms": deadline_at.timestamp_millis(),
    })))
}

#[allow(clippy::too_many_arguments)]
async fn build_dynamic_state_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    moderator_pubkey: &[u8],
    state: &StateRow,
    target: &StateTarget,
    config: &BatonConfig,
    participants: &[BatonParticipant],
    delta: RevisionDelta,
    transition: &TransitionSpec,
    now: DateTime<Utc>,
) -> Result<(Event, i64, i64, i64, i64, Value)> {
    let floor_revision = state.floor_revision + i64::from(delta.floor);
    let intent_revision = state.intent_revision + i64::from(delta.intent);
    let speech_revision = state.speech_revision + i64::from(delta.speech);
    let state_revision = state.state_revision + 1;
    let transition_json = transition.json(now);
    let pending_intents = pending_intents_json_tx(tx, community_id, session_id).await?;
    let human_queue = human_queue_json_tx(tx, community_id, session_id).await?;
    let unresolved_handoffs = unresolved_handoffs_json_tx(tx, community_id, session_id).await?;
    let offer = offer_json_tx(
        tx,
        community_id,
        session_id,
        target.active_offer_id.as_deref(),
    )
    .await?;
    let grant = grant_json_tx(
        tx,
        community_id,
        session_id,
        target.active_grant_id.as_deref(),
    )
    .await?;
    let active_decision_attempt = active_decision_attempt_json_tx(
        tx,
        community_id,
        session_id,
        target.active_decision_attempt_id.as_deref(),
    )
    .await?;
    let moderator = hex::encode(moderator_pubkey);
    let content = json!({
        "phase": target.phase,
        "state_revision": state_revision,
        "floor_revision": floor_revision,
        "intent_revision": intent_revision,
        "speech_revision": speech_revision,
        "control_epoch": target.control_epoch,
        "decision_epoch": target.decision_epoch,
        "decision_attempt": target.decision_attempt,
        "active_decision_attempt": active_decision_attempt,
        "baton_config": config,
        "moderator_pubkey": moderator,
        "participants": participants,
        "pending_intents": pending_intents,
        "human_queue": human_queue,
        "unresolved_handoffs": unresolved_handoffs,
        "handoff_depth": target.handoff_depth,
        "consecutive_moderator_speeches": target.consecutive_moderator_speeches,
        "forced_return_to_moderator": target.forced_return_to_moderator,
        "moderator_decision_deadline_ms": timestamp_ms(target.moderator_decision_deadline),
        "next_action_at_ms": timestamp_ms(target.next_action_at),
        "offer": offer,
        "grant": grant,
        "transition": transition_json,
    });
    let session = session_id.to_string();
    let floor = floor_revision.to_string();
    let intent = intent_revision.to_string();
    let speech = speech_revision.to_string();
    let state_revision_text = state_revision.to_string();
    let mut tags = vec![
        parse_tag(["h", session.as_str()])?,
        parse_tag(["v", "2"])?,
        parse_tag(["policy", BATON_POLICY_VERSION])?,
        parse_tag(["phase", target.phase.as_str()])?,
        parse_tag(["floor-revision", floor.as_str()])?,
        parse_tag(["intent-revision", intent.as_str()])?,
        parse_tag(["speech-revision", speech.as_str()])?,
        parse_tag(["state-revision", state_revision_text.as_str()])?,
        parse_tag(["moderator", moderator.as_str()])?,
    ];
    let active_target = offer
        .as_ref()
        .and_then(|value| value.get("target_pubkey"))
        .or_else(|| grant.as_ref().and_then(|value| value.get("holder_pubkey")))
        .and_then(Value::as_str);
    if let Some(active_target) = active_target {
        tags.push(parse_tag(["p", active_target])?);
    }
    let timestamp =
        u64::try_from(now.timestamp()).map_err(|_| DbError::InvalidTimestamp(now.timestamp()))?;
    let event = EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_MEETING_STATE as u16),
        serde_json::to_string(&content)?,
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(timestamp))
    .sign_with_keys(relay_keys)
    .map_err(|error| DbError::InvalidData(format!("sign Meeting V1 State: {error}")))?;
    Ok((
        event,
        floor_revision,
        intent_revision,
        speech_revision,
        state_revision,
        transition_json,
    ))
}

#[allow(clippy::too_many_arguments)]
async fn commit_transition_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    state: &StateRow,
    target: StateTarget,
    delta: RevisionDelta,
    transition: TransitionSpec,
    now: DateTime<Utc>,
) -> Result<(StateRow, BatonTransitionResult)> {
    let config = load_config_tx(tx, community_id, session_id).await?;
    let participants = load_participants_tx(tx, community_id, session_id).await?;
    let moderator = load_moderator_tx(tx, community_id, session_id).await?;
    let (event, floor_revision, intent_revision, speech_revision, state_revision, transition_json) =
        build_dynamic_state_event_tx(
            tx,
            community_id,
            session_id,
            relay_keys,
            &moderator,
            state,
            &target,
            &config,
            &participants,
            delta,
            &transition,
            now,
        )
        .await?;
    persist_state_event_tx(tx, community_id, session_id, &event, now).await?;
    insert_history_tx(
        tx,
        community_id,
        session_id,
        &event,
        state_revision,
        floor_revision,
        intent_revision,
        speech_revision,
        target.control_epoch,
        target.decision_epoch,
        transition.primary_type,
        transition_json
            .get("effects")
            .cloned()
            .unwrap_or_else(|| json!([])),
        now,
    )
    .await?;
    sqlx::query(
        "UPDATE meeting_baton_state \
         SET phase = $3, floor_revision = $4, intent_revision = $5, \
             speech_revision = $6, state_revision = $7, control_epoch = $8, \
             decision_epoch = $9, state_event_id = $10, active_offer_id = $11, \
             active_grant_id = $12, handoff_depth = $13, \
             consecutive_moderator_speeches = $14, \
             forced_return_to_moderator = $15, recall_event_id = $16, \
             moderator_decision_started_at = $17, moderator_decision_deadline = $18, \
             next_action_at = $19, decision_attempt = $20, \
             active_decision_attempt_id = $21, recovery_retry_at = '-infinity', \
             recovery_attempts = 0, updated_at = $22 \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(target.phase.as_str())
    .bind(floor_revision)
    .bind(intent_revision)
    .bind(speech_revision)
    .bind(state_revision)
    .bind(target.control_epoch)
    .bind(target.decision_epoch)
    .bind(event.id.as_bytes().as_slice())
    .bind(&target.active_offer_id)
    .bind(&target.active_grant_id)
    .bind(target.handoff_depth)
    .bind(target.consecutive_moderator_speeches)
    .bind(target.forced_return_to_moderator)
    .bind(&target.recall_event_id)
    .bind(target.moderator_decision_started_at)
    .bind(target.moderator_decision_deadline)
    .bind(target.next_action_at)
    .bind(target.decision_attempt)
    .bind(&target.active_decision_attempt_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        community_id,
        session_id,
        event.id.as_bytes().as_slice(),
    )
    .await?;
    let next = load_state_tx(tx, community_id, session_id, false).await?;
    Ok((
        next,
        BatonTransitionResult {
            primary_type: transition.primary_type.to_string(),
            state_revision,
            state_event_id: event.id.as_bytes().to_vec(),
        },
    ))
}

#[derive(Debug)]
struct OfferDraft {
    offer_id: Vec<u8>,
    target_pubkey: Vec<u8>,
    allocation_source: &'static str,
    turn_role: &'static str,
    allocation_event_id: Option<Vec<u8>>,
    selection_reason: Option<String>,
    source_intent_id: Option<Vec<u8>>,
    source_request_id: Option<Vec<u8>>,
    source_handoff_id: Option<Vec<u8>>,
    source_speech_event_id: Option<Vec<u8>>,
    reason_type: Option<String>,
    reason_text: Option<String>,
    basis_speech_revision: i64,
    depth_mode: &'static str,
    previous_handoff_depth: i32,
    requested_handoff_depth: i32,
}

async fn insert_offer_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    draft: &OfferDraft,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>> {
    let target_type =
        ensure_participant_tx(tx, community_id, session_id, &draft.target_pubkey).await?;
    let ack_ms = match target_type {
        ParticipantType::Human => config.human_offer_ack_ms,
        ParticipantType::Agent => config.agent_offer_ack_ms,
    };
    let ack_deadline = now + Duration::milliseconds(ack_ms);
    sqlx::query(
        "INSERT INTO meeting_baton_offers \
             (community_id, session_id, offer_id, target_pubkey, allocation_source, \
              turn_role, allocation_event_id, selection_reason, source_intent_id, \
              source_request_id, source_handoff_id, source_speech_event_id, \
              reason_type, reason_text, basis_speech_revision, depth_mode, \
              previous_handoff_depth, requested_handoff_depth, ack_deadline, state, \
              created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, \
                 $15, $16, $17, $18, $19, 'pending', $20)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&draft.offer_id)
    .bind(&draft.target_pubkey)
    .bind(draft.allocation_source)
    .bind(draft.turn_role)
    .bind(&draft.allocation_event_id)
    .bind(&draft.selection_reason)
    .bind(&draft.source_intent_id)
    .bind(&draft.source_request_id)
    .bind(&draft.source_handoff_id)
    .bind(&draft.source_speech_event_id)
    .bind(&draft.reason_type)
    .bind(&draft.reason_text)
    .bind(draft.basis_speech_revision)
    .bind(draft.depth_mode)
    .bind(draft.previous_handoff_depth)
    .bind(draft.requested_handoff_depth)
    .bind(ack_deadline)
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    if let Some(intent_id) = draft.source_intent_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_speech_intents \
             SET selection_attempt_count = selection_attempt_count + 1, \
                 last_offer_id = $4, last_attempt_outcome = 'offered', updated_at = $5 \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
               AND state = 'pending'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(intent_id)
        .bind(&draft.offer_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
    }
    if let Some(request_id) = draft.source_request_id.as_deref() {
        let updated = sqlx::query(
            "UPDATE meeting_human_floor_requests \
             SET state = 'offered', offer_id = $4 \
             WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
               AND state = 'queued'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(request_id)
        .bind(&draft.offer_id)
        .execute(tx.as_mut())
        .await?;
        if updated.rows_affected() != 1 {
            return Err(DbError::InvalidData(
                "Human Request changed before Offer creation".to_string(),
            ));
        }
    }
    if let Some(handoff_id) = draft.source_handoff_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_directed_handoffs \
             SET attempt_count = attempt_count + 1, last_offer_id = $4, \
                 last_attempt_outcome = 'offered', blocked_by = NULL, \
                 moderator_retry_blocked_fingerprint = NULL, \
                 moderator_retry_not_before = NULL \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
               AND question_state = 'open'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(handoff_id)
        .bind(&draft.offer_id)
        .execute(tx.as_mut())
        .await?;
    }
    Ok(ack_deadline)
}

async fn earliest_queued_human_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<HumanRequestRow>> {
    let row = sqlx::query(
        "SELECT request_id, requester_pubkey, queue_position, state, offer_id \
         FROM meeting_human_floor_requests \
         WHERE community_id = $1 AND session_id = $2 AND state = 'queued' \
         ORDER BY queue_position \
         LIMIT 1 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(request_from_row).transpose()
}

async fn offer_human_request_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    request: &HumanRequestRow,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<(StateTarget, Vec<u8>)> {
    let offer_id = random_object_id();
    let draft = OfferDraft {
        offer_id: offer_id.clone(),
        target_pubkey: request.requester_pubkey.clone(),
        allocation_source: "human_request",
        turn_role: "participant",
        allocation_event_id: Some(request.request_id.clone()),
        selection_reason: None,
        source_intent_id: None,
        source_request_id: Some(request.request_id.clone()),
        source_handoff_id: None,
        source_speech_event_id: None,
        reason_type: None,
        reason_text: None,
        basis_speech_revision: state.speech_revision,
        depth_mode: "preserve",
        previous_handoff_depth: state.handoff_depth,
        requested_handoff_depth: state.handoff_depth,
    };
    let deadline = insert_offer_tx(tx, community_id, session_id, &draft, config, now).await?;
    Ok((
        StateTarget::offered(state, offer_id.clone(), deadline),
        offer_id,
    ))
}

async fn fallback_candidate_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    eligible_through_epoch: i64,
) -> Result<Option<IntentRow>> {
    let moderator = load_moderator_tx(tx, community_id, session_id).await?;
    let rows = sqlx::query(
        "SELECT i.intent_id, i.author_pubkey, i.current_event_id, i.state, \
                i.deferred_by_offer_id, i.eligible_decision_epoch \
         FROM meeting_speech_intents i \
         WHERE i.community_id = $1 AND i.session_id = $2 AND i.state = 'pending' \
           AND i.deferred_by_offer_id IS NULL \
           AND i.eligible_decision_epoch <= $5 \
           AND NOT EXISTS ( \
               SELECT 1 FROM meeting_baton_fallback_attempts f \
               WHERE f.community_id = i.community_id AND f.session_id = i.session_id \
                 AND f.intent_id = i.intent_id \
                 AND f.current_intent_event_id = i.current_event_id \
                 AND f.speech_revision = $3 \
           ) \
         ORDER BY CASE WHEN i.author_pubkey = $4 THEN 0 ELSE 1 END, \
                  i.created_at, i.intent_id \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(state.speech_revision)
    .bind(&moderator)
    .bind(eligible_through_epoch)
    .fetch_all(tx.as_mut())
    .await?;
    let candidates: Vec<IntentRow> = rows
        .into_iter()
        .map(intent_from_row)
        .collect::<Result<_>>()?;
    let has_other_valid: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
               AND deferred_by_offer_id IS NULL AND author_pubkey <> $3 \
               AND eligible_decision_epoch <= $4 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&moderator)
    .bind(eligible_through_epoch)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(candidates.into_iter().find(|candidate| {
        candidate.author_pubkey != moderator
            || state.consecutive_moderator_speeches == 0
            || !has_other_valid
    }))
}

fn next_decision_epoch(current: i64) -> Result<i64> {
    current
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("meeting decision epoch overflow".to_string()))
}

async fn release_human_request_handoff_blocks_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<Vec<u8>>> {
    let rows = sqlx::query(
        "UPDATE meeting_directed_handoffs \
         SET blocked_by = NULL, moderator_retry_blocked_fingerprint = NULL, \
             moderator_retry_not_before = NULL \
         WHERE community_id = $1 AND session_id = $2 \
           AND question_state = 'open' AND blocked_by = 'human_request' \
           AND NOT EXISTS ( \
               SELECT 1 FROM meeting_human_floor_requests \
               WHERE community_id = $1 AND session_id = $2 \
                 AND state IN ('queued', 'offered') \
           ) \
         RETURNING handoff_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    let mut handoff_ids = rows
        .into_iter()
        .map(|row| row.try_get("handoff_id"))
        .collect::<std::result::Result<Vec<Vec<u8>>, _>>()?;
    handoff_ids.sort();
    Ok(handoff_ids)
}

async fn clear_handoff_retry_suppression_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<()> {
    sqlx::query(
        "UPDATE meeting_directed_handoffs \
         SET moderator_retry_blocked_fingerprint = NULL, moderator_retry_not_before = NULL \
         WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
           AND moderator_retry_blocked_fingerprint IS NOT NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn suppress_handoff_cohort_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    decision_epoch: i64,
    fingerprint: &[u8],
    retry_not_before: DateTime<Utc>,
) -> Result<Vec<Vec<u8>>> {
    let rows = sqlx::query(
        "UPDATE meeting_directed_handoffs \
         SET moderator_retry_blocked_fingerprint = $4, \
             moderator_retry_not_before = $5 \
         WHERE community_id = $1 AND session_id = $2 \
           AND question_state = 'open' AND blocked_by IS NULL \
           AND eligible_decision_epoch <= $3 \
         RETURNING handoff_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(decision_epoch)
    .bind(fingerprint)
    .bind(retry_not_before)
    .fetch_all(tx.as_mut())
    .await?;
    let mut handoff_ids = rows
        .into_iter()
        .map(|row| row.try_get("handoff_id"))
        .collect::<std::result::Result<Vec<Vec<u8>>, _>>()?;
    handoff_ids.sort();
    Ok(handoff_ids)
}

struct ModeratorControlReturn {
    target: StateTarget,
    unblocked_handoff_ids: Vec<Vec<u8>>,
}

impl ModeratorControlReturn {
    fn into_target_with_effects(self, effects: &mut Vec<Value>) -> StateTarget {
        for handoff_id in self.unblocked_handoff_ids {
            effects.push(effect(
                "handoff_unblocked",
                "handoff",
                &handoff_id,
                Some("human_request"),
                None,
            ));
        }
        self.target
    }
}

async fn return_control_to_moderator_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    config: &BatonConfig,
    now: DateTime<Utc>,
    increment_control_epoch: bool,
) -> Result<ModeratorControlReturn> {
    let unblocked_handoff_ids =
        release_human_request_handoff_blocks_tx(tx, community_id, session_id).await?;
    let next_epoch = next_decision_epoch(state.decision_epoch)?;
    let next_has_intent = fallback_candidate_tx(tx, community_id, session_id, state, next_epoch)
        .await?
        .is_some();
    let next_has_handoff =
        current_cohort_has_handoffs_tx(tx, community_id, session_id, next_epoch).await?;
    let next_exists = next_has_intent || next_has_handoff;
    let mut target = StateTarget::from_state(state);
    target.active_offer_id = None;
    target.active_grant_id = None;
    target.handoff_depth = 0;
    target.forced_return_to_moderator = false;
    target.recall_event_id = None;
    target.decision_attempt = 0;
    if increment_control_epoch {
        target.control_epoch += 1;
    }
    if next_exists {
        target.decision_epoch = next_epoch;
    }
    if next_has_intent {
        clear_handoff_retry_suppression_tx(tx, community_id, session_id).await?;
        target.phase = BatonPhase::ModeratorControl;
        target.moderator_decision_started_at = Some(now);
        let deadline = now + Duration::milliseconds(config.moderator_decision_ms);
        target.moderator_decision_deadline = Some(deadline);
        target.next_action_at = Some(deadline);
    } else {
        target.phase = BatonPhase::ModeratorIdle;
        if next_has_handoff && target.active_decision_attempt_id.is_some() {
            // A still-running Attempt belongs to the pre-Human epoch. Bound
            // the wait for its natural terminal with the new control
            // window; never inherit its stale deadline into this Cohort.
            target.moderator_decision_started_at = Some(now);
            let deadline = now + Duration::milliseconds(config.moderator_decision_ms);
            target.moderator_decision_deadline = Some(deadline);
            target.next_action_at = Some(deadline);
        } else if let Some(attempt_id) = target.active_decision_attempt_id.as_deref() {
            let attempt = load_moderator_attempt_tx(tx, community_id, session_id, attempt_id)
                .await?
                .ok_or_else(|| {
                    DbError::InvalidData(
                        "active moderator DecisionAttempt is missing during control return"
                            .to_string(),
                    )
                })?;
            target.moderator_decision_started_at = Some(attempt.started_at);
            target.moderator_decision_deadline = Some(attempt.deadline_at);
            target.next_action_at = Some(attempt.deadline_at);
        } else {
            target.moderator_decision_started_at = None;
            target.moderator_decision_deadline = None;
            target.next_action_at = None;
        }
    }
    Ok(ModeratorControlReturn {
        target,
        unblocked_handoff_ids,
    })
}

async fn ensure_moderator_window_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<StateTarget> {
    if !matches!(
        state.phase,
        BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
    ) || state.active_decision_attempt_id.is_some()
    {
        return Ok(StateTarget::from_state(state));
    }
    let eligible_through_epoch = if state.phase == BatonPhase::ModeratorIdle {
        next_decision_epoch(state.decision_epoch)?
    } else {
        state.decision_epoch
    };
    let candidate =
        fallback_candidate_tx(tx, community_id, session_id, state, eligible_through_epoch).await?;
    let mut target = StateTarget::from_state(state);
    match (state.phase, candidate.is_some()) {
        (BatonPhase::ModeratorIdle, true) => {
            clear_handoff_retry_suppression_tx(tx, community_id, session_id).await?;
            target.phase = BatonPhase::ModeratorControl;
            target.decision_epoch = eligible_through_epoch;
            target.decision_attempt = 0;
            target.active_decision_attempt_id = None;
            target.moderator_decision_started_at = Some(now);
            let deadline = now + Duration::milliseconds(config.moderator_decision_ms);
            target.moderator_decision_deadline = Some(deadline);
            target.next_action_at = Some(deadline);
        }
        (BatonPhase::ModeratorControl, false) => {
            target.phase = BatonPhase::ModeratorIdle;
            target.moderator_decision_started_at = None;
            target.moderator_decision_deadline = None;
            target.next_action_at = None;
        }
        _ => {}
    }
    Ok(target)
}

async fn release_offer_deferrals_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    offer_id: &[u8],
    now: DateTime<Utc>,
) -> Result<Vec<Vec<u8>>> {
    let rows = sqlx::query(
        "UPDATE meeting_speech_intents \
         SET deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL, \
             updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 \
           AND deferred_by_offer_id = $3 AND state = 'pending' \
         RETURNING intent_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(offer_id)
    .bind(now)
    .fetch_all(tx.as_mut())
    .await?;
    let mut intent_ids = rows
        .into_iter()
        .map(|row| row.try_get("intent_id"))
        .collect::<std::result::Result<Vec<Vec<u8>>, sqlx::Error>>()?;
    intent_ids.sort();
    Ok(intent_ids)
}

#[derive(Debug)]
struct OfferFailureResult {
    target: StateTarget,
    intent_changed: bool,
    effects: Vec<Value>,
}

#[allow(clippy::too_many_arguments)]
async fn fail_active_offer_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    offer: &OfferRow,
    terminal_state: &'static str,
    response_event_id: Option<&[u8]>,
    response_reason: Option<&str>,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<OfferFailureResult> {
    let updated = sqlx::query(
        "UPDATE meeting_baton_offers \
         SET state = $4, response_event_id = $5, response_reason = $6, resolved_at = $7 \
         WHERE community_id = $1 AND session_id = $2 AND offer_id = $3 \
           AND state = 'pending'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&offer.offer_id)
    .bind(terminal_state)
    .bind(response_event_id)
    .bind(response_reason)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "active Offer changed while holding the Session lock".to_string(),
        ));
    }
    let mut effects = vec![effect(
        match terminal_state {
            "declined" => "offer_declined",
            "timed_out" => "offer_timed_out",
            "preempted" => "offer_preempted",
            "recalled" => "offer_recalled",
            "source_changed" => "offer_source_changed",
            "source_withdrawn" => "offer_source_withdrawn",
            _ => "offer_ended",
        },
        "offer",
        &offer.offer_id,
        Some("pending"),
        Some(terminal_state),
    )];
    let reactivated_intent_ids =
        release_offer_deferrals_tx(tx, community_id, session_id, &offer.offer_id, now).await?;
    let mut intent_changed = !reactivated_intent_ids.is_empty();
    for intent_id in reactivated_intent_ids {
        effects.push(effect(
            "intent_reactivated",
            "intent",
            &intent_id,
            Some("deferred"),
            Some("pending"),
        ));
    }
    if let Some(intent_id) = offer.source_intent_id.as_deref() {
        let outcome = terminal_state;
        sqlx::query(
            "UPDATE meeting_speech_intents \
             SET last_attempt_outcome = $4, updated_at = $5 \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
               AND state = 'pending'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(intent_id)
        .bind(outcome)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        intent_changed = true;
        effects.push(effect(
            "intent_attempt_failed",
            "intent",
            intent_id,
            Some("pending"),
            Some("pending"),
        ));
    }
    sort_effects_by_object_id(&mut effects[1..]);
    if let Some(request_id) = offer.source_request_id.as_deref() {
        let request_state = match terminal_state {
            "declined" => "declined",
            "timed_out" => "timed_out",
            "source_withdrawn" => "withdrawn",
            "ended" => "ended",
            _ => "queued",
        };
        if request_state == "queued" {
            sqlx::query(
                "UPDATE meeting_human_floor_requests \
                 SET state = 'queued', offer_id = NULL \
                 WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
                   AND state = 'offered'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(request_id)
            .execute(tx.as_mut())
            .await?;
        } else {
            sqlx::query(
                "UPDATE meeting_human_floor_requests \
                 SET state = $4, terminal_event_id = $5, terminal_at = $6 \
                 WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
                   AND state = 'offered'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(request_id)
            .bind(request_state)
            .bind(response_event_id)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
        }
        intent_changed = true;
        effects.push(effect(
            match request_state {
                "declined" => "human_declined",
                "timed_out" => "human_timed_out",
                "withdrawn" => "human_withdrawn",
                "ended" => "human_ended",
                _ => "human_requested",
            },
            "human_request",
            request_id,
            Some("offered"),
            Some(request_state),
        ));
    }
    if let Some(handoff_id) = offer.source_handoff_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_directed_handoffs \
             SET last_attempt_outcome = $4 \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
               AND question_state = 'open'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(handoff_id)
        .bind(terminal_state)
        .execute(tx.as_mut())
        .await?;
        effects.push(effect(
            "handoff_attempt_failed",
            "handoff",
            handoff_id,
            Some("open"),
            Some("open"),
        ));
    }
    let target = if let Some(request) =
        earliest_queued_human_tx(tx, community_id, session_id).await?
    {
        let (target, new_offer_id) =
            offer_human_request_tx(tx, community_id, session_id, state, &request, config, now)
                .await?;
        effects.push(effect(
            "human_offered",
            "human_request",
            &request.request_id,
            Some("queued"),
            Some("offered"),
        ));
        effects.push(effect(
            "offer_created",
            "offer",
            &new_offer_id,
            None,
            Some("pending"),
        ));
        target
    } else {
        let target =
            return_control_to_moderator_tx(tx, community_id, session_id, state, config, now, true)
                .await?
                .into_target_with_effects(&mut effects);
        append_control_return_effects(&mut effects, state, session_id);
        target
    };
    Ok(OfferFailureResult {
        target,
        intent_changed,
        effects,
    })
}

#[derive(Debug)]
struct GrantFailureResult {
    target: StateTarget,
    intent_changed: bool,
    effects: Vec<Value>,
}

#[allow(clippy::too_many_arguments)]
async fn fail_active_grant_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    grant: &GrantRow,
    terminal_state: &'static str,
    terminal_event_id: Option<&[u8]>,
    terminal_reason: Option<&str>,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<GrantFailureResult> {
    let updated = sqlx::query(
        "UPDATE meeting_baton_grants \
         SET state = $4, terminal_event_id = $5, terminal_reason = $6, terminal_at = $7 \
         WHERE community_id = $1 AND session_id = $2 AND grant_id = $3 \
           AND state = 'active'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&grant.grant_id)
    .bind(terminal_state)
    .bind(terminal_event_id)
    .bind(terminal_reason)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "active Grant changed while holding the Session lock".to_string(),
        ));
    }
    let effect_type = match terminal_state {
        "yielded" => "grant_yielded",
        "soft_expired" => "grant_soft_expired",
        "hard_expired" => "grant_hard_expired",
        _ => "grant_ended",
    };
    let mut effects = vec![effect(
        effect_type,
        "grant",
        &grant.grant_id,
        Some("active"),
        Some(terminal_state),
    )];
    let reactivated_intent_ids =
        release_offer_deferrals_tx(tx, community_id, session_id, &grant.source_offer_id, now)
            .await?;
    let mut intent_changed = !reactivated_intent_ids.is_empty();
    for intent_id in reactivated_intent_ids {
        effects.push(effect(
            "intent_reactivated",
            "intent",
            &intent_id,
            Some("deferred"),
            Some("pending"),
        ));
    }
    if let Some(intent_id) = grant.source_intent_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_speech_intents \
             SET state = 'stale', last_attempt_outcome = $4, \
                 terminal_event_id = $5, terminal_at = $6, updated_at = $6, \
                 deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
               AND state = 'selected'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(intent_id)
        .bind(terminal_state)
        .bind(terminal_event_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        intent_changed = true;
        effects.push(effect(
            "intent_stale",
            "intent",
            intent_id,
            Some("selected"),
            Some("stale"),
        ));
    }
    sort_effects_by_object_id(&mut effects[1..]);
    if let Some(handoff_id) = grant.source_handoff_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_directed_handoffs \
             SET last_attempt_outcome = $4 \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
               AND question_state = 'open'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(handoff_id)
        .bind(terminal_state)
        .execute(tx.as_mut())
        .await?;
        effects.push(effect(
            "handoff_attempt_failed",
            "handoff",
            handoff_id,
            Some("open"),
            Some("open"),
        ));
    }
    let mut scheduling_state = state.clone();
    if grant.depth_mode == "increment_provisional" {
        scheduling_state.handoff_depth = grant.previous_handoff_depth;
    }
    let target =
        if let Some(request) = earliest_queued_human_tx(tx, community_id, session_id).await? {
            let (target, offer_id) = offer_human_request_tx(
                tx,
                community_id,
                session_id,
                &scheduling_state,
                &request,
                config,
                now,
            )
            .await?;
            effects.push(effect(
                "human_offered",
                "human_request",
                &request.request_id,
                Some("queued"),
                Some("offered"),
            ));
            effects.push(effect(
                "offer_created",
                "offer",
                &offer_id,
                None,
                Some("pending"),
            ));
            target
        } else {
            let target = return_control_to_moderator_tx(
                tx,
                community_id,
                session_id,
                &scheduling_state,
                config,
                now,
                true,
            )
            .await?
            .into_target_with_effects(&mut effects);
            append_control_return_effects(&mut effects, &scheduling_state, session_id);
            target
        };
    effects.push(phase_effect(session_id, state.phase, target.phase));
    Ok(GrantFailureResult {
        target,
        intent_changed,
        effects,
    })
}

async fn advance_due_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    mut state: StateRow,
    now: DateTime<Utc>,
) -> Result<(StateRow, Vec<BatonTransitionResult>)> {
    let config = load_config_tx(tx, community_id, session_id).await?;
    let mut transitions = Vec::new();
    if state.phase == BatonPhase::Offered {
        let offer_id = state.active_offer_id.as_deref().ok_or_else(|| {
            DbError::InvalidData("offered Baton state has no active Offer".to_string())
        })?;
        let offer = load_offer_tx(tx, community_id, session_id, offer_id)
            .await?
            .ok_or_else(|| DbError::InvalidData("active Offer is missing".to_string()))?;
        if now >= offer.ack_deadline {
            let failed = fail_active_offer_tx(
                tx,
                community_id,
                session_id,
                &state,
                &offer,
                "timed_out",
                None,
                None,
                &config,
                now,
            )
            .await?;
            let mut effects = failed.effects;
            if state.phase != failed.target.phase {
                effects.push(phase_effect(session_id, state.phase, failed.target.phase));
            }
            let delta = RevisionDelta {
                floor: true,
                intent: failed.intent_changed,
                speech: false,
            };
            let transition = TransitionSpec::deadline(
                "offer_timed_out",
                Some(offer.offer_id),
                "offer_ack",
                effects,
            );
            let (next, result) = commit_transition_tx(
                tx,
                community_id,
                session_id,
                relay_keys,
                &state,
                failed.target,
                delta,
                transition,
                now,
            )
            .await?;
            state = next;
            transitions.push(result);
        }
    } else if state.phase == BatonPhase::Granted {
        let grant_id = state.active_grant_id.as_deref().ok_or_else(|| {
            DbError::InvalidData("granted Baton state has no active Grant".to_string())
        })?;
        let grant = load_grant_tx(tx, community_id, session_id, grant_id)
            .await?
            .ok_or_else(|| DbError::InvalidData("active Grant is missing".to_string()))?;
        let terminal = if now >= grant.hard_deadline {
            Some(("hard_expired", "grant_hard_expired", "grant_hard"))
        } else if now >= grant.soft_lease_expires_at {
            Some(("soft_expired", "grant_soft_expired", "grant_soft"))
        } else {
            None
        };
        if let Some((terminal_state, primary_type, deadline_type)) = terminal {
            let failed = fail_active_grant_tx(
                tx,
                community_id,
                session_id,
                &state,
                &grant,
                terminal_state,
                None,
                None,
                &config,
                now,
            )
            .await?;
            let delta = RevisionDelta {
                floor: true,
                intent: failed.intent_changed,
                speech: false,
            };
            let transition = TransitionSpec::deadline(
                primary_type,
                Some(grant.grant_id),
                deadline_type,
                failed.effects,
            );
            let (next, result) = commit_transition_tx(
                tx,
                community_id,
                session_id,
                relay_keys,
                &state,
                failed.target,
                delta,
                transition,
                now,
            )
            .await?;
            state = next;
            transitions.push(result);
        }
    } else if matches!(
        state.phase,
        BatonPhase::ModeratorControl | BatonPhase::ModeratorIdle
    ) && state
        .moderator_decision_deadline
        .is_some_and(|deadline| now >= deadline)
    {
        let mut effects = Vec::new();
        if let Some(attempt_id) = state.active_decision_attempt_id.as_deref() {
            let updated = sqlx::query(
                "UPDATE meeting_moderator_decision_attempts \
                 SET state = 'timed_out', terminal_reason = 'deadline_expired', \
                     terminal_at = $4 \
                 WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
                   AND state = 'running'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(attempt_id)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            if updated.rows_affected() == 1 {
                effects.push(effect(
                    "moderator_decision_attempt_timed_out",
                    "moderator_decision_attempt",
                    attempt_id,
                    Some("running"),
                    Some("timed_out"),
                ));
            }
            let (fingerprint, attempt_decision_epoch): (Vec<u8>, i64) = sqlx::query_as(
                "SELECT candidate_snapshot_hash, decision_epoch \
                 FROM meeting_moderator_decision_attempts \
                 WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(attempt_id)
            .fetch_one(tx.as_mut())
            .await?;
            let suppressed =
                if updated.rows_affected() == 1 && attempt_decision_epoch == state.decision_epoch {
                    suppress_handoff_cohort_tx(
                        tx,
                        community_id,
                        session_id,
                        state.decision_epoch,
                        &fingerprint,
                        now + Duration::milliseconds(config.moderator_decision_ms),
                    )
                    .await?
                } else {
                    Vec::new()
                };
            for handoff_id in suppressed {
                effects.push(effect(
                    "handoff_moderator_retry_suppressed",
                    "handoff",
                    &handoff_id,
                    None,
                    Some("suppressed"),
                ));
            }
        }
        let candidate =
            fallback_candidate_tx(tx, community_id, session_id, &state, state.decision_epoch)
                .await?;
        let (mut target, object_id, delta) = if let Some(candidate) = candidate {
            let moderator = load_moderator_tx(tx, community_id, session_id).await?;
            let offer_id = random_object_id();
            let turn_role = if candidate.author_pubkey == moderator {
                "moderator_self"
            } else {
                "participant"
            };
            let draft = OfferDraft {
                offer_id: offer_id.clone(),
                target_pubkey: candidate.author_pubkey.clone(),
                allocation_source: "fallback",
                turn_role,
                allocation_event_id: None,
                selection_reason: None,
                source_intent_id: Some(candidate.intent_id.clone()),
                source_request_id: None,
                source_handoff_id: None,
                source_speech_event_id: None,
                reason_type: None,
                reason_text: None,
                basis_speech_revision: state.speech_revision,
                depth_mode: "reset",
                previous_handoff_depth: state.handoff_depth,
                requested_handoff_depth: 0,
            };
            let deadline =
                insert_offer_tx(tx, community_id, session_id, &draft, &config, now).await?;
            sqlx::query(
                "INSERT INTO meeting_baton_fallback_attempts \
                     (community_id, session_id, intent_id, current_intent_event_id, \
                      speech_revision, offer_id, attempted_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&candidate.intent_id)
            .bind(&candidate.current_event_id)
            .bind(state.speech_revision)
            .bind(&offer_id)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            effects.push(effect(
                "intent_attempted",
                "intent",
                &candidate.intent_id,
                Some("pending"),
                Some("pending"),
            ));
            effects.push(effect(
                "offer_created",
                "offer",
                &offer_id,
                None,
                Some("pending"),
            ));
            effects.push(control_effect(session_id, "fallback_attempt_recorded"));
            (
                StateTarget::offered(&state, offer_id.clone(), deadline),
                Some(offer_id),
                RevisionDelta::FLOOR_INTENT,
            )
        } else {
            let mut target = StateTarget::from_state(&state);
            target.phase = BatonPhase::ModeratorIdle;
            target.moderator_decision_started_at = None;
            target.moderator_decision_deadline = None;
            target.next_action_at = None;
            (target, None, RevisionDelta::FLOOR)
        };
        target.active_decision_attempt_id = None;
        effects.push(phase_effect(session_id, state.phase, target.phase));
        let transition = TransitionSpec::deadline(
            "moderator_fallback",
            object_id,
            "moderator_decision",
            effects,
        );
        let (next, result) = commit_transition_tx(
            tx,
            community_id,
            session_id,
            relay_keys,
            &state,
            target,
            delta,
            transition,
            now,
        )
        .await?;
        state = next;
        transitions.push(result);
    }
    Ok((state, transitions))
}

/// List a bounded set of Meeting V1 Sessions whose current deadline is due.
///
/// Each candidate must subsequently pass through [`recover_meeting_v1`],
/// which locks the Session and rechecks the database clock. Claiming is atomic
/// and advances a bounded exponential retry fence, so concurrent workers do
/// not select the same Session and one corrupt due row cannot starve later
/// rows in a bounded batch.
pub async fn claim_due_baton_sessions(db: &Db, limit: i64) -> Result<Vec<BatonDueSession>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "WITH due AS ( \
             SELECT s.community_id, s.session_id \
             FROM meeting_baton_state s \
             JOIN meeting_sessions m \
               ON m.community_id = s.community_id AND m.session_id = s.session_id \
             WHERE s.next_action_at <= clock_timestamp() \
               AND s.recovery_retry_at <= clock_timestamp() \
               AND m.status = 'active' AND m.schema_version = 2 \
               AND m.floor_policy_version = $1 \
             ORDER BY s.next_action_at, s.community_id, s.session_id \
             FOR UPDATE OF s SKIP LOCKED \
             LIMIT $2 \
         ), claimed AS ( \
             UPDATE meeting_baton_state s \
             SET recovery_retry_at = clock_timestamp() \
                   + make_interval(secs => LEAST(300, \
                       5 * (1 << LEAST(s.recovery_attempts, 6)))), \
                 recovery_attempts = s.recovery_attempts + 1 \
             FROM due \
             WHERE s.community_id = due.community_id \
               AND s.session_id = due.session_id \
             RETURNING s.community_id, s.session_id, s.next_action_at \
         ) \
         SELECT community_id, session_id, next_action_at \
         FROM claimed \
         ORDER BY next_action_at, community_id, session_id",
    )
    .bind(BATON_POLICY_VERSION)
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(BatonDueSession {
                community_id: CommunityId::from_uuid(row.try_get("community_id")?),
                session_id: row.try_get("session_id")?,
                next_action_at: row.try_get("next_action_at")?,
            })
        })
        .collect()
}

/// Recover any due Offer, Grant, or moderator-decision deadline for one V1
/// Session and commit Relay State/outbox rows atomically.
///
/// Returning an empty vector means another worker or a participant command
/// already advanced the Session, or its deadline has moved into the future.
pub async fn recover_meeting_v1(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
) -> Result<Vec<BatonTransitionResult>> {
    let mut tx = db.begin_transaction().await?;
    let session = lock_v1_session_tx(&mut tx, community_id, session_id).await?;
    if session.status != "active" {
        tx.commit().await?;
        return Ok(Vec::new());
    }
    if let Some(snapshot) = crate::meeting_revocation::recover_revoked_roster_v1_tx(
        &mut tx,
        community_id,
        session_id,
        relay_keys,
    )
    .await?
    {
        let transition = BatonTransitionResult {
            primary_type: "participant_revoked".to_string(),
            state_revision: snapshot.state_revision,
            state_event_id: snapshot.state_event_id,
        };
        tx.commit().await?;
        return Ok(vec![transition]);
    }
    let state = load_state_tx(&mut tx, community_id, session_id, true).await?;
    let now = database_now(&mut tx).await?;
    let (_, transitions) =
        advance_due_locked_tx(&mut tx, community_id, session_id, relay_keys, state, now).await?;
    if transitions.is_empty() {
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET recovery_retry_at = '-infinity', recovery_attempts = 0 \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .execute(tx.as_mut())
        .await?;
    }
    tx.commit().await?;
    Ok(transitions)
}

enum ApplyResult {
    Accepted {
        canonical_object_id: Option<Vec<u8>>,
        state_revision: i64,
    },
    Rejected {
        code: &'static str,
        canonical_object_id: Option<Vec<u8>>,
    },
    RejectedWithRetry {
        code: &'static str,
        canonical_object_id: Option<Vec<u8>>,
        retry_ticket: RetryTicketDraft,
    },
}

fn rejection_was_caused_by_recovery(
    command: &BatonCommand,
    before: &StateRow,
    after: &StateRow,
    transitions: &[BatonTransitionResult],
    command_depended_on_recovered_object: bool,
) -> bool {
    if transitions.is_empty() {
        return false;
    }
    match command {
        BatonCommand::OfferAck { offer_id } | BatonCommand::OfferDecline { offer_id, .. } => {
            before.active_offer_id.as_deref() == Some(offer_id)
                && after.active_offer_id.as_deref() != Some(offer_id)
        }
        BatonCommand::GrantProgress { grant_id, .. }
        | BatonCommand::GrantYield { grant_id, .. }
        | BatonCommand::Speech { grant_id, .. } => {
            before.active_grant_id.as_deref() == Some(grant_id)
                && after.active_grant_id.as_deref() != Some(grant_id)
        }
        BatonCommand::ModeratorSelect { .. } => {
            before.phase == BatonPhase::ModeratorControl
                && transitions
                    .iter()
                    .any(|transition| transition.primary_type == "moderator_fallback")
        }
        BatonCommand::ModeratorRecall { control_epoch, .. } => {
            before.control_epoch == *control_epoch
                && after.control_epoch != *control_epoch
                && transitions.iter().any(|transition| {
                    matches!(
                        transition.primary_type.as_str(),
                        "offer_timed_out" | "grant_soft_expired" | "grant_hard_expired"
                    )
                })
        }
        BatonCommand::HumanWithdraw { .. } => command_depended_on_recovered_object,
        _ => false,
    }
}

async fn command_depends_on_current_deadline_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    command: &BatonCommand,
) -> Result<bool> {
    let BatonCommand::HumanWithdraw { request_id } = command else {
        return Ok(false);
    };
    let Some(offer_id) = state.active_offer_id.as_deref() else {
        return Ok(false);
    };
    let offer = load_offer_tx(tx, community_id, session_id, offer_id).await?;
    Ok(offer.is_some_and(|offer| offer.source_request_id.as_deref() == Some(request_id)))
}

/// Execute one strictly parsed Meeting V1 command with lazy deadline recovery,
/// private command receipts, authoritative State, and outbox writes.
///
/// Authorization and semantic validation happen while holding the
/// `meeting_sessions` row lock. Accepted commands are persisted only inside
/// the successful command savepoint; terminal semantic rejections roll that
/// savepoint back and commit only a private receipt.
pub async fn execute_baton_command(
    db: &Db,
    params: BatonCommandTxParams<'_>,
) -> Result<BatonCommitResult> {
    preflight_command(&params)?;
    let author = params.event.pubkey.as_bytes().to_vec();
    let event_id = params.event.id.as_bytes().to_vec();
    let action = params.command.action();
    let mut tx = db.begin_transaction().await?;
    let session = lock_v1_session_tx(&mut tx, params.community_id, params.session_id).await?;
    let actor = load_actor_tx(&mut tx, params.community_id, params.session_id, &author).await?;
    let now = database_now(&mut tx).await?;
    let state = load_state_tx(&mut tx, params.community_id, params.session_id, true).await?;
    let state_before_recovery = state.clone();
    let command_depended_on_recovered_object = command_depends_on_current_deadline_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        &state,
        &params.command,
    )
    .await?;
    if session.status == "active" {
        if let Some(snapshot) = crate::meeting_revocation::recover_revoked_roster_v1_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
        )
        .await?
        {
            let transition = BatonTransitionResult {
                primary_type: "participant_revoked".to_string(),
                state_revision: snapshot.state_revision,
                state_event_id: snapshot.state_event_id.clone(),
            };
            if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                &actor.pubkey,
            )
            .await?
            {
                tx.commit().await?;
                return Err(DbError::AccessDenied(
                    "meeting actor was durably revoked from this Session".to_string(),
                ));
            }
            if !actor_security_active_tx(&mut tx, params.community_id, &actor.pubkey).await? {
                tx.commit().await?;
                return Err(DbError::AccessDenied(
                    "meeting actor is no longer an active writable community principal".to_string(),
                ));
            }
            if let Some(receipt) = load_receipt_tx(&mut tx, params.community_id, &event_id).await? {
                if receipt.author_pubkey != actor.pubkey {
                    return Err(DbError::AccessDenied(
                        "not authorized for this private meeting receipt".to_string(),
                    ));
                }
                tx.commit().await?;
                return Ok(BatonCommitResult {
                    recovery_transitions: vec![transition],
                    command_outcome: BatonCommandOutcome::Duplicate {
                        accepted: receipt.accepted,
                        outcome_class: receipt.outcome_class,
                        canonical_object_id: receipt.canonical_object_id,
                        state_revision: receipt.state_revision,
                        outcome_code: receipt.outcome_code,
                        retry_ticket_id: receipt.retry_ticket_id,
                    },
                    snapshot,
                });
            }
            insert_receipt_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                params.event,
                action,
                false,
                "rejected_after_recovery",
                "participant_revoked",
                None,
                Some(snapshot.state_revision),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(BatonCommitResult {
                recovery_transitions: vec![transition],
                command_outcome: BatonCommandOutcome::RejectedAfterRecovery {
                    code: "participant_revoked".to_string(),
                    canonical_object_id: None,
                    retry_ticket_id: None,
                },
                snapshot,
            });
        }
    }
    if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        &actor.pubkey,
    )
    .await?
    {
        return Err(DbError::AccessDenied(
            "meeting actor was durably revoked from this Session".to_string(),
        ));
    }
    if !actor_security_active_tx(&mut tx, params.community_id, &actor.pubkey).await? {
        if session.status == "active" {
            if let Some(snapshot) = crate::meeting_revocation::recover_revoked_roster_v1_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                params.relay_keys,
            )
            .await?
            {
                let transition = BatonTransitionResult {
                    primary_type: "participant_revoked".to_string(),
                    state_revision: snapshot.state_revision,
                    state_event_id: snapshot.state_event_id.clone(),
                };
                tx.commit().await?;
                return Ok(BatonCommitResult {
                    recovery_transitions: vec![transition],
                    command_outcome: BatonCommandOutcome::RejectedAfterRecovery {
                        code: "participant_revoked".to_string(),
                        canonical_object_id: None,
                        retry_ticket_id: None,
                    },
                    snapshot,
                });
            }
        }
        return Err(DbError::AccessDenied(
            "meeting access has been revoked".to_string(),
        ));
    }
    // Stable capability checks must observe the command's referenced object
    // before deadline recovery can replace or terminalize it. If recovery is
    // due, it is still committed below, but the unauthorized command never
    // receives a receipt and never reaches the command savepoint.
    let preauthorization = preauthorize_command_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        params.event,
        &actor,
        &params.command,
    )
    .await
    .err();
    let (state, recovery_transitions) = if session.status == "active" {
        advance_due_locked_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
            state,
            now,
        )
        .await?
    } else {
        (state, Vec::new())
    };
    if let Some(error) = preauthorization {
        if !recovery_transitions.is_empty() {
            tx.commit().await?;
        }
        return Err(error);
    }
    if let Some(receipt) = load_receipt_tx(&mut tx, params.community_id, &event_id).await? {
        if receipt.author_pubkey != actor.pubkey {
            return Err(DbError::AccessDenied(
                "not authorized for this private meeting receipt".to_string(),
            ));
        }
        let snapshot = load_snapshot_tx(&mut tx, params.community_id, params.session_id).await?;
        tx.commit().await?;
        return Ok(BatonCommitResult {
            recovery_transitions,
            command_outcome: BatonCommandOutcome::Duplicate {
                accepted: receipt.accepted,
                outcome_class: receipt.outcome_class,
                canonical_object_id: receipt.canonical_object_id,
                state_revision: receipt.state_revision,
                outcome_code: receipt.outcome_code,
                retry_ticket_id: receipt.retry_ticket_id,
            },
            snapshot,
        });
    }
    if session.status != "active" || state.phase == BatonPhase::Ended {
        insert_receipt_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.event,
            action,
            false,
            "rejected_terminal",
            "meeting_ended",
            None,
            Some(state.state_revision),
            None,
        )
        .await?;
        let snapshot = load_snapshot_tx(&mut tx, params.community_id, params.session_id).await?;
        tx.commit().await?;
        return Ok(BatonCommitResult {
            recovery_transitions,
            command_outcome: BatonCommandOutcome::RejectedTerminal {
                code: "meeting_ended".to_string(),
                canonical_object_id: None,
                retry_ticket_id: None,
            },
            snapshot,
        });
    }

    sqlx::query("SAVEPOINT meeting_v1_command")
        .execute(tx.as_mut())
        .await?;
    let applied = apply_command_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        params.event,
        params.relay_keys,
        &params.command,
        &actor,
        &state,
        now,
    )
    .await?;
    let command_outcome = match applied {
        ApplyResult::Accepted {
            canonical_object_id,
            state_revision,
        } => {
            sqlx::query("RELEASE SAVEPOINT meeting_v1_command")
                .execute(tx.as_mut())
                .await?;
            insert_receipt_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                params.event,
                action,
                true,
                "accepted",
                "accepted",
                canonical_object_id.as_deref(),
                Some(state_revision),
                None,
            )
            .await?;
            BatonCommandOutcome::Accepted {
                canonical_object_id,
                state_revision,
            }
        }
        ApplyResult::Rejected {
            code,
            canonical_object_id,
        } => {
            sqlx::query("ROLLBACK TO SAVEPOINT meeting_v1_command")
                .execute(tx.as_mut())
                .await?;
            sqlx::query("RELEASE SAVEPOINT meeting_v1_command")
                .execute(tx.as_mut())
                .await?;
            let outcome_class = if rejection_was_caused_by_recovery(
                &params.command,
                &state_before_recovery,
                &state,
                &recovery_transitions,
                command_depended_on_recovered_object,
            ) {
                "rejected_after_recovery"
            } else {
                "rejected_terminal"
            };
            insert_receipt_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                params.event,
                action,
                false,
                outcome_class,
                code,
                canonical_object_id.as_deref(),
                Some(state.state_revision),
                None,
            )
            .await?;
            if outcome_class == "rejected_terminal" {
                BatonCommandOutcome::RejectedTerminal {
                    code: code.to_string(),
                    canonical_object_id,
                    retry_ticket_id: None,
                }
            } else {
                BatonCommandOutcome::RejectedAfterRecovery {
                    code: code.to_string(),
                    canonical_object_id,
                    retry_ticket_id: None,
                }
            }
        }
        ApplyResult::RejectedWithRetry {
            code,
            canonical_object_id,
            retry_ticket,
        } => {
            sqlx::query("ROLLBACK TO SAVEPOINT meeting_v1_command")
                .execute(tx.as_mut())
                .await?;
            sqlx::query("RELEASE SAVEPOINT meeting_v1_command")
                .execute(tx.as_mut())
                .await?;
            insert_retry_ticket_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                &retry_ticket,
                now,
            )
            .await?;
            insert_receipt_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                params.event,
                action,
                false,
                "rejected_terminal",
                code,
                canonical_object_id.as_deref(),
                Some(state.state_revision),
                Some(&retry_ticket.retry_ticket_id),
            )
            .await?;
            BatonCommandOutcome::RejectedTerminal {
                code: code.to_string(),
                canonical_object_id,
                retry_ticket_id: Some(retry_ticket.retry_ticket_id),
            }
        }
    };
    let snapshot = load_snapshot_tx(&mut tx, params.community_id, params.session_id).await?;
    tx.commit().await?;
    Ok(BatonCommitResult {
        recovery_transitions,
        command_outcome,
        snapshot,
    })
}

#[allow(clippy::too_many_arguments)]
async fn apply_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    command: &BatonCommand,
    actor: &Actor,
    state: &StateRow,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    match command {
        BatonCommand::IntentSubmit {
            basis_speech_revision,
            summary,
            addressed_to,
        } => {
            apply_intent_submit_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                *basis_speech_revision,
                summary,
                addressed_to.as_deref(),
                now,
            )
            .await
        }
        BatonCommand::IntentRefresh {
            intent_id,
            previous_event_id,
            basis_speech_revision,
            summary,
            addressed_to,
        } => {
            apply_intent_refresh_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                intent_id,
                previous_event_id,
                *basis_speech_revision,
                summary,
                addressed_to.as_deref(),
                now,
            )
            .await
        }
        BatonCommand::IntentWithdraw {
            intent_id,
            previous_event_id,
        } => {
            apply_intent_withdraw_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                intent_id,
                previous_event_id,
                now,
            )
            .await
        }
        BatonCommand::ModeratorSelect { .. } => {
            apply_moderator_select_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::ModeratorDecisionAttemptStart { .. } => {
            apply_moderator_attempt_start_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::ModeratorDecisionAttemptFinish { .. } => {
            apply_moderator_attempt_finish_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::ModeratorDecisionRetry {
            attempt_id,
            retry_ticket_id,
            failed_action_event_id,
            expected_control_epoch,
            expected_decision_epoch,
            expected_attempt_number,
        } => {
            apply_moderator_retry_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                attempt_id,
                retry_ticket_id,
                failed_action_event_id,
                *expected_control_epoch,
                *expected_decision_epoch,
                *expected_attempt_number,
                now,
            )
            .await
        }
        BatonCommand::ModeratorCompleteCohort {
            attempt_id,
            expected_control_epoch,
            expected_decision_epoch,
        } => {
            apply_moderator_complete_cohort_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                attempt_id,
                *expected_control_epoch,
                *expected_decision_epoch,
                now,
            )
            .await
        }
        BatonCommand::ModeratorDecisionAttemptAbandon { attempt_id } => {
            apply_moderator_attempt_abandon_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                attempt_id,
                now,
            )
            .await
        }
        BatonCommand::ModeratorWithdrawSelf {
            attempt_id,
            intent_id,
            previous_event_id,
        } => {
            apply_moderator_withdraw_self_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                attempt_id,
                intent_id,
                previous_event_id,
                now,
            )
            .await
        }
        BatonCommand::ModeratorReject { .. }
        | BatonCommand::ModeratorDismissHandoff { .. }
        | BatonCommand::ModeratorRecall { .. } => {
            apply_moderator_other_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::HumanRequest | BatonCommand::HumanWithdraw { .. } => {
            apply_human_command_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::OfferAck { .. } | BatonCommand::OfferDecline { .. } => {
            apply_offer_command_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::GrantProgress { .. } | BatonCommand::GrantYield { .. } => {
            apply_grant_command_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
        BatonCommand::Speech { .. } => {
            apply_speech_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                actor,
                state,
                command,
                now,
            )
            .await
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish_accepted_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    _event: &Event,
    relay_keys: &Keys,
    state: &StateRow,
    target: StateTarget,
    delta: RevisionDelta,
    transition: TransitionSpec,
    canonical_object_id: Option<Vec<u8>>,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let (_, result) = commit_transition_tx(
        tx,
        community_id,
        session_id,
        relay_keys,
        state,
        target,
        delta,
        transition,
        now,
    )
    .await?;
    Ok(ApplyResult::Accepted {
        canonical_object_id,
        state_revision: result.state_revision,
    })
}

async fn existing_pending_intent_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    author_pubkey: &[u8],
) -> Result<Option<Vec<u8>>> {
    Ok(sqlx::query_scalar(
        "SELECT intent_id FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 AND author_pubkey = $3 \
           AND state = 'pending' \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(author_pubkey)
    .fetch_optional(tx.as_mut())
    .await?)
}

#[allow(clippy::too_many_arguments)]
async fn apply_intent_submit_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    basis_speech_revision: i64,
    summary: &str,
    addressed_to: Option<&[u8]>,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if basis_speech_revision > state.speech_revision {
        return Ok(ApplyResult::Rejected {
            code: "future_speech_revision",
            canonical_object_id: None,
        });
    }
    if let Some(existing) =
        existing_pending_intent_tx(tx, community_id, session_id, &actor.pubkey).await?
    {
        return Ok(ApplyResult::Rejected {
            code: "pending_intent_exists",
            canonical_object_id: Some(existing),
        });
    }
    if let Some(addressed_to) = addressed_to {
        if addressed_to == actor.pubkey.as_slice() {
            return Err(DbError::InvalidData(
                "Intent addressee must be another participant".to_string(),
            ));
        }
        ensure_participant_tx(tx, community_id, session_id, addressed_to).await?;
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let intent_id = event.id.as_bytes().to_vec();
    let current_cohort_is_open = state.decision_attempt == 0
        && match state.phase {
            BatonPhase::ModeratorControl => true,
            BatonPhase::ModeratorIdle => {
                current_cohort_has_candidates_tx(tx, community_id, session_id, state.decision_epoch)
                    .await?
            }
            BatonPhase::Offered | BatonPhase::Granted | BatonPhase::Ended => false,
        };
    let eligible_decision_epoch = if current_cohort_is_open {
        state.decision_epoch
    } else {
        next_decision_epoch(state.decision_epoch)?
    };
    sqlx::query(
        "INSERT INTO meeting_speech_intents \
             (community_id, session_id, intent_id, author_pubkey, current_event_id, \
              basis_speech_revision, summary, addressed_to, state, \
              eligible_decision_epoch, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $3, $5, $6, $7, 'pending', $8, $9, $9)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&intent_id)
    .bind(&actor.pubkey)
    .bind(basis_speech_revision)
    .bind(summary)
    .bind(addressed_to)
    .bind(eligible_decision_epoch)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let target =
        ensure_moderator_window_tx(tx, community_id, session_id, state, &config, now).await?;
    let phase_changed = target.phase != state.phase;
    let mut effects = vec![effect(
        "intent_submitted",
        "intent",
        &intent_id,
        None,
        Some("pending"),
    )];
    if phase_changed {
        effects.push(phase_effect(session_id, state.phase, target.phase));
    }
    let transition = TransitionSpec::command(
        "intent_submitted",
        Some(intent_id.clone()),
        &intent_id,
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta {
            floor: phase_changed,
            intent: true,
            speech: false,
        },
        transition,
        Some(intent_id),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_intent_refresh_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    intent_id: &[u8],
    previous_event_id: &[u8],
    basis_speech_revision: i64,
    summary: &str,
    addressed_to: Option<&[u8]>,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "intent_not_found",
            canonical_object_id: None,
        });
    };
    if intent.author_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "only the Intent author can refresh it".to_string(),
        ));
    }
    if intent.state != "pending" {
        return Ok(ApplyResult::Rejected {
            code: "intent_not_pending",
            canonical_object_id: Some(intent.intent_id),
        });
    }
    if intent.current_event_id != previous_event_id {
        return Ok(ApplyResult::Rejected {
            code: "stale_intent_event",
            canonical_object_id: Some(intent.current_event_id),
        });
    }
    if basis_speech_revision > state.speech_revision {
        return Ok(ApplyResult::Rejected {
            code: "future_speech_revision",
            canonical_object_id: Some(intent.intent_id),
        });
    }
    if let Some(addressed_to) = addressed_to {
        if addressed_to == actor.pubkey.as_slice() {
            return Err(DbError::InvalidData(
                "Intent addressee must be another participant".to_string(),
            ));
        }
        ensure_participant_tx(tx, community_id, session_id, addressed_to).await?;
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let active_source_offer = if state.phase == BatonPhase::Offered {
        if let Some(active_offer_id) = state.active_offer_id.as_deref() {
            let active = load_offer_tx(tx, community_id, session_id, active_offer_id).await?;
            active.filter(|offer| offer.source_intent_id.as_deref() == Some(intent_id))
        } else {
            None
        }
    } else {
        None
    };
    let mut effects = Vec::new();
    let mut floor_changed = false;
    let target = if let Some(offer) = active_source_offer {
        let failed = fail_active_offer_tx(
            tx,
            community_id,
            session_id,
            state,
            &offer,
            "source_changed",
            Some(event.id.as_bytes().as_slice()),
            None,
            &config,
            now,
        )
        .await?;
        effects.extend(failed.effects);
        floor_changed = true;
        failed.target
    } else {
        StateTarget::from_state(state)
    };
    sqlx::query(
        "UPDATE meeting_speech_intents \
         SET current_event_id = $4, basis_speech_revision = $5, summary = $6, \
             addressed_to = $7, last_attempt_outcome = CASE \
                 WHEN last_attempt_outcome = 'source_changed' THEN 'source_changed' \
                 ELSE last_attempt_outcome END, updated_at = $8 \
         WHERE community_id = $1 AND session_id = $2 AND intent_id = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(intent_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(basis_speech_revision)
    .bind(summary)
    .bind(addressed_to)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let target = if floor_changed && target.phase != BatonPhase::Offered {
        return_control_to_moderator_tx(tx, community_id, session_id, state, &config, now, true)
            .await?
            .into_target_with_effects(&mut effects)
    } else if floor_changed {
        target
    } else {
        ensure_moderator_window_tx(tx, community_id, session_id, state, &config, now).await?
    };
    let phase_changed = target.phase != state.phase;
    effects.insert(
        0,
        effect(
            "intent_refreshed",
            "intent",
            intent_id,
            Some("pending"),
            Some("pending"),
        ),
    );
    if phase_changed {
        effects.push(phase_effect(session_id, state.phase, target.phase));
    }
    let transition = TransitionSpec::command(
        "intent_refreshed",
        Some(intent_id.to_vec()),
        event.id.as_bytes().as_slice(),
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta {
            floor: floor_changed || phase_changed,
            intent: true,
            speech: false,
        },
        transition,
        Some(intent_id.to_vec()),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_intent_withdraw_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    intent_id: &[u8],
    previous_event_id: &[u8],
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "intent_not_found",
            canonical_object_id: None,
        });
    };
    if intent.author_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "only the Intent author can withdraw it".to_string(),
        ));
    }
    if intent.state != "pending" {
        return Ok(ApplyResult::Rejected {
            code: "intent_not_pending",
            canonical_object_id: Some(intent.intent_id),
        });
    }
    if intent.current_event_id != previous_event_id {
        return Ok(ApplyResult::Rejected {
            code: "stale_intent_event",
            canonical_object_id: Some(intent.current_event_id),
        });
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let active_source_offer = if state.phase == BatonPhase::Offered {
        if let Some(active_offer_id) = state.active_offer_id.as_deref() {
            load_offer_tx(tx, community_id, session_id, active_offer_id)
                .await?
                .filter(|offer| offer.source_intent_id.as_deref() == Some(intent_id))
        } else {
            None
        }
    } else {
        None
    };
    let mut effects = Vec::new();
    let mut floor_changed = false;
    let base_target = if let Some(offer) = active_source_offer {
        let failed = fail_active_offer_tx(
            tx,
            community_id,
            session_id,
            state,
            &offer,
            "source_withdrawn",
            Some(event.id.as_bytes().as_slice()),
            None,
            &config,
            now,
        )
        .await?;
        effects.extend(failed.effects);
        floor_changed = true;
        failed.target
    } else {
        StateTarget::from_state(state)
    };
    sqlx::query(
        "UPDATE meeting_speech_intents \
         SET state = 'withdrawn', terminal_event_id = $4, terminal_at = $5, \
             updated_at = $5, deferred_by_offer_id = NULL, defer_event_id = NULL, \
             defer_reason = NULL \
         WHERE community_id = $1 AND session_id = $2 AND intent_id = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(intent_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let target = if floor_changed && base_target.phase != BatonPhase::Offered {
        return_control_to_moderator_tx(tx, community_id, session_id, state, &config, now, true)
            .await?
            .into_target_with_effects(&mut effects)
    } else if floor_changed {
        base_target
    } else {
        ensure_moderator_window_tx(tx, community_id, session_id, state, &config, now).await?
    };
    let phase_changed = target.phase != state.phase;
    effects.insert(
        0,
        effect(
            "intent_withdrawn",
            "intent",
            intent_id,
            Some("pending"),
            Some("withdrawn"),
        ),
    );
    if phase_changed {
        effects.push(phase_effect(session_id, state.phase, target.phase));
    }
    let transition = TransitionSpec::command(
        "intent_withdrawn",
        Some(intent_id.to_vec()),
        event.id.as_bytes().as_slice(),
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta {
            floor: floor_changed || phase_changed,
            intent: true,
            speech: false,
        },
        transition,
        Some(intent_id.to_vec()),
        now,
    )
    .await
}

async fn has_queued_human_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_human_floor_requests \
             WHERE community_id = $1 AND session_id = $2 AND state = 'queued' \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_one(tx.as_mut())
    .await?)
}

async fn pending_self_intent_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    moderator: &[u8],
    decision_epoch: i64,
) -> Result<Option<Vec<u8>>> {
    Ok(sqlx::query_scalar(
        "SELECT intent_id FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 AND author_pubkey = $3 \
           AND state = 'pending' AND deferred_by_offer_id IS NULL \
           AND eligible_decision_epoch <= $4 \
         ORDER BY created_at, intent_id \
         LIMIT 1 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(moderator)
    .bind(decision_epoch)
    .fetch_optional(tx.as_mut())
    .await?)
}

async fn next_cohort_target_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    config: &BatonConfig,
    now: DateTime<Utc>,
) -> Result<StateTarget> {
    let next_epoch = next_decision_epoch(state.decision_epoch)?;
    let next_exists =
        current_cohort_has_candidates_tx(tx, community_id, session_id, next_epoch).await?;
    let mut target = StateTarget::from_state(state);
    target.active_decision_attempt_id = None;
    target.decision_attempt = 0;
    target.active_offer_id = None;
    target.active_grant_id = None;
    if !next_exists {
        target.phase = BatonPhase::ModeratorIdle;
        target.moderator_decision_started_at = None;
        target.moderator_decision_deadline = None;
        target.next_action_at = None;
        return Ok(target);
    }

    target.decision_epoch = next_epoch;
    let has_intent: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
               AND deferred_by_offer_id IS NULL AND eligible_decision_epoch <= $3 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(next_epoch)
    .fetch_one(tx.as_mut())
    .await?;
    if has_intent {
        target.phase = BatonPhase::ModeratorControl;
        target.moderator_decision_started_at = Some(now);
        let deadline = now + Duration::milliseconds(config.moderator_decision_ms);
        target.moderator_decision_deadline = Some(deadline);
        target.next_action_at = Some(deadline);
    } else {
        target.phase = BatonPhase::ModeratorIdle;
        target.moderator_decision_started_at = None;
        target.moderator_decision_deadline = None;
        target.next_action_at = None;
    }
    Ok(target)
}

#[allow(clippy::too_many_arguments)]
async fn insert_moderator_attempt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    attempt_id: &[u8],
    moderator_pubkey: &[u8],
    control_epoch: i64,
    decision_epoch: i64,
    attempt_number: i32,
    speech_revision: i64,
    snapshot_intent_revision: i64,
    snapshot_state_event_id: &[u8],
    candidate_snapshot_json: &Value,
    candidate_snapshot_hash: &[u8],
    replacement_of_attempt_id: Option<&[u8]>,
    started_by_event_id: &[u8],
    started_at: DateTime<Utc>,
    deadline_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_moderator_decision_attempts \
             (community_id, session_id, attempt_id, moderator_pubkey, control_epoch, \
              decision_epoch, attempt_number, speech_revision, snapshot_intent_revision, \
              snapshot_state_event_id, candidate_snapshot_json, candidate_snapshot_hash, \
              state, replacement_of_attempt_id, started_by_event_id, started_at, deadline_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, \
                 'running', $13, $14, $15, $16)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(attempt_id)
    .bind(moderator_pubkey)
    .bind(control_epoch)
    .bind(decision_epoch)
    .bind(attempt_number)
    .bind(speech_revision)
    .bind(snapshot_intent_revision)
    .bind(snapshot_state_event_id)
    .bind(candidate_snapshot_json)
    .bind(candidate_snapshot_hash)
    .bind(replacement_of_attempt_id)
    .bind(started_by_event_id)
    .bind(started_at)
    .bind(deadline_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_attempt_start_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let BatonCommand::ModeratorDecisionAttemptStart {
        expected_control_epoch,
        expected_decision_epoch,
        expected_intent_revision,
        expected_speech_revision,
        expected_state_event_id,
        replacement_of_attempt_id,
    } = command
    else {
        return Err(DbError::InvalidData(
            "invalid moderator DecisionAttempt Start command".to_string(),
        ));
    };
    if !actor.is_moderator || actor.participant_type != ParticipantType::Agent {
        return Ok(ApplyResult::Rejected {
            code: "agent_moderator_required",
            canonical_object_id: None,
        });
    }
    if !matches!(
        state.phase,
        BatonPhase::ModeratorControl | BatonPhase::ModeratorIdle
    ) || state.active_offer_id.is_some()
        || state.active_grant_id.is_some()
    {
        return Ok(ApplyResult::Rejected {
            code: "moderator_does_not_hold_control",
            canonical_object_id: state
                .active_offer_id
                .clone()
                .or_else(|| state.active_grant_id.clone()),
        });
    }
    if state.active_decision_attempt_id.is_some() {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_already_running",
            canonical_object_id: state.active_decision_attempt_id.clone(),
        });
    }
    if has_queued_human_tx(tx, community_id, session_id).await? {
        return Ok(ApplyResult::Rejected {
            code: "human_request_has_priority",
            canonical_object_id: None,
        });
    }
    if state.control_epoch != *expected_control_epoch
        || state.decision_epoch != *expected_decision_epoch
        || state.intent_revision != *expected_intent_revision
        || state.speech_revision != *expected_speech_revision
        || state.state_event_id != *expected_state_event_id
    {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }

    let config = load_config_tx(tx, community_id, session_id).await?;
    let mut decision_epoch = state.decision_epoch;
    if state.phase == BatonPhase::ModeratorIdle
        && !current_cohort_has_candidates_tx(tx, community_id, session_id, state.decision_epoch)
            .await?
    {
        decision_epoch = next_decision_epoch(state.decision_epoch)?;
    }
    let (candidate_snapshot, candidate_snapshot_hash, candidate_count) =
        build_candidate_snapshot_tx(
            tx,
            community_id,
            session_id,
            state,
            &actor.pubkey,
            decision_epoch,
        )
        .await?;
    if candidate_count == 0 {
        return Ok(ApplyResult::Rejected {
            code: "no_current_cohort_candidates",
            canonical_object_id: None,
        });
    }

    let (attempt_number, deadline_at) =
        if let Some(replacement_id) = replacement_of_attempt_id.as_deref() {
            let Some(replaced) =
                load_moderator_attempt_tx(tx, community_id, session_id, replacement_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "replacement_attempt_not_found",
                    canonical_object_id: None,
                });
            };
            if replaced.state != "abandoned"
                || replaced.moderator_pubkey != actor.pubkey
                || replaced.control_epoch != state.control_epoch
                || replaced.decision_epoch != decision_epoch
            {
                return Ok(ApplyResult::Rejected {
                    code: "replacement_attempt_not_eligible",
                    canonical_object_id: Some(replaced.attempt_id),
                });
            }
            if now >= replaced.deadline_at {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_attempt_expired",
                    canonical_object_id: Some(replaced.attempt_id),
                });
            }
            let attempt_number = replaced.attempt_number.checked_add(1).ok_or_else(|| {
                DbError::InvalidData("moderator attempt number overflow".to_string())
            })?;
            (attempt_number, replaced.deadline_at)
        } else {
            let already_started: bool = sqlx::query_scalar(
                "SELECT EXISTS ( \
                     SELECT 1 FROM meeting_moderator_decision_attempts \
                     WHERE community_id = $1 AND session_id = $2 \
                       AND control_epoch = $3 AND decision_epoch = $4 \
                 )",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(state.control_epoch)
            .bind(decision_epoch)
            .fetch_one(tx.as_mut())
            .await?;
            if already_started {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_attempt_already_started",
                    canonical_object_id: None,
                });
            }
            (
                1,
                now + Duration::milliseconds(config.moderator_decision_ms),
            )
        };
    if attempt_number > config.moderator_max_rejudgments + 1 {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_limit_reached",
            canonical_object_id: replacement_of_attempt_id.clone(),
        });
    }

    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let attempt_id = random_object_id();
    insert_moderator_attempt_tx(
        tx,
        community_id,
        session_id,
        &attempt_id,
        &actor.pubkey,
        state.control_epoch,
        decision_epoch,
        attempt_number,
        state.speech_revision,
        state.intent_revision,
        &state.state_event_id,
        &candidate_snapshot,
        &candidate_snapshot_hash,
        replacement_of_attempt_id.as_deref(),
        event.id.as_bytes().as_slice(),
        now,
        deadline_at,
    )
    .await?;

    let mut target = StateTarget::from_state(state);
    target.decision_epoch = decision_epoch;
    target.decision_attempt = attempt_number;
    target.active_decision_attempt_id = Some(attempt_id.clone());
    target.moderator_decision_started_at = Some(now);
    target.moderator_decision_deadline = Some(deadline_at);
    target.next_action_at = Some(deadline_at);
    if target.phase == BatonPhase::ModeratorIdle {
        let has_intent =
            candidate_snapshot["candidate_refs"]
                .as_array()
                .is_some_and(|candidates| {
                    candidates.iter().any(|candidate| {
                        candidate.get("source_type").and_then(Value::as_str) == Some("intent")
                    })
                });
        if has_intent {
            target.phase = BatonPhase::ModeratorControl;
        }
    }
    let mut started_effect = effect(
        "moderator_decision_attempt_started",
        "moderator_decision_attempt",
        &attempt_id,
        None,
        Some("running"),
    );
    started_effect["candidate_snapshot_hash"] =
        Value::String(hex::encode(&candidate_snapshot_hash));
    let transition = TransitionSpec::command(
        "moderator_decision_attempt_started",
        Some(attempt_id.clone()),
        event.id.as_bytes().as_slice(),
        vec![
            started_effect,
            phase_effect(session_id, state.phase, target.phase),
        ],
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta::FLOOR,
        transition,
        Some(attempt_id),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_attempt_finish_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let BatonCommand::ModeratorDecisionAttemptFinish {
        attempt_id,
        outcome,
        reason_code,
    } = command
    else {
        return Err(DbError::InvalidData(
            "invalid moderator DecisionAttempt Finish command".to_string(),
        ));
    };
    let Some(attempt) = load_moderator_attempt_tx(tx, community_id, session_id, attempt_id).await?
    else {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_not_found",
            canonical_object_id: None,
        });
    };
    if attempt.moderator_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "moderator attempt belongs to another actor".to_string(),
        ));
    }
    if attempt.state != "running" {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_already_terminal",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }

    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    sqlx::query(
        "UPDATE meeting_moderator_decision_attempts \
         SET state = $4, terminal_event_id = $5, terminal_reason = $6, terminal_at = $7 \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
           AND state = 'running'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(attempt_id)
    .bind(outcome.as_str())
    .bind(event.id.as_bytes().as_slice())
    .bind(reason_code)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut target = StateTarget::from_state(state);
    if target.active_decision_attempt_id.as_deref() == Some(attempt_id) {
        target.active_decision_attempt_id = None;
    }
    let transition = TransitionSpec::command(
        "moderator_decision_attempt_finished",
        Some(attempt_id.clone()),
        event.id.as_bytes().as_slice(),
        vec![effect(
            if *outcome == BatonDecisionAttemptFinishOutcome::Completed {
                "moderator_decision_attempt_completed"
            } else {
                "moderator_decision_attempt_discarded"
            },
            "moderator_decision_attempt",
            attempt_id,
            Some("running"),
            Some(outcome.as_str()),
        )],
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta::FLOOR,
        transition,
        Some(attempt_id.clone()),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_attempt_abandon_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    attempt_id: &[u8],
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let Some(attempt) = load_moderator_attempt_tx(tx, community_id, session_id, attempt_id).await?
    else {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_not_found",
            canonical_object_id: None,
        });
    };
    if attempt.moderator_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "moderator attempt belongs to another actor".to_string(),
        ));
    }
    if attempt.state != "running" {
        return Ok(ApplyResult::Rejected {
            code: "moderator_attempt_already_terminal",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    sqlx::query(
        "UPDATE meeting_moderator_decision_attempts \
         SET state = 'abandoned', terminal_event_id = $4, \
             terminal_reason = 'runtime_lost', terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
           AND state = 'running'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(attempt_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut target = StateTarget::from_state(state);
    if target.active_decision_attempt_id.as_deref() == Some(attempt_id) {
        target.active_decision_attempt_id = None;
    }
    let transition = TransitionSpec::command(
        "moderator_decision_attempt_abandoned",
        Some(attempt_id.to_vec()),
        event.id.as_bytes().as_slice(),
        vec![effect(
            "moderator_decision_attempt_abandoned",
            "moderator_decision_attempt",
            attempt_id,
            Some("running"),
            Some("abandoned"),
        )],
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta::FLOOR,
        transition,
        Some(attempt_id.to_vec()),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_complete_cohort_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    attempt_id: &[u8],
    expected_control_epoch: i64,
    expected_decision_epoch: i64,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if state.control_epoch != expected_control_epoch
        || state.decision_epoch != expected_decision_epoch
    {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }
    let authority = moderator_action_authority_tx(
        tx,
        community_id,
        session_id,
        state,
        actor,
        Some(attempt_id),
        now,
    )
    .await?;
    let attempt = match authority {
        ModeratorActionAuthority::Attempt(attempt) => attempt,
        ModeratorActionAuthority::Rejected {
            code,
            canonical_object_id,
        } => {
            return Ok(ApplyResult::Rejected {
                code,
                canonical_object_id,
            });
        }
        ModeratorActionAuthority::Manual => {
            return Ok(ApplyResult::Rejected {
                code: "moderator_attempt_required",
                canonical_object_id: None,
            });
        }
    };
    if current_cohort_has_candidates_tx(tx, community_id, session_id, state.decision_epoch).await? {
        return Ok(ApplyResult::Rejected {
            code: "current_cohort_not_empty",
            canonical_object_id: None,
        });
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    sqlx::query(
        "UPDATE meeting_moderator_decision_attempts \
         SET state = 'committed', terminal_event_id = $4, \
             terminal_reason = 'cohort_complete', terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
           AND state = 'running'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&attempt.attempt_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let target = next_cohort_target_tx(tx, community_id, session_id, state, &config, now).await?;
    let transition = TransitionSpec::command(
        "moderator_cohort_completed",
        Some(attempt.attempt_id.clone()),
        event.id.as_bytes().as_slice(),
        vec![
            effect(
                "moderator_decision_attempt_committed",
                "moderator_decision_attempt",
                &attempt.attempt_id,
                Some("running"),
                Some("committed"),
            ),
            control_effect(session_id, "moderator_cohort_completed"),
            phase_effect(session_id, state.phase, target.phase),
        ],
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta::FLOOR,
        transition,
        Some(attempt.attempt_id),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_retry_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    attempt_id: &[u8],
    retry_ticket_id: &[u8],
    failed_action_event_id: &[u8],
    expected_control_epoch: i64,
    expected_decision_epoch: i64,
    expected_attempt_number: i32,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if state.control_epoch != expected_control_epoch
        || state.decision_epoch != expected_decision_epoch
    {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }
    if has_queued_human_tx(tx, community_id, session_id).await? {
        return Ok(ApplyResult::Rejected {
            code: "human_request_has_priority",
            canonical_object_id: None,
        });
    }
    let authority = moderator_action_authority_tx(
        tx,
        community_id,
        session_id,
        state,
        actor,
        Some(attempt_id),
        now,
    )
    .await?;
    let attempt = match authority {
        ModeratorActionAuthority::Attempt(attempt) => attempt,
        ModeratorActionAuthority::Rejected {
            code,
            canonical_object_id,
        } => {
            return Ok(ApplyResult::Rejected {
                code,
                canonical_object_id,
            });
        }
        ModeratorActionAuthority::Manual => {
            return Ok(ApplyResult::Rejected {
                code: "moderator_attempt_required",
                canonical_object_id: None,
            });
        }
    };
    if attempt.attempt_number != expected_attempt_number {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_attempt_number",
            canonical_object_id: Some(attempt.attempt_id),
        });
    }
    let Some(ticket) = load_retry_ticket_tx(tx, community_id, session_id, retry_ticket_id).await?
    else {
        return Ok(ApplyResult::Rejected {
            code: "retry_ticket_not_found",
            canonical_object_id: None,
        });
    };
    if ticket.consumed_at.is_some() {
        return Ok(ApplyResult::Rejected {
            code: "retry_ticket_already_consumed",
            canonical_object_id: Some(ticket.retry_ticket_id),
        });
    }
    if ticket.attempt_id != attempt.attempt_id
        || ticket.failed_action_event_id != failed_action_event_id
        || ticket.control_epoch != state.control_epoch
        || ticket.decision_epoch != state.decision_epoch
    {
        return Ok(ApplyResult::Rejected {
            code: "retry_ticket_binding_mismatch",
            canonical_object_id: Some(ticket.retry_ticket_id),
        });
    }
    if now >= ticket.deadline_at || ticket.deadline_at != attempt.deadline_at {
        return Ok(ApplyResult::Rejected {
            code: "retry_ticket_expired",
            canonical_object_id: Some(ticket.retry_ticket_id),
        });
    }
    let Some(candidate) = attempt_candidate_ref(&attempt, &ticket.source_type, &ticket.source_id)
    else {
        return Ok(ApplyResult::Rejected {
            code: "retry_source_not_in_attempt_snapshot",
            canonical_object_id: Some(ticket.source_id),
        });
    };
    let conflict_still_present = match ticket.source_type.as_str() {
        "intent" => {
            let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
            if ticket.snapshot_source_event_id.as_deref() != Some(snapshot_event_id.as_slice()) {
                return Ok(ApplyResult::Rejected {
                    code: "retry_ticket_binding_mismatch",
                    canonical_object_id: Some(ticket.retry_ticket_id),
                });
            }
            match load_intent_tx(tx, community_id, session_id, &ticket.source_id).await? {
                Some(intent) => {
                    intent.state != "pending"
                        || intent.current_event_id != snapshot_event_id
                        || intent.eligible_decision_epoch > state.decision_epoch
                }
                None => true,
            }
        }
        "handoff" => {
            let snapshot_attempt = candidate
                .get("attempt_count")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok())
                .ok_or_else(|| {
                    DbError::InvalidData(
                        "Handoff candidate snapshot has no valid attempt count".to_string(),
                    )
                })?;
            if ticket.snapshot_handoff_attempt_count != Some(snapshot_attempt) {
                return Ok(ApplyResult::Rejected {
                    code: "retry_ticket_binding_mismatch",
                    canonical_object_id: Some(ticket.retry_ticket_id),
                });
            }
            match load_handoff_tx(tx, community_id, session_id, &ticket.source_id).await? {
                Some(handoff) => {
                    handoff.question_state != "open"
                        || handoff.blocked_by.is_some()
                        || handoff.attempt_count != snapshot_attempt
                        || handoff.eligible_decision_epoch > state.decision_epoch
                }
                None => true,
            }
        }
        _ => {
            return Err(DbError::InvalidData(
                "retry ticket has an unsupported source type".to_string(),
            ));
        }
    };
    if !conflict_still_present {
        return Ok(ApplyResult::Rejected {
            code: "retry_conflict_no_longer_present",
            canonical_object_id: Some(ticket.source_id),
        });
    }

    let config = load_config_tx(tx, community_id, session_id).await?;
    let next_attempt_number = attempt
        .attempt_number
        .checked_add(1)
        .ok_or_else(|| DbError::InvalidData("moderator attempt number overflow".to_string()))?;
    let (candidate_snapshot, candidate_snapshot_hash, candidate_count) =
        build_candidate_snapshot_tx(
            tx,
            community_id,
            session_id,
            state,
            &actor.pubkey,
            state.decision_epoch,
        )
        .await?;

    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    sqlx::query(
        "UPDATE meeting_moderator_retry_tickets \
         SET consumed_at = $4, consumed_by_event_id = $5 \
         WHERE community_id = $1 AND session_id = $2 AND retry_ticket_id = $3 \
           AND consumed_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(retry_ticket_id)
    .bind(now)
    .bind(event.id.as_bytes().as_slice())
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "UPDATE meeting_moderator_decision_attempts \
         SET state = 'retry_required', terminal_event_id = $4, \
             terminal_reason = 'selected_source_changed', terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
           AND state = 'running'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&attempt.attempt_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    let mut effects = vec![
        effect(
            "moderator_decision_attempt_retry_required",
            "moderator_decision_attempt",
            &attempt.attempt_id,
            Some("running"),
            Some("retry_required"),
        ),
        effect(
            "moderator_retry_ticket_consumed",
            "moderator_retry_ticket",
            retry_ticket_id,
            Some("available"),
            Some("consumed"),
        ),
    ];

    if candidate_count == 0 {
        let target =
            next_cohort_target_tx(tx, community_id, session_id, state, &config, now).await?;
        effects.push(control_effect(session_id, "moderator_cohort_completed"));
        effects.push(phase_effect(session_id, state.phase, target.phase));
        let transition = TransitionSpec::command(
            "moderator_cohort_completed_after_conflict",
            Some(attempt.attempt_id.clone()),
            event.id.as_bytes().as_slice(),
            effects,
        );
        return finish_accepted_tx(
            tx,
            community_id,
            session_id,
            event,
            relay_keys,
            state,
            target,
            RevisionDelta::FLOOR,
            transition,
            Some(attempt.attempt_id),
            now,
        )
        .await;
    }

    if next_attempt_number > config.moderator_max_rejudgments + 1 {
        let fallback =
            fallback_candidate_tx(tx, community_id, session_id, state, state.decision_epoch)
                .await?;
        let (mut target, object_id, delta) = if let Some(candidate) = fallback {
            let moderator = load_moderator_tx(tx, community_id, session_id).await?;
            let offer_id = random_object_id();
            let draft = OfferDraft {
                offer_id: offer_id.clone(),
                target_pubkey: candidate.author_pubkey.clone(),
                allocation_source: "fallback",
                turn_role: if candidate.author_pubkey == moderator {
                    "moderator_self"
                } else {
                    "participant"
                },
                allocation_event_id: None,
                selection_reason: Some("moderator retry limit reached".to_string()),
                source_intent_id: Some(candidate.intent_id.clone()),
                source_request_id: None,
                source_handoff_id: None,
                source_speech_event_id: None,
                reason_type: None,
                reason_text: None,
                basis_speech_revision: state.speech_revision,
                depth_mode: "reset",
                previous_handoff_depth: state.handoff_depth,
                requested_handoff_depth: 0,
            };
            let deadline =
                insert_offer_tx(tx, community_id, session_id, &draft, &config, now).await?;
            sqlx::query(
                "INSERT INTO meeting_baton_fallback_attempts \
                     (community_id, session_id, intent_id, current_intent_event_id, \
                      speech_revision, offer_id, attempted_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&candidate.intent_id)
            .bind(&candidate.current_event_id)
            .bind(state.speech_revision)
            .bind(&offer_id)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            effects.push(effect(
                "intent_attempted",
                "intent",
                &candidate.intent_id,
                Some("pending"),
                Some("pending"),
            ));
            effects.push(effect(
                "offer_created",
                "offer",
                &offer_id,
                None,
                Some("pending"),
            ));
            (
                StateTarget::offered(state, offer_id.clone(), deadline),
                Some(offer_id),
                RevisionDelta::FLOOR_INTENT,
            )
        } else {
            let mut target = StateTarget::from_state(state);
            target.phase = BatonPhase::ModeratorIdle;
            target.moderator_decision_started_at = None;
            target.moderator_decision_deadline = None;
            target.next_action_at = None;
            let suppressed = suppress_handoff_cohort_tx(
                tx,
                community_id,
                session_id,
                state.decision_epoch,
                &candidate_snapshot_hash,
                now + Duration::milliseconds(config.moderator_decision_ms),
            )
            .await?;
            for handoff_id in suppressed {
                effects.push(effect(
                    "handoff_moderator_retry_suppressed",
                    "handoff",
                    &handoff_id,
                    None,
                    Some("suppressed"),
                ));
            }
            (target, None, RevisionDelta::FLOOR)
        };
        target.active_decision_attempt_id = None;
        effects.push(control_effect(session_id, "moderator_retry_limit_fallback"));
        effects.push(phase_effect(session_id, state.phase, target.phase));
        let transition = TransitionSpec::command(
            "moderator_retry_limit_fallback",
            object_id.clone(),
            event.id.as_bytes().as_slice(),
            effects,
        );
        return finish_accepted_tx(
            tx,
            community_id,
            session_id,
            event,
            relay_keys,
            state,
            target,
            delta,
            transition,
            object_id,
            now,
        )
        .await;
    }

    let new_attempt_id = random_object_id();
    let deadline_at = now + Duration::milliseconds(config.moderator_decision_ms);
    insert_moderator_attempt_tx(
        tx,
        community_id,
        session_id,
        &new_attempt_id,
        &actor.pubkey,
        state.control_epoch,
        state.decision_epoch,
        next_attempt_number,
        state.speech_revision,
        state.intent_revision,
        &state.state_event_id,
        &candidate_snapshot,
        &candidate_snapshot_hash,
        Some(&attempt.attempt_id),
        event.id.as_bytes().as_slice(),
        now,
        deadline_at,
    )
    .await?;
    let mut target = StateTarget::from_state(state);
    target.decision_attempt = next_attempt_number;
    target.active_decision_attempt_id = Some(new_attempt_id.clone());
    target.moderator_decision_started_at = Some(now);
    target.moderator_decision_deadline = Some(deadline_at);
    target.next_action_at = Some(deadline_at);
    let mut started_effect = effect(
        "moderator_decision_attempt_started",
        "moderator_decision_attempt",
        &new_attempt_id,
        None,
        Some("running"),
    );
    started_effect["candidate_snapshot_hash"] =
        Value::String(hex::encode(&candidate_snapshot_hash));
    effects.push(started_effect);
    let transition = TransitionSpec::command(
        "moderator_decision_retried",
        Some(new_attempt_id.clone()),
        event.id.as_bytes().as_slice(),
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta::FLOOR,
        transition,
        Some(new_attempt_id),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_withdraw_self_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    attempt_id: &[u8],
    intent_id: &[u8],
    previous_event_id: &[u8],
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let authority = moderator_action_authority_tx(
        tx,
        community_id,
        session_id,
        state,
        actor,
        Some(attempt_id),
        now,
    )
    .await?;
    let attempt = match authority {
        ModeratorActionAuthority::Attempt(attempt) => attempt,
        ModeratorActionAuthority::Rejected {
            code,
            canonical_object_id,
        } => {
            return Ok(ApplyResult::Rejected {
                code,
                canonical_object_id,
            });
        }
        ModeratorActionAuthority::Manual => {
            return Ok(ApplyResult::Rejected {
                code: "moderator_attempt_required",
                canonical_object_id: None,
            });
        }
    };
    let Some(candidate) = attempt_candidate_ref(&attempt, "intent", intent_id) else {
        return Ok(ApplyResult::Rejected {
            code: "source_not_in_attempt_snapshot",
            canonical_object_id: Some(intent_id.to_vec()),
        });
    };
    let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
    if snapshot_event_id != previous_event_id {
        return Ok(ApplyResult::Rejected {
            code: "source_version_not_bound_to_attempt",
            canonical_object_id: Some(snapshot_event_id),
        });
    }
    let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "intent_not_found",
            canonical_object_id: None,
        });
    };
    if intent.author_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "the moderator can only withdraw its own Intent".to_string(),
        ));
    }
    if intent.state != "pending" || intent.current_event_id != previous_event_id {
        return Ok(ApplyResult::Rejected {
            code: "dependency_stale",
            canonical_object_id: Some(intent.current_event_id),
        });
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    sqlx::query(
        "UPDATE meeting_speech_intents \
         SET state = 'withdrawn', terminal_event_id = $4, terminal_at = $5, \
             updated_at = $5, deferred_by_offer_id = NULL, defer_event_id = NULL, \
             defer_reason = NULL \
         WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
           AND state = 'pending' AND current_event_id = $6",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(intent_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .bind(previous_event_id)
    .execute(tx.as_mut())
    .await?;
    let transition = TransitionSpec::command(
        "moderator_self_intent_withdrawn",
        Some(intent_id.to_vec()),
        event.id.as_bytes().as_slice(),
        vec![effect(
            "intent_withdrawn",
            "intent",
            intent_id,
            Some("pending"),
            Some("withdrawn"),
        )],
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        StateTarget::from_state(state),
        RevisionDelta::FLOOR_INTENT,
        transition,
        Some(intent_id.to_vec()),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_select_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if !actor.is_moderator {
        return Err(DbError::AccessDenied(
            "only the frozen Meeting moderator can select a speaker".to_string(),
        ));
    }
    let BatonCommand::ModeratorSelect {
        source,
        expected_control_epoch,
        expected_decision_epoch,
        expected_intent_revision,
        expected_speech_revision,
        selection_reason,
        deferrals,
        attempt_id,
        expected_source_event_id,
    } = command
    else {
        return Err(DbError::InvalidData(
            "invalid moderator Select command".to_string(),
        ));
    };
    if !matches!(
        state.phase,
        BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
    ) {
        return Ok(ApplyResult::Rejected {
            code: "moderator_does_not_hold_control",
            canonical_object_id: state
                .active_offer_id
                .clone()
                .or_else(|| state.active_grant_id.clone()),
        });
    }
    let authority = moderator_action_authority_tx(
        tx,
        community_id,
        session_id,
        state,
        actor,
        attempt_id.as_deref(),
        now,
    )
    .await?;
    let attempt = match authority {
        ModeratorActionAuthority::Manual => None,
        ModeratorActionAuthority::Attempt(attempt) => Some(attempt),
        ModeratorActionAuthority::Rejected {
            code,
            canonical_object_id,
        } => {
            return Ok(ApplyResult::Rejected {
                code,
                canonical_object_id,
            });
        }
    };
    if state.control_epoch != *expected_control_epoch
        || state.decision_epoch != *expected_decision_epoch
        || state.speech_revision != *expected_speech_revision
    {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }
    if has_queued_human_tx(tx, community_id, session_id).await? {
        return Ok(ApplyResult::Rejected {
            code: "human_request_has_priority",
            canonical_object_id: None,
        });
    }

    let moderator = actor.pubkey.as_slice();
    let self_intent = pending_self_intent_tx(
        tx,
        community_id,
        session_id,
        moderator,
        state.decision_epoch,
    )
    .await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let offer_id = random_object_id();
    let (draft, source_id, source_is_self) = match source {
        BatonSelectionSource::Intent { intent_id } => {
            let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "intent_not_found",
                    canonical_object_id: None,
                });
            };
            if intent.state != "pending" || intent.deferred_by_offer_id.is_some() {
                if let Some(attempt) = attempt.as_ref() {
                    let Some(candidate) =
                        attempt_candidate_ref(attempt, "intent", intent_id.as_slice())
                    else {
                        return Ok(ApplyResult::Rejected {
                            code: "source_not_in_attempt_snapshot",
                            canonical_object_id: Some(intent.intent_id),
                        });
                    };
                    let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(intent.current_event_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "intent",
                            intent_id,
                            Some(snapshot_event_id),
                            None,
                        ),
                    });
                }
                return Ok(ApplyResult::Rejected {
                    code: "intent_not_selectable",
                    canonical_object_id: Some(intent.intent_id),
                });
            }
            if intent.eligible_decision_epoch > state.decision_epoch {
                if let Some(attempt) = attempt.as_ref() {
                    let Some(candidate) =
                        attempt_candidate_ref(attempt, "intent", intent_id.as_slice())
                    else {
                        return Ok(ApplyResult::Rejected {
                            code: "source_not_in_attempt_snapshot",
                            canonical_object_id: Some(intent.intent_id),
                        });
                    };
                    let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(intent.intent_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "intent",
                            intent_id,
                            Some(snapshot_event_id),
                            None,
                        ),
                    });
                }
                return Ok(ApplyResult::Rejected {
                    code: "source_not_in_current_cohort",
                    canonical_object_id: Some(intent.intent_id),
                });
            }
            if let Some(attempt) = attempt.as_ref() {
                let Some(candidate) =
                    attempt_candidate_ref(attempt, "intent", intent_id.as_slice())
                else {
                    return Ok(ApplyResult::Rejected {
                        code: "source_not_in_attempt_snapshot",
                        canonical_object_id: Some(intent.intent_id),
                    });
                };
                let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
                if expected_source_event_id.as_deref() != Some(snapshot_event_id.as_slice()) {
                    return Ok(ApplyResult::Rejected {
                        code: "source_version_not_bound_to_attempt",
                        canonical_object_id: Some(snapshot_event_id),
                    });
                }
                if intent.current_event_id != snapshot_event_id {
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(intent.current_event_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "intent",
                            intent_id,
                            Some(snapshot_event_id),
                            None,
                        ),
                    });
                }
            }
            let is_self = intent.author_pubkey == actor.pubkey;
            if !is_self && self_intent.is_some() {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_self_intent_has_priority",
                    canonical_object_id: self_intent,
                });
            }
            if !is_self && !deferrals.is_empty() {
                return Err(DbError::InvalidData(
                    "only moderator-self Select can carry deferrals".to_string(),
                ));
            }
            (
                OfferDraft {
                    offer_id: offer_id.clone(),
                    target_pubkey: intent.author_pubkey,
                    allocation_source: "moderator_select",
                    turn_role: if is_self {
                        "moderator_self"
                    } else {
                        "participant"
                    },
                    allocation_event_id: Some(event.id.as_bytes().to_vec()),
                    selection_reason: selection_reason.clone(),
                    source_intent_id: Some(intent_id.clone()),
                    source_request_id: None,
                    source_handoff_id: None,
                    source_speech_event_id: None,
                    reason_type: None,
                    reason_text: None,
                    basis_speech_revision: state.speech_revision,
                    depth_mode: "reset",
                    previous_handoff_depth: state.handoff_depth,
                    requested_handoff_depth: 0,
                },
                intent_id.clone(),
                is_self,
            )
        }
        BatonSelectionSource::Handoff {
            handoff_id,
            expected_attempt_count,
        } => {
            if self_intent.is_some() {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_self_intent_has_priority",
                    canonical_object_id: self_intent,
                });
            }
            if !deferrals.is_empty() {
                return Err(DbError::InvalidData(
                    "Handoff Select cannot carry Intent deferrals".to_string(),
                ));
            }
            let Some(handoff) = load_handoff_tx(tx, community_id, session_id, handoff_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "handoff_not_found",
                    canonical_object_id: None,
                });
            };
            if handoff.question_state != "open" {
                if let Some(attempt) = attempt.as_ref() {
                    let Some(candidate) =
                        attempt_candidate_ref(attempt, "handoff", handoff_id.as_slice())
                    else {
                        return Ok(ApplyResult::Rejected {
                            code: "source_not_in_attempt_snapshot",
                            canonical_object_id: Some(handoff.handoff_id),
                        });
                    };
                    let snapshot_attempt = candidate
                        .get("attempt_count")
                        .and_then(Value::as_i64)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| {
                            DbError::InvalidData(
                                "Handoff candidate snapshot has no valid attempt count".to_string(),
                            )
                        })?;
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(handoff.handoff_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "handoff",
                            handoff_id,
                            None,
                            Some(snapshot_attempt),
                        ),
                    });
                }
                return Ok(ApplyResult::Rejected {
                    code: "handoff_not_open",
                    canonical_object_id: Some(handoff.handoff_id),
                });
            }
            if handoff.blocked_by.is_some() {
                if let Some(attempt) = attempt.as_ref() {
                    let Some(candidate) =
                        attempt_candidate_ref(attempt, "handoff", handoff_id.as_slice())
                    else {
                        return Ok(ApplyResult::Rejected {
                            code: "source_not_in_attempt_snapshot",
                            canonical_object_id: Some(handoff.handoff_id),
                        });
                    };
                    let snapshot_attempt = candidate
                        .get("attempt_count")
                        .and_then(Value::as_i64)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| {
                            DbError::InvalidData(
                                "Handoff candidate snapshot has no valid attempt count".to_string(),
                            )
                        })?;
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(handoff.handoff_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "handoff",
                            handoff_id,
                            None,
                            Some(snapshot_attempt),
                        ),
                    });
                }
                return Ok(ApplyResult::Rejected {
                    code: "handoff_blocked",
                    canonical_object_id: Some(handoff.handoff_id),
                });
            }
            if handoff.eligible_decision_epoch > state.decision_epoch {
                if let Some(attempt) = attempt.as_ref() {
                    let Some(candidate) =
                        attempt_candidate_ref(attempt, "handoff", handoff_id.as_slice())
                    else {
                        return Ok(ApplyResult::Rejected {
                            code: "source_not_in_attempt_snapshot",
                            canonical_object_id: Some(handoff.handoff_id),
                        });
                    };
                    let snapshot_attempt = candidate
                        .get("attempt_count")
                        .and_then(Value::as_i64)
                        .and_then(|value| i32::try_from(value).ok())
                        .ok_or_else(|| {
                            DbError::InvalidData(
                                "Handoff candidate snapshot has no valid attempt count".to_string(),
                            )
                        })?;
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: Some(handoff.handoff_id),
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "handoff",
                            handoff_id,
                            None,
                            Some(snapshot_attempt),
                        ),
                    });
                }
                return Ok(ApplyResult::Rejected {
                    code: "source_not_in_current_cohort",
                    canonical_object_id: Some(handoff.handoff_id),
                });
            }
            if let Some(attempt) = attempt.as_ref() {
                let Some(candidate) =
                    attempt_candidate_ref(attempt, "handoff", handoff_id.as_slice())
                else {
                    return Ok(ApplyResult::Rejected {
                        code: "source_not_in_attempt_snapshot",
                        canonical_object_id: Some(handoff.handoff_id),
                    });
                };
                let snapshot_attempt = candidate
                    .get("attempt_count")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DbError::InvalidData(
                            "Handoff candidate snapshot has no valid attempt count".to_string(),
                        )
                    })?;
                if *expected_attempt_count != snapshot_attempt {
                    return Ok(ApplyResult::Rejected {
                        code: "source_version_not_bound_to_attempt",
                        canonical_object_id: Some(handoff.handoff_id),
                    });
                }
                if handoff.attempt_count != snapshot_attempt {
                    return Ok(ApplyResult::RejectedWithRetry {
                        code: "selected_source_changed",
                        canonical_object_id: handoff.last_offer_id,
                        retry_ticket: selected_source_retry_ticket(
                            attempt,
                            event.id.as_bytes().as_slice(),
                            "handoff",
                            handoff_id,
                            None,
                            Some(snapshot_attempt),
                        ),
                    });
                }
            } else if handoff.attempt_count != *expected_attempt_count {
                return Ok(ApplyResult::Rejected {
                    code: "stale_handoff_attempt",
                    canonical_object_id: handoff.last_offer_id,
                });
            }
            (
                OfferDraft {
                    offer_id: offer_id.clone(),
                    target_pubkey: handoff.to_pubkey,
                    allocation_source: "moderator_select",
                    turn_role: "participant",
                    allocation_event_id: Some(event.id.as_bytes().to_vec()),
                    selection_reason: selection_reason.clone(),
                    source_intent_id: None,
                    source_request_id: None,
                    source_handoff_id: Some(handoff_id.clone()),
                    source_speech_event_id: Some(handoff.source_speech_event_id),
                    reason_type: Some(handoff.reason_type),
                    reason_text: Some(handoff.reason_text),
                    basis_speech_revision: state.speech_revision,
                    depth_mode: "reset",
                    previous_handoff_depth: state.handoff_depth,
                    requested_handoff_depth: 0,
                },
                handoff_id.clone(),
                false,
            )
        }
    };

    if state.intent_revision != *expected_intent_revision {
        return Ok(ApplyResult::Rejected {
            code: "stale_moderator_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }

    if source_is_self {
        let rows = sqlx::query(
            "SELECT intent_id, current_event_id \
             FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
               AND author_pubkey <> $3 AND deferred_by_offer_id IS NULL \
               AND eligible_decision_epoch <= $4 \
             ORDER BY intent_id \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&actor.pubkey)
        .bind(state.decision_epoch)
        .fetch_all(tx.as_mut())
        .await?;
        let required: Vec<(Vec<u8>, Vec<u8>)> = rows
            .into_iter()
            .map(|row| Ok((row.try_get("intent_id")?, row.try_get("current_event_id")?)))
            .collect::<Result<_>>()?;
        if state.consecutive_moderator_speeches >= 1
            && required.iter().any(|(id, current)| {
                !deferrals
                    .iter()
                    .any(|defer| defer.intent_id == *id && defer.previous_event_id == *current)
            })
        {
            return Ok(ApplyResult::Rejected {
                code: "consecutive_moderator_speech_requires_deferrals",
                canonical_object_id: required.first().map(|(id, _)| id.clone()),
            });
        }
        for deferral in deferrals {
            if !required.iter().any(|(id, current)| {
                *id == deferral.intent_id && *current == deferral.previous_event_id
            }) {
                return Ok(ApplyResult::Rejected {
                    code: "stale_intent_deferral",
                    canonical_object_id: Some(deferral.intent_id.clone()),
                });
            }
        }
    } else if !deferrals.is_empty() {
        return Err(DbError::InvalidData(
            "only moderator-self Select can carry deferrals".to_string(),
        ));
    }

    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let deadline = insert_offer_tx(tx, community_id, session_id, &draft, &config, now).await?;
    for deferral in deferrals {
        let updated = sqlx::query(
            "UPDATE meeting_speech_intents \
             SET deferred_by_offer_id = $4, defer_event_id = $5, defer_reason = $6, \
                 updated_at = $7 \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
               AND state = 'pending' AND current_event_id = $8",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&deferral.intent_id)
        .bind(&offer_id)
        .bind(event.id.as_bytes().as_slice())
        .bind(&deferral.reason)
        .bind(now)
        .bind(&deferral.previous_event_id)
        .execute(tx.as_mut())
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(ApplyResult::Rejected {
                code: "stale_intent_deferral",
                canonical_object_id: Some(deferral.intent_id.clone()),
            });
        }
    }
    let mut target = StateTarget::offered(state, offer_id.clone(), deadline);
    let mut effects = Vec::new();
    if let Some(attempt) = attempt.as_ref() {
        let updated = sqlx::query(
            "UPDATE meeting_moderator_decision_attempts \
             SET state = 'committed', terminal_event_id = $4, \
                 terminal_reason = 'primary_action_committed', terminal_at = $5 \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3 \
               AND state = 'running'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&attempt.attempt_id)
        .bind(event.id.as_bytes().as_slice())
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(ApplyResult::Rejected {
                code: "moderator_attempt_not_active",
                canonical_object_id: Some(attempt.attempt_id.clone()),
            });
        }
        target.active_decision_attempt_id = None;
        effects.push(effect(
            "moderator_decision_attempt_committed",
            "moderator_decision_attempt",
            &attempt.attempt_id,
            Some("running"),
            Some("committed"),
        ));
    }
    if matches!(source, BatonSelectionSource::Intent { .. }) {
        effects.push(effect(
            "intent_attempted",
            "intent",
            &source_id,
            Some("pending"),
            Some("pending"),
        ));
    } else {
        effects.push(effect(
            "handoff_attempted",
            "handoff",
            &source_id,
            Some("open"),
            Some("open"),
        ));
    }
    for deferral in deferrals {
        effects.push(effect(
            "intent_deferred",
            "intent",
            &deferral.intent_id,
            Some("pending"),
            Some("pending"),
        ));
    }
    if matches!(source, BatonSelectionSource::Intent { .. }) {
        sort_effects_by_object_id(&mut effects);
    }
    effects.push(effect(
        "offer_created",
        "offer",
        &offer_id,
        None,
        Some("pending"),
    ));
    effects.push(phase_effect(session_id, state.phase, target.phase));
    let transition = TransitionSpec::command(
        "offer_created",
        Some(offer_id.clone()),
        event.id.as_bytes().as_slice(),
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        RevisionDelta {
            floor: true,
            intent: matches!(source, BatonSelectionSource::Intent { .. }) || !deferrals.is_empty(),
            speech: false,
        },
        transition,
        Some(offer_id),
        now,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn apply_moderator_other_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if !actor.is_moderator {
        return Err(DbError::AccessDenied(
            "only the frozen Meeting moderator can issue this command".to_string(),
        ));
    }
    match command {
        BatonCommand::ModeratorReject {
            intent_id,
            previous_event_id,
            author_pubkey,
            reason_code,
            reason_text,
            attempt_id,
        } => {
            let authority = moderator_action_authority_tx(
                tx,
                community_id,
                session_id,
                state,
                actor,
                attempt_id.as_deref(),
                now,
            )
            .await?;
            let attempt = match authority {
                ModeratorActionAuthority::Manual => None,
                ModeratorActionAuthority::Attempt(attempt) => Some(attempt),
                ModeratorActionAuthority::Rejected {
                    code,
                    canonical_object_id,
                } => {
                    return Ok(ApplyResult::Rejected {
                        code,
                        canonical_object_id,
                    });
                }
            };
            if attempt.is_some()
                && !matches!(
                    state.phase,
                    BatonPhase::ModeratorControl | BatonPhase::ModeratorIdle
                )
            {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_does_not_hold_control",
                    canonical_object_id: state
                        .active_offer_id
                        .clone()
                        .or_else(|| state.active_grant_id.clone()),
                });
            }
            let Some(intent) = load_intent_tx(tx, community_id, session_id, intent_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "intent_not_found",
                    canonical_object_id: None,
                });
            };
            if intent.state != "pending" {
                return Ok(ApplyResult::Rejected {
                    code: "intent_not_pending",
                    canonical_object_id: Some(intent.intent_id),
                });
            }
            if intent.current_event_id != *previous_event_id {
                return Ok(ApplyResult::Rejected {
                    code: if attempt.is_some() {
                        "dependency_stale"
                    } else {
                        "stale_intent_event"
                    },
                    canonical_object_id: Some(intent.current_event_id),
                });
            }
            if let Some(attempt) = attempt.as_ref() {
                let Some(candidate) =
                    attempt_candidate_ref(attempt, "intent", intent_id.as_slice())
                else {
                    return Ok(ApplyResult::Rejected {
                        code: "source_not_in_attempt_snapshot",
                        canonical_object_id: Some(intent.intent_id),
                    });
                };
                let snapshot_event_id = candidate_hex(candidate, "current_event_id")?;
                if snapshot_event_id != *previous_event_id
                    || intent.eligible_decision_epoch > state.decision_epoch
                {
                    return Ok(ApplyResult::Rejected {
                        code: "dependency_stale",
                        canonical_object_id: Some(intent.current_event_id),
                    });
                }
            }
            if intent.author_pubkey != *author_pubkey {
                return Err(DbError::InvalidData(
                    "rejection notification pubkey does not match the Intent author".to_string(),
                ));
            }
            persist_command_event_tx(tx, community_id, session_id, event, now).await?;
            let config = load_config_tx(tx, community_id, session_id).await?;
            let active_source_offer = if state.phase == BatonPhase::Offered {
                if let Some(offer_id) = state.active_offer_id.as_deref() {
                    load_offer_tx(tx, community_id, session_id, offer_id)
                        .await?
                        .filter(|offer| offer.source_intent_id.as_deref() == Some(intent_id))
                } else {
                    None
                }
            } else {
                None
            };
            let mut effects = Vec::new();
            let mut floor_changed = false;
            let base_target = if let Some(offer) = active_source_offer {
                let failed = fail_active_offer_tx(
                    tx,
                    community_id,
                    session_id,
                    state,
                    &offer,
                    "source_changed",
                    Some(event.id.as_bytes().as_slice()),
                    None,
                    &config,
                    now,
                )
                .await?;
                effects.extend(failed.effects);
                floor_changed = true;
                failed.target
            } else {
                StateTarget::from_state(state)
            };
            sqlx::query(
                "UPDATE meeting_speech_intents \
                 SET state = 'rejected', reason_code = $4, reason_text = $5, \
                     terminal_event_id = $6, terminal_at = $7, updated_at = $7, \
                     deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL \
                 WHERE community_id = $1 AND session_id = $2 AND intent_id = $3",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(intent_id)
            .bind(reason_code)
            .bind(reason_text)
            .bind(event.id.as_bytes().as_slice())
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let target = if floor_changed && base_target.phase != BatonPhase::Offered {
                return_control_to_moderator_tx(
                    tx,
                    community_id,
                    session_id,
                    state,
                    &config,
                    now,
                    true,
                )
                .await?
                .into_target_with_effects(&mut effects)
            } else if floor_changed {
                base_target
            } else {
                ensure_moderator_window_tx(tx, community_id, session_id, state, &config, now)
                    .await?
            };
            let phase_changed = target.phase != state.phase;
            effects.insert(
                0,
                effect(
                    "intent_rejected",
                    "intent",
                    intent_id,
                    Some("pending"),
                    Some("rejected"),
                ),
            );
            if phase_changed {
                effects.push(phase_effect(session_id, state.phase, target.phase));
            }
            let transition = TransitionSpec::command(
                "intent_rejected",
                Some(intent_id.clone()),
                event.id.as_bytes().as_slice(),
                effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                target,
                RevisionDelta {
                    floor: floor_changed || phase_changed,
                    intent: true,
                    speech: false,
                },
                transition,
                Some(intent_id.clone()),
                now,
            )
            .await
        }
        BatonCommand::ModeratorDismissHandoff {
            handoff_id,
            expected_speech_revision,
            expected_attempt_count,
            reason_code,
            reason_text,
            attempt_id,
        } => {
            let authority = moderator_action_authority_tx(
                tx,
                community_id,
                session_id,
                state,
                actor,
                attempt_id.as_deref(),
                now,
            )
            .await?;
            let attempt = match authority {
                ModeratorActionAuthority::Manual => None,
                ModeratorActionAuthority::Attempt(attempt) => Some(attempt),
                ModeratorActionAuthority::Rejected {
                    code,
                    canonical_object_id,
                } => {
                    return Ok(ApplyResult::Rejected {
                        code,
                        canonical_object_id,
                    });
                }
            };
            if attempt.is_some()
                && !matches!(
                    state.phase,
                    BatonPhase::ModeratorControl | BatonPhase::ModeratorIdle
                )
            {
                return Ok(ApplyResult::Rejected {
                    code: "moderator_does_not_hold_control",
                    canonical_object_id: state
                        .active_offer_id
                        .clone()
                        .or_else(|| state.active_grant_id.clone()),
                });
            }
            let Some(handoff) = load_handoff_tx(tx, community_id, session_id, handoff_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "handoff_not_found",
                    canonical_object_id: None,
                });
            };
            if handoff.question_state != "open" {
                return Ok(ApplyResult::Rejected {
                    code: "handoff_not_open",
                    canonical_object_id: Some(handoff.handoff_id),
                });
            }
            if state.speech_revision != *expected_speech_revision
                || handoff.attempt_count != *expected_attempt_count
            {
                return Ok(ApplyResult::Rejected {
                    code: if attempt.is_some() {
                        "dependency_stale"
                    } else {
                        "stale_handoff_revision"
                    },
                    canonical_object_id: handoff.last_offer_id.or(handoff.last_grant_id),
                });
            }
            if let Some(attempt) = attempt.as_ref() {
                let Some(candidate) =
                    attempt_candidate_ref(attempt, "handoff", handoff_id.as_slice())
                else {
                    return Ok(ApplyResult::Rejected {
                        code: "source_not_in_attempt_snapshot",
                        canonical_object_id: Some(handoff.handoff_id),
                    });
                };
                let snapshot_attempt = candidate
                    .get("attempt_count")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok())
                    .ok_or_else(|| {
                        DbError::InvalidData(
                            "Handoff candidate snapshot has no valid attempt count".to_string(),
                        )
                    })?;
                if snapshot_attempt != *expected_attempt_count
                    || handoff.blocked_by.is_some()
                    || handoff.eligible_decision_epoch > state.decision_epoch
                {
                    return Ok(ApplyResult::Rejected {
                        code: "dependency_stale",
                        canonical_object_id: Some(handoff.handoff_id),
                    });
                }
            }
            let active_reference = if let Some(offer_id) = state.active_offer_id.as_deref() {
                load_offer_tx(tx, community_id, session_id, offer_id)
                    .await?
                    .is_some_and(|offer| offer.source_handoff_id.as_deref() == Some(handoff_id))
            } else if let Some(grant_id) = state.active_grant_id.as_deref() {
                load_grant_tx(tx, community_id, session_id, grant_id)
                    .await?
                    .is_some_and(|grant| grant.source_handoff_id.as_deref() == Some(handoff_id))
            } else {
                false
            };
            if active_reference {
                return Ok(ApplyResult::Rejected {
                    code: "handoff_attempt_active",
                    canonical_object_id: state
                        .active_offer_id
                        .clone()
                        .or_else(|| state.active_grant_id.clone()),
                });
            }
            persist_command_event_tx(tx, community_id, session_id, event, now).await?;
            sqlx::query(
                "UPDATE meeting_directed_handoffs \
                 SET question_state = 'dismissed', blocked_by = NULL, dismiss_event_id = $4, \
                     dismiss_reason_code = $5, dismiss_reason_text = $6, \
                     dismissed_at = $7, terminal_at = $7 \
                 WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(handoff_id)
            .bind(event.id.as_bytes().as_slice())
            .bind(reason_code)
            .bind(reason_text)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let transition = TransitionSpec::command(
                "handoff_dismissed",
                Some(handoff_id.clone()),
                event.id.as_bytes().as_slice(),
                vec![effect(
                    "handoff_dismissed",
                    "handoff",
                    handoff_id,
                    Some("open"),
                    Some("dismissed"),
                )],
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                StateTarget::from_state(state),
                RevisionDelta::FLOOR,
                transition,
                Some(handoff_id.clone()),
                now,
            )
            .await
        }
        BatonCommand::ModeratorRecall {
            control_epoch,
            reason,
        } => {
            if *control_epoch != state.control_epoch {
                return Ok(ApplyResult::Rejected {
                    code: if *control_epoch < state.control_epoch {
                        "control_already_returned"
                    } else {
                        "stale_control_epoch"
                    },
                    canonical_object_id: Some(state.state_event_id.clone()),
                });
            }
            if matches!(
                state.phase,
                BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
            ) {
                return Ok(ApplyResult::Rejected {
                    code: "control_already_returned",
                    canonical_object_id: Some(state.state_event_id.clone()),
                });
            }
            persist_command_event_tx(tx, community_id, session_id, event, now).await?;
            let config = load_config_tx(tx, community_id, session_id).await?;
            let event_id = event.id.as_bytes().to_vec();
            if state.phase == BatonPhase::Offered {
                let offer_id = state.active_offer_id.as_deref().ok_or_else(|| {
                    DbError::InvalidData("offered state has no active Offer".to_string())
                })?;
                let offer = load_offer_tx(tx, community_id, session_id, offer_id)
                    .await?
                    .ok_or_else(|| DbError::InvalidData("active Offer is missing".to_string()))?;
                if offer.allocation_source == "human_request" {
                    let mut target = StateTarget::from_state(state);
                    target.forced_return_to_moderator = true;
                    target.recall_event_id = Some(event_id.clone());
                    let transition = TransitionSpec::command(
                        "recall_latched",
                        Some(event_id.clone()),
                        &event_id,
                        vec![effect(
                            "recall_latched",
                            "recall",
                            &event_id,
                            None,
                            Some("latched"),
                        )],
                    );
                    finish_accepted_tx(
                        tx,
                        community_id,
                        session_id,
                        event,
                        relay_keys,
                        state,
                        target,
                        RevisionDelta::FLOOR,
                        transition,
                        Some(event_id),
                        now,
                    )
                    .await
                } else {
                    let failed = fail_active_offer_tx(
                        tx,
                        community_id,
                        session_id,
                        state,
                        &offer,
                        "recalled",
                        Some(event.id.as_bytes().as_slice()),
                        reason.as_deref(),
                        &config,
                        now,
                    )
                    .await?;
                    let mut effects = failed.effects;
                    if state.phase != failed.target.phase {
                        effects.push(phase_effect(session_id, state.phase, failed.target.phase));
                    }
                    let transition = TransitionSpec::command(
                        "offer_recalled",
                        Some(offer.offer_id.clone()),
                        event.id.as_bytes().as_slice(),
                        effects,
                    );
                    finish_accepted_tx(
                        tx,
                        community_id,
                        session_id,
                        event,
                        relay_keys,
                        state,
                        failed.target,
                        RevisionDelta {
                            floor: true,
                            intent: failed.intent_changed,
                            speech: false,
                        },
                        transition,
                        Some(offer.offer_id),
                        now,
                    )
                    .await
                }
            } else {
                let mut target = StateTarget::from_state(state);
                target.forced_return_to_moderator = true;
                target.recall_event_id = Some(event_id.clone());
                let transition = TransitionSpec::command(
                    "recall_latched",
                    Some(event_id.clone()),
                    &event_id,
                    vec![effect(
                        "recall_latched",
                        "recall",
                        &event_id,
                        None,
                        Some("latched"),
                    )],
                );
                finish_accepted_tx(
                    tx,
                    community_id,
                    session_id,
                    event,
                    relay_keys,
                    state,
                    target,
                    RevisionDelta::FLOOR,
                    transition,
                    Some(event_id),
                    now,
                )
                .await
            }
        }
        _ => Err(DbError::InvalidData(
            "invalid moderator command dispatch".to_string(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_human_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    if actor.participant_type != ParticipantType::Human || actor.is_moderator {
        return Err(DbError::AccessDenied(
            "only a non-moderator frozen Human participant can request the floor".to_string(),
        ));
    }
    let config = load_config_tx(tx, community_id, session_id).await?;
    match command {
        BatonCommand::HumanRequest => {
            let existing: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT request_id FROM meeting_human_floor_requests \
                 WHERE community_id = $1 AND session_id = $2 AND requester_pubkey = $3 \
                   AND state IN ('queued', 'offered') \
                 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&actor.pubkey)
            .fetch_optional(tx.as_mut())
            .await?;
            if let Some(existing) = existing {
                return Ok(ApplyResult::Rejected {
                    code: "active_human_request_exists",
                    canonical_object_id: Some(existing),
                });
            }
            persist_command_event_tx(tx, community_id, session_id, event, now).await?;
            let request_id = event.id.as_bytes().to_vec();
            sqlx::query(
                "INSERT INTO meeting_human_floor_requests \
                     (community_id, session_id, request_id, requester_pubkey, state, \
                      request_event_id, created_at) \
                 VALUES ($1, $2, $3, $4, 'queued', $3, $5)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&request_id)
            .bind(&actor.pubkey)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let request = load_request_tx(tx, community_id, session_id, &request_id)
                .await?
                .ok_or_else(|| DbError::InvalidData("new Human Request is missing".to_string()))?;
            let mut effects = vec![effect(
                "human_requested",
                "human_request",
                &request_id,
                None,
                Some("queued"),
            )];
            let mut intent_changed = true;
            let target = match state.phase {
                BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl => {
                    let (target, offer_id) = offer_human_request_tx(
                        tx,
                        community_id,
                        session_id,
                        state,
                        &request,
                        &config,
                        now,
                    )
                    .await?;
                    effects.push(effect(
                        "human_offered",
                        "human_request",
                        &request_id,
                        Some("queued"),
                        Some("offered"),
                    ));
                    effects.push(effect(
                        "offer_created",
                        "offer",
                        &offer_id,
                        None,
                        Some("pending"),
                    ));
                    target
                }
                BatonPhase::Offered => {
                    let offer_id = state.active_offer_id.as_deref().ok_or_else(|| {
                        DbError::InvalidData("offered state has no active Offer".to_string())
                    })?;
                    let offer = load_offer_tx(tx, community_id, session_id, offer_id)
                        .await?
                        .ok_or_else(|| {
                            DbError::InvalidData("active Offer is missing".to_string())
                        })?;
                    if offer.allocation_source == "human_request" {
                        StateTarget::from_state(state)
                    } else {
                        let failed = fail_active_offer_tx(
                            tx,
                            community_id,
                            session_id,
                            state,
                            &offer,
                            "preempted",
                            Some(event.id.as_bytes().as_slice()),
                            None,
                            &config,
                            now,
                        )
                        .await?;
                        intent_changed |= failed.intent_changed;
                        effects.extend(failed.effects);
                        failed.target
                    }
                }
                BatonPhase::Granted => StateTarget::from_state(state),
                BatonPhase::Ended => {
                    return Ok(ApplyResult::Rejected {
                        code: "meeting_ended",
                        canonical_object_id: None,
                    });
                }
            };
            if target.phase != state.phase
                && !effects
                    .iter()
                    .any(|value| value.get("type") == Some(&Value::String("phase_changed".into())))
            {
                effects.push(phase_effect(session_id, state.phase, target.phase));
            }
            let floor_changed =
                target.phase != state.phase || target.active_offer_id != state.active_offer_id;
            let transition = TransitionSpec::command(
                "human_requested",
                Some(request_id.clone()),
                event.id.as_bytes().as_slice(),
                effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                target,
                RevisionDelta {
                    floor: floor_changed,
                    intent: intent_changed,
                    speech: false,
                },
                transition,
                Some(request_id),
                now,
            )
            .await
        }
        BatonCommand::HumanWithdraw { request_id } => {
            let Some(request) = load_request_tx(tx, community_id, session_id, request_id).await?
            else {
                return Ok(ApplyResult::Rejected {
                    code: "human_request_not_found",
                    canonical_object_id: None,
                });
            };
            if request.requester_pubkey != actor.pubkey {
                return Err(DbError::AccessDenied(
                    "only the Human Request author can withdraw it".to_string(),
                ));
            }
            if !matches!(request.state.as_str(), "queued" | "offered") {
                return Ok(ApplyResult::Rejected {
                    code: "human_request_consumed",
                    canonical_object_id: Some(request.request_id),
                });
            }
            persist_command_event_tx(tx, community_id, session_id, event, now).await?;
            let mut effects = Vec::new();
            let (target, floor_changed) = if request.state == "offered"
                && state.active_offer_id.as_deref() == request.offer_id.as_deref()
            {
                let offer_id = request.offer_id.as_deref().ok_or_else(|| {
                    DbError::InvalidData("offered Human Request has no Offer".to_string())
                })?;
                let offer = load_offer_tx(tx, community_id, session_id, offer_id)
                    .await?
                    .ok_or_else(|| DbError::InvalidData("active Offer is missing".to_string()))?;
                let failed = fail_active_offer_tx(
                    tx,
                    community_id,
                    session_id,
                    state,
                    &offer,
                    "source_withdrawn",
                    Some(event.id.as_bytes().as_slice()),
                    None,
                    &config,
                    now,
                )
                .await?;
                effects.extend(failed.effects);
                (failed.target, true)
            } else if request.state == "queued" {
                sqlx::query(
                    "UPDATE meeting_human_floor_requests \
                     SET state = 'withdrawn', terminal_event_id = $4, terminal_at = $5 \
                     WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
                       AND state = 'queued'",
                )
                .bind(community_id.as_uuid())
                .bind(session_id)
                .bind(request_id)
                .bind(event.id.as_bytes().as_slice())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                (StateTarget::from_state(state), false)
            } else {
                return Ok(ApplyResult::Rejected {
                    code: "human_request_not_active",
                    canonical_object_id: request.offer_id,
                });
            };
            effects.retain(|value| {
                value.get("type").and_then(Value::as_str) != Some("human_withdrawn")
                    || value.get("object_type").and_then(Value::as_str) != Some("human_request")
            });
            effects.insert(
                0,
                effect(
                    "human_withdrawn",
                    "human_request",
                    request_id,
                    Some(&request.state),
                    Some("withdrawn"),
                ),
            );
            if state.phase != target.phase {
                effects.push(phase_effect(session_id, state.phase, target.phase));
            }
            let transition = TransitionSpec::command(
                "human_withdrawn",
                Some(request_id.clone()),
                event.id.as_bytes().as_slice(),
                effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                target,
                RevisionDelta {
                    floor: floor_changed,
                    intent: true,
                    speech: false,
                },
                transition,
                Some(request_id.clone()),
                now,
            )
            .await
        }
        _ => Err(DbError::InvalidData(
            "invalid Human command dispatch".to_string(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_offer_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let (offer_id, decline_reason) = match command {
        BatonCommand::OfferAck { offer_id } => (offer_id, None),
        BatonCommand::OfferDecline { offer_id, reason } => (offer_id, reason.as_deref()),
        _ => {
            return Err(DbError::InvalidData(
                "invalid Offer command dispatch".to_string(),
            ));
        }
    };
    let Some(offer) = load_offer_tx(tx, community_id, session_id, offer_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "offer_not_found",
            canonical_object_id: None,
        });
    };
    if offer.target_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "only the current Offer target can respond".to_string(),
        ));
    }
    if state.phase != BatonPhase::Offered || state.active_offer_id.as_deref() != Some(offer_id) {
        return Ok(ApplyResult::Rejected {
            code: "offer_not_active",
            canonical_object_id: Some(offer.offer_id),
        });
    }
    if offer.state != "pending" {
        return Ok(ApplyResult::Rejected {
            code: "offer_already_resolved",
            canonical_object_id: Some(offer.offer_id),
        });
    }
    let config = load_config_tx(tx, community_id, session_id).await?;
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    match command {
        BatonCommand::OfferDecline { .. } => {
            let failed = fail_active_offer_tx(
                tx,
                community_id,
                session_id,
                state,
                &offer,
                "declined",
                Some(event.id.as_bytes().as_slice()),
                decline_reason,
                &config,
                now,
            )
            .await?;
            let mut effects = failed.effects;
            if state.phase != failed.target.phase {
                effects.push(phase_effect(session_id, state.phase, failed.target.phase));
            }
            let transition = TransitionSpec::command(
                "offer_declined",
                Some(offer.offer_id.clone()),
                event.id.as_bytes().as_slice(),
                effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                failed.target,
                RevisionDelta {
                    floor: true,
                    intent: failed.intent_changed,
                    speech: false,
                },
                transition,
                Some(offer.offer_id),
                now,
            )
            .await
        }
        BatonCommand::OfferAck { .. } => {
            sqlx::query(
                "UPDATE meeting_baton_offers \
                 SET state = 'acked', response_event_id = $4, resolved_at = $5 \
                 WHERE community_id = $1 AND session_id = $2 AND offer_id = $3 \
                   AND state = 'pending'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&offer.offer_id)
            .bind(event.id.as_bytes().as_slice())
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let grant_id = random_object_id();
            let hard_deadline = now + Duration::milliseconds(config.grant_hard_deadline_ms);
            let soft_deadline =
                (now + Duration::milliseconds(config.grant_soft_lease_ms)).min(hard_deadline);
            let grant_depth = match offer.depth_mode.as_str() {
                "reset" => 0,
                "preserve" => offer.previous_handoff_depth,
                "increment_provisional" => offer.requested_handoff_depth,
                other => {
                    return Err(DbError::InvalidData(format!(
                        "unknown Offer depth mode: {other}"
                    )));
                }
            };
            sqlx::query(
                "INSERT INTO meeting_baton_grants \
                     (community_id, session_id, grant_id, holder_pubkey, allocation_source, \
                      turn_role, source_offer_id, allocation_event_id, selection_reason, \
                      source_intent_id, source_request_id, source_handoff_id, \
                      source_speech_event_id, basis_speech_revision, depth_mode, \
                      previous_handoff_depth, handoff_depth, soft_lease_expires_at, \
                      hard_deadline, state, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
                         $14, $15, $16, $17, $18, $19, 'active', $20)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(&grant_id)
            .bind(&offer.target_pubkey)
            .bind(&offer.allocation_source)
            .bind(&offer.turn_role)
            .bind(&offer.offer_id)
            .bind(&offer.allocation_event_id)
            .bind(&offer.selection_reason)
            .bind(&offer.source_intent_id)
            .bind(&offer.source_request_id)
            .bind(&offer.source_handoff_id)
            .bind(&offer.source_speech_event_id)
            .bind(offer.basis_speech_revision)
            .bind(&offer.depth_mode)
            .bind(offer.previous_handoff_depth)
            .bind(grant_depth)
            .bind(soft_deadline)
            .bind(hard_deadline)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let mut intent_changed = false;
            let mut effects = vec![effect(
                "offer_acked",
                "offer",
                &offer.offer_id,
                Some("pending"),
                Some("acked"),
            )];
            if let Some(intent_id) = offer.source_intent_id.as_deref() {
                sqlx::query(
                    "UPDATE meeting_speech_intents \
                     SET state = 'selected', selected_grant_id = $4, \
                         last_attempt_outcome = 'granted', updated_at = $5 \
                     WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
                       AND state = 'pending'",
                )
                .bind(community_id.as_uuid())
                .bind(session_id)
                .bind(intent_id)
                .bind(&grant_id)
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                intent_changed = true;
                effects.push(effect(
                    "intent_selected",
                    "intent",
                    intent_id,
                    Some("pending"),
                    Some("selected"),
                ));
            }
            if let Some(request_id) = offer.source_request_id.as_deref() {
                sqlx::query(
                    "UPDATE meeting_human_floor_requests \
                     SET state = 'granted', grant_id = $4, terminal_event_id = $5, \
                         terminal_at = $6 \
                     WHERE community_id = $1 AND session_id = $2 AND request_id = $3 \
                       AND state = 'offered'",
                )
                .bind(community_id.as_uuid())
                .bind(session_id)
                .bind(request_id)
                .bind(&grant_id)
                .bind(event.id.as_bytes().as_slice())
                .bind(now)
                .execute(tx.as_mut())
                .await?;
                intent_changed = true;
                effects.push(effect(
                    "human_granted",
                    "human_request",
                    request_id,
                    Some("offered"),
                    Some("granted"),
                ));
            }
            if let Some(handoff_id) = offer.source_handoff_id.as_deref() {
                sqlx::query(
                    "UPDATE meeting_directed_handoffs \
                     SET last_grant_id = $4, last_attempt_outcome = 'granted' \
                     WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
                       AND question_state = 'open'",
                )
                .bind(community_id.as_uuid())
                .bind(session_id)
                .bind(handoff_id)
                .bind(&grant_id)
                .execute(tx.as_mut())
                .await?;
            }
            effects.push(effect(
                "grant_created",
                "grant",
                &grant_id,
                None,
                Some("active"),
            ));
            let target = StateTarget::granted(state, grant_id.clone(), soft_deadline, grant_depth);
            effects.push(phase_effect(session_id, state.phase, target.phase));
            let transition = TransitionSpec::command(
                "offer_acked",
                Some(offer.offer_id),
                event.id.as_bytes().as_slice(),
                effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                target,
                RevisionDelta {
                    floor: true,
                    intent: intent_changed,
                    speech: false,
                },
                transition,
                Some(grant_id),
                now,
            )
            .await
        }
        _ => Err(DbError::InvalidData(
            "invalid Offer command dispatch".to_string(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_grant_command_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let grant_id = match command {
        BatonCommand::GrantProgress { grant_id, .. }
        | BatonCommand::GrantYield { grant_id, .. } => grant_id,
        _ => {
            return Err(DbError::InvalidData(
                "invalid Grant command dispatch".to_string(),
            ));
        }
    };
    let Some(grant) = load_grant_tx(tx, community_id, session_id, grant_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "grant_not_found",
            canonical_object_id: None,
        });
    };
    if grant.holder_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "only the active Grant holder can signal it".to_string(),
        ));
    }
    if state.phase != BatonPhase::Granted || state.active_grant_id.as_deref() != Some(grant_id) {
        return Ok(ApplyResult::Rejected {
            code: "grant_not_active",
            canonical_object_id: Some(grant.grant_id),
        });
    }
    if grant.state != "active" {
        return Ok(ApplyResult::Rejected {
            code: "grant_already_terminal",
            canonical_object_id: Some(grant.grant_id),
        });
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    match command {
        BatonCommand::GrantProgress {
            progress_seq,
            stage,
            ..
        } => {
            let expected = grant.progress_seq + 1;
            if *progress_seq != expected {
                return Ok(ApplyResult::Rejected {
                    code: "stale_progress_sequence",
                    canonical_object_id: Some(grant.grant_id),
                });
            }
            let soft_deadline =
                (now + Duration::milliseconds(config.grant_soft_lease_ms)).min(grant.hard_deadline);
            sqlx::query(
                "UPDATE meeting_baton_grants \
                 SET progress_seq = $4, soft_lease_expires_at = $5 \
                 WHERE community_id = $1 AND session_id = $2 AND grant_id = $3 \
                   AND state = 'active'",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(grant_id)
            .bind(progress_seq)
            .bind(soft_deadline)
            .execute(tx.as_mut())
            .await?;
            sqlx::query(
                "INSERT INTO meeting_grant_progress \
                     (community_id, session_id, grant_id, progress_seq, progress_event_id, \
                      stage, soft_lease_expires_at, accepted_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(grant_id)
            .bind(progress_seq)
            .bind(event.id.as_bytes().as_slice())
            .bind(stage.as_str())
            .bind(soft_deadline)
            .bind(now)
            .execute(tx.as_mut())
            .await?;
            let mut target = StateTarget::from_state(state);
            target.next_action_at = Some(soft_deadline.min(grant.hard_deadline));
            let transition = TransitionSpec::command(
                "grant_progressed",
                Some(grant_id.clone()),
                event.id.as_bytes().as_slice(),
                vec![effect(
                    "grant_progressed",
                    "grant",
                    grant_id,
                    Some("active"),
                    Some("active"),
                )],
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                target,
                RevisionDelta::FLOOR,
                transition,
                Some(grant_id.clone()),
                now,
            )
            .await
        }
        BatonCommand::GrantYield {
            reason_code,
            reason,
            ..
        } => {
            let terminal_reason = reason.as_deref().or(reason_code.as_deref());
            let failed = fail_active_grant_tx(
                tx,
                community_id,
                session_id,
                state,
                &grant,
                "yielded",
                Some(event.id.as_bytes().as_slice()),
                terminal_reason,
                &config,
                now,
            )
            .await?;
            let transition = TransitionSpec::command(
                "grant_yielded",
                Some(grant_id.clone()),
                event.id.as_bytes().as_slice(),
                failed.effects,
            );
            finish_accepted_tx(
                tx,
                community_id,
                session_id,
                event,
                relay_keys,
                state,
                failed.target,
                RevisionDelta {
                    floor: true,
                    intent: failed.intent_changed,
                    speech: false,
                },
                transition,
                Some(grant_id.clone()),
                now,
            )
            .await
        }
        _ => Err(DbError::InvalidData(
            "invalid Grant command dispatch".to_string(),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
async fn apply_speech_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    relay_keys: &Keys,
    actor: &Actor,
    state: &StateRow,
    command: &BatonCommand,
    now: DateTime<Utc>,
) -> Result<ApplyResult> {
    let BatonCommand::Speech {
        grant_id,
        speech_revision,
        handoff,
    } = command
    else {
        return Err(DbError::InvalidData(
            "invalid Meeting speech dispatch".to_string(),
        ));
    };
    if event.content.is_empty() || event.content.len() > 256 * 1024 {
        return Err(DbError::InvalidData(
            "Meeting speech content must contain 1..=262144 UTF-8 bytes".to_string(),
        ));
    }
    let Some(grant) = load_grant_tx(tx, community_id, session_id, grant_id).await? else {
        return Ok(ApplyResult::Rejected {
            code: "grant_not_found",
            canonical_object_id: None,
        });
    };
    if grant.holder_pubkey != actor.pubkey {
        return Err(DbError::AccessDenied(
            "only the active Grant holder can publish its speech".to_string(),
        ));
    }
    if state.phase != BatonPhase::Granted || state.active_grant_id.as_deref() != Some(grant_id) {
        return Ok(ApplyResult::Rejected {
            code: "grant_not_active",
            canonical_object_id: Some(grant.grant_id),
        });
    }
    if grant.state != "active" {
        return Ok(ApplyResult::Rejected {
            code: "grant_already_terminal",
            canonical_object_id: Some(grant.grant_id),
        });
    }
    if grant.basis_speech_revision != state.speech_revision {
        return Err(DbError::InvalidData(
            "active Grant basis does not match the authoritative speech revision".to_string(),
        ));
    }
    if *speech_revision != state.speech_revision + 1 {
        return Ok(ApplyResult::Rejected {
            code: "stale_speech_revision",
            canonical_object_id: Some(state.state_event_id.clone()),
        });
    }
    if let Some(handoff) = handoff {
        if handoff.to_pubkey == actor.pubkey {
            return Err(DbError::InvalidData(
                "Directed Handoff target must be another participant".to_string(),
            ));
        }
        ensure_participant_tx(tx, community_id, session_id, &handoff.to_pubkey).await?;
    }
    let mut mention_pubkeys = HashSet::new();
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        if parts.first().map(String::as_str) != Some("p") {
            continue;
        }
        let value = parts.get(1).ok_or_else(|| {
            DbError::InvalidData("Meeting speech p tag is missing its pubkey".to_string())
        })?;
        let pubkey = hex::decode(value)
            .map_err(|_| DbError::InvalidData("Meeting speech p tag is not hex".to_string()))?;
        validate_id(&pubkey, "Meeting speech mention pubkey")?;
        if !mention_pubkeys.insert(pubkey.clone()) {
            return Err(DbError::InvalidData(
                "Meeting speech cannot mention the same participant twice".to_string(),
            ));
        }
        if mention_pubkeys.len() > MAX_MEETING_PARTICIPANTS {
            return Err(DbError::InvalidData(format!(
                "Meeting speech supports at most {MAX_MEETING_PARTICIPANTS} participant mentions"
            )));
        }
        ensure_participant_tx(tx, community_id, session_id, &pubkey).await?;
    }
    persist_command_event_tx(tx, community_id, session_id, event, now).await?;
    let speech_id = event.id.as_bytes().to_vec();
    sqlx::query(
        "UPDATE meeting_baton_grants \
         SET state = 'spoken', speech_event_id = $4, terminal_event_id = $4, \
             terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND grant_id = $3 \
           AND state = 'active'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(grant_id)
    .bind(&speech_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut effects = vec![
        effect(
            "grant_spoken",
            "grant",
            grant_id,
            Some("active"),
            Some("spoken"),
        ),
        effect(
            "speech_accepted",
            "speech",
            &speech_id,
            None,
            Some("canonical"),
        ),
    ];
    let reactivated_intent_ids =
        release_offer_deferrals_tx(tx, community_id, session_id, &grant.source_offer_id, now)
            .await?;
    let mut intent_changed = !reactivated_intent_ids.is_empty();
    for intent_id in reactivated_intent_ids {
        effects.push(effect(
            "intent_reactivated",
            "intent",
            &intent_id,
            Some("deferred"),
            Some("pending"),
        ));
    }
    if let Some(intent_id) = grant.source_intent_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_speech_intents \
             SET state = 'consumed', last_attempt_outcome = 'spoken', \
                 terminal_event_id = $4, terminal_at = $5, updated_at = $5, \
                 deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3 \
               AND state = 'selected'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(intent_id)
        .bind(&speech_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        intent_changed = true;
        effects.push(effect(
            "intent_consumed",
            "intent",
            intent_id,
            Some("selected"),
            Some("consumed"),
        ));
    }
    sort_effects_by_object_id(&mut effects[2..]);
    if let Some(source_handoff_id) = grant.source_handoff_id.as_deref() {
        sqlx::query(
            "UPDATE meeting_directed_handoffs \
             SET question_state = 'answered', last_attempt_outcome = 'spoken', \
                 answered_by_speech_event_id = $4, answered_at = $5, terminal_at = $5 \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3 \
               AND question_state = 'open'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(source_handoff_id)
        .bind(&speech_id)
        .bind(now)
        .execute(tx.as_mut())
        .await?;
        effects.push(effect(
            "handoff_answered",
            "handoff",
            source_handoff_id,
            Some("open"),
            Some("answered"),
        ));
    }
    let config = load_config_tx(tx, community_id, session_id).await?;
    let mut scheduling_state = state.clone();
    scheduling_state.speech_revision += 1;
    scheduling_state.consecutive_moderator_speeches = if grant.turn_role == "moderator_self" {
        state.consecutive_moderator_speeches + 1
    } else {
        0
    };
    scheduling_state.handoff_depth = grant.handoff_depth;
    let queued_human = earliest_queued_human_tx(tx, community_id, session_id).await?;
    let mut new_handoff: Option<HandoffRow> = None;
    let mut handoff_can_offer = false;
    if let Some(handoff) = handoff {
        let open_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND question_state = 'open'",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(tx.as_mut())
        .await?;
        let requested_depth = if grant.turn_role == "moderator_self" {
            0
        } else {
            scheduling_state.handoff_depth.saturating_add(1)
        };
        let (question_state, initial_disposition, blocked_by, terminal_at) =
            if open_count >= i64::from(config.max_open_handoffs) {
                ("blocked", "blocked", Some("open_question_limit"), Some(now))
            } else if queued_human.is_some() {
                ("open", "blocked", Some("human_request"), None)
            } else if state.forced_return_to_moderator {
                ("open", "blocked", Some("recall"), None)
            } else if grant.turn_role != "moderator_self"
                && scheduling_state.handoff_depth >= config.max_handoff_depth
            {
                ("open", "blocked", Some("max_depth"), None)
            } else {
                handoff_can_offer = true;
                ("open", "offered", None, None)
            };
        let eligible_decision_epoch = next_decision_epoch(state.decision_epoch)?;
        sqlx::query(
            "INSERT INTO meeting_directed_handoffs \
                 (community_id, session_id, handoff_id, source_speech_event_id, \
                  from_pubkey, to_pubkey, reason_type, reason_text, requested_depth, \
                  question_state, initial_disposition, blocked_by, \
                  eligible_decision_epoch, created_at, terminal_at) \
             VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&speech_id)
        .bind(&actor.pubkey)
        .bind(&handoff.to_pubkey)
        .bind(&handoff.reason_type)
        .bind(&handoff.reason_text)
        .bind(requested_depth)
        .bind(question_state)
        .bind(initial_disposition)
        .bind(blocked_by)
        .bind(eligible_decision_epoch)
        .bind(now)
        .bind(terminal_at)
        .execute(tx.as_mut())
        .await?;
        effects.push(effect(
            if question_state == "blocked" {
                "handoff_open_limit_blocked"
            } else {
                "handoff_created"
            },
            "handoff",
            &speech_id,
            None,
            Some(question_state),
        ));
        new_handoff = Some(HandoffRow {
            handoff_id: speech_id.clone(),
            source_speech_event_id: speech_id.clone(),
            from_pubkey: actor.pubkey.clone(),
            to_pubkey: handoff.to_pubkey.clone(),
            reason_type: handoff.reason_type.clone(),
            reason_text: handoff.reason_text.clone(),
            question_state: question_state.to_string(),
            blocked_by: blocked_by.map(str::to_string),
            last_offer_id: None,
            last_grant_id: None,
            attempt_count: 0,
            eligible_decision_epoch,
        });
    }

    let target = if let Some(request) = queued_human {
        let (target, offer_id) = offer_human_request_tx(
            tx,
            community_id,
            session_id,
            &scheduling_state,
            &request,
            &config,
            now,
        )
        .await?;
        effects.push(effect(
            "human_offered",
            "human_request",
            &request.request_id,
            Some("queued"),
            Some("offered"),
        ));
        effects.push(effect(
            "offer_created",
            "offer",
            &offer_id,
            None,
            Some("pending"),
        ));
        target
    } else if state.forced_return_to_moderator
        || (grant.turn_role != "moderator_self"
            && scheduling_state.handoff_depth >= config.max_handoff_depth)
    {
        let target = return_control_to_moderator_tx(
            tx,
            community_id,
            session_id,
            &scheduling_state,
            &config,
            now,
            true,
        )
        .await?
        .into_target_with_effects(&mut effects);
        if state.forced_return_to_moderator {
            if let Some(recall_event_id) = state.recall_event_id.as_deref() {
                effects.push(effect(
                    "recall_cleared",
                    "recall",
                    recall_event_id,
                    Some("latched"),
                    Some("cleared"),
                ));
            }
        }
        effects.push(control_effect(session_id, "forced_return_completed"));
        target
    } else if handoff_can_offer {
        let handoff = new_handoff.as_ref().ok_or_else(|| {
            DbError::InvalidData("offerable Directed Handoff is missing".to_string())
        })?;
        if grant.turn_role == "moderator_self" {
            scheduling_state.handoff_depth = 0;
        }
        let offer_id = random_object_id();
        let depth_mode = if grant.turn_role == "moderator_self" {
            "reset"
        } else {
            "increment_provisional"
        };
        let requested_depth = if depth_mode == "reset" {
            0
        } else {
            scheduling_state.handoff_depth.saturating_add(1)
        };
        let draft = OfferDraft {
            offer_id: offer_id.clone(),
            target_pubkey: handoff.to_pubkey.clone(),
            allocation_source: "directed_handoff",
            turn_role: "participant",
            allocation_event_id: Some(speech_id.clone()),
            selection_reason: None,
            source_intent_id: None,
            source_request_id: None,
            source_handoff_id: Some(speech_id.clone()),
            source_speech_event_id: Some(speech_id.clone()),
            reason_type: Some(handoff.reason_type.clone()),
            reason_text: Some(handoff.reason_text.clone()),
            basis_speech_revision: scheduling_state.speech_revision,
            depth_mode,
            previous_handoff_depth: scheduling_state.handoff_depth,
            requested_handoff_depth: requested_depth,
        };
        let deadline = insert_offer_tx(tx, community_id, session_id, &draft, &config, now).await?;
        effects.push(effect(
            "handoff_attempted",
            "handoff",
            &speech_id,
            Some("open"),
            Some("open"),
        ));
        effects.push(effect(
            "offer_created",
            "offer",
            &offer_id,
            None,
            Some("pending"),
        ));
        StateTarget::offered(&scheduling_state, offer_id, deadline)
    } else {
        let target = return_control_to_moderator_tx(
            tx,
            community_id,
            session_id,
            &scheduling_state,
            &config,
            now,
            true,
        )
        .await?
        .into_target_with_effects(&mut effects);
        effects.push(control_effect(session_id, "control_returned"));
        target
    };
    let mut target = target;
    target.consecutive_moderator_speeches = scheduling_state.consecutive_moderator_speeches;
    effects.push(phase_effect(session_id, state.phase, target.phase));
    let transition = TransitionSpec::command(
        "speech_accepted",
        Some(speech_id.clone()),
        &speech_id,
        effects,
    );
    finish_accepted_tx(
        tx,
        community_id,
        session_id,
        event,
        relay_keys,
        state,
        target,
        if intent_changed {
            RevisionDelta::ALL
        } else {
            RevisionDelta::FLOOR_SPEECH
        },
        transition,
        Some(speech_id),
        now,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_event(
        keys: &Keys,
        kind: u32,
        session_id: Uuid,
        content: &str,
        mentions: &[Vec<u8>],
    ) -> Event {
        let session = session_id.to_string();
        let nonce = Uuid::new_v4().to_string();
        let mut tags = vec![
            Tag::parse(["h", session.as_str()]).expect("build test h tag"),
            Tag::parse(["v", "2"]).expect("build test v tag"),
            Tag::parse(["test-nonce", nonce.as_str()]).expect("build unique test tag"),
        ];
        for mention in mentions {
            let mention = hex::encode(mention);
            tags.push(Tag::parse(["p", mention.as_str()]).expect("build test p tag"));
        }
        EventBuilder::new(
            Kind::Custom(u16::try_from(kind).expect("test kind fits u16")),
            content,
        )
        .tags(tags)
        .sign_with_keys(keys)
        .expect("sign test Meeting command")
    }

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to Meeting V1 stage-two test database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V1 stage-two migrations");
        pool
    }

    async fn seed_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("meeting-v1-stage2-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert stage-two test community");
        CommunityId::from_uuid(id)
    }

    async fn seed_identity(
        pool: &PgPool,
        community_id: CommunityId,
        keys: &Keys,
        relay_role: &str,
        agent_owner_pubkey: Option<&[u8]>,
    ) {
        let pubkey = keys.public_key().to_bytes();
        sqlx::query(
            "INSERT INTO users \
                 (community_id, pubkey, agent_owner_pubkey, channel_add_policy) \
             VALUES ($1, $2, $3, 'anyone')",
        )
        .bind(community_id.as_uuid())
        .bind(pubkey.as_slice())
        .bind(agent_owner_pubkey)
        .execute(pool)
        .await
        .expect("insert stage-two test identity");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(pubkey))
        .bind(relay_role)
        .execute(pool)
        .await
        .expect("insert stage-two relay member");
    }

    async fn persist_existing_event_tx(
        tx: &mut Transaction<'_, Postgres>,
        community_id: CommunityId,
        session_id: Uuid,
        event: &Event,
    ) {
        let created_at = DateTime::from_timestamp(event.created_at.as_secs() as i64, 0)
            .expect("valid test event timestamp");
        sqlx::query(
            "INSERT INTO events \
                 (community_id, id, pubkey, created_at, kind, tags, content, sig, \
                  received_at, channel_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, clock_timestamp(), $9)",
        )
        .bind(community_id.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .bind(event.pubkey.as_bytes())
        .bind(created_at)
        .bind(event.kind.as_u16() as i32)
        .bind(serde_json::to_value(&event.tags).expect("serialize test tags"))
        .bind(&event.content)
        .bind(event.sig.serialize().as_slice())
        .bind(session_id)
        .execute(tx.as_mut())
        .await
        .expect("persist existing test command");
    }

    struct TestMeeting {
        db: Db,
        community_id: CommunityId,
        session_id: Uuid,
        relay: Keys,
        moderator: Keys,
        agent: Keys,
        human: Keys,
        human_two: Keys,
    }

    async fn create_test_meeting() -> TestMeeting {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = seed_community(&pool).await;
        let relay = Keys::generate();
        let moderator = Keys::generate();
        let agent = Keys::generate();
        let human = Keys::generate();
        let human_two = Keys::generate();
        let moderator_pubkey = moderator.public_key().to_bytes().to_vec();
        seed_identity(&pool, community_id, &moderator, "owner", None).await;
        seed_identity(
            &pool,
            community_id,
            &agent,
            "member",
            Some(&moderator_pubkey),
        )
        .await;
        seed_identity(&pool, community_id, &human, "member", None).await;
        seed_identity(&pool, community_id, &human_two, "member", None).await;
        let session_id = Uuid::new_v4();
        let create_event = signed_event(
            &moderator,
            buzz_core::kind::KIND_MEETING_CREATE,
            session_id,
            "",
            &[],
        );
        let roster = vec![
            moderator_pubkey.clone(),
            agent.public_key().to_bytes().to_vec(),
            human.public_key().to_bytes().to_vec(),
            human_two.public_key().to_bytes().to_vec(),
        ];
        let mut tx = pool.begin().await.expect("begin test Meeting create");
        persist_existing_event_tx(&mut tx, community_id, session_id, &create_event).await;
        create_meeting_v1_tx(
            &mut tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Stage Two",
                description: None,
                source_channel_id: None,
                host_pubkey: &moderator_pubkey,
                moderator_pubkey: &moderator_pubkey,
                create_event_id: create_event.id.as_bytes().as_slice(),
                participant_pubkeys: &roster,
                relay_keys: &relay,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect("create test Meeting V1");
        tx.commit().await.expect("commit test Meeting V1");
        TestMeeting {
            db,
            community_id,
            session_id,
            relay,
            moderator,
            agent,
            human,
            human_two,
        }
    }

    async fn create_agent_moderated_test_meeting() -> TestMeeting {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = seed_community(&pool).await;
        let relay = Keys::generate();
        let human = Keys::generate();
        let moderator = Keys::generate();
        let agent = Keys::generate();
        let human_two = Keys::generate();
        let host_pubkey = human.public_key().to_bytes().to_vec();
        let moderator_pubkey = moderator.public_key().to_bytes().to_vec();
        seed_identity(&pool, community_id, &human, "owner", None).await;
        seed_identity(
            &pool,
            community_id,
            &moderator,
            "member",
            Some(&host_pubkey),
        )
        .await;
        seed_identity(&pool, community_id, &agent, "member", Some(&host_pubkey)).await;
        seed_identity(&pool, community_id, &human_two, "member", None).await;
        let session_id = Uuid::new_v4();
        let create_event = signed_event(
            &human,
            buzz_core::kind::KIND_MEETING_CREATE,
            session_id,
            "",
            &[],
        );
        let roster = vec![
            host_pubkey.clone(),
            moderator_pubkey.clone(),
            agent.public_key().to_bytes().to_vec(),
            human_two.public_key().to_bytes().to_vec(),
        ];
        let mut tx = pool
            .begin()
            .await
            .expect("begin Agent-moderated Meeting create");
        persist_existing_event_tx(&mut tx, community_id, session_id, &create_event).await;
        create_meeting_v1_tx(
            &mut tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Agent Moderator Retry",
                description: None,
                source_channel_id: None,
                host_pubkey: &host_pubkey,
                moderator_pubkey: &moderator_pubkey,
                create_event_id: create_event.id.as_bytes().as_slice(),
                participant_pubkeys: &roster,
                relay_keys: &relay,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect("create Agent-moderated Meeting V1");
        tx.commit()
            .await
            .expect("commit Agent-moderated Meeting V1");
        TestMeeting {
            db,
            community_id,
            session_id,
            relay,
            moderator,
            agent,
            human,
            human_two,
        }
    }

    fn accepted_id(result: &BatonCommitResult) -> Vec<u8> {
        match &result.command_outcome {
            BatonCommandOutcome::Accepted {
                canonical_object_id: Some(id),
                ..
            } => id.clone(),
            other => panic!("expected accepted object, got {other:?}"),
        }
    }

    async fn latest_state_content(meeting: &TestMeeting) -> Value {
        let content: String = sqlx::query_scalar(
            "SELECT e.content \
             FROM meeting_baton_state s \
             JOIN events e \
               ON e.community_id = s.community_id AND e.id = s.state_event_id \
             WHERE s.community_id = $1 AND s.session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load latest authoritative State content");
        serde_json::from_str::<Value>(&content).expect("parse latest authoritative State")
    }

    async fn latest_transition(meeting: &TestMeeting) -> Value {
        latest_state_content(meeting).await["transition"].clone()
    }

    async fn assert_command_not_persisted(meeting: &TestMeeting, event: &Event) {
        let (event_exists, receipt_exists): (bool, bool) = sqlx::query_as(
            "SELECT \
                 EXISTS(SELECT 1 FROM events \
                        WHERE community_id = $1 AND id = $2), \
                 EXISTS(SELECT 1 FROM meeting_v1_command_receipts \
                        WHERE community_id = $1 AND command_event_id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check unauthorized command persistence");
        assert!(!event_exists, "unauthorized command event must not persist");
        assert!(
            !receipt_exists,
            "unauthorized command must not receive a private receipt"
        );
    }

    async fn assert_serial_state_and_outbox(meeting: &TestMeeting) -> BatonSnapshot {
        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load authoritative State after concurrent commands");
        let current_state_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count current State rows");
        assert_eq!(
            current_state_count, 1,
            "one Session must retain exactly one authoritative State row"
        );

        let (history_count, min_revision, max_revision, distinct_state_events): (
            i64,
            Option<i64>,
            Option<i64>,
            i64,
        ) = sqlx::query_as(
            "SELECT count(*), min(state_revision), max(state_revision), \
                    count(DISTINCT state_event_id) \
             FROM meeting_baton_state_history \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load State revision chain");
        assert_eq!(min_revision, Some(1));
        assert_eq!(max_revision, Some(snapshot.state_revision));
        assert_eq!(
            history_count, snapshot.state_revision,
            "State revisions must be gap-free from revision one"
        );
        assert_eq!(
            distinct_state_events, history_count,
            "each State revision must have one unique signed State event"
        );

        let state_outbox_count: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM meeting_baton_state_history history \
             JOIN meeting_event_outbox outbox \
               ON outbox.community_id = history.community_id \
              AND outbox.session_id = history.session_id \
              AND outbox.event_id = history.state_event_id \
             WHERE history.community_id = $1 AND history.session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count State outbox rows");
        assert_eq!(
            state_outbox_count, history_count,
            "every canonical State must have exactly one outbox row"
        );
        let (outbox_count, distinct_outbox_events): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(DISTINCT event_id) \
             FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check Meeting outbox uniqueness");
        assert_eq!(
            outbox_count, distinct_outbox_events,
            "concurrent transitions must not duplicate an outbox event"
        );
        snapshot
    }

    async fn end_test_meeting(meeting: &TestMeeting) -> Value {
        let create_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT create_event_id FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load test Meeting Create id");
        let end_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_END,
            meeting.session_id,
            "",
            &[],
        );
        let mut tx = meeting
            .db
            .begin_transaction()
            .await
            .expect("begin test Meeting End");
        persist_existing_event_tx(
            &mut tx,
            meeting.community_id,
            meeting.session_id,
            &end_event,
        )
        .await;
        end_meeting_v1_tx(
            &mut tx,
            EndMeetingV1Params {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                actor_pubkey: meeting.moderator.public_key().to_bytes().as_slice(),
                create_event_id: &create_event_id,
                end_event_id: end_event.id.as_bytes().as_slice(),
                relay_keys: &meeting.relay,
            },
        )
        .await
        .expect("end test Meeting V1");
        tx.commit().await.expect("commit test Meeting End");
        latest_transition(meeting).await
    }

    async fn submit_intent(
        meeting: &TestMeeting,
        keys: &Keys,
        summary: &str,
    ) -> (Event, BatonCommitResult) {
        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("read pre-Intent snapshot");
        let event = signed_event(
            keys,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            meeting.session_id,
            summary,
            &[],
        );
        let result = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &event,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentSubmit {
                    basis_speech_revision: snapshot.speech_revision,
                    summary: summary.to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect("submit test Intent");
        (event, result)
    }

    async fn moderator_select_intent(
        meeting: &TestMeeting,
        intent_id: Vec<u8>,
    ) -> (Event, BatonCommitResult) {
        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("read pre-Select snapshot");
        let event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let result = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Intent { intent_id },
                    expected_control_epoch: snapshot.control_epoch,
                    expected_decision_epoch: snapshot.decision_epoch,
                    expected_intent_revision: snapshot.intent_revision,
                    expected_speech_revision: snapshot.speech_revision,
                    selection_reason: Some("next relevant contribution".to_string()),
                    deferrals: Vec::new(),
                    attempt_id: None,
                    expected_source_event_id: None,
                },
            },
        )
        .await
        .expect("moderator Select test Intent");
        (event, result)
    }

    async fn start_agent_moderator_attempt(
        meeting: &TestMeeting,
        snapshot: &BatonSnapshot,
        replacement_of_attempt_id: Option<Vec<u8>>,
    ) -> (Event, BatonCommitResult) {
        let event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let result = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorDecisionAttemptStart {
                    expected_control_epoch: snapshot.control_epoch,
                    expected_decision_epoch: snapshot.decision_epoch,
                    expected_intent_revision: snapshot.intent_revision,
                    expected_speech_revision: snapshot.speech_revision,
                    expected_state_event_id: snapshot.state_event_id.clone(),
                    replacement_of_attempt_id,
                },
            },
        )
        .await
        .expect("start Agent moderator DecisionAttempt");
        (event, result)
    }

    async fn agent_moderator_select_intent(
        meeting: &TestMeeting,
        snapshot: &BatonSnapshot,
        intent_id: Vec<u8>,
        attempt_id: Option<Vec<u8>>,
        expected_source_event_id: Option<Vec<u8>>,
    ) -> (Event, BatonCommitResult) {
        let event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let result = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Intent { intent_id },
                    expected_control_epoch: snapshot.control_epoch,
                    expected_decision_epoch: snapshot.decision_epoch,
                    expected_intent_revision: snapshot.intent_revision,
                    expected_speech_revision: snapshot.speech_revision,
                    selection_reason: Some("attempt-bound selection".to_string()),
                    deferrals: Vec::new(),
                    attempt_id,
                    expected_source_event_id,
                },
            },
        )
        .await
        .expect("submit Agent moderator Select");
        (event, result)
    }

    async fn create_agent_grant(meeting: &TestMeeting, summary: &str) -> (Vec<u8>, Vec<u8>) {
        let (_, submitted) = submit_intent(meeting, &meeting.agent, summary).await;
        let (_, selected) = moderator_select_intent(meeting, accepted_id(&submitted)).await;
        let offer_id = accepted_id(&selected);
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: offer_id.clone(),
                },
            },
        )
        .await
        .expect("ACK test Agent Offer");
        (offer_id, accepted_id(&acked))
    }

    async fn ack_test_offer(
        meeting: &TestMeeting,
        target: &Keys,
        offer_id: Vec<u8>,
    ) -> BatonCommitResult {
        let ack = signed_event(
            target,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck { offer_id },
            },
        )
        .await
        .expect("ACK test Offer")
    }

    async fn submit_handoff_speech(
        meeting: &TestMeeting,
        speaker: &Keys,
        grant_id: Vec<u8>,
        speech_revision: i64,
        target: &Keys,
        content: &str,
    ) -> (Event, BatonCommitResult) {
        let target_pubkey = target.public_key().to_bytes().to_vec();
        let speech = signed_event(
            speaker,
            9,
            meeting.session_id,
            content,
            std::slice::from_ref(&target_pubkey),
        );
        let result = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id,
                    speech_revision,
                    handoff: Some(BatonHandoffInput {
                        to_pubkey: target_pubkey,
                        reason_type: "question".to_string(),
                        reason_text: format!(
                            "Directed test question at revision {speech_revision}"
                        ),
                    }),
                },
            },
        )
        .await
        .expect("submit test Directed Handoff speech");
        (speech, result)
    }

    async fn create_recalled_human_offer(
        meeting: &TestMeeting,
        human: &Keys,
    ) -> (Vec<u8>, Vec<u8>, Event) {
        let request = signed_event(
            human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("request Human Offer for Recall test");
        let request_id = accepted_id(&requested);
        let offer_id = requested
            .snapshot
            .active_offer_id
            .clone()
            .expect("Human Request creates an Offer");
        let recall = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "return after Human",
            &[],
        );
        let recalled = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &recall,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorRecall {
                    control_epoch: requested.snapshot.control_epoch,
                    reason: Some("return after Human".to_string()),
                },
            },
        )
        .await
        .expect("latch Recall during Human Offer");
        assert!(recalled.snapshot.forced_return_to_moderator);
        (request_id, offer_id, recall)
    }

    async fn ack_human_offer(meeting: &TestMeeting, human: &Keys, offer_id: Vec<u8>) -> Vec<u8> {
        let ack = signed_event(
            human,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck { offer_id },
            },
        )
        .await
        .expect("ACK recalled Human Offer");
        accepted_id(&acked)
    }

    fn assert_recall_completed(transition: &Value, recall: &Event) {
        let effects = transition["effects"]
            .as_array()
            .expect("forced-return transition effects");
        assert!(effects.iter().any(|effect| {
            effect["type"] == "recall_cleared"
                && effect["object_id"] == hex::encode(recall.id.as_bytes())
        }));
        assert!(effects
            .iter()
            .any(|effect| effect["type"] == "forced_return_completed"));
        assert!(effects
            .iter()
            .all(|effect| effect["type"] != "control_returned"));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn agent_moderator_attempt_freezes_cohort_without_retrying_for_late_intents() {
        let meeting = create_agent_moderated_test_meeting().await;
        let (source_event, submitted) =
            submit_intent(&meeting, &meeting.agent, "Original candidate").await;
        let intent_id = accepted_id(&submitted);

        let (_, missing_attempt) = agent_moderator_select_intent(
            &meeting,
            &submitted.snapshot,
            intent_id.clone(),
            None,
            None,
        )
        .await;
        assert!(matches!(
            missing_attempt.command_outcome,
            BatonCommandOutcome::RejectedTerminal {
                ref code,
                retry_ticket_id: None,
                ..
            } if code == "moderator_attempt_required"
        ));

        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let attempt_id = accepted_id(&started);
        let state = latest_state_content(&meeting).await;
        let active_attempt = &state["active_decision_attempt"];
        assert_eq!(
            active_attempt["attempt_id"],
            hex::encode(attempt_id.as_slice())
        );
        let candidate_refs = active_attempt["candidate_refs"]
            .as_array()
            .expect("attempt candidate refs");
        assert_eq!(candidate_refs.len(), 1);
        assert_eq!(
            candidate_refs[0]["current_event_id"],
            hex::encode(source_event.id.as_bytes())
        );
        let persisted_hash: Vec<u8> = sqlx::query_scalar(
            "SELECT candidate_snapshot_hash \
             FROM meeting_moderator_decision_attempts \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&attempt_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load persisted Candidate Cohort hash");
        assert_eq!(
            active_attempt["candidate_snapshot_hash"],
            hex::encode(&persisted_hash)
        );
        assert_eq!(
            state["transition"]["effects"][0]["candidate_snapshot_hash"],
            hex::encode(persisted_hash)
        );

        let (_, late) = submit_intent(
            &meeting,
            &meeting.moderator,
            "Late moderator-self next-cohort candidate",
        )
        .await;
        assert_eq!(
            late.snapshot.decision_epoch,
            started.snapshot.decision_epoch
        );
        let (source_epoch, late_epoch): (i64, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT eligible_decision_epoch FROM meeting_speech_intents \
                  WHERE community_id = $1 AND session_id = $2 AND intent_id = $3), \
                 (SELECT eligible_decision_epoch FROM meeting_speech_intents \
                  WHERE community_id = $1 AND session_id = $2 AND intent_id = $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&intent_id)
        .bind(accepted_id(&late))
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load frozen Cohort epochs");
        assert_eq!(source_epoch, started.snapshot.decision_epoch);
        assert_eq!(late_epoch, started.snapshot.decision_epoch + 1);

        let (_, stale_cas) = agent_moderator_select_intent(
            &meeting,
            &started.snapshot,
            intent_id.clone(),
            Some(attempt_id.clone()),
            Some(source_event.id.as_bytes().to_vec()),
        )
        .await;
        assert!(matches!(
            stale_cas.command_outcome,
            BatonCommandOutcome::RejectedTerminal {
                ref code,
                retry_ticket_id: None,
                ..
            } if code == "stale_moderator_revision"
        ));

        let (_, selected) = agent_moderator_select_intent(
            &meeting,
            &late.snapshot,
            intent_id,
            Some(attempt_id),
            Some(source_event.id.as_bytes().to_vec()),
        )
        .await;
        assert!(matches!(
            selected.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(selected.snapshot.phase, BatonPhase::Offered);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn moderator_fallback_cannot_select_a_late_self_intent() {
        let meeting = create_agent_moderated_test_meeting().await;
        let (_, submitted) =
            submit_intent(&meeting, &meeting.agent, "Current Cohort candidate").await;
        let current_intent_id = accepted_id(&submitted);
        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let (_, late_self) = submit_intent(
            &meeting,
            &meeting.moderator,
            "Late moderator-self candidate",
        )
        .await;
        let late_self_id = accepted_id(&late_self);
        assert_eq!(
            late_self.snapshot.decision_epoch,
            started.snapshot.decision_epoch
        );

        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = due.deadline, next_action_at = due.deadline \
             FROM (SELECT clock_timestamp() - interval '1 second' AS deadline) due \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force moderator DecisionAttempt deadline");
        let transitions = recover_meeting_v1(
            &meeting.db,
            meeting.community_id,
            meeting.session_id,
            &meeting.relay,
        )
        .await
        .expect("run deterministic moderator fallback");
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].primary_type, "moderator_fallback");

        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load fallback State");
        let offer_id = snapshot
            .active_offer_id
            .expect("current Cohort candidate receives fallback Offer");
        let source_intent_id: Vec<u8> = sqlx::query_scalar(
            "SELECT source_intent_id FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(offer_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load fallback Offer source");
        assert_eq!(source_intent_id, current_intent_id);
        let (late_state, late_attempts): (String, i32) = sqlx::query_as(
            "SELECT state, selection_attempt_count \
             FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(late_self_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load late moderator-self Intent");
        assert_eq!(late_state, "pending");
        assert_eq!(late_attempts, 0);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn handoff_only_attempt_times_out_once_and_suppresses_the_same_snapshot() {
        let meeting = create_agent_moderated_test_meeting().await;
        let request_event = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("request setup Human floor");
        let setup_offer = requested
            .snapshot
            .active_offer_id
            .expect("Human Request creates setup Offer");
        let setup_grant = ack_test_offer(&meeting, &meeting.human, setup_offer).await;
        let (handoff_speech, handed_off) = submit_handoff_speech(
            &meeting,
            &meeting.human,
            accepted_id(&setup_grant),
            1,
            &meeting.agent,
            "Agent, please answer this Handoff-only question",
        )
        .await;
        let handoff_id = handoff_speech.id.as_bytes().to_vec();
        let handoff_offer = handed_off
            .snapshot
            .active_offer_id
            .expect("Directed Handoff creates Agent Offer");
        let decline_event = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let declined = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &decline_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: handoff_offer,
                    reason: Some("needs moderator routing".to_string()),
                },
            },
        )
        .await
        .expect("decline direct Handoff Offer");
        assert_eq!(declined.snapshot.phase, BatonPhase::ModeratorIdle);
        assert!(declined.snapshot.moderator_decision_deadline.is_none());

        let (_, started) = start_agent_moderator_attempt(&meeting, &declined.snapshot, None).await;
        assert_eq!(
            started.snapshot.decision_epoch, declined.snapshot.decision_epoch,
            "control return already exposed the Handoff-only Cohort"
        );
        assert_eq!(started.snapshot.phase, BatonPhase::ModeratorIdle);
        let active_state = latest_state_content(&meeting).await;
        assert_eq!(
            active_state["active_decision_attempt"]["candidate_refs"][0]["source_id"],
            hex::encode(&handoff_id)
        );

        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = due.deadline, next_action_at = due.deadline \
             FROM (SELECT clock_timestamp() - interval '1 second' AS deadline) due \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force Handoff-only moderator deadline");
        let transitions = recover_meeting_v1(
            &meeting.db,
            meeting.community_id,
            meeting.session_id,
            &meeting.relay,
        )
        .await
        .expect("recover Handoff-only timeout");
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].primary_type, "moderator_fallback");
        let (question_state, suppression): (String, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT question_state, moderator_retry_blocked_fingerprint \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load timed-out Handoff suppression");
        assert_eq!(question_state, "open");
        assert!(suppression.is_some());
        let suppressed_state = latest_state_content(&meeting).await;
        let projected_handoff = suppressed_state["unresolved_handoffs"]
            .as_array()
            .and_then(|handoffs| {
                handoffs
                    .iter()
                    .find(|handoff| handoff["handoff_id"] == hex::encode(&handoff_id))
            })
            .expect("suppressed Handoff remains visible in Relay State");
        assert_eq!(
            projected_handoff["moderator_retry_blocked"],
            Value::Bool(true),
            "ACP must be able to distinguish a timeout-suppressed Handoff from a startable one"
        );

        let recovered = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load recovered Handoff-only State");
        assert_eq!(recovered.phase, BatonPhase::ModeratorIdle);
        assert!(recovered.active_decision_attempt_id.is_none());
        assert!(recovered.moderator_decision_deadline.is_none());
        let (_, reopened) = start_agent_moderator_attempt(&meeting, &recovered, None).await;
        assert!(matches!(
            reopened.command_outcome,
            BatonCommandOutcome::RejectedTerminal { ref code, .. }
                if code == "no_current_cohort_candidates"
        ));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn human_preemption_does_not_suppress_a_handoff_in_the_next_cohort() {
        let meeting = create_agent_moderated_test_meeting().await;
        let setup_request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let setup_requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &setup_request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("request setup Human floor");
        let setup_offer = setup_requested
            .snapshot
            .active_offer_id
            .expect("setup Human Request creates an Offer");
        let setup_grant = ack_test_offer(&meeting, &meeting.human, setup_offer).await;
        let (handoff_speech, handed_off) = submit_handoff_speech(
            &meeting,
            &meeting.human,
            accepted_id(&setup_grant),
            1,
            &meeting.agent,
            "Agent, please answer after moderator review",
        )
        .await;
        let handoff_id = handoff_speech.id.as_bytes().to_vec();
        let handoff_offer = handed_off
            .snapshot
            .active_offer_id
            .expect("Directed Handoff creates an Offer");
        let decline_event = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let declined = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &decline_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: handoff_offer,
                    reason: Some("moderator should route this question".to_string()),
                },
            },
        )
        .await
        .expect("decline direct Handoff Offer");
        let (_, started) = start_agent_moderator_attempt(&meeting, &declined.snapshot, None).await;
        let old_attempt_id = accepted_id(&started);
        let old_decision_epoch = started.snapshot.decision_epoch;

        let preempt_request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let preempted = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &preempt_request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("Human preempts Handoff-only moderator attempt");
        assert_eq!(
            preempted.snapshot.active_decision_attempt_id,
            Some(old_attempt_id.clone())
        );
        let human_offer = preempted
            .snapshot
            .active_offer_id
            .expect("preempting Human receives an Offer");
        let human_grant = ack_test_offer(&meeting, &meeting.human, human_offer).await;

        sqlx::query(
            "UPDATE meeting_moderator_decision_attempts \
             SET deadline_at = clock_timestamp() + interval '5 seconds' \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&old_attempt_id)
        .execute(&meeting.db.pool)
        .await
        .expect("shorten pre-Human attempt deadline");

        let human_speech = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_STREAM_MESSAGE,
            meeting.session_id,
            "Human contribution before returning control",
            &[],
        );
        let returned = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &human_speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id: accepted_id(&human_grant),
                    speech_revision: 2,
                    handoff: None,
                },
            },
        )
        .await
        .expect("complete preempting Human speech");
        assert_eq!(returned.snapshot.phase, BatonPhase::ModeratorIdle);
        assert_eq!(
            returned.snapshot.decision_epoch,
            old_decision_epoch + 1,
            "Human speech opens a new Handoff Cohort"
        );
        assert_eq!(
            returned.snapshot.active_decision_attempt_id,
            Some(old_attempt_id.clone()),
            "the old model Turn remains single-flight until its natural terminal"
        );
        let remaining_ms: i64 = sqlx::query_scalar(
            "SELECT (EXTRACT(EPOCH FROM \
                 (moderator_decision_deadline - clock_timestamp())) * 1000)::bigint \
             FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("measure new Handoff control window");
        assert!(
            remaining_ms > BatonConfig::default().moderator_decision_ms - 10_000,
            "the next Cohort must not inherit the old Attempt's five-second deadline"
        );

        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = due.deadline, next_action_at = due.deadline \
             FROM (SELECT clock_timestamp() - interval '1 second' AS deadline) due \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force the new base control deadline");
        let transitions = recover_meeting_v1(
            &meeting.db,
            meeting.community_id,
            meeting.session_id,
            &meeting.relay,
        )
        .await
        .expect("recover expired Human-postponed control window");
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].primary_type, "moderator_fallback");

        let (attempt_state, suppression): (String, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT \
                 (SELECT state FROM meeting_moderator_decision_attempts \
                  WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3), \
                 (SELECT moderator_retry_blocked_fingerprint \
                  FROM meeting_directed_handoffs \
                  WHERE community_id = $1 AND session_id = $2 AND handoff_id = $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&old_attempt_id)
        .bind(&handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load post-Human Attempt and Handoff state");
        assert_eq!(attempt_state, "timed_out");
        assert!(
            suppression.is_none(),
            "an old-epoch Attempt must not suppress a new-epoch Handoff"
        );

        let recovered = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load recovered post-Human State");
        assert!(recovered.active_decision_attempt_id.is_none());
        let (_, restarted) = start_agent_moderator_attempt(&meeting, &recovered, None).await;
        assert!(matches!(
            restarted.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(restarted.snapshot.decision_epoch, recovered.decision_epoch);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn complete_cohort_atomically_exposes_the_next_epoch() {
        let meeting = create_agent_moderated_test_meeting().await;
        let (current_event, submitted) =
            submit_intent(&meeting, &meeting.agent, "Current Cohort management target").await;
        let current_intent_id = accepted_id(&submitted);
        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let attempt_id = accepted_id(&started);
        let (_, late) = submit_intent(&meeting, &meeting.human_two, "Next Cohort candidate").await;
        let late_intent_id = accepted_id(&late);

        let reject_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "already covered",
            &[],
        );
        let rejected_current = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &reject_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorReject {
                    intent_id: current_intent_id,
                    previous_event_id: current_event.id.as_bytes().to_vec(),
                    author_pubkey: meeting.agent.public_key().to_bytes().to_vec(),
                    reason_code: "duplicate".to_string(),
                    reason_text: "already covered".to_string(),
                    attempt_id: Some(attempt_id.clone()),
                },
            },
        )
        .await
        .expect("reject final current-Cohort Intent");
        assert!(matches!(
            rejected_current.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(
            rejected_current.snapshot.active_decision_attempt_id,
            Some(attempt_id.clone())
        );

        let complete_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let completed = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &complete_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorCompleteCohort {
                    attempt_id: attempt_id.clone(),
                    expected_control_epoch: rejected_current.snapshot.control_epoch,
                    expected_decision_epoch: rejected_current.snapshot.decision_epoch,
                },
            },
        )
        .await
        .expect("complete exhausted current Cohort");
        assert!(matches!(
            completed.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(
            completed.snapshot.decision_epoch,
            rejected_current.snapshot.decision_epoch + 1
        );
        assert_eq!(completed.snapshot.phase, BatonPhase::ModeratorControl);
        assert!(completed.snapshot.active_decision_attempt_id.is_none());
        assert!(completed.snapshot.moderator_decision_deadline.is_some());

        let (late_epoch, attempt_state): (i64, String) = sqlx::query_as(
            "SELECT \
                 (SELECT eligible_decision_epoch FROM meeting_speech_intents \
                  WHERE community_id = $1 AND session_id = $2 AND intent_id = $3), \
                 (SELECT state FROM meeting_moderator_decision_attempts \
                  WHERE community_id = $1 AND session_id = $2 AND attempt_id = $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(late_intent_id)
        .bind(attempt_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load next Cohort and completed attempt");
        assert_eq!(late_epoch, completed.snapshot.decision_epoch);
        assert_eq!(attempt_state, "committed");

        let (_, next_attempt) =
            start_agent_moderator_attempt(&meeting, &completed.snapshot, None).await;
        assert!(matches!(
            next_attempt.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(next_attempt.snapshot.decision_attempt, 1);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn selected_source_change_issues_one_use_retry_with_a_fresh_deadline() {
        let meeting = create_agent_moderated_test_meeting().await;
        let (source_event, submitted) =
            submit_intent(&meeting, &meeting.agent, "Candidate version one").await;
        let intent_id = accepted_id(&submitted);
        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let attempt_id = accepted_id(&started);

        let refresh_event = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            meeting.session_id,
            "Candidate version two",
            &[],
        );
        let refreshed = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &refresh_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentRefresh {
                    intent_id: intent_id.clone(),
                    previous_event_id: source_event.id.as_bytes().to_vec(),
                    basis_speech_revision: started.snapshot.speech_revision,
                    summary: "Candidate version two".to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect("refresh selected Intent during Agent moderator attempt");

        let failed_select_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let failed_select_command = BatonCommand::ModeratorSelect {
            source: BatonSelectionSource::Intent {
                intent_id: intent_id.clone(),
            },
            expected_control_epoch: refreshed.snapshot.control_epoch,
            expected_decision_epoch: refreshed.snapshot.decision_epoch,
            expected_intent_revision: refreshed.snapshot.intent_revision,
            expected_speech_revision: refreshed.snapshot.speech_revision,
            selection_reason: Some("select frozen version".to_string()),
            deferrals: Vec::new(),
            attempt_id: Some(attempt_id.clone()),
            expected_source_event_id: Some(source_event.id.as_bytes().to_vec()),
        };
        let rejected = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &failed_select_event,
                relay_keys: &meeting.relay,
                command: failed_select_command.clone(),
            },
        )
        .await
        .expect("reject Select whose chosen source changed");
        let retry_ticket_id = match &rejected.command_outcome {
            BatonCommandOutcome::RejectedTerminal {
                code,
                retry_ticket_id: Some(retry_ticket_id),
                ..
            } if code == "selected_source_changed" => retry_ticket_id.clone(),
            other => panic!("expected selected-source retry ticket, got {other:?}"),
        };

        let duplicate = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &failed_select_event,
                relay_keys: &meeting.relay,
                command: failed_select_command,
            },
        )
        .await
        .expect("replay failed Select");
        assert!(matches!(
            duplicate.command_outcome,
            BatonCommandOutcome::Duplicate {
                accepted: false,
                ref outcome_code,
                retry_ticket_id: Some(ref duplicate_ticket_id),
                ..
            } if outcome_code == "selected_source_changed"
                && duplicate_ticket_id == &retry_ticket_id
        ));

        let retry_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let retried = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &retry_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorDecisionRetry {
                    attempt_id: attempt_id.clone(),
                    retry_ticket_id: retry_ticket_id.clone(),
                    failed_action_event_id: failed_select_event.id.as_bytes().to_vec(),
                    expected_control_epoch: rejected.snapshot.control_epoch,
                    expected_decision_epoch: rejected.snapshot.decision_epoch,
                    expected_attempt_number: 1,
                },
            },
        )
        .await
        .expect("consume selected-source retry ticket");
        let replacement_id = accepted_id(&retried);
        assert_eq!(retried.snapshot.decision_attempt, 2);
        assert_eq!(
            retried.snapshot.active_decision_attempt_id,
            Some(replacement_id.clone())
        );

        let (started_at, deadline_at, replacement_of, candidate_snapshot): (
            DateTime<Utc>,
            DateTime<Utc>,
            Option<Vec<u8>>,
            Value,
        ) = sqlx::query_as(
            "SELECT started_at, deadline_at, replacement_of_attempt_id, \
                    candidate_snapshot_json \
             FROM meeting_moderator_decision_attempts \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&replacement_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load replacement moderator attempt");
        assert_eq!(
            deadline_at - started_at,
            Duration::milliseconds(BatonConfig::default().moderator_decision_ms)
        );
        assert_eq!(replacement_of, Some(attempt_id));
        assert_eq!(
            candidate_snapshot["candidate_refs"][0]["current_event_id"],
            hex::encode(refresh_event.id.as_bytes())
        );

        let reused_ticket_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let reused = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &reused_ticket_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorDecisionRetry {
                    attempt_id: replacement_id,
                    retry_ticket_id,
                    failed_action_event_id: failed_select_event.id.as_bytes().to_vec(),
                    expected_control_epoch: retried.snapshot.control_epoch,
                    expected_decision_epoch: retried.snapshot.decision_epoch,
                    expected_attempt_number: 2,
                },
            },
        )
        .await
        .expect("reject consumed retry ticket");
        assert!(matches!(
            reused.command_outcome,
            BatonCommandOutcome::RejectedTerminal { ref code, .. }
                if code == "retry_ticket_already_consumed"
        ));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn retry_limit_uses_current_cohort_fallback_without_starting_another_model_attempt() {
        let meeting = create_agent_moderated_test_meeting().await;
        sqlx::query(
            "UPDATE meeting_baton_config \
             SET moderator_max_rejudgments = 0 \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("set zero-rejudgment test policy");

        let (source_event, submitted) =
            submit_intent(&meeting, &meeting.agent, "Bounded retry candidate").await;
        let intent_id = accepted_id(&submitted);
        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let attempt_id = accepted_id(&started);
        let refresh_event = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            meeting.session_id,
            "Bounded retry candidate refreshed",
            &[],
        );
        let refreshed = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &refresh_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentRefresh {
                    intent_id: intent_id.clone(),
                    previous_event_id: source_event.id.as_bytes().to_vec(),
                    basis_speech_revision: started.snapshot.speech_revision,
                    summary: "Bounded retry candidate refreshed".to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect("refresh source for retry-limit test");
        let failed_action = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let rejected = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &failed_action,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Intent {
                        intent_id: intent_id.clone(),
                    },
                    expected_control_epoch: refreshed.snapshot.control_epoch,
                    expected_decision_epoch: refreshed.snapshot.decision_epoch,
                    expected_intent_revision: refreshed.snapshot.intent_revision,
                    expected_speech_revision: refreshed.snapshot.speech_revision,
                    selection_reason: Some("exercise retry bound".to_string()),
                    deferrals: Vec::new(),
                    attempt_id: Some(attempt_id.clone()),
                    expected_source_event_id: Some(source_event.id.as_bytes().to_vec()),
                },
            },
        )
        .await
        .expect("obtain bounded retry ticket");
        let ticket_id = match &rejected.command_outcome {
            BatonCommandOutcome::RejectedTerminal {
                code,
                retry_ticket_id: Some(ticket_id),
                ..
            } if code == "selected_source_changed" => ticket_id.clone(),
            other => panic!("expected selected-source retry ticket, got {other:?}"),
        };
        let retry_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let fallback = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &retry_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorDecisionRetry {
                    attempt_id: attempt_id.clone(),
                    retry_ticket_id: ticket_id,
                    failed_action_event_id: failed_action.id.as_bytes().to_vec(),
                    expected_control_epoch: rejected.snapshot.control_epoch,
                    expected_decision_epoch: rejected.snapshot.decision_epoch,
                    expected_attempt_number: 1,
                },
            },
        )
        .await
        .expect("apply retry-limit fallback");
        assert!(matches!(
            fallback.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert_eq!(fallback.snapshot.phase, BatonPhase::Offered);
        assert!(fallback.snapshot.active_decision_attempt_id.is_none());
        assert_eq!(
            latest_transition(&meeting).await["primary_type"],
            "moderator_retry_limit_fallback"
        );

        let offer_id = fallback
            .snapshot
            .active_offer_id
            .expect("retry limit creates deterministic fallback Offer");
        let (allocation_source, source_intent_id): (String, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT allocation_source, source_intent_id \
             FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(offer_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load retry-limit fallback Offer");
        assert_eq!(allocation_source, "fallback");
        assert_eq!(source_intent_id, Some(intent_id));
        let (attempt_state, attempt_count): (String, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT state FROM meeting_moderator_decision_attempts \
                  WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3), \
                 (SELECT count(*) FROM meeting_moderator_decision_attempts \
                  WHERE community_id = $1 AND session_id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(attempt_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load bounded moderator attempts");
        assert_eq!(attempt_state, "retry_required");
        assert_eq!(attempt_count, 1);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn abandoned_attempt_replacement_inherits_the_original_deadline() {
        let meeting = create_agent_moderated_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Recoverable candidate").await;
        let (_, started) = start_agent_moderator_attempt(&meeting, &submitted.snapshot, None).await;
        let attempt_id = accepted_id(&started);
        let original_deadline: DateTime<Utc> = sqlx::query_scalar(
            "SELECT deadline_at FROM meeting_moderator_decision_attempts \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&attempt_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load original moderator attempt deadline");

        let abandon_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let abandoned = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &abandon_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorDecisionAttemptAbandon {
                    attempt_id: attempt_id.clone(),
                },
            },
        )
        .await
        .expect("abandon lost Runtime attempt");
        assert!(matches!(
            abandoned.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        assert!(abandoned.snapshot.active_decision_attempt_id.is_none());

        let (_, replacement) =
            start_agent_moderator_attempt(&meeting, &abandoned.snapshot, Some(attempt_id.clone()))
                .await;
        let replacement_id = accepted_id(&replacement);
        let (replacement_deadline, replacement_of, attempt_number): (
            DateTime<Utc>,
            Option<Vec<u8>>,
            i32,
        ) = sqlx::query_as(
            "SELECT deadline_at, replacement_of_attempt_id, attempt_number \
             FROM meeting_moderator_decision_attempts \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(replacement_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load replacement attempt deadline");
        assert_eq!(replacement_deadline, original_deadline);
        assert_eq!(replacement_of, Some(attempt_id));
        assert_eq!(attempt_number, 2);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn baton_happy_path_receipt_and_deadline_recovery_are_atomic() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Agent contribution").await;
        let intent_id = accepted_id(&submitted);
        let (_, selected) = moderator_select_intent(&meeting, intent_id).await;
        let offer_id = accepted_id(&selected);
        let unauthorized_ack = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let unauthorized = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &unauthorized_ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: offer_id.clone(),
                },
            },
        )
        .await
        .expect_err("non-target cannot obtain an Offer conflict receipt");
        assert!(matches!(unauthorized, DbError::AccessDenied(_)));
        let unauthorized_receipt: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_v1_command_receipts \
                 WHERE community_id = $1 AND command_event_id = $2 \
             )",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(unauthorized_ack.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check unauthorized Offer receipt");
        assert!(!unauthorized_receipt);
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let ack_command = BatonCommand::OfferAck {
            offer_id: offer_id.clone(),
        };
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: ack_command.clone(),
            },
        )
        .await
        .expect("ACK test Offer");
        let grant_id = accepted_id(&acked);
        let unauthorized_yield = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
            meeting.session_id,
            "",
            &[],
        );
        let unauthorized = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &unauthorized_yield,
                relay_keys: &meeting.relay,
                command: BatonCommand::GrantYield {
                    grant_id: grant_id.clone(),
                    reason_code: None,
                    reason: None,
                },
            },
        )
        .await
        .expect_err("non-holder cannot obtain a Grant conflict receipt");
        assert!(matches!(unauthorized, DbError::AccessDenied(_)));
        let unauthorized_speech =
            signed_event(&meeting.human, 9, meeting.session_id, "Not my Grant", &[]);
        let unauthorized = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &unauthorized_speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id: grant_id.clone(),
                    speech_revision: 1,
                    handoff: None,
                },
            },
        )
        .await
        .expect_err("non-holder cannot obtain a Speech conflict receipt");
        assert!(matches!(unauthorized, DbError::AccessDenied(_)));
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Canonical contribution",
            &[],
        );
        let spoken = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id,
                    speech_revision: 1,
                    handoff: None,
                },
            },
        )
        .await
        .expect("publish Grant-bound speech");
        assert_eq!(spoken.snapshot.speech_revision, 1);
        assert_eq!(spoken.snapshot.phase, BatonPhase::ModeratorIdle);
        let replay = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: ack_command,
            },
        )
        .await
        .expect("replay accepted ACK receipt");
        assert!(matches!(
            replay.command_outcome,
            BatonCommandOutcome::Duplicate {
                accepted: true,
                ref outcome_class,
                ..
            } if outcome_class == "accepted"
        ));
        assert_eq!(
            replay.snapshot.state_revision,
            spoken.snapshot.state_revision
        );

        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Late ACK basis").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let late_offer_id = accepted_id(&selected);
        let stale_ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let stale = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &stale_ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: offer_id.clone(),
                },
            },
        )
        .await
        .expect("authorized target gets terminal receipt for its old Offer");
        assert!(matches!(
            stale.command_outcome,
            BatonCommandOutcome::RejectedTerminal {
                ref code,
                canonical_object_id: Some(ref canonical),
                ..
            } if code == "offer_not_active" && canonical == &offer_id
        ));
        sqlx::query(
            "UPDATE meeting_baton_offers SET ack_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&late_offer_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force test Offer deadline");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due Baton state");
        let late_ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let late = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &late_ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: late_offer_id,
                },
            },
        )
        .await
        .expect("late ACK commits timeout recovery");
        assert!(matches!(
            late.command_outcome,
            BatonCommandOutcome::RejectedAfterRecovery { ref code, .. }
                if code == "offer_not_active"
        ));
        let published_late_command: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE community_id = $1 AND id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(late_ack.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check rejected late command");
        assert!(!published_late_command);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn due_recovery_commits_before_stable_capability_rejections_without_receipts() {
        let offer_meeting = create_test_meeting().await;
        let (_, submitted) =
            submit_intent(&offer_meeting, &offer_meeting.agent, "Due Agent Offer").await;
        let (_, selected) = moderator_select_intent(&offer_meeting, accepted_id(&submitted)).await;
        let offer_id = accepted_id(&selected);
        let offer_revision = selected.snapshot.state_revision;
        sqlx::query(
            "UPDATE meeting_baton_offers \
             SET ack_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(offer_meeting.community_id.as_uuid())
        .bind(offer_meeting.session_id)
        .bind(&offer_id)
        .execute(&offer_meeting.db.pool)
        .await
        .expect("force due Offer for stable authorization");
        let unauthorized_ack = signed_event(
            &offer_meeting.human,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            offer_meeting.session_id,
            "",
            &[],
        );
        let error = execute_baton_command(
            &offer_meeting.db,
            BatonCommandTxParams {
                community_id: offer_meeting.community_id,
                session_id: offer_meeting.session_id,
                event: &unauthorized_ack,
                relay_keys: &offer_meeting.relay,
                command: BatonCommand::OfferAck { offer_id },
            },
        )
        .await
        .expect_err("non-target ACK must remain unauthorized after Offer timeout");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&offer_meeting, &unauthorized_ack).await;
        let snapshot = get_baton_snapshot(
            &offer_meeting.db,
            offer_meeting.community_id,
            offer_meeting.session_id,
        )
        .await
        .expect("read Offer recovery snapshot");
        assert!(snapshot.state_revision > offer_revision);
        assert_eq!(
            latest_transition(&offer_meeting).await["primary_type"],
            "offer_timed_out"
        );

        let speech_meeting = create_test_meeting().await;
        let (_, grant_id) = create_agent_grant(&speech_meeting, "Due Agent speech").await;
        let grant_revision = get_baton_snapshot(
            &speech_meeting.db,
            speech_meeting.community_id,
            speech_meeting.session_id,
        )
        .await
        .expect("read pre-soft-timeout Grant")
        .state_revision;
        sqlx::query(
            "UPDATE meeting_baton_grants \
             SET soft_lease_expires_at = clock_timestamp() - interval '1 second', \
                 hard_deadline = clock_timestamp() + interval '1 minute' \
             WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
        )
        .bind(speech_meeting.community_id.as_uuid())
        .bind(speech_meeting.session_id)
        .bind(&grant_id)
        .execute(&speech_meeting.db.pool)
        .await
        .expect("force due soft Grant for stable authorization");
        let unauthorized_speech = signed_event(
            &speech_meeting.human,
            9,
            speech_meeting.session_id,
            "Not my due Grant",
            &[],
        );
        let error = execute_baton_command(
            &speech_meeting.db,
            BatonCommandTxParams {
                community_id: speech_meeting.community_id,
                session_id: speech_meeting.session_id,
                event: &unauthorized_speech,
                relay_keys: &speech_meeting.relay,
                command: BatonCommand::Speech {
                    grant_id,
                    speech_revision: 1,
                    handoff: None,
                },
            },
        )
        .await
        .expect_err("non-holder SAY must remain unauthorized after Grant timeout");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&speech_meeting, &unauthorized_speech).await;
        let snapshot = get_baton_snapshot(
            &speech_meeting.db,
            speech_meeting.community_id,
            speech_meeting.session_id,
        )
        .await
        .expect("read soft Grant recovery snapshot");
        assert!(snapshot.state_revision > grant_revision);
        assert_eq!(
            latest_transition(&speech_meeting).await["primary_type"],
            "grant_soft_expired"
        );

        let progress_meeting = create_test_meeting().await;
        let (_, grant_id) = create_agent_grant(&progress_meeting, "Due Agent progress").await;
        let grant_revision = get_baton_snapshot(
            &progress_meeting.db,
            progress_meeting.community_id,
            progress_meeting.session_id,
        )
        .await
        .expect("read pre-hard-timeout Grant")
        .state_revision;
        sqlx::query(
            "UPDATE meeting_baton_grants \
             SET soft_lease_expires_at = clock_timestamp() - interval '2 seconds', \
                 hard_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
        )
        .bind(progress_meeting.community_id.as_uuid())
        .bind(progress_meeting.session_id)
        .bind(&grant_id)
        .execute(&progress_meeting.db.pool)
        .await
        .expect("force due hard Grant for stable authorization");
        let unauthorized_progress = signed_event(
            &progress_meeting.human,
            buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
            progress_meeting.session_id,
            "",
            &[],
        );
        let error = execute_baton_command(
            &progress_meeting.db,
            BatonCommandTxParams {
                community_id: progress_meeting.community_id,
                session_id: progress_meeting.session_id,
                event: &unauthorized_progress,
                relay_keys: &progress_meeting.relay,
                command: BatonCommand::GrantProgress {
                    grant_id,
                    progress_seq: 1,
                    stage: BatonProgressStage::ToolUse,
                },
            },
        )
        .await
        .expect_err("non-holder Progress must remain unauthorized after Grant timeout");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&progress_meeting, &unauthorized_progress).await;
        let snapshot = get_baton_snapshot(
            &progress_meeting.db,
            progress_meeting.community_id,
            progress_meeting.session_id,
        )
        .await
        .expect("read hard Grant recovery snapshot");
        assert!(snapshot.state_revision > grant_revision);
        assert_eq!(
            latest_transition(&progress_meeting).await["primary_type"],
            "grant_hard_expired"
        );

        let withdraw_meeting = create_test_meeting().await;
        let (intent_event, submitted) = submit_intent(
            &withdraw_meeting,
            &withdraw_meeting.agent,
            "Due source Intent",
        )
        .await;
        let intent_id = accepted_id(&submitted);
        let moderator_revision = submitted.snapshot.state_revision;
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = statement_timestamp() - interval '1 second', \
                 next_action_at = statement_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(withdraw_meeting.community_id.as_uuid())
        .bind(withdraw_meeting.session_id)
        .execute(&withdraw_meeting.db.pool)
        .await
        .expect("force due moderator window before foreign Withdraw");
        let unauthorized_withdraw = signed_event(
            &withdraw_meeting.human,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            withdraw_meeting.session_id,
            "",
            &[],
        );
        let error = execute_baton_command(
            &withdraw_meeting.db,
            BatonCommandTxParams {
                community_id: withdraw_meeting.community_id,
                session_id: withdraw_meeting.session_id,
                event: &unauthorized_withdraw,
                relay_keys: &withdraw_meeting.relay,
                command: BatonCommand::IntentWithdraw {
                    intent_id,
                    previous_event_id: intent_event.id.as_bytes().to_vec(),
                },
            },
        )
        .await
        .expect_err("non-author Withdraw must remain unauthorized after fallback");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&withdraw_meeting, &unauthorized_withdraw).await;
        let snapshot = get_baton_snapshot(
            &withdraw_meeting.db,
            withdraw_meeting.community_id,
            withdraw_meeting.session_id,
        )
        .await
        .expect("read fallback recovery snapshot");
        assert!(snapshot.state_revision > moderator_revision);
        assert_eq!(
            latest_transition(&withdraw_meeting).await["primary_type"],
            "moderator_fallback"
        );

        let select_meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(
            &select_meeting,
            &select_meeting.agent,
            "Due moderator Select",
        )
        .await;
        let intent_id = accepted_id(&submitted);
        let before = submitted.snapshot;
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = statement_timestamp() - interval '1 second', \
                 next_action_at = statement_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(select_meeting.community_id.as_uuid())
        .bind(select_meeting.session_id)
        .execute(&select_meeting.db.pool)
        .await
        .expect("force due moderator window before non-moderator Select");
        let unauthorized_select = signed_event(
            &select_meeting.human,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            select_meeting.session_id,
            "",
            &[],
        );
        let error = execute_baton_command(
            &select_meeting.db,
            BatonCommandTxParams {
                community_id: select_meeting.community_id,
                session_id: select_meeting.session_id,
                event: &unauthorized_select,
                relay_keys: &select_meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Intent { intent_id },
                    expected_control_epoch: before.control_epoch,
                    expected_decision_epoch: before.decision_epoch,
                    expected_intent_revision: before.intent_revision,
                    expected_speech_revision: before.speech_revision,
                    selection_reason: None,
                    deferrals: Vec::new(),
                    attempt_id: None,
                    expected_source_event_id: None,
                },
            },
        )
        .await
        .expect_err("non-moderator Select must remain unauthorized after fallback");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&select_meeting, &unauthorized_select).await;
        let snapshot = get_baton_snapshot(
            &select_meeting.db,
            select_meeting.community_id,
            select_meeting.session_id,
        )
        .await
        .expect("read non-moderator fallback recovery snapshot");
        assert!(snapshot.state_revision > before.state_revision);
        assert_eq!(
            latest_transition(&select_meeting).await["primary_type"],
            "moderator_fallback"
        );

        let moderator_human = create_test_meeting().await;
        let (_, submitted) = submit_intent(
            &moderator_human,
            &moderator_human.agent,
            "Due before moderator Human",
        )
        .await;
        let (_, selected) =
            moderator_select_intent(&moderator_human, accepted_id(&submitted)).await;
        let offer_id = accepted_id(&selected);
        sqlx::query(
            "UPDATE meeting_baton_offers \
             SET ack_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(moderator_human.community_id.as_uuid())
        .bind(moderator_human.session_id)
        .bind(offer_id)
        .execute(&moderator_human.db.pool)
        .await
        .expect("force due Offer before moderator Human Request");
        let moderator_request = signed_event(
            &moderator_human.moderator,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            moderator_human.session_id,
            "",
            &[],
        );
        let error = execute_baton_command(
            &moderator_human.db,
            BatonCommandTxParams {
                community_id: moderator_human.community_id,
                session_id: moderator_human.session_id,
                event: &moderator_request,
                relay_keys: &moderator_human.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect_err("Human moderator cannot use the direct Human floor path");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&moderator_human, &moderator_request).await;
        assert_eq!(
            latest_transition(&moderator_human).await["primary_type"],
            "offer_timed_out"
        );

        for source_mode in ["participant_intent", "handoff"] {
            let meeting = create_test_meeting().await;
            let (intent_event, submitted) =
                submit_intent(&meeting, &meeting.agent, "Deferral shape basis").await;
            let intent_id = accepted_id(&submitted);
            let before = submitted.snapshot;
            sqlx::query(
                "UPDATE meeting_baton_state \
                 SET moderator_decision_deadline = statement_timestamp() - interval '1 second', \
                     next_action_at = statement_timestamp() - interval '1 second' \
                 WHERE community_id = $1 AND session_id = $2",
            )
            .bind(meeting.community_id.as_uuid())
            .bind(meeting.session_id)
            .execute(&meeting.db.pool)
            .await
            .expect("force due moderator window before invalid deferrals");
            let select = signed_event(
                &meeting.moderator,
                buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
                meeting.session_id,
                "",
                &[],
            );
            let source = if source_mode == "participant_intent" {
                BatonSelectionSource::Intent {
                    intent_id: intent_id.clone(),
                }
            } else {
                BatonSelectionSource::Handoff {
                    handoff_id: vec![0x8a; 32],
                    expected_attempt_count: 0,
                }
            };
            let error = execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &select,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::ModeratorSelect {
                        source,
                        expected_control_epoch: before.control_epoch,
                        expected_decision_epoch: before.decision_epoch,
                        expected_intent_revision: before.intent_revision,
                        expected_speech_revision: before.speech_revision,
                        selection_reason: None,
                        deferrals: vec![BatonIntentDeferral {
                            intent_id,
                            previous_event_id: intent_event.id.as_bytes().to_vec(),
                            reason: "invalid for this selected source".to_string(),
                        }],
                        attempt_id: None,
                        expected_source_event_id: None,
                    },
                },
            )
            .await
            .expect_err("stable invalid Select deferrals must not reach apply");
            assert!(matches!(error, DbError::InvalidData(_)));
            assert_command_not_persisted(&meeting, &select).await;
            assert_eq!(
                latest_transition(&meeting).await["primary_type"],
                "moderator_fallback"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn durable_actor_revocation_hides_receipts_after_rapid_restore() {
        let meeting = create_test_meeting().await;
        let (intent_event, submitted) =
            submit_intent(&meeting, &meeting.agent, "Receipt must become inaccessible").await;
        assert!(matches!(
            submitted.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        ));
        crate::moderation::ban_member_with_revocation(
            &meeting.db.pool,
            meeting.community_id,
            meeting.agent.public_key().as_bytes(),
            meeting.moderator.public_key().as_bytes(),
            Some("durable Meeting revocation"),
            None,
            &[0xc1; 32],
        )
        .await
        .expect("ban Meeting actor with durable cleanup");
        assert!(crate::moderation::unban_member(
            &meeting.db.pool,
            meeting.community_id,
            meeting.agent.public_key().as_bytes(),
            meeting.moderator.public_key().as_bytes(),
        )
        .await
        .expect("rapidly restore Meeting actor"));
        assert!(
            crate::meeting::is_meeting_actor_security_active(
                &meeting.db,
                meeting.community_id,
                meeting.agent.public_key().as_bytes(),
            )
            .await
            .expect("check restored current actor security"),
            "rapid restore makes the global/current gate active again"
        );

        let replay_error = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &intent_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentSubmit {
                    basis_speech_revision: 0,
                    summary: "Receipt must become inaccessible".to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect_err("durably revoked actor cannot replay its accepted receipt");
        assert!(matches!(replay_error, DbError::AccessDenied(_)));
        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("read lazy revocation terminal State");
        assert_eq!(snapshot.phase, BatonPhase::Ended);
        assert_eq!(
            latest_transition(&meeting).await["primary_type"],
            "participant_revoked"
        );

        let new_event = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            meeting.session_id,
            "Still fenced",
            &[],
        );
        let ended_error = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &new_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentSubmit {
                    basis_speech_revision: snapshot.speech_revision,
                    summary: "Still fenced".to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect_err("durable fence survives terminal Session replay");
        assert!(matches!(ended_error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&meeting, &new_event).await;
        assert!(!crate::meeting::is_meeting_actor_session_security_active(
            &meeting.db,
            meeting.community_id,
            meeting.session_id,
            meeting.agent.public_key().as_bytes(),
        )
        .await
        .expect("check Session-relative durable gate"));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn owner_deactivated_agent_commits_recovery_without_receipt_or_snapshot() {
        let meeting = create_test_meeting().await;
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.moderator.public_key().as_bytes())
        .execute(&meeting.db.pool)
        .await
        .expect("deactivate authoritative Agent owner without a durable job");
        let command = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_SPEECH_INTENT,
            meeting.session_id,
            "Must not receive the recovery snapshot",
            &[],
        );
        let error = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &command,
                relay_keys: &meeting.relay,
                command: BatonCommand::IntentSubmit {
                    basis_speech_revision: 0,
                    summary: "Must not receive the recovery snapshot".to_string(),
                    addressed_to: None,
                },
            },
        )
        .await
        .expect_err("owner-deactivated Agent must not receive a recovery result");
        assert!(matches!(error, DbError::AccessDenied(_)));
        assert_command_not_persisted(&meeting, &command).await;
        let snapshot = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("read terminal owner-deactivation State out of band");
        assert_eq!(snapshot.phase, BatonPhase::Ended);
        assert_eq!(
            latest_transition(&meeting).await["primary_type"],
            "participant_revoked"
        );
        let durable_jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_revocation_jobs \
             WHERE community_id = $1 AND revoked_pubkey IN ($2, $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.moderator.public_key().as_bytes())
        .bind(meeting.agent.public_key().as_bytes())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("confirm owner deactivation used current-state lazy recovery");
        assert_eq!(durable_jobs, 0);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn human_preemption_recall_and_directed_handoff_follow_priority() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Preempt me").await;
        let preempted_intent_id = accepted_id(&submitted);
        let (_, selected) = moderator_select_intent(&meeting, preempted_intent_id.clone()).await;
        let preempted_offer_id = accepted_id(&selected);
        let request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("Human preempts Agent Offer");
        assert_eq!(requested.snapshot.phase, BatonPhase::Offered);
        assert_ne!(
            requested.snapshot.active_offer_id.as_deref(),
            Some(preempted_offer_id.as_slice())
        );
        let recall = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "return after Human",
            &[],
        );
        let recalled = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &recall,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorRecall {
                    control_epoch: requested.snapshot.control_epoch,
                    reason: Some("return after Human".to_string()),
                },
            },
        )
        .await
        .expect("latch Recall during Human Offer");
        assert!(recalled.snapshot.forced_return_to_moderator);

        let human_offer_id = recalled
            .snapshot
            .active_offer_id
            .clone()
            .expect("Human Offer remains active");
        let ack = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: human_offer_id,
                },
            },
        )
        .await
        .expect("ACK Human Offer");
        let yield_event = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
            meeting.session_id,
            "done",
            &[],
        );
        let yielded = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &yield_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::GrantYield {
                    grant_id: accepted_id(&acked),
                    reason_code: Some("cancelled".to_string()),
                    reason: Some("done".to_string()),
                },
            },
        )
        .await
        .expect("Yield Human Grant");
        assert!(!yielded.snapshot.forced_return_to_moderator);
        assert!(matches!(
            yielded.snapshot.phase,
            BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
        ));
        let yielded_transition = latest_transition(&meeting).await;
        let yielded_effects = yielded_transition["effects"]
            .as_array()
            .expect("forced Human Yield transition effects");
        assert!(yielded_effects.iter().any(|effect| {
            effect["type"] == "recall_cleared"
                && effect["object_id"] == hex::encode(recall.id.as_bytes())
        }));
        assert!(yielded_effects
            .iter()
            .any(|effect| effect["type"] == "forced_return_completed"));
        assert!(yielded_effects
            .iter()
            .all(|effect| effect["type"] != "control_returned"));

        let (_, selected) = moderator_select_intent(&meeting, preempted_intent_id).await;
        let offer = accepted_id(&selected);
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck { offer_id: offer },
            },
        )
        .await
        .expect("ACK Agent Offer");
        let human_pubkey = meeting.human.public_key().to_bytes().to_vec();
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Can you confirm?",
            std::slice::from_ref(&human_pubkey),
        );
        let handed_off = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id: accepted_id(&acked),
                    speech_revision: 1,
                    handoff: Some(BatonHandoffInput {
                        to_pubkey: human_pubkey,
                        reason_type: "question".to_string(),
                        reason_text: "Please confirm the observed result".to_string(),
                    }),
                },
            },
        )
        .await
        .expect("speech creates Directed Handoff Offer");
        assert_eq!(handed_off.snapshot.phase, BatonPhase::Offered);
        let handoff_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 \
               AND source_speech_event_id = $3 AND question_state = 'open'",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(speech.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count open Directed Handoff");
        assert_eq!(handoff_count, 1);

        let request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("Human Request preempts Directed Handoff Offer");
        let requested_transition = latest_transition(&meeting).await;
        let requested_effects = requested_transition["effects"]
            .as_array()
            .expect("Human Request transition effects");
        assert!(
            requested_effects
                .iter()
                .all(|effect| effect["type"] != "phase_changed"),
            "offered-to-offered preemption must not publish a false phase change"
        );
        assert!(requested_effects
            .iter()
            .any(|effect| effect["type"] == "human_offered"));

        let withdraw = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &withdraw,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanWithdraw {
                    request_id: accepted_id(&requested),
                },
            },
        )
        .await
        .expect("withdraw active Human Offer");
        let withdrawn_transition = latest_transition(&meeting).await;
        assert_eq!(
            withdrawn_transition["effects"]
                .as_array()
                .expect("Human Withdraw transition effects")
                .iter()
                .filter(|effect| effect["type"] == "human_withdrawn")
                .count(),
            1,
            "active Human withdrawal has one canonical request effect"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn human_priority_release_unblocks_directed_handoff_for_moderator_selection() {
        let meeting = create_test_meeting().await;
        let (_, agent_grant_id) = create_agent_grant(&meeting, "Speak while a Human waits").await;

        let request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let requested = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("queue Human Request during Agent Grant");
        assert_eq!(requested.snapshot.phase, BatonPhase::Granted);
        let request_id = accepted_id(&requested);

        let target_pubkey = meeting.human_two.public_key().to_bytes().to_vec();
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Human priority should delay, not strand, this question",
            std::slice::from_ref(&target_pubkey),
        );
        let spoken = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id: agent_grant_id,
                    speech_revision: 1,
                    handoff: Some(BatonHandoffInput {
                        to_pubkey: target_pubkey.clone(),
                        reason_type: "question".to_string(),
                        reason_text: "Answer after Human priority completes".to_string(),
                    }),
                },
            },
        )
        .await
        .expect("Agent speech creates Human-blocked Directed Handoff");
        assert_eq!(spoken.snapshot.phase, BatonPhase::Offered);
        let handoff_id = speech.id.as_bytes().to_vec();
        let (initial_disposition, blocked_by): (String, Option<String>) = sqlx::query_as(
            "SELECT initial_disposition, blocked_by \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load Human-blocked Directed Handoff");
        assert_eq!(initial_disposition, "blocked");
        assert_eq!(blocked_by.as_deref(), Some("human_request"));

        let withdraw = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let withdrawn = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &withdraw,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanWithdraw { request_id },
            },
        )
        .await
        .expect("withdraw Human priority Offer");
        assert!(matches!(
            withdrawn.snapshot.phase,
            BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
        ));

        let (question_state, initial_disposition, blocked_by): (String, String, Option<String>) =
            sqlx::query_as(
                "SELECT question_state, initial_disposition, blocked_by \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
            )
            .bind(meeting.community_id.as_uuid())
            .bind(meeting.session_id)
            .bind(&handoff_id)
            .fetch_one(&meeting.db.pool)
            .await
            .expect("load released Directed Handoff");
        assert_eq!(question_state, "open");
        assert_eq!(initial_disposition, "blocked");
        assert!(blocked_by.is_none());

        let released_state = latest_state_content(&meeting).await;
        let released_handoff = released_state["unresolved_handoffs"]
            .as_array()
            .expect("released State unresolved Handoffs")
            .iter()
            .find(|handoff| handoff["handoff_id"] == hex::encode(&handoff_id))
            .expect("released Handoff remains canonical");
        assert!(released_handoff["blocked_by"].is_null());
        let released_effects = released_state["transition"]["effects"]
            .as_array()
            .expect("Human release effects");
        assert!(released_effects.iter().any(|value| {
            value["type"] == "handoff_unblocked"
                && value["object_id"] == hex::encode(&handoff_id)
                && value["from"] == "human_request"
                && value["to"].is_null()
        }));

        let select = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "resume the delayed question",
            &[],
        );
        let selected = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &select,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Handoff {
                        handoff_id: handoff_id.clone(),
                        expected_attempt_count: 0,
                    },
                    expected_control_epoch: withdrawn.snapshot.control_epoch,
                    expected_decision_epoch: withdrawn.snapshot.decision_epoch,
                    expected_intent_revision: withdrawn.snapshot.intent_revision,
                    expected_speech_revision: withdrawn.snapshot.speech_revision,
                    selection_reason: Some(
                        "Human priority completed; resume the directed question".to_string(),
                    ),
                    deferrals: Vec::new(),
                    attempt_id: None,
                    expected_source_event_id: None,
                },
            },
        )
        .await
        .expect("moderator selects released Directed Handoff");
        assert_eq!(selected.snapshot.phase, BatonPhase::Offered);
        let offer_id = accepted_id(&selected);
        let (allocation_source, target, source_handoff_id): (String, Vec<u8>, Option<Vec<u8>>) =
            sqlx::query_as(
                "SELECT allocation_source, target_pubkey, source_handoff_id \
             FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
            )
            .bind(meeting.community_id.as_uuid())
            .bind(meeting.session_id)
            .bind(offer_id)
            .fetch_one(&meeting.db.pool)
            .await
            .expect("load selected Handoff Offer");
        assert_eq!(allocation_source, "moderator_select");
        assert_eq!(target, target_pubkey);
        assert_eq!(source_handoff_id.as_deref(), Some(handoff_id.as_slice()));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn directed_handoff_decline_reselect_and_max_depth_chain_are_canonical() {
        let meeting = create_test_meeting().await;
        let (_, setup_grant_id) =
            create_agent_grant(&meeting, "Start the Directed Handoff chain").await;
        let (_, setup_handoff) = submit_handoff_speech(
            &meeting,
            &meeting.agent,
            setup_grant_id,
            1,
            &meeting.human,
            "Pass the setup turn to the Human",
        )
        .await;
        let setup_offer_id = setup_handoff
            .snapshot
            .active_offer_id
            .expect("setup Handoff creates a Human Offer");
        let setup_ack = ack_test_offer(&meeting, &meeting.human, setup_offer_id).await;
        let (initial_speech, initial_handoff) = submit_handoff_speech(
            &meeting,
            &meeting.human,
            accepted_id(&setup_ack),
            2,
            &meeting.agent,
            "Initial question for the Agent",
        )
        .await;
        let initial_handoff_id = initial_speech.id.as_bytes().to_vec();
        let first_offer_id = initial_handoff
            .snapshot
            .active_offer_id
            .clone()
            .expect("initial Handoff creates an Offer");
        assert_eq!(initial_handoff.snapshot.phase, BatonPhase::Offered);
        let agent_pubkey = hex::encode(meeting.agent.public_key().to_bytes());
        let initial_state = latest_state_content(&meeting).await;
        assert_eq!(initial_state["offer"]["target_pubkey"], agent_pubkey);
        assert_eq!(initial_state["offer"]["target_participant_type"], "agent");
        let (
            initial_question_state,
            initial_attempt_outcome,
            initial_attempt_count,
            initial_last_offer_id,
        ): (String, Option<String>, i32, Option<Vec<u8>>) = sqlx::query_as(
            "SELECT question_state, last_attempt_outcome, attempt_count, last_offer_id \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&initial_handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load initial Directed Handoff attempt");
        assert_eq!(initial_question_state, "open");
        assert_eq!(initial_attempt_outcome.as_deref(), Some("offered"));
        assert_eq!(initial_attempt_count, 1);
        assert_eq!(
            initial_last_offer_id.as_deref(),
            Some(first_offer_id.as_slice())
        );

        let decline = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let declined = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &decline,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: first_offer_id.clone(),
                    reason: Some("not ready yet".to_string()),
                },
            },
        )
        .await
        .expect("Directed Handoff target declines its Offer");
        assert!(matches!(
            declined.snapshot.phase,
            BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
        ));
        assert!(declined.snapshot.active_offer_id.is_none());
        assert!(declined.snapshot.active_grant_id.is_none());
        assert_eq!(declined.snapshot.handoff_depth, 0);
        let (declined_state, declined_outcome, declined_attempt_count): (
            String,
            Option<String>,
            i32,
        ) = sqlx::query_as(
            "SELECT question_state, last_attempt_outcome, attempt_count \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&initial_handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load declined Directed Handoff attempt");
        assert_eq!(declined_state, "open");
        assert_eq!(declined_outcome.as_deref(), Some("declined"));
        assert_eq!(declined_attempt_count, 1);
        let declined_canonical_state = latest_state_content(&meeting).await;
        let declined_unresolved = declined_canonical_state["unresolved_handoffs"]
            .as_array()
            .expect("declined State unresolved Handoffs");
        assert_eq!(declined_unresolved.len(), 1);
        assert_eq!(
            declined_unresolved[0]["handoff_id"],
            hex::encode(&initial_handoff_id)
        );
        assert_eq!(declined_unresolved[0]["attempt_count"], 1);
        assert_eq!(declined_unresolved[0]["last_attempt_outcome"], "declined");
        let decline_transition = &declined_canonical_state["transition"];
        assert_eq!(decline_transition["primary_type"], "offer_declined");
        let decline_effects = decline_transition["effects"]
            .as_array()
            .expect("decline canonical effects");
        assert!(decline_effects.iter().any(|effect| {
            effect["type"] == "offer_declined"
                && effect["object_id"] == hex::encode(&first_offer_id)
        }));
        assert!(decline_effects.iter().any(|effect| {
            effect["type"] == "handoff_attempt_failed"
                && effect["object_id"] == hex::encode(&initial_handoff_id)
        }));

        let reselect_event = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "retry the open question",
            &[],
        );
        let reselected = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &reselect_event,
                relay_keys: &meeting.relay,
                command: BatonCommand::ModeratorSelect {
                    source: BatonSelectionSource::Handoff {
                        handoff_id: initial_handoff_id.clone(),
                        expected_attempt_count: 1,
                    },
                    expected_control_epoch: declined.snapshot.control_epoch,
                    expected_decision_epoch: declined.snapshot.decision_epoch,
                    expected_intent_revision: declined.snapshot.intent_revision,
                    expected_speech_revision: declined.snapshot.speech_revision,
                    selection_reason: Some("retry the still-open question".to_string()),
                    deferrals: Vec::new(),
                    attempt_id: None,
                    expected_source_event_id: None,
                },
            },
        )
        .await
        .expect("moderator re-selects the declined open Handoff");
        let second_offer_id = accepted_id(&reselected);
        assert_ne!(second_offer_id, first_offer_id);
        assert_eq!(reselected.snapshot.phase, BatonPhase::Offered);
        assert_eq!(
            reselected.snapshot.active_offer_id.as_deref(),
            Some(second_offer_id.as_slice())
        );
        let (reselected_attempt_count, reselected_outcome, reselected_last_offer): (
            i32,
            Option<String>,
            Option<Vec<u8>>,
        ) = sqlx::query_as(
            "SELECT attempt_count, last_attempt_outcome, last_offer_id \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&initial_handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load re-selected Directed Handoff");
        assert_eq!(reselected_attempt_count, 2);
        assert_eq!(reselected_outcome.as_deref(), Some("offered"));
        assert_eq!(
            reselected_last_offer.as_deref(),
            Some(second_offer_id.as_slice())
        );
        let reselected_state = latest_state_content(&meeting).await;
        assert_eq!(
            reselected_state["offer"]["offer_id"],
            hex::encode(&second_offer_id)
        );
        assert_eq!(
            reselected_state["offer"]["source_handoff_id"],
            hex::encode(&initial_handoff_id)
        );
        assert_eq!(reselected_state["offer"]["target_pubkey"], agent_pubkey);
        assert_eq!(
            reselected_state["offer"]["target_participant_type"],
            "agent"
        );
        let reselect_effects = reselected_state["transition"]["effects"]
            .as_array()
            .expect("re-select canonical effects");
        assert!(reselect_effects.iter().any(|effect| {
            effect["type"] == "handoff_attempted"
                && effect["object_id"] == hex::encode(&initial_handoff_id)
        }));
        assert!(reselect_effects.iter().any(|effect| {
            effect["type"] == "offer_created"
                && effect["object_id"] == hex::encode(&second_offer_id)
        }));

        let reselected_ack =
            ack_test_offer(&meeting, &meeting.agent, second_offer_id.clone()).await;
        let mut grant_id = accepted_id(&reselected_ack);
        assert_eq!(reselected_ack.snapshot.handoff_depth, 0);
        assert_eq!(
            reselected_ack.snapshot.active_grant_id.as_deref(),
            Some(grant_id.as_slice())
        );
        let mut source_handoff_id = initial_handoff_id;

        for hop in 1_i64..=5 {
            let (speaker, target) = if hop % 2 == 1 {
                (&meeting.agent, &meeting.human)
            } else {
                (&meeting.human, &meeting.agent)
            };
            let (speech, handed_off) = submit_handoff_speech(
                &meeting,
                speaker,
                grant_id,
                hop + 2,
                target,
                &format!("Direct Handoff hop {hop}"),
            )
            .await;
            let handoff_id = speech.id.as_bytes().to_vec();
            let offer_id = handed_off
                .snapshot
                .active_offer_id
                .clone()
                .expect("each of the first five direct Handoffs creates an Offer");
            assert_eq!(handed_off.snapshot.phase, BatonPhase::Offered);
            assert!(handed_off.snapshot.active_grant_id.is_none());
            let speech_transition = latest_transition(&meeting).await;
            assert_eq!(speech_transition["primary_type"], "speech_accepted");
            let speech_effects = speech_transition["effects"]
                .as_array()
                .expect("direct Handoff speech effects");
            assert!(speech_effects.iter().any(|effect| {
                effect["type"] == "handoff_answered"
                    && effect["object_id"] == hex::encode(&source_handoff_id)
            }));
            assert!(speech_effects.iter().any(|effect| {
                effect["type"] == "handoff_created"
                    && effect["object_id"] == hex::encode(&handoff_id)
            }));
            assert!(speech_effects.iter().any(|effect| {
                effect["type"] == "offer_created" && effect["object_id"] == hex::encode(&offer_id)
            }));

            let acked = ack_test_offer(&meeting, target, offer_id).await;
            let next_grant_id = accepted_id(&acked);
            assert_eq!(
                acked.snapshot.handoff_depth,
                i32::try_from(hop).expect("test hop fits i32")
            );
            assert_eq!(
                acked.snapshot.active_grant_id.as_deref(),
                Some(next_grant_id.as_slice())
            );
            grant_id = next_grant_id;
            source_handoff_id = handoff_id;
        }

        let depth_five = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load depth-five Grant");
        assert_eq!(
            depth_five.handoff_depth,
            depth_five.config.max_handoff_depth
        );
        assert_eq!(depth_five.handoff_depth, 5);
        assert_eq!(depth_five.speech_revision, 7);
        let (blocked_speech, returned) = submit_handoff_speech(
            &meeting,
            &meeting.human,
            grant_id,
            8,
            &meeting.agent,
            "Depth-five target speaks and proposes a sixth Handoff",
        )
        .await;
        let blocked_handoff_id = blocked_speech.id.as_bytes().to_vec();
        assert!(matches!(
            returned.snapshot.phase,
            BatonPhase::ModeratorIdle | BatonPhase::ModeratorControl
        ));
        assert!(returned.snapshot.active_offer_id.is_none());
        assert!(returned.snapshot.active_grant_id.is_none());
        assert_eq!(returned.snapshot.handoff_depth, 0);
        assert_eq!(returned.snapshot.speech_revision, 8);
        assert_eq!(
            returned.snapshot.control_epoch,
            depth_five.control_epoch + 1
        );

        let blocked_handoff = sqlx::query(
            "SELECT question_state, initial_disposition, blocked_by, requested_depth, \
                    attempt_count, last_offer_id, last_grant_id, last_attempt_outcome \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 AND handoff_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&blocked_handoff_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load max-depth-blocked Handoff");
        assert_eq!(
            blocked_handoff
                .try_get::<String, _>("question_state")
                .expect("blocked Handoff question_state"),
            "open"
        );
        assert_eq!(
            blocked_handoff
                .try_get::<String, _>("initial_disposition")
                .expect("blocked Handoff initial_disposition"),
            "blocked"
        );
        assert_eq!(
            blocked_handoff
                .try_get::<Option<String>, _>("blocked_by")
                .expect("blocked Handoff blocked_by")
                .as_deref(),
            Some("max_depth")
        );
        assert_eq!(
            blocked_handoff
                .try_get::<i32, _>("requested_depth")
                .expect("blocked Handoff requested_depth"),
            6
        );
        assert_eq!(
            blocked_handoff
                .try_get::<i32, _>("attempt_count")
                .expect("blocked Handoff attempt_count"),
            0
        );
        assert!(blocked_handoff
            .try_get::<Option<Vec<u8>>, _>("last_offer_id")
            .expect("blocked Handoff last_offer_id")
            .is_none());
        assert!(blocked_handoff
            .try_get::<Option<Vec<u8>>, _>("last_grant_id")
            .expect("blocked Handoff last_grant_id")
            .is_none());
        assert!(blocked_handoff
            .try_get::<Option<String>, _>("last_attempt_outcome")
            .expect("blocked Handoff last_attempt_outcome")
            .is_none());

        let (pending_offers, active_grants): (i64, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT count(*) FROM meeting_baton_offers \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'pending'), \
                 (SELECT count(*) FROM meeting_baton_grants \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'active')",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("confirm max-depth return has no active Offer or Grant");
        assert_eq!((pending_offers, active_grants), (0, 0));

        let final_state = latest_state_content(&meeting).await;
        assert!(final_state["offer"].is_null());
        assert!(final_state["grant"].is_null());
        assert_eq!(final_state["handoff_depth"], 0);
        let unresolved = final_state["unresolved_handoffs"]
            .as_array()
            .expect("final State unresolved Handoffs");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(
            unresolved[0]["handoff_id"],
            hex::encode(&blocked_handoff_id)
        );
        assert_eq!(unresolved[0]["question_state"], "open");
        assert_eq!(unresolved[0]["blocked_by"], "max_depth");
        assert_eq!(unresolved[0]["attempt_count"], 0);
        assert!(unresolved[0]["last_offer_id"].is_null());
        assert!(unresolved[0]["last_grant_id"].is_null());
        let final_transition = &final_state["transition"];
        assert_eq!(final_transition["primary_type"], "speech_accepted");
        assert_eq!(
            final_transition["primary_object_id"],
            hex::encode(&blocked_handoff_id)
        );
        let final_effects = final_transition["effects"]
            .as_array()
            .expect("max-depth speech effects");
        assert!(final_effects.iter().any(|effect| {
            effect["type"] == "handoff_answered"
                && effect["object_id"] == hex::encode(&source_handoff_id)
        }));
        assert!(final_effects.iter().any(|effect| {
            effect["type"] == "handoff_created"
                && effect["object_id"] == hex::encode(&blocked_handoff_id)
        }));
        assert!(final_effects
            .iter()
            .any(|effect| effect["type"] == "forced_return_completed"));
        assert!(final_effects
            .iter()
            .all(|effect| effect["type"] != "offer_created"));

        let canonical = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(canonical.state_event_id, returned.snapshot.state_event_id);
        assert_eq!(canonical.state_revision, returned.snapshot.state_revision);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn latched_recall_clears_only_when_human_priority_is_exhausted() {
        for terminal_mode in ["withdraw", "timeout"] {
            let meeting = create_test_meeting().await;
            let (request_id, offer_id, recall) =
                create_recalled_human_offer(&meeting, &meeting.human).await;
            if terminal_mode == "withdraw" {
                let withdraw = signed_event(
                    &meeting.human,
                    buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
                    meeting.session_id,
                    "",
                    &[],
                );
                execute_baton_command(
                    &meeting.db,
                    BatonCommandTxParams {
                        community_id: meeting.community_id,
                        session_id: meeting.session_id,
                        event: &withdraw,
                        relay_keys: &meeting.relay,
                        command: BatonCommand::HumanWithdraw { request_id },
                    },
                )
                .await
                .expect("withdraw recalled Human Offer");
            } else {
                sqlx::query(
                    "UPDATE meeting_baton_offers \
                     SET ack_deadline = clock_timestamp() - interval '1 second' \
                     WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
                )
                .bind(meeting.community_id.as_uuid())
                .bind(meeting.session_id)
                .bind(offer_id)
                .execute(&meeting.db.pool)
                .await
                .expect("force recalled Human Offer timeout");
                let transitions = recover_meeting_v1(
                    &meeting.db,
                    meeting.community_id,
                    meeting.session_id,
                    &meeting.relay,
                )
                .await
                .expect("recover recalled Human Offer timeout");
                assert_eq!(transitions.len(), 1);
            }
            let snapshot =
                get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
                    .await
                    .expect("read post-Human-Offer Recall snapshot");
            assert!(!snapshot.forced_return_to_moderator);
            assert_recall_completed(&latest_transition(&meeting).await, &recall);
        }

        let queued = create_test_meeting().await;
        let (_, first_offer_id, recall) = create_recalled_human_offer(&queued, &queued.human).await;
        let second_request = signed_event(
            &queued.human_two,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            queued.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &queued.db,
            BatonCommandTxParams {
                community_id: queued.community_id,
                session_id: queued.session_id,
                event: &second_request,
                relay_keys: &queued.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("queue second Human behind recalled Offer");
        let first_decline = signed_event(
            &queued.human,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            queued.session_id,
            "",
            &[],
        );
        let advanced = execute_baton_command(
            &queued.db,
            BatonCommandTxParams {
                community_id: queued.community_id,
                session_id: queued.session_id,
                event: &first_decline,
                relay_keys: &queued.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: first_offer_id,
                    reason: None,
                },
            },
        )
        .await
        .expect("decline first Human while another Human is queued");
        assert!(advanced.snapshot.forced_return_to_moderator);
        let retained_transition = latest_transition(&queued).await;
        let retained_effects = retained_transition["effects"]
            .as_array()
            .expect("queued Human transition effects");
        assert!(retained_effects
            .iter()
            .all(|effect| effect["type"] != "recall_cleared"));
        assert!(retained_effects
            .iter()
            .all(|effect| effect["type"] != "forced_return_completed"));
        let second_offer_id = advanced
            .snapshot
            .active_offer_id
            .expect("queued Human receives the next Offer");
        let second_decline = signed_event(
            &queued.human_two,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            queued.session_id,
            "",
            &[],
        );
        let completed = execute_baton_command(
            &queued.db,
            BatonCommandTxParams {
                community_id: queued.community_id,
                session_id: queued.session_id,
                event: &second_decline,
                relay_keys: &queued.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: second_offer_id,
                    reason: None,
                },
            },
        )
        .await
        .expect("decline final queued Human Offer");
        assert!(!completed.snapshot.forced_return_to_moderator);
        assert_recall_completed(&latest_transition(&queued).await, &recall);

        for terminal_mode in ["speech", "soft_timeout", "hard_timeout"] {
            let meeting = create_test_meeting().await;
            let (_, offer_id, recall) = create_recalled_human_offer(&meeting, &meeting.human).await;
            let grant_id = ack_human_offer(&meeting, &meeting.human, offer_id).await;
            match terminal_mode {
                "speech" => {
                    let speech =
                        signed_event(&meeting.human, 9, meeting.session_id, "Human answer", &[]);
                    execute_baton_command(
                        &meeting.db,
                        BatonCommandTxParams {
                            community_id: meeting.community_id,
                            session_id: meeting.session_id,
                            event: &speech,
                            relay_keys: &meeting.relay,
                            command: BatonCommand::Speech {
                                grant_id,
                                speech_revision: 1,
                                handoff: None,
                            },
                        },
                    )
                    .await
                    .expect("complete recalled Human Grant with SAY");
                }
                "soft_timeout" => {
                    sqlx::query(
                        "UPDATE meeting_baton_grants \
                         SET soft_lease_expires_at = clock_timestamp() - interval '1 second', \
                             hard_deadline = clock_timestamp() + interval '1 minute' \
                         WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
                    )
                    .bind(meeting.community_id.as_uuid())
                    .bind(meeting.session_id)
                    .bind(grant_id)
                    .execute(&meeting.db.pool)
                    .await
                    .expect("force recalled Human Grant soft timeout");
                    recover_meeting_v1(
                        &meeting.db,
                        meeting.community_id,
                        meeting.session_id,
                        &meeting.relay,
                    )
                    .await
                    .expect("recover recalled Human Grant soft timeout");
                }
                "hard_timeout" => {
                    sqlx::query(
                        "UPDATE meeting_baton_grants \
                         SET soft_lease_expires_at = clock_timestamp() - interval '2 seconds', \
                             hard_deadline = clock_timestamp() - interval '1 second' \
                         WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
                    )
                    .bind(meeting.community_id.as_uuid())
                    .bind(meeting.session_id)
                    .bind(grant_id)
                    .execute(&meeting.db.pool)
                    .await
                    .expect("force recalled Human Grant hard timeout");
                    recover_meeting_v1(
                        &meeting.db,
                        meeting.community_id,
                        meeting.session_id,
                        &meeting.relay,
                    )
                    .await
                    .expect("recover recalled Human Grant hard timeout");
                }
                _ => unreachable!("closed terminal-mode test table"),
            }
            let snapshot =
                get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
                    .await
                    .expect("read post-Human-Grant Recall snapshot");
            assert!(!snapshot.forced_return_to_moderator);
            assert_recall_completed(&latest_transition(&meeting).await, &recall);
        }
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn deferral_release_effects_use_intent_ids_and_canonical_order() {
        for terminal_mode in ["speech", "yield"] {
            let meeting = create_test_meeting().await;
            let (deferred_event, deferred) =
                submit_intent(&meeting, &meeting.agent, "Deferred contribution").await;
            let deferred_id = accepted_id(&deferred);
            let (_, moderator_intent) =
                submit_intent(&meeting, &meeting.moderator, "Moderator contribution").await;
            let moderator_intent_id = accepted_id(&moderator_intent);
            let snapshot =
                get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
                    .await
                    .expect("read pre-deferral Select snapshot");
            let select = signed_event(
                &meeting.moderator,
                buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
                meeting.session_id,
                "",
                &[],
            );
            let selected = execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &select,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::ModeratorSelect {
                        source: BatonSelectionSource::Intent {
                            intent_id: moderator_intent_id,
                        },
                        expected_control_epoch: snapshot.control_epoch,
                        expected_decision_epoch: snapshot.decision_epoch,
                        expected_intent_revision: snapshot.intent_revision,
                        expected_speech_revision: snapshot.speech_revision,
                        selection_reason: Some("moderator priority".to_string()),
                        deferrals: vec![BatonIntentDeferral {
                            intent_id: deferred_id.clone(),
                            previous_event_id: deferred_event.id.as_bytes().to_vec(),
                            reason: "moderator needs to frame the discussion".to_string(),
                        }],
                        attempt_id: None,
                        expected_source_event_id: None,
                    },
                },
            )
            .await
            .expect("Select moderator with Intent deferral");
            let ack = signed_event(
                &meeting.moderator,
                buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
                meeting.session_id,
                "",
                &[],
            );
            let acked = execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &ack,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::OfferAck {
                        offer_id: accepted_id(&selected),
                    },
                },
            )
            .await
            .expect("ACK moderator Offer");
            let grant_id = accepted_id(&acked);
            if terminal_mode == "speech" {
                let speech = signed_event(
                    &meeting.moderator,
                    9,
                    meeting.session_id,
                    "Moderator framing",
                    &[],
                );
                execute_baton_command(
                    &meeting.db,
                    BatonCommandTxParams {
                        community_id: meeting.community_id,
                        session_id: meeting.session_id,
                        event: &speech,
                        relay_keys: &meeting.relay,
                        command: BatonCommand::Speech {
                            grant_id,
                            speech_revision: 1,
                            handoff: None,
                        },
                    },
                )
                .await
                .expect("speak and release deferrals");
            } else {
                let yielded = signed_event(
                    &meeting.moderator,
                    buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
                    meeting.session_id,
                    "",
                    &[],
                );
                execute_baton_command(
                    &meeting.db,
                    BatonCommandTxParams {
                        community_id: meeting.community_id,
                        session_id: meeting.session_id,
                        event: &yielded,
                        relay_keys: &meeting.relay,
                        command: BatonCommand::GrantYield {
                            grant_id,
                            reason_code: None,
                            reason: None,
                        },
                    },
                )
                .await
                .expect("Yield and release deferrals");
            }
            let transition = latest_transition(&meeting).await;
            let intent_effects: Vec<&Value> = transition["effects"]
                .as_array()
                .expect("terminal Grant effects")
                .iter()
                .filter(|effect| effect["object_type"] == "intent")
                .collect();
            assert!(intent_effects.iter().any(|effect| {
                effect["type"] == "intent_reactivated"
                    && effect["object_id"] == hex::encode(&deferred_id)
            }));
            let object_ids: Vec<&str> = intent_effects
                .iter()
                .map(|effect| {
                    effect["object_id"]
                        .as_str()
                        .expect("Intent effect object id")
                })
                .collect();
            assert!(
                object_ids.windows(2).all(|pair| pair[0] <= pair[1]),
                "Intent projection effects must be bytewise canonical"
            );
        }
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn end_lists_every_live_projection_in_canonical_effect_order() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Question setup").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let acked = execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &ack,
                relay_keys: &meeting.relay,
                command: BatonCommand::OfferAck {
                    offer_id: accepted_id(&selected),
                },
            },
        )
        .await
        .expect("ACK setup Offer");
        let human_pubkey = meeting.human.public_key().to_bytes().to_vec();
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Please answer this",
            std::slice::from_ref(&human_pubkey),
        );
        execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &speech,
                relay_keys: &meeting.relay,
                command: BatonCommand::Speech {
                    grant_id: accepted_id(&acked),
                    speech_revision: 1,
                    handoff: Some(BatonHandoffInput {
                        to_pubkey: human_pubkey,
                        reason_type: "question".to_string(),
                        reason_text: "Need the Human's answer".to_string(),
                    }),
                },
            },
        )
        .await
        .expect("create open Handoff and Offer");
        submit_intent(
            &meeting,
            &meeting.moderator,
            "Pending moderator observation",
        )
        .await;
        let request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &meeting.db,
            BatonCommandTxParams {
                community_id: meeting.community_id,
                session_id: meeting.session_id,
                event: &request,
                relay_keys: &meeting.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("create active Human Request Offer");

        let transition = end_test_meeting(&meeting).await;
        let effect_types: Vec<&str> = transition["effects"]
            .as_array()
            .expect("compound End effects")
            .iter()
            .map(|effect| effect["type"].as_str().expect("End effect type"))
            .collect();
        assert_eq!(
            effect_types,
            vec![
                "meeting_ended",
                "offer_ended",
                "intent_ended",
                "human_ended",
                "handoff_ended",
                "phase_changed",
            ]
        );
        let handoff_outcome: String = sqlx::query_scalar(
            "SELECT last_attempt_outcome \
             FROM meeting_directed_handoffs \
             WHERE community_id = $1 AND session_id = $2 \
               AND source_speech_event_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(speech.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load Ended Handoff outcome");
        assert_eq!(handoff_outcome, "ended");

        let granted = create_test_meeting().await;
        let (_, submitted) = submit_intent(&granted, &granted.agent, "Active Grant").await;
        let (_, selected) = moderator_select_intent(&granted, accepted_id(&submitted)).await;
        let ack = signed_event(
            &granted.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            granted.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &granted.db,
            BatonCommandTxParams {
                community_id: granted.community_id,
                session_id: granted.session_id,
                event: &ack,
                relay_keys: &granted.relay,
                command: BatonCommand::OfferAck {
                    offer_id: accepted_id(&selected),
                },
            },
        )
        .await
        .expect("create active Grant before End");
        let transition = end_test_meeting(&granted).await;
        let effect_types: Vec<&str> = transition["effects"]
            .as_array()
            .expect("Grant End effects")
            .iter()
            .map(|effect| effect["type"].as_str().expect("End effect type"))
            .collect();
        assert_eq!(
            effect_types,
            vec![
                "meeting_ended",
                "grant_ended",
                "intent_ended",
                "phase_changed",
            ]
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_ack_and_speech_each_commit_one_canonical_winner() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Race once").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let offer_id = accepted_id(&selected);
        let first_ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let second_ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let first_ack_command = BatonCommand::OfferAck {
            offer_id: offer_id.clone(),
        };
        let second_ack_command = BatonCommand::OfferAck { offer_id };
        let (first, second) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &first_ack,
                    relay_keys: &meeting.relay,
                    command: first_ack_command,
                },
            ),
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &second_ack,
                    relay_keys: &meeting.relay,
                    command: second_ack_command,
                },
            )
        );
        let ack_results = [
            first.expect("first concurrent ACK result"),
            second.expect("second concurrent ACK result"),
        ];
        assert_eq!(
            ack_results
                .iter()
                .filter(|result| matches!(
                    result.command_outcome,
                    BatonCommandOutcome::Accepted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            ack_results
                .iter()
                .filter(|result| matches!(
                    result.command_outcome,
                    BatonCommandOutcome::RejectedTerminal { .. }
                ))
                .count(),
            1
        );
        let grant_id = ack_results
            .iter()
            .find_map(|result| match &result.command_outcome {
                BatonCommandOutcome::Accepted {
                    canonical_object_id: Some(id),
                    ..
                } => Some(id.clone()),
                _ => None,
            })
            .expect("one ACK creates the Grant");

        let first_speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "First competing speech",
            &[],
        );
        let second_speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Second competing speech",
            &[],
        );
        let first_speech_command = BatonCommand::Speech {
            grant_id: grant_id.clone(),
            speech_revision: 1,
            handoff: None,
        };
        let second_speech_command = BatonCommand::Speech {
            grant_id,
            speech_revision: 1,
            handoff: None,
        };
        let (first, second) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &first_speech,
                    relay_keys: &meeting.relay,
                    command: first_speech_command,
                },
            ),
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &second_speech,
                    relay_keys: &meeting.relay,
                    command: second_speech_command,
                },
            )
        );
        let speech_results = [
            first.expect("first concurrent speech result"),
            second.expect("second concurrent speech result"),
        ];
        assert_eq!(
            speech_results
                .iter()
                .filter(|result| matches!(
                    result.command_outcome,
                    BatonCommandOutcome::Accepted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            speech_results
                .iter()
                .filter(|result| matches!(
                    result.command_outcome,
                    BatonCommandOutcome::RejectedTerminal { .. }
                ))
                .count(),
            1
        );
        let canonical_speech_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND channel_id = $2 AND kind = 9 \
               AND id IN ($3, $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(first_speech.id.as_bytes().as_slice())
        .bind(second_speech.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count concurrent canonical speeches");
        assert_eq!(canonical_speech_count, 1);
        let (history_count, max_revision): (i64, i64) = sqlx::query_as(
            "SELECT count(*), max(state_revision) \
             FROM meeting_baton_state_history \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check canonical State revision continuity");
        assert_eq!(history_count, max_revision);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_ack_and_human_request_have_one_serial_canonical_floor() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "ACK or Human priority").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let before_revision = selected.snapshot.state_revision;
        let offer_id = accepted_id(&selected);
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );
        let request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );

        let (ack_result, request_result) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &ack,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::OfferAck { offer_id },
                },
            ),
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &request,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::HumanRequest,
                },
            )
        );
        let ack_result = ack_result.expect("concurrent ACK result");
        let request_result = request_result.expect("concurrent Human Request result");
        assert!(matches!(
            &request_result.command_outcome,
            BatonCommandOutcome::Accepted {
                canonical_object_id: Some(id),
                ..
            } if id == request.id.as_bytes().as_slice()
        ));
        let ack_won = matches!(
            &ack_result.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        );
        if !ack_won {
            assert!(matches!(
                &ack_result.command_outcome,
                BatonCommandOutcome::RejectedTerminal { code, .. }
                    if code == "offer_not_active"
            ));
        }

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        let request_state: String = sqlx::query_scalar(
            "SELECT state FROM meeting_human_floor_requests \
             WHERE community_id = $1 AND session_id = $2 AND request_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(request.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load concurrent Human Request state");
        let (pending_offers, active_grants): (i64, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT count(*) FROM meeting_baton_offers \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'pending'), \
                 (SELECT count(*) FROM meeting_baton_grants \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'active')",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count active floor objects after ACK/Human race");
        if ack_won {
            assert_eq!(snapshot.phase, BatonPhase::Granted);
            assert_eq!(snapshot.state_revision, before_revision + 2);
            assert_eq!(request_state, "queued");
            assert_eq!((pending_offers, active_grants), (0, 1));
        } else {
            assert_eq!(snapshot.phase, BatonPhase::Offered);
            assert_eq!(snapshot.state_revision, before_revision + 1);
            assert_eq!(request_state, "offered");
            assert_eq!((pending_offers, active_grants), (1, 0));
        }
        let persisted_commands: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND id IN ($2, $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(ack.id.as_bytes().as_slice())
        .bind(request.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count canonical ACK/Human commands");
        assert_eq!(persisted_commands, if ack_won { 2 } else { 1 });
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_ack_and_offer_timeout_commit_exactly_one_recovery() {
        let meeting = create_test_meeting().await;
        let (_, submitted) =
            submit_intent(&meeting, &meeting.agent, "ACK at timeout boundary").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let before_revision = selected.snapshot.state_revision;
        let offer_id = accepted_id(&selected);
        sqlx::query(
            "UPDATE meeting_baton_offers \
             SET ack_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&offer_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force Offer deadline before concurrent recovery");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due State hint before concurrent Offer recovery");
        let ack = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            meeting.session_id,
            "",
            &[],
        );

        let (ack_result, recovered) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &ack,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::OfferAck {
                        offer_id: offer_id.clone(),
                    },
                },
            ),
            recover_meeting_v1(
                &meeting.db,
                meeting.community_id,
                meeting.session_id,
                &meeting.relay,
            )
        );
        let ack_result = ack_result.expect("concurrent late ACK result");
        let recovered = recovered.expect("concurrent Offer recovery result");
        assert!(matches!(
            &ack_result.command_outcome,
            BatonCommandOutcome::RejectedAfterRecovery { code, .. }
                | BatonCommandOutcome::RejectedTerminal { code, .. }
                if code == "offer_not_active"
        ));
        let recovery_types: Vec<_> = ack_result
            .recovery_transitions
            .iter()
            .chain(recovered.iter())
            .map(|transition| transition.primary_type.as_str())
            .collect();
        assert_eq!(recovery_types, vec!["offer_timed_out"]);

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.state_revision, before_revision + 1);
        assert!(snapshot.active_offer_id.is_none());
        let offer_state: String = sqlx::query_scalar(
            "SELECT state FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(offer_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load timed-out Offer");
        assert_eq!(offer_state, "timed_out");
        let ack_persisted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE community_id = $1 AND id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(ack.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check late ACK canonical log");
        assert!(!ack_persisted);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_speech_and_hard_deadline_commit_exactly_one_recovery() {
        let meeting = create_test_meeting().await;
        let (_, grant_id) = create_agent_grant(&meeting, "SAY at hard deadline").await;
        let before = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load Grant before hard-deadline race");
        sqlx::query(
            "UPDATE meeting_baton_grants \
             SET soft_lease_expires_at = clock_timestamp() - interval '2 seconds', \
                 hard_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&grant_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force hard Grant deadline before concurrent recovery");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due State hint before hard Grant recovery");
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Too late at the hard boundary",
            &[],
        );

        let (speech_result, recovered) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &speech,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::Speech {
                        grant_id: grant_id.clone(),
                        speech_revision: before.speech_revision + 1,
                        handoff: None,
                    },
                },
            ),
            recover_meeting_v1(
                &meeting.db,
                meeting.community_id,
                meeting.session_id,
                &meeting.relay,
            )
        );
        let speech_result = speech_result.expect("concurrent hard-deadline SAY result");
        let recovered = recovered.expect("concurrent hard Grant recovery result");
        assert!(matches!(
            &speech_result.command_outcome,
            BatonCommandOutcome::RejectedAfterRecovery { code, .. }
                | BatonCommandOutcome::RejectedTerminal { code, .. }
                if code == "grant_not_active"
        ));
        let recovery_types: Vec<_> = speech_result
            .recovery_transitions
            .iter()
            .chain(recovered.iter())
            .map(|transition| transition.primary_type.as_str())
            .collect();
        assert_eq!(recovery_types, vec!["grant_hard_expired"]);

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.state_revision, before.state_revision + 1);
        assert_eq!(snapshot.speech_revision, before.speech_revision);
        assert!(snapshot.active_grant_id.is_none());
        let grant_state: String = sqlx::query_scalar(
            "SELECT state FROM meeting_baton_grants \
             WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(grant_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load hard-expired Grant");
        assert_eq!(grant_state, "hard_expired");
        let speech_persisted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE community_id = $1 AND id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(speech.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check hard-deadline SAY canonical log");
        assert!(!speech_persisted);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_progress_and_soft_expiry_commit_exactly_one_recovery() {
        let meeting = create_test_meeting().await;
        let (_, grant_id) = create_agent_grant(&meeting, "Progress at soft expiry").await;
        let before = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load Grant before soft-expiry race");
        sqlx::query(
            "UPDATE meeting_baton_grants \
             SET soft_lease_expires_at = clock_timestamp() - interval '1 second', \
                 hard_deadline = clock_timestamp() + interval '1 minute' \
             WHERE community_id = $1 AND session_id = $2 AND grant_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(&grant_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force soft Grant expiry before concurrent recovery");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due State hint before soft Grant recovery");
        let progress = signed_event(
            &meeting.agent,
            buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
            meeting.session_id,
            "",
            &[],
        );

        let (progress_result, recovered) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &progress,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::GrantProgress {
                        grant_id: grant_id.clone(),
                        progress_seq: 1,
                        stage: BatonProgressStage::ToolUse,
                    },
                },
            ),
            recover_meeting_v1(
                &meeting.db,
                meeting.community_id,
                meeting.session_id,
                &meeting.relay,
            )
        );
        let progress_result = progress_result.expect("concurrent soft-expiry Progress result");
        let recovered = recovered.expect("concurrent soft Grant recovery result");
        assert!(matches!(
            &progress_result.command_outcome,
            BatonCommandOutcome::RejectedAfterRecovery { code, .. }
                | BatonCommandOutcome::RejectedTerminal { code, .. }
                if code == "grant_not_active"
        ));
        let recovery_types: Vec<_> = progress_result
            .recovery_transitions
            .iter()
            .chain(recovered.iter())
            .map(|transition| transition.primary_type.as_str())
            .collect();
        assert_eq!(recovery_types, vec!["grant_soft_expired"]);

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.state_revision, before.state_revision + 1);
        assert_eq!(snapshot.speech_revision, before.speech_revision);
        assert!(snapshot.active_grant_id.is_none());
        let (grant_state, progress_rows): (String, i64) = sqlx::query_as(
            "SELECT \
                 (SELECT state FROM meeting_baton_grants \
                  WHERE community_id = $1 AND session_id = $2 AND grant_id = $3), \
                 (SELECT count(*) FROM meeting_grant_progress \
                  WHERE community_id = $1 AND session_id = $2 AND grant_id = $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(grant_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load soft-expired Grant and Progress rows");
        assert_eq!(grant_state, "soft_expired");
        assert_eq!(progress_rows, 0);
        let progress_persisted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM events WHERE community_id = $1 AND id = $2)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(progress.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("check soft-expiry Progress canonical log");
        assert!(!progress_persisted);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_speech_and_end_produce_one_valid_terminal_order() {
        let meeting = create_test_meeting().await;
        let (_, grant_id) = create_agent_grant(&meeting, "SAY while End races").await;
        let before = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load Grant before SAY/End race");
        let speech = signed_event(
            &meeting.agent,
            9,
            meeting.session_id,
            "Possibly the final contribution",
            &[],
        );

        let (speech_result, end_transition) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &speech,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::Speech {
                        grant_id: grant_id.clone(),
                        speech_revision: before.speech_revision + 1,
                        handoff: None,
                    },
                },
            ),
            end_test_meeting(&meeting)
        );
        let speech_result = speech_result.expect("concurrent SAY/End result");
        assert_eq!(end_transition["primary_type"], "meeting_ended");
        let speech_won = matches!(
            &speech_result.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        );
        if !speech_won {
            assert!(matches!(
                &speech_result.command_outcome,
                BatonCommandOutcome::RejectedTerminal { code, .. }
                    if code == "meeting_ended"
            ));
        }

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.phase, BatonPhase::Ended);
        assert_eq!(
            snapshot.state_revision,
            before.state_revision + if speech_won { 2 } else { 1 }
        );
        assert_eq!(
            snapshot.speech_revision,
            before.speech_revision + i64::from(speech_won)
        );
        let (speech_count, grant_state): (i64, String) = sqlx::query_as(
            "SELECT \
                 (SELECT count(*) FROM events \
                  WHERE community_id = $1 AND id = $3), \
                 (SELECT state FROM meeting_baton_grants \
                  WHERE community_id = $1 AND session_id = $2 AND grant_id = $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(speech.id.as_bytes().as_slice())
        .bind(grant_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load canonical SAY/End outcome");
        assert_eq!(speech_count, i64::from(speech_won));
        assert_eq!(grant_state, if speech_won { "spoken" } else { "ended" });
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_human_requests_create_one_offer_and_one_queue_entry() {
        let meeting = create_test_meeting().await;
        let before = get_baton_snapshot(&meeting.db, meeting.community_id, meeting.session_id)
            .await
            .expect("load State before concurrent Human Requests");
        let first_request = signed_event(
            &meeting.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );
        let second_request = signed_event(
            &meeting.human_two,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            meeting.session_id,
            "",
            &[],
        );

        let (first, second) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &first_request,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::HumanRequest,
                },
            ),
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &second_request,
                    relay_keys: &meeting.relay,
                    command: BatonCommand::HumanRequest,
                },
            )
        );
        let results = [
            first.expect("first concurrent Human Request"),
            second.expect("second concurrent Human Request"),
        ];
        assert!(results.iter().all(|result| matches!(
            &result.command_outcome,
            BatonCommandOutcome::Accepted { .. }
        )));

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.phase, BatonPhase::Offered);
        assert_eq!(snapshot.state_revision, before.state_revision + 2);
        let (offered_requests, queued_requests, pending_offers, offered_source_matches): (
            i64,
            i64,
            i64,
            i64,
        ) = sqlx::query_as(
            "SELECT \
                 (SELECT count(*) FROM meeting_human_floor_requests \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'offered'), \
                 (SELECT count(*) FROM meeting_human_floor_requests \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'queued'), \
                 (SELECT count(*) FROM meeting_baton_offers \
                  WHERE community_id = $1 AND session_id = $2 AND state = 'pending'), \
                 (SELECT count(*) \
                  FROM meeting_baton_state state \
                  JOIN meeting_baton_offers offer \
                    ON offer.community_id = state.community_id \
                   AND offer.session_id = state.session_id \
                   AND offer.offer_id = state.active_offer_id \
                  JOIN meeting_human_floor_requests request \
                    ON request.community_id = offer.community_id \
                   AND request.session_id = offer.session_id \
                   AND request.request_id = offer.source_request_id \
                  WHERE state.community_id = $1 AND state.session_id = $2 \
                    AND request.state = 'offered')",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load concurrent Human Request queue shape");
        assert_eq!(
            (
                offered_requests,
                queued_requests,
                pending_offers,
                offered_source_matches,
            ),
            (1, 1, 1, 1)
        );
        let canonical_request_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND id IN ($2, $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(first_request.id.as_bytes().as_slice())
        .bind(second_request.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count canonical concurrent Human Requests");
        assert_eq!(canonical_request_events, 2);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn concurrent_moderator_selects_create_exactly_one_offer() {
        let meeting = create_test_meeting().await;
        let (_, first_intent) =
            submit_intent(&meeting, &meeting.human, "First selectable Intent").await;
        let first_intent_id = accepted_id(&first_intent);
        let (_, second_intent) =
            submit_intent(&meeting, &meeting.human_two, "Second selectable Intent").await;
        let second_intent_id = accepted_id(&second_intent);
        let before = second_intent.snapshot;
        let first_select = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let second_select = signed_event(
            &meeting.moderator,
            buzz_core::kind::KIND_MEETING_MODERATOR_COMMAND,
            meeting.session_id,
            "",
            &[],
        );
        let select_command = |intent_id| BatonCommand::ModeratorSelect {
            source: BatonSelectionSource::Intent { intent_id },
            expected_control_epoch: before.control_epoch,
            expected_decision_epoch: before.decision_epoch,
            expected_intent_revision: before.intent_revision,
            expected_speech_revision: before.speech_revision,
            selection_reason: Some("concurrent selection".to_string()),
            deferrals: Vec::new(),
            attempt_id: None,
            expected_source_event_id: None,
        };

        let (first, second) = tokio::join!(
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &first_select,
                    relay_keys: &meeting.relay,
                    command: select_command(first_intent_id.clone()),
                },
            ),
            execute_baton_command(
                &meeting.db,
                BatonCommandTxParams {
                    community_id: meeting.community_id,
                    session_id: meeting.session_id,
                    event: &second_select,
                    relay_keys: &meeting.relay,
                    command: select_command(second_intent_id.clone()),
                },
            )
        );
        let results = [
            first.expect("first concurrent moderator Select"),
            second.expect("second concurrent moderator Select"),
        ];
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    &result.command_outcome,
                    BatonCommandOutcome::Accepted { .. }
                ))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    &result.command_outcome,
                    BatonCommandOutcome::RejectedTerminal { code, .. }
                        if code == "moderator_does_not_hold_control"
                ))
                .count(),
            1
        );

        let snapshot = assert_serial_state_and_outbox(&meeting).await;
        assert_eq!(snapshot.phase, BatonPhase::Offered);
        assert_eq!(snapshot.state_revision, before.state_revision + 1);
        let active_offer_id = snapshot.active_offer_id.expect("one active Offer");
        let source_intent_id: Vec<u8> = sqlx::query_scalar(
            "SELECT source_intent_id FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3 \
               AND state = 'pending'",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(active_offer_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("load winning moderator Select source");
        assert!(
            source_intent_id == first_intent_id || source_intent_id == second_intent_id,
            "the active Offer must reference exactly one competing Intent"
        );
        let canonical_select_events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND id IN ($2, $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(first_select.id.as_bytes().as_slice())
        .bind(second_select.id.as_bytes().as_slice())
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count canonical concurrent moderator Select commands");
        assert_eq!(canonical_select_events, 1);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn frozen_timing_fallback_and_progress_deadlines_are_deterministic() {
        let fallback = create_test_meeting().await;
        submit_intent(&fallback, &fallback.agent, "Let fallback select me").await;
        let (decision_ms, configured_ms): (i64, i64) = sqlx::query_as(
            "SELECT \
                 round(extract(epoch FROM (s.moderator_decision_deadline \
                     - s.moderator_decision_started_at)) * 1000)::bigint, \
                 c.moderator_decision_ms \
             FROM meeting_baton_state s \
             JOIN meeting_baton_config c \
               ON c.community_id = s.community_id AND c.session_id = s.session_id \
             WHERE s.community_id = $1 AND s.session_id = $2",
        )
        .bind(fallback.community_id.as_uuid())
        .bind(fallback.session_id)
        .fetch_one(&fallback.db.pool)
        .await
        .expect("load frozen moderator decision window");
        assert_eq!(decision_ms, 180_000);
        assert_eq!(decision_ms, configured_ms);
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET moderator_decision_deadline = statement_timestamp() - interval '1 second', \
                 next_action_at = statement_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(fallback.community_id.as_uuid())
        .bind(fallback.session_id)
        .execute(&fallback.db.pool)
        .await
        .expect("force moderator fallback deadline");
        let transitions = recover_meeting_v1(
            &fallback.db,
            fallback.community_id,
            fallback.session_id,
            &fallback.relay,
        )
        .await
        .expect("run deterministic moderator fallback");
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].primary_type, "moderator_fallback");
        let (fallback_offer_id, agent_offer_ms): (Vec<u8>, i64) = sqlx::query_as(
            "SELECT offer_id, \
                 round(extract(epoch FROM (ack_deadline - created_at)) * 1000)::bigint \
             FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending'",
        )
        .bind(fallback.community_id.as_uuid())
        .bind(fallback.session_id)
        .fetch_one(&fallback.db.pool)
        .await
        .expect("load fallback Agent Offer timing");
        assert_eq!(agent_offer_ms, 5_000);
        let fallback_transition = latest_transition(&fallback).await;
        let fallback_control = fallback_transition["effects"]
            .as_array()
            .expect("fallback effects")
            .iter()
            .find(|effect| effect["type"] == "fallback_attempt_recorded")
            .expect("fallback attempt control effect");
        assert_eq!(
            fallback_control["object_id"],
            fallback.session_id.to_string()
        );
        let decline = signed_event(
            &fallback.agent,
            buzz_core::kind::KIND_MEETING_OFFER_RESPONSE,
            fallback.session_id,
            "",
            &[],
        );
        let declined = execute_baton_command(
            &fallback.db,
            BatonCommandTxParams {
                community_id: fallback.community_id,
                session_id: fallback.session_id,
                event: &decline,
                relay_keys: &fallback.relay,
                command: BatonCommand::OfferDecline {
                    offer_id: fallback_offer_id,
                    reason: Some("not ready".to_string()),
                },
            },
        )
        .await
        .expect("decline fallback Offer");
        assert_eq!(declined.snapshot.phase, BatonPhase::ModeratorIdle);
        assert!(recover_meeting_v1(
            &fallback.db,
            fallback.community_id,
            fallback.session_id,
            &fallback.relay,
        )
        .await
        .expect("do not repeat fallback basis")
        .is_empty());
        let fallback_attempts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_fallback_attempts \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(fallback.community_id.as_uuid())
        .bind(fallback.session_id)
        .fetch_one(&fallback.db.pool)
        .await
        .expect("count fallback basis attempts");
        assert_eq!(fallback_attempts, 1);

        let human = create_test_meeting().await;
        let request = signed_event(
            &human.human,
            buzz_core::kind::KIND_MEETING_HUMAN_FLOOR_REQUEST,
            human.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &human.db,
            BatonCommandTxParams {
                community_id: human.community_id,
                session_id: human.session_id,
                event: &request,
                relay_keys: &human.relay,
                command: BatonCommand::HumanRequest,
            },
        )
        .await
        .expect("create Human Offer");
        let human_offer_ms: i64 = sqlx::query_scalar(
            "SELECT round(extract(epoch FROM (ack_deadline - created_at)) * 1000)::bigint \
             FROM meeting_baton_offers \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending'",
        )
        .bind(human.community_id.as_uuid())
        .bind(human.session_id)
        .fetch_one(&human.db.pool)
        .await
        .expect("load Human Offer timing");
        assert_eq!(human_offer_ms, 15_000);

        let hard = create_test_meeting().await;
        let (_, hard_grant_id) = create_agent_grant(&hard, "Hard-capped work").await;
        sqlx::query(
            "WITH deadlines AS ( \
                 SELECT statement_timestamp() + interval '1 second' AS soft, \
                        statement_timestamp() + interval '2 seconds' AS hard \
             ) \
             UPDATE meeting_baton_grants g \
             SET soft_lease_expires_at = deadlines.soft, hard_deadline = deadlines.hard \
             FROM deadlines \
             WHERE g.community_id = $1 AND g.session_id = $2 AND g.grant_id = $3",
        )
        .bind(hard.community_id.as_uuid())
        .bind(hard.session_id)
        .bind(&hard_grant_id)
        .execute(&hard.db.pool)
        .await
        .expect("shorten test Grant hard cap");
        let progress = signed_event(
            &hard.agent,
            buzz_core::kind::KIND_MEETING_GRANT_SIGNAL,
            hard.session_id,
            "",
            &[],
        );
        execute_baton_command(
            &hard.db,
            BatonCommandTxParams {
                community_id: hard.community_id,
                session_id: hard.session_id,
                event: &progress,
                relay_keys: &hard.relay,
                command: BatonCommand::GrantProgress {
                    grant_id: hard_grant_id.clone(),
                    progress_seq: 1,
                    stage: BatonProgressStage::ToolUse,
                },
            },
        )
        .await
        .expect("Progress extends soft lease only to hard cap");
        let (soft_at_hard, state_at_hard): (bool, bool) = sqlx::query_as(
            "SELECT g.soft_lease_expires_at = g.hard_deadline, \
                    s.next_action_at = g.hard_deadline \
             FROM meeting_baton_grants g \
             JOIN meeting_baton_state s \
               ON s.community_id = g.community_id AND s.session_id = g.session_id \
             WHERE g.community_id = $1 AND g.session_id = $2 AND g.grant_id = $3",
        )
        .bind(hard.community_id.as_uuid())
        .bind(hard.session_id)
        .bind(&hard_grant_id)
        .fetch_one(&hard.db.pool)
        .await
        .expect("verify Progress hard cap");
        assert!(soft_at_hard);
        assert!(state_at_hard);
        sqlx::query(
            "WITH deadlines AS ( \
                 SELECT statement_timestamp() - interval '2 seconds' AS soft, \
                        statement_timestamp() - interval '1 second' AS hard \
             ) \
             UPDATE meeting_baton_grants g \
             SET soft_lease_expires_at = deadlines.soft, hard_deadline = deadlines.hard \
             FROM deadlines \
             WHERE g.community_id = $1 AND g.session_id = $2 AND g.grant_id = $3",
        )
        .bind(hard.community_id.as_uuid())
        .bind(hard.session_id)
        .bind(&hard_grant_id)
        .execute(&hard.db.pool)
        .await
        .expect("force hard-expired Grant");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = statement_timestamp() - interval '2 seconds' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(hard.community_id.as_uuid())
        .bind(hard.session_id)
        .execute(&hard.db.pool)
        .await
        .expect("force hard-expired State hint");
        let transitions =
            recover_meeting_v1(&hard.db, hard.community_id, hard.session_id, &hard.relay)
                .await
                .expect("recover hard-expired Grant");
        assert_eq!(transitions[0].primary_type, "grant_hard_expired");

        let soft = create_test_meeting().await;
        let (_, soft_grant_id) = create_agent_grant(&soft, "Soft-expiring work").await;
        sqlx::query(
            "WITH deadlines AS ( \
                 SELECT statement_timestamp() - interval '1 second' AS soft, \
                        statement_timestamp() + interval '1 minute' AS hard \
             ) \
             UPDATE meeting_baton_grants g \
             SET soft_lease_expires_at = deadlines.soft, hard_deadline = deadlines.hard \
             FROM deadlines \
             WHERE g.community_id = $1 AND g.session_id = $2 AND g.grant_id = $3",
        )
        .bind(soft.community_id.as_uuid())
        .bind(soft.session_id)
        .bind(&soft_grant_id)
        .execute(&soft.db.pool)
        .await
        .expect("force soft-expired Grant");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = statement_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(soft.community_id.as_uuid())
        .bind(soft.session_id)
        .execute(&soft.db.pool)
        .await
        .expect("force soft-expired State hint");
        let transitions =
            recover_meeting_v1(&soft.db, soft.community_id, soft.session_id, &soft.relay)
                .await
                .expect("recover soft-expired Grant");
        assert_eq!(transitions[0].primary_type, "grant_soft_expired");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn sweeper_prioritizes_roster_revocation_over_due_offer() {
        let meeting = create_test_meeting().await;
        let (_, submitted) = submit_intent(&meeting, &meeting.agent, "Due but revoked").await;
        let (_, selected) = moderator_select_intent(&meeting, accepted_id(&submitted)).await;
        let offer_id = accepted_id(&selected);
        sqlx::query(
            "UPDATE meeting_baton_offers \
             SET ack_deadline = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2 AND offer_id = $3",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .bind(offer_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due Offer before revocation recovery");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET next_action_at = clock_timestamp() - interval '1 second' \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .execute(&meeting.db.pool)
        .await
        .expect("force due Baton State before revocation recovery");
        let revoked_pubkey = meeting.agent.public_key().to_bytes().to_vec();
        sqlx::query(
            "INSERT INTO community_bans \
                 (community_id, pubkey, banned, actor_pubkey) \
             VALUES ($1, $2, true, $3)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(&revoked_pubkey)
        .bind(meeting.moderator.public_key().to_bytes().as_slice())
        .execute(&meeting.db.pool)
        .await
        .expect("ban test participant");
        sqlx::query(
            "INSERT INTO meeting_revocation_jobs \
                 (community_id, job_id, revoked_pubkey, revocation_event_id) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(Uuid::new_v4())
        .bind(&revoked_pubkey)
        .bind(vec![0x5a_u8; 32])
        .execute(&meeting.db.pool)
        .await
        .expect("persist pending revocation job");

        let transitions = recover_meeting_v1(
            &meeting.db,
            meeting.community_id,
            meeting.session_id,
            &meeting.relay,
        )
        .await
        .expect("recover revoked due Meeting");
        assert_eq!(transitions.len(), 1);
        assert_eq!(transitions[0].primary_type, "participant_revoked");
        let timeout_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_state_history \
             WHERE community_id = $1 AND session_id = $2 \
               AND transition_primary_type = 'offer_timed_out'",
        )
        .bind(meeting.community_id.as_uuid())
        .bind(meeting.session_id)
        .fetch_one(&meeting.db.pool)
        .await
        .expect("count forbidden timeout transition");
        assert_eq!(timeout_count, 0);
        let transition = latest_transition(&meeting).await;
        let effect_types: Vec<&str> = transition["effects"]
            .as_array()
            .expect("revocation End effects")
            .iter()
            .map(|effect| effect["type"].as_str().expect("effect type"))
            .collect();
        assert!(effect_types.contains(&"offer_ended"));
        assert!(effect_types.contains(&"intent_ended"));
        assert_eq!(effect_types.last(), Some(&"phase_changed"));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn due_claim_retry_fence_prevents_a_first_row_from_starving_the_batch() {
        let first = create_test_meeting().await;
        let second = create_test_meeting().await;
        for (meeting, age) in [(&first, 2_i32), (&second, 1_i32)] {
            let (_, submitted) = submit_intent(meeting, &meeting.agent, "Due recovery").await;
            moderator_select_intent(meeting, accepted_id(&submitted)).await;
            sqlx::query(
                "UPDATE meeting_baton_state \
                 SET next_action_at = clock_timestamp() - make_interval(secs => $3) \
                 WHERE community_id = $1 AND session_id = $2",
            )
            .bind(meeting.community_id.as_uuid())
            .bind(meeting.session_id)
            .bind(age)
            .execute(&meeting.db.pool)
            .await
            .expect("force ordered due State");
        }
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET recovery_retry_at = clock_timestamp() + interval '1 hour' \
             WHERE (community_id, session_id) <> ($1, $2) \
               AND (community_id, session_id) <> ($3, $4)",
        )
        .bind(first.community_id.as_uuid())
        .bind(first.session_id)
        .bind(second.community_id.as_uuid())
        .bind(second.session_id)
        .execute(&first.db.pool)
        .await
        .expect("isolate due-claim test from earlier Sessions");
        let first_deadline: DateTime<Utc> = sqlx::query_scalar(
            "SELECT next_action_at FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(first.community_id.as_uuid())
        .bind(first.session_id)
        .fetch_one(&first.db.pool)
        .await
        .expect("load first claimed recovery deadline");
        let second_deadline: DateTime<Utc> = sqlx::query_scalar(
            "SELECT next_action_at FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(second.community_id.as_uuid())
        .bind(second.session_id)
        .fetch_one(&second.db.pool)
        .await
        .expect("load second claimed recovery deadline");

        let first_claim = claim_due_baton_sessions(&first.db, 1)
            .await
            .expect("claim first due State");
        assert_eq!(first_claim.len(), 1);
        assert_eq!(first_claim[0].session_id, first.session_id);
        assert_eq!(first_claim[0].next_action_at, first_deadline);
        let second_claim = claim_due_baton_sessions(&first.db, 1)
            .await
            .expect("claim around retry-fenced first State");
        assert_eq!(second_claim.len(), 1);
        assert_eq!(second_claim[0].session_id, second.session_id);
        assert_eq!(second_claim[0].next_action_at, second_deadline);
        assert!(claim_due_baton_sessions(&first.db, 1)
            .await
            .expect("both due States remain retry fenced")
            .is_empty());
    }

    #[test]
    fn recovery_rejection_classification_requires_a_causal_object_change() {
        let offer = vec![1_u8; 32];
        let grant = vec![2_u8; 32];
        let state_event = vec![3_u8; 32];
        let before = StateRow {
            phase: BatonPhase::Offered,
            floor_revision: 1,
            intent_revision: 1,
            speech_revision: 0,
            state_revision: 2,
            control_epoch: 1,
            decision_epoch: 1,
            decision_attempt: 0,
            active_decision_attempt_id: None,
            state_event_id: state_event.clone(),
            active_offer_id: Some(offer.clone()),
            active_grant_id: None,
            handoff_depth: 0,
            consecutive_moderator_speeches: 0,
            forced_return_to_moderator: false,
            recall_event_id: None,
            moderator_decision_started_at: None,
            moderator_decision_deadline: None,
            next_action_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let mut after = before.clone();
        after.phase = BatonPhase::ModeratorIdle;
        after.active_offer_id = None;
        let transitions = vec![BatonTransitionResult {
            primary_type: "offer_timed_out".to_string(),
            state_revision: 3,
            state_event_id: vec![4_u8; 32],
        }];
        assert!(rejection_was_caused_by_recovery(
            &BatonCommand::OfferAck {
                offer_id: offer.clone()
            },
            &before,
            &after,
            &transitions,
            false,
        ));
        assert!(!rejection_was_caused_by_recovery(
            &BatonCommand::OfferAck {
                offer_id: vec![9_u8; 32]
            },
            &before,
            &after,
            &transitions,
            false,
        ));
        assert!(!rejection_was_caused_by_recovery(
            &BatonCommand::IntentWithdraw {
                intent_id: grant,
                previous_event_id: state_event,
            },
            &before,
            &after,
            &transitions,
            false,
        ));
        assert!(rejection_was_caused_by_recovery(
            &BatonCommand::HumanWithdraw {
                request_id: vec![8_u8; 32],
            },
            &before,
            &after,
            &transitions,
            true,
        ));
        after.control_epoch += 1;
        assert!(rejection_was_caused_by_recovery(
            &BatonCommand::ModeratorRecall {
                control_epoch: before.control_epoch,
                reason: None,
            },
            &before,
            &after,
            &transitions,
            false,
        ));
        assert!(!rejection_was_caused_by_recovery(
            &BatonCommand::ModeratorDismissHandoff {
                handoff_id: vec![7_u8; 32],
                expected_speech_revision: 0,
                expected_attempt_count: 0,
                reason_code: "superseded".to_string(),
                reason_text: "stale cleanup".to_string(),
                attempt_id: None,
            },
            &before,
            &after,
            &transitions,
            false,
        ));
    }
}
