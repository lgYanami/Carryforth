//! Meeting V1 moderated-baton command state machine.
//!
//! Relay handlers validate the signed wire vocabulary and translate it into
//! the typed commands in this module. Every public write entry point repeats
//! the security and logical-state checks while holding the Meeting Session row
//! lock, so an in-memory Relay state can never become authoritative.

use super::*;

use chrono::Duration;
use serde_json::{json, Value};

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
    },
    /// A semantic conflict was terminal without a deadline transition.
    RejectedTerminal {
        /// Stable machine-readable rejection code.
        code: String,
        /// Canonical object that won the race, when known.
        canonical_object_id: Option<Vec<u8>>,
    },
    /// Lazy recovery committed first and made this command terminally late.
    RejectedAfterRecovery {
        /// Stable machine-readable rejection code.
        code: String,
        /// Canonical expired/timed-out object, when known.
        canonical_object_id: Option<Vec<u8>>,
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
                    validate_id(intent_id, "selected Intent id")?
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
                }
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
        }
        BatonCommand::ModeratorDismissHandoff {
            handoff_id,
            expected_speech_revision,
            expected_attempt_count,
            reason_code,
            reason_text,
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
    last_offer_id: Option<Vec<u8>>,
    last_grant_id: Option<Vec<u8>>,
    attempt_count: i32,
}

#[derive(Debug)]
struct ReceiptRow {
    author_pubkey: Vec<u8>,
    accepted: bool,
    outcome_class: String,
    outcome_code: String,
    canonical_object_id: Option<Vec<u8>>,
    state_revision: Option<i64>,
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
        BatonCommand::ModeratorDismissHandoff { .. } | BatonCommand::ModeratorRecall { .. } => {
            if !actor.is_moderator {
                return Err(DbError::AccessDenied(
                    "only the frozen Meeting moderator can issue this command".to_string(),
                ));
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
                response_json \
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
) -> Result<()> {
    let response_json = json!({
        "version": 1,
        "accepted": accepted,
        "outcome_class": outcome_class,
        "outcome_code": outcome_code,
        "canonical_object_id": canonical_object_id.map(hex::encode),
        "state_revision": state_revision,
    });
    sqlx::query(
        "INSERT INTO meeting_v1_command_receipts \
             (community_id, session_id, command_event_id, author_pubkey, kind, action, \
              accepted, outcome_code, canonical_object_id, state_revision, response_json) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
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
    .bind(response_json)
    .execute(tx.as_mut())
    .await?;
    Ok(())
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
        "SELECT intent_id, author_pubkey, current_event_id, basis_speech_revision, \
                summary, addressed_to, state, selection_attempt_count, last_offer_id, \
                last_attempt_outcome, deferred_by_offer_id, created_at \
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
                reason_type, reason_text, requested_depth, question_state, blocked_by, \
                last_offer_id, last_grant_id, last_attempt_outcome, attempt_count, created_at \
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
        last_offer_id: row.try_get("last_offer_id")?,
        last_grant_id: row.try_get("last_grant_id")?,
        attempt_count: row.try_get("attempt_count")?,
    })
}

async fn pending_intents_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<Value>> {
    let rows = sqlx::query(
        "SELECT intent_id, current_event_id, author_pubkey, basis_speech_revision, \
                summary, addressed_to, created_at, deferred_by_offer_id IS NOT NULL AS deferred, \
                selection_attempt_count, last_offer_id, last_attempt_outcome \
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
                last_offer_id, last_grant_id, last_attempt_outcome, blocked_by \
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
    let moderator = hex::encode(moderator_pubkey);
    let content = json!({
        "phase": target.phase,
        "state_revision": state_revision,
        "floor_revision": floor_revision,
        "intent_revision": intent_revision,
        "speech_revision": speech_revision,
        "control_epoch": target.control_epoch,
        "decision_epoch": target.decision_epoch,
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
             next_action_at = $19, recovery_retry_at = '-infinity', \
             recovery_attempts = 0, updated_at = $20 \
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
                 last_attempt_outcome = 'offered', blocked_by = NULL \
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
) -> Result<Option<IntentRow>> {
    let moderator = load_moderator_tx(tx, community_id, session_id).await?;
    let rows = sqlx::query(
        "SELECT i.intent_id, i.author_pubkey, i.current_event_id, \
                i.basis_speech_revision, i.summary, i.addressed_to, i.state, \
                i.selection_attempt_count, i.last_offer_id, i.last_attempt_outcome, \
                i.deferred_by_offer_id, i.created_at \
         FROM meeting_speech_intents i \
         WHERE i.community_id = $1 AND i.session_id = $2 AND i.state = 'pending' \
           AND i.deferred_by_offer_id IS NULL \
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
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&moderator)
    .fetch_one(tx.as_mut())
    .await?;
    Ok(candidates.into_iter().find(|candidate| {
        candidate.author_pubkey != moderator
            || state.consecutive_moderator_speeches == 0
            || !has_other_valid
    }))
}

async fn return_control_to_moderator_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    state: &StateRow,
    config: &BatonConfig,
    now: DateTime<Utc>,
    increment_control_epoch: bool,
) -> Result<StateTarget> {
    let candidate = fallback_candidate_tx(tx, community_id, session_id, state).await?;
    let mut target = StateTarget::from_state(state);
    target.active_offer_id = None;
    target.active_grant_id = None;
    target.handoff_depth = 0;
    target.forced_return_to_moderator = false;
    target.recall_event_id = None;
    if increment_control_epoch {
        target.control_epoch += 1;
    }
    if candidate.is_some() {
        target.phase = BatonPhase::ModeratorControl;
        target.decision_epoch += 1;
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
    ) {
        return Ok(StateTarget::from_state(state));
    }
    let candidate = fallback_candidate_tx(tx, community_id, session_id, state).await?;
    let mut target = StateTarget::from_state(state);
    match (state.phase, candidate.is_some()) {
        (BatonPhase::ModeratorIdle, true) => {
            target.phase = BatonPhase::ModeratorControl;
            target.decision_epoch += 1;
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
                .await?;
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
            .await?;
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
    } else if state.phase == BatonPhase::ModeratorControl
        && state
            .moderator_decision_deadline
            .is_some_and(|deadline| now >= deadline)
    {
        let candidate = fallback_candidate_tx(tx, community_id, session_id, &state).await?;
        let mut effects = Vec::new();
        let (target, object_id, delta) = if let Some(candidate) = candidate {
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
         SELECT community_id, session_id \
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
            )
            .await?;
            tx.commit().await?;
            return Ok(BatonCommitResult {
                recovery_transitions: vec![transition],
                command_outcome: BatonCommandOutcome::RejectedAfterRecovery {
                    code: "participant_revoked".to_string(),
                    canonical_object_id: None,
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
        )
        .await?;
        let snapshot = load_snapshot_tx(&mut tx, params.community_id, params.session_id).await?;
        tx.commit().await?;
        return Ok(BatonCommitResult {
            recovery_transitions,
            command_outcome: BatonCommandOutcome::RejectedTerminal {
                code: "meeting_ended".to_string(),
                canonical_object_id: None,
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
            )
            .await?;
            if outcome_class == "rejected_terminal" {
                BatonCommandOutcome::RejectedTerminal {
                    code: code.to_string(),
                    canonical_object_id,
                }
            } else {
                BatonCommandOutcome::RejectedAfterRecovery {
                    code: code.to_string(),
                    canonical_object_id,
                }
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
    sqlx::query(
        "INSERT INTO meeting_speech_intents \
             (community_id, session_id, intent_id, author_pubkey, current_event_id, \
              basis_speech_revision, summary, addressed_to, state, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $3, $5, $6, $7, 'pending', $8, $8)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&intent_id)
    .bind(&actor.pubkey)
    .bind(basis_speech_revision)
    .bind(summary)
    .bind(addressed_to)
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
) -> Result<Option<Vec<u8>>> {
    existing_pending_intent_tx(tx, community_id, session_id, moderator).await
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
    if state.control_epoch != *expected_control_epoch
        || state.decision_epoch != *expected_decision_epoch
        || state.intent_revision != *expected_intent_revision
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
    let self_intent = pending_self_intent_tx(tx, community_id, session_id, moderator).await?;
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
                return Ok(ApplyResult::Rejected {
                    code: "intent_not_selectable",
                    canonical_object_id: Some(intent.intent_id),
                });
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
                return Ok(ApplyResult::Rejected {
                    code: "handoff_not_open",
                    canonical_object_id: Some(handoff.handoff_id),
                });
            }
            if handoff.attempt_count != *expected_attempt_count {
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

    if source_is_self {
        let rows = sqlx::query(
            "SELECT intent_id, current_event_id \
             FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
               AND author_pubkey <> $3 AND deferred_by_offer_id IS NULL \
             ORDER BY intent_id \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&actor.pubkey)
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
    let target = StateTarget::offered(state, offer_id.clone(), deadline);
    let mut effects = Vec::new();
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
        } => {
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
                    code: "stale_intent_event",
                    canonical_object_id: Some(intent.current_event_id),
                });
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
        } => {
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
                    code: "stale_handoff_revision",
                    canonical_object_id: handoff.last_offer_id.or(handoff.last_grant_id),
                });
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
        sqlx::query(
            "INSERT INTO meeting_directed_handoffs \
                 (community_id, session_id, handoff_id, source_speech_event_id, \
                  from_pubkey, to_pubkey, reason_type, reason_text, requested_depth, \
                  question_state, initial_disposition, blocked_by, created_at, terminal_at) \
             VALUES ($1, $2, $3, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
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
            last_offer_id: None,
            last_grant_id: None,
            attempt_count: 0,
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
        .await?;
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
        .await?;
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

    fn accepted_id(result: &BatonCommitResult) -> Vec<u8> {
        match &result.command_outcome {
            BatonCommandOutcome::Accepted {
                canonical_object_id: Some(id),
                ..
            } => id.clone(),
            other => panic!("expected accepted object, got {other:?}"),
        }
    }

    async fn latest_transition(meeting: &TestMeeting) -> Value {
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
            ["transition"]
            .clone()
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
                },
            },
        )
        .await
        .expect("moderator Select test Intent");
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

        let first_claim = claim_due_baton_sessions(&first.db, 1)
            .await
            .expect("claim first due State");
        assert_eq!(first_claim.len(), 1);
        assert_eq!(first_claim[0].session_id, first.session_id);
        let second_claim = claim_due_baton_sessions(&first.db, 1)
            .await
            .expect("claim around retry-fenced first State");
        assert_eq!(second_claim.len(), 1);
        assert_eq!(second_claim[0].session_id, second.session_id);
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
            },
            &before,
            &after,
            &transitions,
            false,
        ));
    }
}
