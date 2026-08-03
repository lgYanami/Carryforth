//! Meeting V2 Board-gated moderated lifecycle persistence.
//!
//! Creation atomically freezes the private roster, persists exactly one
//! current Markdown board, and records a durable Board/Floor control gate.

use std::collections::HashSet;

use buzz_core::CommunityId;
use chrono::{DateTime, Duration, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde_json::{json, Value};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::meeting::{is_meeting_reader_authorized_for_channel, MAX_MEETING_PARTICIPANTS};
use crate::meeting_baton::{
    create_moderated_meeting_base_tx, initialize_baton_runtime_tx, BatonConfig, BatonParticipant,
    BatonProtocol, CreateModeratedMeetingBaseParams,
};
use crate::{Db, DbError, Result};

/// Persisted Meeting V2 wire schema version.
pub const SCHEMA_VERSION: i32 = 3;
/// Persisted Meeting V2 floor policy.
pub const BOARD_POLICY_VERSION: &str = buzz_sdk::MEETING_V2_POLICY;
/// Persisted action-capable Meeting V2 floor policy.
pub const ACTIONS_POLICY_VERSION: &str = buzz_sdk::MEETING_V2_ACTIONS_POLICY;
/// Lazy-upgrade runtime marker retained for stage-one V2 sessions.
pub const BOOTSTRAP_RUNTIME_PHASE: &str = "bootstrap_locked";
/// Frozen default V2 Board timing profile.
pub const DEFAULT_TIMING_PROFILE_VERSION: &str = "moderated-board-v1-default";
/// Frozen default V2 Baton/Floor timing profile.
pub const DEFAULT_BATON_TIMING_PROFILE_VERSION: &str = "moderated-board-v1-baton-default";
/// Default Board Maintenance budget.
pub const DEFAULT_BOARD_MAINTENANCE_MS: i64 = 180_000;

/// Persisted policy variant for a newly created Meeting V2 session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingV2Policy {
    /// Existing Board-gated Meeting V2 without action finalization.
    Board,
    /// Board-gated Meeting V2 with optional action finalization before close.
    Actions,
}

impl MeetingV2Policy {
    const fn policy(self) -> &'static str {
        match self {
            Self::Board => BOARD_POLICY_VERSION,
            Self::Actions => ACTIONS_POLICY_VERSION,
        }
    }

    const fn baton_protocol(self) -> BatonProtocol {
        match self {
            Self::Board => BatonProtocol::V2,
            Self::Actions => BatonProtocol::V2Actions,
        }
    }
}

/// Parameters for atomically creating a Meeting V2 session.
pub struct CreateMeetingV2Params<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting identity; also the backing Channel UUID.
    pub session_id: Uuid,
    /// Persisted Meeting V2 policy discriminator.
    pub policy: MeetingV2Policy,
    /// Human-readable Meeting title.
    pub title: &'a str,
    /// Optional Meeting description.
    pub description: Option<&'a str>,
    /// Optional source Channel used only as context/navigation.
    pub source_channel_id: Option<Uuid>,
    /// Signed Create author, Channel owner, and immutable moderator.
    pub host_pubkey: &'a [u8],
    /// Event ID of the already-persisted signed Create command.
    pub create_event_id: &'a [u8],
    /// Complete frozen roster, including the host exactly once.
    pub participant_pubkeys: &'a [Vec<u8>],
    /// Strict initial current-board envelope.
    pub initial_board: &'a buzz_sdk::MeetingV2BoardContent,
    /// Relay identity used to sign the current-board projection.
    pub relay_keys: &'a Keys,
    /// Frozen Baton timing and capacity configuration.
    pub baton_config: BatonConfig,
    /// Frozen Board Maintenance budget.
    pub board_maintenance_ms: i64,
}

/// Result of an atomic Meeting V2 creation.
#[derive(Debug, Clone)]
pub struct CreateMeetingV2Snapshot {
    /// Meeting/channel identity.
    pub session_id: Uuid,
    /// Immutable moderator pubkey.
    pub moderator_pubkey: Vec<u8>,
    /// Frozen participants sorted by pubkey.
    pub participants: Vec<BatonParticipant>,
    /// Relay-signed current-board event ID.
    pub board_event_id: Vec<u8>,
    /// Database creation time.
    pub created_at: DateTime<Utc>,
}

/// Current Meeting V2 board projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentMeetingBoard {
    /// Meeting/channel identity.
    pub session_id: Uuid,
    /// Relay-signed projection event ID.
    pub event_id: Vec<u8>,
    /// Immutable moderator pubkey.
    pub moderator_pubkey: Vec<u8>,
    /// Board format; Meeting V2 accepts only `markdown`.
    pub format: String,
    /// Complete current board document.
    pub body: String,
    /// Initial projection creation time.
    pub created_at: DateTime<Utc>,
    /// Current projection update time.
    pub updated_at: DateTime<Utc>,
}

/// Typed result of a participant-signed V2 Board Maintenance command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardAction {
    /// Replace the complete current Board document.
    Update(buzz_sdk::MeetingV2BoardContent),
    /// Explicitly confirm that the current Board remains unchanged.
    Unchanged,
}

impl BoardAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Update(_) => "update",
            Self::Unchanged => "unchanged",
        }
    }
}

/// Inputs for atomically applying one V2 Board Maintenance result.
pub struct BoardActionTxParams<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Strict participant-signed Board command.
    pub event: &'a Event,
    /// Relay identity used for Board and State projections.
    pub relay_keys: &'a Keys,
    /// Control Token epoch observed by the moderator.
    pub expected_control_epoch: i64,
    /// Internal Board window fencing token observed by the moderator.
    pub board_window: i64,
    /// Update or explicit unchanged outcome.
    pub action: BoardAction,
}

/// Canonical outcome of one V2 Board command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BoardActionOutcome {
    /// The command completed the current Board window.
    Accepted {
        /// State revision that opened the subsequent Floor window.
        state_revision: i64,
        /// Current Board projection after completion.
        board_event_id: Vec<u8>,
    },
    /// The identical signed command was already processed.
    Duplicate {
        /// Whether the first execution was accepted.
        accepted: bool,
        /// First execution outcome class.
        outcome_class: String,
        /// Stable machine-readable result.
        outcome_code: String,
        /// State revision recorded by the first execution.
        state_revision: Option<i64>,
        /// Board projection recorded by the first execution.
        board_event_id: Option<Vec<u8>>,
    },
    /// The command lost a race or targeted an inactive window.
    Rejected {
        /// Stable machine-readable reason.
        code: String,
        /// Whether lazy deadline recovery committed before rejection.
        after_recovery: bool,
    },
}

/// Fully committed Board command and any preceding recovery.
#[derive(Debug, Clone)]
pub struct BoardActionCommit {
    /// Canonical command outcome.
    pub outcome: BoardActionOutcome,
    /// State transition committed by Board timeout recovery, when any.
    pub recovery_transition: Option<crate::meeting_baton::BatonTransitionResult>,
}

/// Product result stored on a terminal Meeting V2 Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalOutcome {
    /// The moderator declares the discussion goal complete.
    Closed,
    /// The Meeting ended without declaring success.
    Aborted,
}

impl TerminalOutcome {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Aborted => "aborted",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "closed" => Ok(Self::Closed),
            "aborted" => Ok(Self::Aborted),
            other => Err(DbError::InvalidData(format!(
                "unknown Meeting V2 terminal outcome: {other}"
            ))),
        }
    }
}

/// Read the durable terminal classification of a Meeting V2 Session.
///
/// Active Sessions return `None`; an inconsistent V2 lifecycle projection is
/// rejected instead of being interpreted as a terminal result.
pub async fn get_terminal_outcome(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<TerminalOutcome>> {
    let row = sqlx::query(
        "SELECT status, terminal_outcome FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
           AND schema_version = $3 AND floor_policy_version IN ($4, $5)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(SCHEMA_VERSION)
    .bind(BOARD_POLICY_VERSION)
    .bind(ACTIONS_POLICY_VERSION)
    .fetch_optional(&db.pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("Meeting V2 {session_id}")))?;
    let status: String = row.try_get("status")?;
    let terminal_outcome: Option<String> = row.try_get("terminal_outcome")?;
    match (status.as_str(), terminal_outcome.as_deref()) {
        ("active", None) => Ok(None),
        ("ended", Some(outcome)) => Ok(Some(TerminalOutcome::parse(outcome)?)),
        _ => Err(DbError::InvalidData(format!(
            "Meeting V2 {session_id} has an inconsistent terminal projection"
        ))),
    }
}

/// Action completion fence required by a normal post-materialization close.
#[derive(Debug, Clone, Copy)]
pub struct EndMeetingV2ActionFence<'a> {
    /// Relay-issued active action run ID.
    pub action_run_id: Uuid,
    /// Current action retry-window epoch.
    pub action_window_epoch: i64,
    /// Frozen action plan event ID.
    pub plan_event_id: &'a [u8],
}

/// Parameters for atomically ending a Meeting V2 Session.
pub struct EndMeetingV2Params<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting UUID.
    pub session_id: Uuid,
    /// Signed End author.
    pub actor_pubkey: &'a [u8],
    /// Referenced Create event ID.
    pub create_event_id: &'a [u8],
    /// Already-persisted End event ID.
    pub end_event_id: &'a [u8],
    /// Successful close or abnormal abort.
    pub outcome: TerminalOutcome,
    /// Required machine-readable reason for abort.
    pub reason_code: Option<&'a str>,
    /// Required only for a normal close from `finalizing_actions`.
    pub action_fence: Option<EndMeetingV2ActionFence<'a>>,
    /// Relay identity used for terminal State.
    pub relay_keys: &'a Keys,
}

/// Outcome of a Meeting V2 End command.
#[derive(Debug, Clone)]
pub enum EndMeetingV2Outcome {
    /// This command ended the Session.
    Ended(Box<crate::meeting_baton::BatonSnapshot>),
    /// The Session was already terminal with this persisted result.
    AlreadyEnded(TerminalOutcome),
    /// A revoked roster identity caused security termination before this End.
    ParticipantRevoked(Box<crate::meeting_baton::BatonSnapshot>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimePhase {
    BootstrapLocked,
    BoardPending,
    FloorReady,
    FinalizingActions,
    Ended,
}

impl RuntimePhase {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "bootstrap_locked" => Ok(Self::BootstrapLocked),
            "board_pending" => Ok(Self::BoardPending),
            "floor_ready" => Ok(Self::FloorReady),
            "finalizing_actions" => Ok(Self::FinalizingActions),
            "ended" => Ok(Self::Ended),
            other => Err(DbError::InvalidData(format!(
                "unknown Meeting V2 runtime phase: {other}"
            ))),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::BootstrapLocked => "bootstrap_locked",
            Self::BoardPending => "board_pending",
            Self::FloorReady => "floor_ready",
            Self::FinalizingActions => "finalizing_actions",
            Self::Ended => "ended",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeRow {
    pub(crate) phase: RuntimePhase,
    pub(crate) control_epoch: i64,
    pub(crate) board_window: i64,
    pub(crate) board_started_at: Option<DateTime<Utc>>,
    pub(crate) board_deadline_at: Option<DateTime<Utc>>,
    pub(crate) board_completed_at: Option<DateTime<Utc>>,
    pub(crate) board_outcome: Option<String>,
    pub(crate) terminal_outcome: Option<String>,
    pub(crate) terminal_reason_code: Option<String>,
    pub(crate) terminal_at: Option<DateTime<Utc>>,
}

pub(crate) async fn load_runtime_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    for_update: bool,
) -> Result<RuntimeRow> {
    let row = if for_update {
        sqlx::query(
            "SELECT runtime_phase, control_epoch, board_window, board_started_at, \
                    board_deadline_at, board_completed_at, board_outcome, \
                    terminal_outcome, terminal_reason_code, terminal_at \
             FROM meeting_v2_bootstrap_state \
             WHERE community_id = $1 AND session_id = $2 \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?
    } else {
        sqlx::query(
            "SELECT runtime_phase, control_epoch, board_window, board_started_at, \
                    board_deadline_at, board_completed_at, board_outcome, \
                    terminal_outcome, terminal_reason_code, terminal_at \
             FROM meeting_v2_bootstrap_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?
    }
    .ok_or_else(|| DbError::NotFound(format!("Meeting V2 runtime {session_id}")))?;
    Ok(RuntimeRow {
        phase: RuntimePhase::parse(row.try_get("runtime_phase")?)?,
        control_epoch: row.try_get("control_epoch")?,
        board_window: row.try_get("board_window")?,
        board_started_at: row.try_get("board_started_at")?,
        board_deadline_at: row.try_get("board_deadline_at")?,
        board_completed_at: row.try_get("board_completed_at")?,
        board_outcome: row.try_get("board_outcome")?,
        terminal_outcome: row.try_get("terminal_outcome")?,
        terminal_reason_code: row.try_get("terminal_reason_code")?,
        terminal_at: row.try_get("terminal_at")?,
    })
}

pub(crate) async fn runtime_state_json_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Value> {
    let runtime = load_runtime_tx(tx, community_id, session_id, false).await?;
    let mut projection = json!({
        "phase": runtime.phase.as_str(),
        "control_epoch": runtime.control_epoch,
        "board_window": runtime.board_window,
        "board_started_at_ms": runtime.board_started_at.map(|value| value.timestamp_millis()),
        "board_deadline_at_ms": runtime.board_deadline_at.map(|value| value.timestamp_millis()),
        "board_completed_at_ms": runtime.board_completed_at.map(|value| value.timestamp_millis()),
        "board_outcome": runtime.board_outcome,
        "terminal_outcome": runtime.terminal_outcome,
        "terminal_reason_code": runtime.terminal_reason_code,
        "terminal_at_ms": runtime.terminal_at.map(|value| value.timestamp_millis()),
    });
    if crate::meeting_baton::load_baton_protocol_tx(tx, community_id, session_id)
        .await?
        .has_action_finalization()
    {
        let action =
            crate::meeting_v2_actions::action_state_json_tx(tx, community_id, session_id).await?;
        projection
            .as_object_mut()
            .ok_or_else(|| {
                DbError::InvalidData("Meeting V2 runtime projection is not an object".to_string())
            })?
            .insert("action".to_string(), action.unwrap_or(Value::Null));
    }
    Ok(projection)
}

async fn insert_v2_config_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    board_maintenance_ms: i64,
) -> Result<()> {
    if !(1..=crate::meeting_baton::MAX_BATON_DURATION_MS).contains(&board_maintenance_ms) {
        return Err(DbError::InvalidData(format!(
            "Meeting V2 Board duration must be 1..={}",
            crate::meeting_baton::MAX_BATON_DURATION_MS
        )));
    }
    sqlx::query(
        "INSERT INTO meeting_v2_config \
             (community_id, session_id, timing_profile_version, board_maintenance_ms) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(DEFAULT_TIMING_PROFILE_VERSION)
    .bind(board_maintenance_ms)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn board_maintenance_ms_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<i64> {
    sqlx::query_scalar(
        "SELECT board_maintenance_ms FROM meeting_v2_config \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::InvalidData(format!("Meeting V2 {session_id} has no frozen config")))
}

pub(crate) async fn open_board_window_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    control_epoch: i64,
    now: DateTime<Utc>,
) -> Result<RuntimeRow> {
    if control_epoch <= 0 {
        return Err(DbError::InvalidData(
            "Meeting V2 control epoch must be positive".to_string(),
        ));
    }
    let duration = board_maintenance_ms_tx(tx, community_id, session_id).await?;
    let deadline = now + Duration::milliseconds(duration);
    let row = sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET runtime_phase = 'board_pending', control_epoch = $3, \
             board_window = board_window + 1, board_started_at = $4, \
             board_deadline_at = $5, board_completed_at = NULL, \
             board_outcome = NULL, terminal_outcome = NULL, \
             terminal_reason_code = NULL, terminal_at = NULL, updated_at = $4 \
         WHERE community_id = $1 AND session_id = $2 \
           AND runtime_phase IN ('bootstrap_locked', 'floor_ready', 'finalizing_actions') \
         RETURNING runtime_phase, control_epoch, board_window, board_started_at, \
                   board_deadline_at, board_completed_at, board_outcome, \
                   terminal_outcome, terminal_reason_code, terminal_at",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(control_epoch)
    .bind(now)
    .bind(deadline)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| {
        DbError::InvalidData(format!(
            "Meeting V2 {session_id} cannot open a Board window from its current phase"
        ))
    })?;
    Ok(RuntimeRow {
        phase: RuntimePhase::parse(row.try_get("runtime_phase")?)?,
        control_epoch: row.try_get("control_epoch")?,
        board_window: row.try_get("board_window")?,
        board_started_at: row.try_get("board_started_at")?,
        board_deadline_at: row.try_get("board_deadline_at")?,
        board_completed_at: row.try_get("board_completed_at")?,
        board_outcome: row.try_get("board_outcome")?,
        terminal_outcome: row.try_get("terminal_outcome")?,
        terminal_reason_code: row.try_get("terminal_reason_code")?,
        terminal_at: row.try_get("terminal_at")?,
    })
}

pub(crate) async fn ensure_runtime_initialized_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    now: DateTime<Utc>,
) -> Result<bool> {
    let runtime = load_runtime_tx(tx, community_id, session_id, true).await?;
    if runtime.phase != RuntimePhase::BootstrapLocked {
        return Ok(false);
    }
    let existing_baton: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_one(tx.as_mut())
    .await?;
    if existing_baton {
        return Err(DbError::InvalidData(
            "Meeting V2 bootstrap runtime already has a Baton state".to_string(),
        ));
    }
    let existing_config: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM meeting_v2_config \
             WHERE community_id = $1 AND session_id = $2 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_one(tx.as_mut())
    .await?;
    if !existing_config {
        insert_v2_config_tx(tx, community_id, session_id, DEFAULT_BOARD_MAINTENANCE_MS).await?;
    }
    open_board_window_tx(tx, community_id, session_id, runtime.control_epoch, now).await?;
    let moderator = crate::meeting_baton::load_moderator_tx(tx, community_id, session_id).await?;
    let participants =
        crate::meeting_baton::load_participants_tx(tx, community_id, session_id).await?;
    let baton_config = BatonConfig {
        timing_profile_version: DEFAULT_BATON_TIMING_PROFILE_VERSION.to_string(),
        ..BatonConfig::default()
    };
    let protocol =
        crate::meeting_baton::load_baton_protocol_tx(tx, community_id, session_id).await?;
    if !protocol.is_v2() {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a Meeting V2 session"
        )));
    }
    initialize_baton_runtime_tx(
        tx,
        community_id,
        session_id,
        &moderator,
        &participants,
        relay_keys,
        &baton_config,
        protocol,
        None,
        "meeting_v2_initialized",
        now,
    )
    .await?;
    Ok(true)
}

pub(crate) async fn preempt_board_window_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    now: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET runtime_phase = 'floor_ready', board_deadline_at = NULL, \
             board_completed_at = $3, board_outcome = 'preempted', updated_at = $3 \
         WHERE community_id = $1 AND session_id = $2 \
           AND runtime_phase = 'board_pending'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(result.rows_affected() == 1)
}

async fn complete_runtime_board_window_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    expected_control_epoch: i64,
    board_window: i64,
    outcome: &str,
    now: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET runtime_phase = 'floor_ready', board_deadline_at = NULL, \
             board_completed_at = $6, board_outcome = $5, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 \
           AND runtime_phase = 'board_pending' \
           AND control_epoch = $3 AND board_window = $4",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(expected_control_epoch)
    .bind(board_window)
    .bind(outcome)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(result.rows_affected() == 1)
}

pub(crate) async fn recover_due_board_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
    now: DateTime<Utc>,
) -> Result<Option<crate::meeting_baton::BatonTransitionResult>> {
    let runtime = load_runtime_tx(tx, community_id, session_id, true).await?;
    if runtime.phase != RuntimePhase::BoardPending
        || runtime
            .board_deadline_at
            .is_none_or(|deadline| deadline > now)
    {
        return Ok(None);
    }
    if !complete_runtime_board_window_tx(
        tx,
        community_id,
        session_id,
        runtime.control_epoch,
        runtime.board_window,
        "timed_out",
        now,
    )
    .await?
    {
        return Ok(None);
    }
    let transition = crate::meeting_baton::complete_v2_board_window_state_tx(
        tx,
        community_id,
        session_id,
        relay_keys,
        "board_timed_out",
        None,
        runtime.board_window,
        now,
    )
    .await?;
    Ok(Some(transition))
}

#[derive(Debug)]
struct BoardReceipt {
    author_pubkey: Vec<u8>,
    accepted: bool,
    outcome_class: String,
    outcome_code: String,
    state_revision: Option<i64>,
    board_event_id: Option<Vec<u8>>,
}

async fn load_board_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    event_id: &[u8],
) -> Result<Option<BoardReceipt>> {
    let row = sqlx::query(
        "SELECT author_pubkey, accepted, outcome_class, outcome_code, \
                state_revision, board_event_id \
         FROM meeting_v2_board_command_receipts \
         WHERE community_id = $1 AND command_event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(|row| {
        Ok(BoardReceipt {
            author_pubkey: row.try_get("author_pubkey")?,
            accepted: row.try_get("accepted")?,
            outcome_class: row.try_get("outcome_class")?,
            outcome_code: row.try_get("outcome_code")?,
            state_revision: row.try_get("state_revision")?,
            board_event_id: row.try_get("board_event_id")?,
        })
    })
    .transpose()
}

#[allow(clippy::too_many_arguments)]
async fn insert_board_receipt_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: &BoardActionTxParams<'_>,
    accepted: bool,
    outcome_class: &str,
    outcome_code: &str,
    state_revision: Option<i64>,
    board_event_id: Option<&[u8]>,
) -> Result<()> {
    let response = json!({
        "meeting_id": params.session_id,
        "accepted": accepted,
        "outcome_class": outcome_class,
        "outcome": outcome_code,
        "control_epoch": params.expected_control_epoch,
        "board_window": params.board_window,
        "state_revision": state_revision,
        "board_event_id": board_event_id.map(hex::encode),
    });
    sqlx::query(
        "INSERT INTO meeting_v2_board_command_receipts \
             (community_id, session_id, command_event_id, author_pubkey, action, \
              accepted, outcome_class, outcome_code, control_epoch, board_window, \
              state_revision, board_event_id, response_json) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.event.id.as_bytes().as_slice())
    .bind(params.event.pubkey.as_bytes())
    .bind(params.action.as_str())
    .bind(accepted)
    .bind(outcome_class)
    .bind(outcome_code)
    .bind(params.expected_control_epoch)
    .bind(params.board_window)
    .bind(state_revision)
    .bind(board_event_id)
    .bind(response)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn current_board_event_id_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<u8>> {
    sqlx::query_scalar(
        "SELECT board_event_id FROM meeting_current_boards \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::InvalidData(format!("Meeting V2 {session_id} has no current Board")))
}

async fn replace_current_board_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    moderator_pubkey: &[u8],
    relay_keys: &Keys,
    board: &buzz_sdk::MeetingV2BoardContent,
    now: DateTime<Utc>,
) -> Result<Vec<u8>> {
    buzz_sdk::validate_meeting_v2_board_content(board)
        .map_err(|error| DbError::InvalidData(error.to_string()))?;
    let row = sqlx::query(
        "SELECT board_event_id, board_format, board_content \
         FROM meeting_current_boards \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::InvalidData(format!("Meeting V2 {session_id} has no current Board")))?;
    let old_event_id: Vec<u8> = row.try_get("board_event_id")?;
    let old_format: String = row.try_get("board_format")?;
    let old_content: String = row.try_get("board_content")?;
    if old_format == board.format && old_content == board.body {
        return Ok(old_event_id);
    }
    let policy = crate::meeting_baton::load_baton_protocol_tx(tx, community_id, session_id)
        .await?
        .policy();
    let board_event =
        build_board_event(relay_keys, session_id, moderator_pubkey, board, policy, now)?;
    persist_board_event_tx(tx, community_id, session_id, &board_event, now).await?;
    let new_event_id = board_event.id.as_bytes().to_vec();
    let updated = sqlx::query(
        "UPDATE meeting_current_boards \
         SET board_event_id = $3, board_format = $4, board_content = $5, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND board_event_id = $7",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&new_event_id)
    .bind(&board.format)
    .bind(&board.body)
    .bind(now)
    .bind(&old_event_id)
    .execute(tx.as_mut())
    .await?;
    if updated.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 current Board changed while holding the Session lock".to_string(),
        ));
    }
    sqlx::query(
        "DELETE FROM events \
         WHERE community_id = $1 AND id = $2 AND kind = $3 AND channel_id = $4",
    )
    .bind(community_id.as_uuid())
    .bind(&old_event_id)
    .bind(buzz_core::kind::KIND_MEETING_BOARD as i32)
    .bind(session_id)
    .execute(tx.as_mut())
    .await?;
    Ok(new_event_id)
}

/// Execute one moderator Board Maintenance result under the Session lock.
pub async fn execute_board_action(
    db: &Db,
    params: BoardActionTxParams<'_>,
) -> Result<BoardActionCommit> {
    if params.session_id.is_nil() || params.expected_control_epoch <= 0 || params.board_window <= 0
    {
        return Err(DbError::InvalidData(
            "Meeting V2 Board command has invalid fencing values".to_string(),
        ));
    }
    params.event.verify().map_err(|error| {
        DbError::InvalidData(format!("invalid Meeting V2 Board event: {error}"))
    })?;
    if params.event.kind.as_u16() as u32 != buzz_core::kind::KIND_MEETING_BOARD_COMMAND {
        return Err(DbError::InvalidData(
            "Meeting V2 Board action uses the wrong event kind".to_string(),
        ));
    }
    if let BoardAction::Update(board) = &params.action {
        buzz_sdk::validate_meeting_v2_board_content(board)
            .map_err(|error| DbError::InvalidData(error.to_string()))?;
    }
    let mut tx = db.begin_transaction().await?;
    let session = crate::meeting_baton::lock_baton_session_tx(
        &mut tx,
        params.community_id,
        params.session_id,
    )
    .await?;
    if !session.protocol.is_v2() {
        return Err(DbError::InvalidData(format!(
            "meeting {} is not a Meeting V2 session",
            params.session_id
        )));
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    ensure_runtime_initialized_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        params.relay_keys,
        now,
    )
    .await?;
    let author = params.event.pubkey.as_bytes();
    if author != session.host_pubkey.as_slice() {
        return Err(DbError::AccessDenied(
            "only the immutable Meeting V2 moderator can maintain the Board".to_string(),
        ));
    }
    let event_id = params.event.id.as_bytes().as_slice();
    if session.status == "active" {
        if let Some(snapshot) = crate::meeting_revocation::recover_revoked_roster_v1_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
        )
        .await?
        {
            let transition = crate::meeting_baton::BatonTransitionResult {
                primary_type: "participant_revoked".to_string(),
                state_revision: snapshot.state_revision,
                state_event_id: snapshot.state_event_id,
            };
            if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                author,
            )
            .await?
            {
                tx.commit().await?;
                return Err(DbError::AccessDenied(
                    "Meeting V2 moderator was durably revoked from this Session".to_string(),
                ));
            }
            if !crate::meeting::actor_security_active_tx(&mut tx, params.community_id, author)
                .await?
            {
                tx.commit().await?;
                return Err(DbError::AccessDenied(
                    "Meeting V2 moderator is no longer an active writable principal".to_string(),
                ));
            }
            if let Some(receipt) =
                load_board_receipt_tx(&mut tx, params.community_id, event_id).await?
            {
                if receipt.author_pubkey != author {
                    return Err(DbError::AccessDenied(
                        "not authorized for this private Meeting V2 receipt".to_string(),
                    ));
                }
                tx.commit().await?;
                return Ok(BoardActionCommit {
                    outcome: BoardActionOutcome::Duplicate {
                        accepted: receipt.accepted,
                        outcome_class: receipt.outcome_class,
                        outcome_code: receipt.outcome_code,
                        state_revision: receipt.state_revision,
                        board_event_id: receipt.board_event_id,
                    },
                    recovery_transition: Some(transition),
                });
            }
            insert_board_receipt_tx(
                &mut tx,
                &params,
                false,
                "rejected_after_recovery",
                "participant_revoked",
                Some(transition.state_revision),
                None,
            )
            .await?;
            tx.commit().await?;
            return Ok(BoardActionCommit {
                outcome: BoardActionOutcome::Rejected {
                    code: "participant_revoked".to_string(),
                    after_recovery: true,
                },
                recovery_transition: Some(transition),
            });
        }
    }
    if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        author,
    )
    .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting V2 moderator was durably revoked from this Session".to_string(),
        ));
    }
    if !crate::meeting::actor_security_active_tx(&mut tx, params.community_id, author).await? {
        return Err(DbError::AccessDenied(
            "Meeting V2 moderator is no longer an active writable principal".to_string(),
        ));
    }
    let recovery_transition = if session.status == "active" {
        recover_due_board_locked_tx(
            &mut tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
            now,
        )
        .await?
    } else {
        None
    };
    if let Some(receipt) = load_board_receipt_tx(&mut tx, params.community_id, event_id).await? {
        if receipt.author_pubkey != author {
            return Err(DbError::AccessDenied(
                "not authorized for this private Meeting V2 receipt".to_string(),
            ));
        }
        tx.commit().await?;
        return Ok(BoardActionCommit {
            outcome: BoardActionOutcome::Duplicate {
                accepted: receipt.accepted,
                outcome_class: receipt.outcome_class,
                outcome_code: receipt.outcome_code,
                state_revision: receipt.state_revision,
                board_event_id: receipt.board_event_id,
            },
            recovery_transition,
        });
    }
    if session.status != "active" {
        insert_board_receipt_tx(
            &mut tx,
            &params,
            false,
            "rejected_terminal",
            "meeting_ended",
            None,
            None,
        )
        .await?;
        tx.commit().await?;
        return Ok(BoardActionCommit {
            outcome: BoardActionOutcome::Rejected {
                code: "meeting_ended".to_string(),
                after_recovery: false,
            },
            recovery_transition,
        });
    }
    let runtime = load_runtime_tx(&mut tx, params.community_id, params.session_id, true).await?;
    let conflict_code = if session.protocol.has_action_finalization()
        && runtime.phase == RuntimePhase::FinalizingActions
    {
        Some("meeting_finalizing_actions")
    } else if runtime.phase != RuntimePhase::BoardPending {
        Some(if recovery_transition.is_some() {
            "board_window_timed_out"
        } else {
            "board_window_inactive"
        })
    } else if runtime.control_epoch != params.expected_control_epoch {
        Some("stale_control_epoch")
    } else if runtime.board_window != params.board_window {
        Some("stale_board_window")
    } else {
        None
    };
    if let Some(code) = conflict_code {
        let outcome_class = if recovery_transition.is_some() {
            "rejected_after_recovery"
        } else {
            "rejected_terminal"
        };
        insert_board_receipt_tx(&mut tx, &params, false, outcome_class, code, None, None).await?;
        tx.commit().await?;
        return Ok(BoardActionCommit {
            outcome: BoardActionOutcome::Rejected {
                code: code.to_string(),
                after_recovery: recovery_transition.is_some(),
            },
            recovery_transition,
        });
    }
    let board_event_id = match &params.action {
        BoardAction::Update(board) => {
            replace_current_board_tx(
                &mut tx,
                params.community_id,
                params.session_id,
                author,
                params.relay_keys,
                board,
                now,
            )
            .await?
        }
        BoardAction::Unchanged => {
            current_board_event_id_tx(&mut tx, params.community_id, params.session_id).await?
        }
    };
    let board_outcome = match params.action {
        BoardAction::Update(_) => "updated",
        BoardAction::Unchanged => "unchanged",
    };
    if !complete_runtime_board_window_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        params.expected_control_epoch,
        params.board_window,
        board_outcome,
        now,
    )
    .await?
    {
        return Err(DbError::InvalidData(
            "Meeting V2 Board window changed while holding the Session lock".to_string(),
        ));
    }
    let transition_type = match params.action {
        BoardAction::Update(_) => "board_updated",
        BoardAction::Unchanged => "board_unchanged",
    };
    let transition = crate::meeting_baton::complete_v2_board_window_state_tx(
        &mut tx,
        params.community_id,
        params.session_id,
        params.relay_keys,
        transition_type,
        Some(event_id),
        params.board_window,
        now,
    )
    .await?;
    insert_board_receipt_tx(
        &mut tx,
        &params,
        true,
        "accepted",
        board_outcome,
        Some(transition.state_revision),
        Some(&board_event_id),
    )
    .await?;
    tx.commit().await?;
    Ok(BoardActionCommit {
        outcome: BoardActionOutcome::Accepted {
            state_revision: transition.state_revision,
            board_event_id,
        },
        recovery_transition,
    })
}

/// End an active Meeting V2 as normal closed or abnormal aborted.
pub async fn end_meeting_v2_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: EndMeetingV2Params<'_>,
) -> Result<EndMeetingV2Outcome> {
    validate_32_bytes(params.actor_pubkey, "Meeting V2 End actor")?;
    validate_32_bytes(params.create_event_id, "Meeting V2 Create event id")?;
    validate_32_bytes(params.end_event_id, "Meeting V2 End event id")?;
    crate::meeting_baton::ensure_existing_command_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.end_event_id,
        buzz_core::kind::KIND_MEETING_END as i32,
        params.actor_pubkey,
    )
    .await?;
    let session =
        crate::meeting_baton::lock_baton_session_tx(tx, params.community_id, params.session_id)
            .await?;
    if !session.protocol.is_v2() {
        return Err(DbError::InvalidData(format!(
            "meeting {} is not a Meeting V2 session",
            params.session_id
        )));
    }
    let initialize_now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    if session.status == "active" {
        ensure_runtime_initialized_tx(
            tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
            initialize_now,
        )
        .await?;
    }
    if session.create_event_id != params.create_event_id {
        return Err(DbError::InvalidData(
            "Meeting V2 End references the wrong Create event".to_string(),
        ));
    }
    if session.status == "ended" {
        let terminal_outcome: String = sqlx::query_scalar(
            "SELECT terminal_outcome FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(params.community_id.as_uuid())
        .bind(params.session_id)
        .fetch_one(tx.as_mut())
        .await?;
        return Ok(EndMeetingV2Outcome::AlreadyEnded(TerminalOutcome::parse(
            &terminal_outcome,
        )?));
    }
    if session.status != "active" {
        return Err(DbError::InvalidData(format!(
            "unknown Meeting V2 status: {}",
            session.status
        )));
    }
    if let Some(snapshot) = crate::meeting_revocation::recover_revoked_roster_v1_tx(
        tx,
        params.community_id,
        params.session_id,
        params.relay_keys,
    )
    .await?
    {
        crate::meeting::discard_unenqueued_manual_end_event_tx(
            tx,
            params.community_id,
            params.session_id,
            params.end_event_id,
            params.actor_pubkey,
        )
        .await?;
        return Ok(EndMeetingV2Outcome::ParticipantRevoked(Box::new(snapshot)));
    }
    if crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        tx,
        params.community_id,
        params.session_id,
        params.actor_pubkey,
    )
    .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting V2 End author was durably revoked from this Session".to_string(),
        ));
    }
    if !crate::meeting::actor_security_active_tx(tx, params.community_id, params.actor_pubkey)
        .await?
    {
        return Err(DbError::AccessDenied(
            "Meeting V2 End author is no longer an active writable principal".to_string(),
        ));
    }
    let actor_is_moderator = params.actor_pubkey == session.host_pubkey.as_slice();
    let actor_community_role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(params.community_id.as_uuid())
    .bind(hex::encode(params.actor_pubkey))
    .fetch_optional(tx.as_mut())
    .await?;
    let actor_is_operator = matches!(actor_community_role.as_deref(), Some("owner" | "admin"));
    match params.outcome {
        TerminalOutcome::Closed if !actor_is_moderator => {
            return Err(DbError::AccessDenied(
                "only the immutable Meeting V2 moderator can close normally".to_string(),
            ));
        }
        TerminalOutcome::Aborted if !actor_is_moderator && !actor_is_operator => {
            return Err(DbError::AccessDenied(
                "only the moderator or a Community operator can abort Meeting V2".to_string(),
            ));
        }
        _ => {}
    }
    let reason_code = match params.outcome {
        TerminalOutcome::Closed => {
            if params.reason_code.is_some() {
                return Err(DbError::InvalidData(
                    "Meeting V2 close cannot carry an abort reason".to_string(),
                ));
            }
            None
        }
        TerminalOutcome::Aborted => {
            let reason = params.reason_code.ok_or_else(|| {
                DbError::InvalidData("Meeting V2 abort requires a reason code".to_string())
            })?;
            if reason.is_empty()
                || reason.len() > 128
                || reason.trim() != reason
                || reason.chars().any(char::is_control)
            {
                return Err(DbError::InvalidData(
                    "Meeting V2 abort reason code must be 1..=128 clean bytes".to_string(),
                ));
            }
            Some(reason)
        }
    };
    if params.outcome == TerminalOutcome::Aborted && params.action_fence.is_some() {
        return Err(DbError::InvalidData(
            "Meeting V2 abort cannot carry an action completion fence".to_string(),
        ));
    }
    let runtime = load_runtime_tx(tx, params.community_id, params.session_id, true).await?;
    let moderator_controls_floor = crate::meeting_baton::moderator_controls_floor_tx(
        tx,
        params.community_id,
        params.session_id,
        initialize_now,
    )
    .await?;
    if params.outcome == TerminalOutcome::Closed {
        let explicit_board = matches!(
            runtime.board_outcome.as_deref(),
            Some("updated" | "unchanged")
        );
        let close_gate = if session.protocol.has_action_finalization() {
            match (runtime.phase, params.action_fence) {
                (RuntimePhase::FloorReady, None) => true,
                (RuntimePhase::FinalizingActions, Some(fence)) => {
                    crate::meeting_v2_actions::validate_close_gate_tx(
                        tx,
                        params.community_id,
                        params.session_id,
                        fence.action_run_id,
                        fence.action_window_epoch,
                        fence.plan_event_id,
                    )
                    .await?
                }
                _ => false,
            }
        } else {
            runtime.phase == RuntimePhase::FloorReady && params.action_fence.is_none()
        };
        if !explicit_board || !moderator_controls_floor || !close_gate {
            return Err(DbError::InvalidData(
                "Meeting V2 close requires an explicit final Board result, moderator control, and any required action completion gate"
                    .to_string(),
            ));
        }
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    let ended_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE meeting_sessions \
         SET status = 'ended', ended_at = $3, ended_by = $4, end_event_id = $5, \
             terminal_outcome = $6, terminal_reason_code = $7 \
         WHERE community_id = $1 AND session_id = $2 AND status = 'active' \
           AND schema_version = 3 AND floor_policy_version = $8 \
         RETURNING ended_at",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(now)
    .bind(params.actor_pubkey)
    .bind(params.end_event_id)
    .bind(params.outcome.as_str())
    .bind(reason_code)
    .bind(session.protocol.policy())
    .fetch_one(tx.as_mut())
    .await?;
    let archived = sqlx::query(
        "UPDATE channels \
         SET archived_at = $3, updated_at = $3 \
         WHERE community_id = $1 AND id = $2 \
           AND room_kind = 'meeting' AND archived_at IS NULL AND deleted_at IS NULL",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(ended_at)
    .execute(tx.as_mut())
    .await?;
    if archived.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 backing Channel is missing or inactive".to_string(),
        ));
    }
    if session.protocol.has_action_finalization() {
        let action_terminal = match params.outcome {
            TerminalOutcome::Closed => "completed_closed",
            TerminalOutcome::Aborted => "completed_aborted",
        };
        crate::meeting_v2_actions::mark_active_run_terminal_tx(
            tx,
            params.community_id,
            params.session_id,
            action_terminal,
            now,
        )
        .await?;
    }
    sqlx::query(
        "UPDATE meeting_v2_bootstrap_state \
         SET runtime_phase = 'ended', board_deadline_at = NULL, \
             board_completed_at = COALESCE(board_completed_at, $3), \
             board_outcome = CASE \
                 WHEN runtime_phase = 'board_pending' THEN 'preempted' \
                 ELSE board_outcome END, \
             terminal_outcome = $4, terminal_reason_code = $5, \
             terminal_at = $3, updated_at = $3 \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(now)
    .bind(params.outcome.as_str())
    .bind(reason_code)
    .execute(tx.as_mut())
    .await?;
    let primary_type = match params.outcome {
        TerminalOutcome::Closed => "meeting_closed",
        TerminalOutcome::Aborted => "meeting_aborted",
    };
    let snapshot = crate::meeting_baton::close_baton_locked_tx(
        tx,
        params.community_id,
        params.session_id,
        params.end_event_id,
        primary_type,
        params.relay_keys,
        now,
    )
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.end_event_id,
    )
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        params.community_id,
        params.session_id,
        &snapshot.state_event_id,
    )
    .await?;
    Ok(EndMeetingV2Outcome::Ended(Box::new(snapshot)))
}

/// Atomically create a private Meeting V2 room and its initial current board.
///
/// The signed Create event must already exist in `events` inside `tx` and its
/// envelope contains the initial Board. Create enters the Meeting outbox; the
/// independent current-Board projection and its later replacements do not, so
/// subsequent Board updates remain pull-only.
pub async fn create_meeting_v2_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: CreateMeetingV2Params<'_>,
) -> Result<CreateMeetingV2Snapshot> {
    validate_create_shape(&params)?;
    buzz_sdk::validate_meeting_v2_board_content(params.initial_board)
        .map_err(|error| DbError::InvalidData(error.to_string()))?;

    let base = create_moderated_meeting_base_tx(
        tx,
        CreateModeratedMeetingBaseParams {
            community_id: params.community_id,
            session_id: params.session_id,
            title: params.title,
            description: params.description,
            source_channel_id: params.source_channel_id,
            host_pubkey: params.host_pubkey,
            moderator_pubkey: params.host_pubkey,
            create_event_id: params.create_event_id,
            participant_pubkeys: params.participant_pubkeys,
            schema_version: SCHEMA_VERSION,
            policy_version: params.policy.policy(),
        },
    )
    .await?;

    let board_event = build_board_event(
        params.relay_keys,
        params.session_id,
        params.host_pubkey,
        params.initial_board,
        params.policy.policy(),
        base.created_at,
    )?;
    persist_board_event_tx(
        tx,
        params.community_id,
        params.session_id,
        &board_event,
        base.created_at,
    )
    .await?;
    sqlx::query(
        "INSERT INTO meeting_current_boards \
             (community_id, session_id, board_event_id, board_format, \
              board_content, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $6)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(board_event.id.as_bytes().as_slice())
    .bind(&params.initial_board.format)
    .bind(&params.initial_board.body)
    .bind(base.created_at)
    .execute(tx.as_mut())
    .await?;
    sqlx::query(
        "INSERT INTO meeting_v2_bootstrap_state \
             (community_id, session_id, runtime_phase, control_epoch, created_at, updated_at) \
         VALUES ($1, $2, $3, 1, $4, $4)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(BOOTSTRAP_RUNTIME_PHASE)
    .bind(base.created_at)
    .execute(tx.as_mut())
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.create_event_id,
    )
    .await?;
    insert_v2_config_tx(
        tx,
        params.community_id,
        params.session_id,
        params.board_maintenance_ms,
    )
    .await?;
    open_board_window_tx(
        tx,
        params.community_id,
        params.session_id,
        1,
        base.created_at,
    )
    .await?;
    initialize_baton_runtime_tx(
        tx,
        params.community_id,
        params.session_id,
        params.host_pubkey,
        &base.participants,
        params.relay_keys,
        &params.baton_config,
        params.policy.baton_protocol(),
        Some(params.create_event_id),
        "meeting_created",
        base.created_at,
    )
    .await?;

    Ok(CreateMeetingV2Snapshot {
        session_id: params.session_id,
        moderator_pubkey: params.host_pubkey.to_vec(),
        participants: base.participants,
        board_event_id: board_event.id.as_bytes().to_vec(),
        created_at: base.created_at,
    })
}

impl Db {
    /// Return whether the complete Meeting V2 runtime catalog is present.
    ///
    /// This is a deployment probe, not a per-Session authorization check. It
    /// deliberately verifies the stage-two columns and receipt table rather
    /// than treating the stage-one bootstrap schema as a runnable lifecycle.
    pub async fn meeting_v2_schema_ready(&self) -> Result<bool> {
        let ready = sqlx::query_scalar(
            "SELECT \
                to_regclass('meeting_current_boards') IS NOT NULL \
                AND to_regclass('meeting_v2_config') IS NOT NULL \
                AND to_regclass('meeting_v2_bootstrap_state') IS NOT NULL \
                AND to_regclass('meeting_v2_board_command_receipts') IS NOT NULL \
                AND to_regclass('meeting_v2_action_runs') IS NOT NULL \
                AND to_regclass('meeting_v2_action_steps') IS NOT NULL \
                AND to_regclass('meeting_v2_action_step_attempts') IS NOT NULL \
                AND to_regclass('meeting_v2_action_command_receipts') IS NOT NULL \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_v2_config') \
                      AND attname = 'action_finalization_ms' AND NOT attisdropped \
                ) \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_sessions') \
                      AND attname = 'terminal_outcome' AND NOT attisdropped \
                ) \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_v2_bootstrap_state') \
                      AND attname = 'board_deadline_at' AND NOT attisdropped \
                ) \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_v2_bootstrap_state') \
                      AND attname = 'terminal_outcome' AND NOT attisdropped \
                )",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(ready)
    }

    /// Return whether this Relay pod may serve traffic while Meeting V2 is in
    /// use or creation is enabled.
    ///
    /// A pre-migration deployment with Create disabled and no possible V2 rows
    /// stays ready, allowing the binary-before-migration rollout order. Once an
    /// active V2 exists—or Create is enabled—the complete schema and a stable
    /// Relay signer are mandatory even if operators later close Create.
    pub async fn meeting_v2_deployment_ready(
        &self,
        stable_signer_configured: bool,
        create_enabled: bool,
    ) -> Result<bool> {
        let protocol_columns_ready: bool = sqlx::query_scalar(
            "SELECT \
                to_regclass('meeting_sessions') IS NOT NULL \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_sessions') \
                      AND attname = 'schema_version' AND NOT attisdropped \
                ) \
                AND EXISTS ( \
                    SELECT 1 FROM pg_attribute \
                    WHERE attrelid = to_regclass('meeting_sessions') \
                      AND attname = 'floor_policy_version' AND NOT attisdropped \
                )",
        )
        .fetch_one(&self.pool)
        .await?;
        let active_v2 = if protocol_columns_ready {
            sqlx::query_scalar(
                "SELECT EXISTS ( \
                    SELECT 1 FROM meeting_sessions \
                    WHERE status = 'active' \
                      AND schema_version = $1 \
                      AND floor_policy_version IN ($2, $3) \
                )",
            )
            .bind(SCHEMA_VERSION)
            .bind(BOARD_POLICY_VERSION)
            .bind(ACTIONS_POLICY_VERSION)
            .fetch_one(&self.pool)
            .await?
        } else {
            false
        };
        if !meeting_v2_runtime_required(create_enabled, active_v2) {
            return Ok(true);
        }
        Ok(stable_signer_configured && self.meeting_v2_schema_ready().await?)
    }
}

const fn meeting_v2_runtime_required(create_enabled: bool, active_v2: bool) -> bool {
    create_enabled || active_v2
}

/// Load the current board without applying a caller authorization decision.
///
/// Relay query paths should normally use their existing Meeting reader fence;
/// direct consumers that possess a reader identity should use
/// [`get_current_board_for_reader`].
pub async fn get_current_board(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<CurrentMeetingBoard>> {
    let row = sqlx::query(
        "SELECT b.board_event_id, b.board_format, b.board_content, \
                b.created_at, b.updated_at, s.moderator_pubkey \
         FROM meeting_current_boards b \
         JOIN meeting_sessions s \
           ON s.community_id = b.community_id AND s.session_id = b.session_id \
         WHERE b.community_id = $1 AND b.session_id = $2 \
           AND s.schema_version = $3 AND s.floor_policy_version IN ($4, $5)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(SCHEMA_VERSION)
    .bind(BOARD_POLICY_VERSION)
    .bind(ACTIONS_POLICY_VERSION)
    .fetch_optional(&db.pool)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let moderator_pubkey: Option<Vec<u8>> = row.try_get("moderator_pubkey")?;
    let moderator_pubkey = moderator_pubkey.ok_or_else(|| {
        DbError::InvalidData(format!(
            "Meeting V2 {session_id} has no persisted moderator"
        ))
    })?;
    Ok(Some(CurrentMeetingBoard {
        session_id,
        event_id: row.try_get("board_event_id")?,
        moderator_pubkey,
        format: row.try_get("board_format")?,
        body: row.try_get("board_content")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    }))
}

/// Load the current board after enforcing the immutable Meeting roster and
/// current security/revocation reader fence.
pub async fn get_current_board_for_reader(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    reader_pubkey: &[u8],
) -> Result<Option<CurrentMeetingBoard>> {
    validate_32_bytes(reader_pubkey, "meeting board reader pubkey")?;
    match is_meeting_reader_authorized_for_channel(db, community_id, session_id, reader_pubkey)
        .await?
    {
        Some(true) => get_current_board(db, community_id, session_id).await,
        Some(false) => Err(DbError::AccessDenied(
            "meeting board is restricted to the frozen participant roster".to_string(),
        )),
        None => Ok(None),
    }
}

fn build_board_event(
    relay_keys: &Keys,
    session_id: Uuid,
    moderator_pubkey: &[u8],
    board: &buzz_sdk::MeetingV2BoardContent,
    policy: &str,
    now: DateTime<Utc>,
) -> Result<Event> {
    let session = session_id.to_string();
    let moderator = hex::encode(moderator_pubkey);
    let tags = vec![
        parse_tag(["h", session.as_str()])?,
        parse_tag(["v", buzz_sdk::MEETING_V2_SCHEMA_VERSION])?,
        parse_tag(["policy", policy])?,
        parse_tag(["format", buzz_sdk::MEETING_V2_BOARD_FORMAT])?,
        parse_tag(["moderator", moderator.as_str()])?,
    ];
    let content = serde_json::to_string(board)?;
    let timestamp =
        u64::try_from(now.timestamp()).map_err(|_| DbError::InvalidTimestamp(now.timestamp()))?;
    EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_MEETING_BOARD as u16),
        content,
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(timestamp))
    .sign_with_keys(relay_keys)
    .map_err(|error| DbError::InvalidData(format!("sign Meeting V2 board: {error}")))
}

async fn persist_board_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    received_at: DateTime<Utc>,
) -> Result<()> {
    let created_at_secs = event.created_at.as_secs() as i64;
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
            "Meeting V2 board event {} already exists without its projection",
            event.id
        )));
    }
    Ok(())
}

fn validate_create_shape(params: &CreateMeetingV2Params<'_>) -> Result<()> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting session id must not be nil".to_string(),
        ));
    }
    if params.source_channel_id == Some(params.session_id) {
        return Err(DbError::InvalidData(
            "meeting source channel must differ from the meeting session".to_string(),
        ));
    }
    validate_32_bytes(params.host_pubkey, "host pubkey")?;
    validate_32_bytes(params.create_event_id, "create event id")?;
    let title = buzz_core::channel::canonical_channel_name(params.title);
    if title.trim().is_empty() {
        return Err(DbError::InvalidData(
            "meeting title is required".to_string(),
        ));
    }
    if title.chars().count() > 255 {
        return Err(DbError::InvalidData(
            "meeting title exceeds 255 characters".to_string(),
        ));
    }
    if !(2..=MAX_MEETING_PARTICIPANTS).contains(&params.participant_pubkeys.len()) {
        return Err(DbError::InvalidData(format!(
            "meeting requires 2-{MAX_MEETING_PARTICIPANTS} participants"
        )));
    }
    let mut unique = HashSet::with_capacity(params.participant_pubkeys.len());
    let mut host_count = 0usize;
    for pubkey in params.participant_pubkeys {
        validate_32_bytes(pubkey, "participant pubkey")?;
        if !unique.insert(pubkey.as_slice()) {
            return Err(DbError::InvalidData(format!(
                "duplicate participant: {}",
                hex::encode(pubkey)
            )));
        }
        if pubkey.as_slice() == params.host_pubkey {
            host_count += 1;
        }
    }
    if host_count != 1 {
        return Err(DbError::InvalidData(
            "Meeting V2 host must appear exactly once in the complete roster".to_string(),
        ));
    }
    Ok(())
}

fn validate_32_bytes(value: &[u8], field: &str) -> Result<()> {
    if value.len() != 32 {
        return Err(DbError::InvalidData(format!(
            "{field} must be 32 bytes, got {}",
            value.len()
        )));
    }
    Ok(())
}

fn parse_tag<const N: usize>(parts: [&str; N]) -> Result<Tag> {
    Tag::parse(parts).map_err(|error| DbError::InvalidData(format!("build meeting tag: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sqlx::PgPool;

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to Meeting V2 test database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V2 migrations");
        pool
    }

    async fn setup_isolated_pool(prefix: &str) -> (PgPool, PgPool, String) {
        let admin_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let admin = PgPool::connect(&admin_url)
            .await
            .expect("connect to Meeting V2 database server");
        let database_name = format!("{prefix}_{}", Uuid::new_v4().simple());
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE DATABASE {database_name}"
        )))
        .execute(&admin)
        .await
        .expect("create isolated Meeting V2 database");
        let slash = admin_url.rfind('/').expect("database URL has path");
        let database_url = format!("{}/{}", &admin_url[..slash], database_name);
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect isolated Meeting V2 database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V2 migrations to isolated database");
        (pool, admin, database_name)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn deployment_probe_requires_a_stable_signer_when_create_is_enabled() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool);
        assert!(db.meeting_v2_schema_ready().await.expect("schema probe"));
        assert!(!db
            .meeting_v2_deployment_ready(false, true)
            .await
            .expect("enabled Create without signer"));
        assert!(db
            .meeting_v2_deployment_ready(true, true)
            .await
            .expect("enabled Create with signer"));
    }

    #[test]
    fn deployment_probe_only_requires_v2_runtime_when_create_or_drain_needs_it() {
        assert!(!meeting_v2_runtime_required(false, false));
        assert!(meeting_v2_runtime_required(true, false));
        assert!(meeting_v2_runtime_required(false, true));
        assert!(meeting_v2_runtime_required(true, true));
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("meeting-v2-test-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert Meeting V2 test community");
        CommunityId::from_uuid(id)
    }

    async fn seed_identity(pool: &PgPool, community_id: CommunityId, pubkey: &[u8], role: &str) {
        sqlx::query(
            "INSERT INTO users (community_id, pubkey, channel_add_policy) \
             VALUES ($1, $2, 'anyone'::channel_add_policy)",
        )
        .bind(community_id.as_uuid())
        .bind(pubkey)
        .execute(pool)
        .await
        .expect("insert Meeting V2 identity");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(pubkey))
        .bind(role)
        .execute(pool)
        .await
        .expect("insert Meeting V2 Relay membership");
    }

    async fn insert_create_event_tx(
        tx: &mut Transaction<'_, Postgres>,
        community_id: CommunityId,
        session_id: Uuid,
        event: &Event,
    ) {
        let created_at_secs = event.created_at.as_secs() as i64;
        let created_at = DateTime::from_timestamp(created_at_secs, 0)
            .expect("valid Meeting V2 Create timestamp");
        sqlx::query(
            "INSERT INTO events \
                 (community_id, id, pubkey, created_at, kind, tags, content, sig, \
                  received_at, channel_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $4, $9)",
        )
        .bind(community_id.as_uuid())
        .bind(event.id.as_bytes().as_slice())
        .bind(event.pubkey.as_bytes())
        .bind(created_at)
        .bind(event.kind.as_u16() as i32)
        .bind(json!(event.tags))
        .bind(&event.content)
        .bind(event.sig.serialize().as_slice())
        .bind(session_id)
        .execute(tx.as_mut())
        .await
        .expect("insert signed Meeting V2 Create");
    }

    #[test]
    fn create_shape_requires_creator_once_in_roster() {
        let host = vec![1; 32];
        let participant = vec![2; 32];
        let event_id = vec![3; 32];
        let relay_keys = Keys::generate();
        let board = buzz_sdk::MeetingV2BoardContent {
            format: buzz_sdk::MEETING_V2_BOARD_FORMAT.to_string(),
            body: "# Goal".to_string(),
        };
        let roster = vec![host.clone(), participant];
        let params = CreateMeetingV2Params {
            community_id: CommunityId::from_uuid(Uuid::new_v4()),
            session_id: Uuid::new_v4(),
            policy: MeetingV2Policy::Board,
            title: "V2",
            description: None,
            source_channel_id: None,
            host_pubkey: &host,
            create_event_id: &event_id,
            participant_pubkeys: &roster,
            initial_board: &board,
            relay_keys: &relay_keys,
            baton_config: BatonConfig::default(),
            board_maintenance_ms: DEFAULT_BOARD_MAINTENANCE_MS,
        };
        assert!(validate_create_shape(&params).is_ok());

        let missing_host = vec![vec![4; 32], vec![5; 32]];
        assert!(validate_create_shape(&CreateMeetingV2Params {
            participant_pubkeys: &missing_host,
            ..params
        })
        .is_err());
    }

    #[test]
    fn board_event_has_no_revision_and_uses_v2_protocol_identity() {
        let keys = Keys::generate();
        let session_id = Uuid::new_v4();
        let moderator = vec![1; 32];
        let board = buzz_sdk::MeetingV2BoardContent {
            format: buzz_sdk::MEETING_V2_BOARD_FORMAT.to_string(),
            body: "# Goal\nDecide.".to_string(),
        };
        let event = build_board_event(
            &keys,
            session_id,
            &moderator,
            &board,
            BOARD_POLICY_VERSION,
            Utc::now(),
        )
        .expect("build board event");

        assert_eq!(
            event.kind.as_u16() as u32,
            buzz_core::kind::KIND_MEETING_BOARD
        );
        let tags: Vec<Vec<String>> = event
            .tags
            .iter()
            .map(|tag| tag.as_slice().iter().map(ToString::to_string).collect())
            .collect();
        assert!(tags.contains(&vec!["h".to_string(), session_id.to_string()]));
        assert!(tags.contains(&vec![
            "policy".to_string(),
            BOARD_POLICY_VERSION.to_string()
        ]));
        assert!(!tags
            .iter()
            .any(|tag| tag.first().is_some_and(|name| name.contains("revision"))));
        assert_eq!(
            serde_json::from_str::<buzz_sdk::MeetingV2BoardContent>(&event.content)
                .expect("board content"),
            board
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn create_is_atomic_pull_only_readable_and_opens_the_board_gate() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host_keys = Keys::generate();
        let participant_keys = Keys::generate();
        let outsider_keys = Keys::generate();
        let relay_keys = Keys::generate();
        let host = host_keys.public_key().to_bytes().to_vec();
        let participant = participant_keys.public_key().to_bytes().to_vec();
        let outsider = outsider_keys.public_key().to_bytes().to_vec();
        seed_identity(&pool, community_id, &host, "owner").await;
        seed_identity(&pool, community_id, &participant, "member").await;
        seed_identity(&pool, community_id, &outsider, "member").await;

        let session_id = Uuid::new_v4();
        let participant_hex = participant_keys.public_key().to_hex();
        let host_hex = host_keys.public_key().to_hex();
        let board_body = "# Goal\nDecide whether to ship.\n\n## Agenda\n- Evidence";
        let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
            session_id,
            title: "Stage one acceptance",
            description: Some("pull-only current board"),
            source_channel_id: None,
            author_pubkey: &host_hex,
            participant_pubkeys: &[participant_hex.as_str()],
            initial_board: board_body,
        })
        .expect("build Meeting V2 Create")
        .sign_with_keys(&host_keys)
        .expect("sign Meeting V2 Create");
        let board = buzz_sdk::parse_meeting_v2_board_content(&create.content)
            .expect("parse initial Meeting V2 board");
        let roster = vec![host.clone(), participant.clone()];

        let mut tx = pool.begin().await.expect("begin Meeting V2 Create");
        insert_create_event_tx(&mut tx, community_id, session_id, &create).await;
        let snapshot = create_meeting_v2_tx(
            &mut tx,
            CreateMeetingV2Params {
                community_id,
                session_id,
                policy: MeetingV2Policy::Board,
                title: "Stage one acceptance",
                description: Some("pull-only current board"),
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                participant_pubkeys: &roster,
                initial_board: &board,
                relay_keys: &relay_keys,
                baton_config: BatonConfig::default(),
                board_maintenance_ms: DEFAULT_BOARD_MAINTENANCE_MS,
            },
        )
        .await
        .expect("atomically create Meeting V2");
        tx.commit().await.expect("commit Meeting V2 Create");

        assert_eq!(snapshot.session_id, session_id);
        assert_eq!(snapshot.moderator_pubkey, host);
        assert_eq!(snapshot.participants.len(), 2);
        assert!(!db
            .meeting_v2_deployment_ready(false, false)
            .await
            .expect("active V2 without a stable signer must fail readiness"));
        assert!(db
            .meeting_v2_deployment_ready(true, false)
            .await
            .expect("closed Create can drain active V2 with a stable signer"));
        let host_board = get_current_board_for_reader(&db, community_id, session_id, &host)
            .await
            .expect("host reads current board")
            .expect("host board exists");
        let participant_board =
            get_current_board_for_reader(&db, community_id, session_id, &participant)
                .await
                .expect("participant reads current board")
                .expect("participant board exists");
        assert_eq!(host_board, participant_board);
        assert_eq!(host_board.body, board_body);
        assert_eq!(host_board.moderator_pubkey, host);
        assert_eq!(host_board.event_id, snapshot.board_event_id);
        assert!(matches!(
            get_current_board_for_reader(&db, community_id, session_id, &outsider).await,
            Err(DbError::AccessDenied(_))
        ));

        let session: (i32, String, Vec<u8>, Vec<u8>) = sqlx::query_as(
            "SELECT schema_version, floor_policy_version, host_pubkey, moderator_pubkey \
             FROM meeting_sessions WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read persisted Meeting V2 protocol");
        assert_eq!(
            session,
            (
                SCHEMA_VERSION,
                BOARD_POLICY_VERSION.to_string(),
                host.clone(),
                host.clone(),
            )
        );
        let channel_owner: Vec<u8> = sqlx::query_scalar(
            "SELECT created_by FROM channels WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read Meeting V2 Channel owner");
        assert_eq!(channel_owner, host);
        let runtime: (String, i64, i64) = sqlx::query_as(
            "SELECT runtime_phase, control_epoch, board_window \
             FROM meeting_v2_bootstrap_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read Meeting V2 runtime");
        assert_eq!(runtime, ("board_pending".to_string(), 1, 1));
        let board_event_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND channel_id = $2 AND kind = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(buzz_core::kind::KIND_MEETING_BOARD as i32)
        .fetch_one(&pool)
        .await
        .expect("count current Meeting V2 board events");
        assert_eq!(board_event_count, 1);
        let state_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT state_event_id FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read initial Meeting V2 State");
        let state_content: String =
            sqlx::query_scalar("SELECT content FROM events WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(&state_event_id)
                .fetch_one(&pool)
                .await
                .expect("read initial Meeting V2 State content");
        let state_content: Value =
            serde_json::from_str(&state_content).expect("parse initial Meeting V2 State content");
        assert!(
            state_content["board_control"].get("action").is_none(),
            "the legacy moderated-board-v1 State wire shape must remain unchanged"
        );
        let outbox_counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT \
                 count(*) FILTER (WHERE event_id = $3), \
                 count(*) FILTER (WHERE event_id = $4), \
                 count(*) FILTER (WHERE event_id = $5) \
             FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(create.id.as_bytes().as_slice())
        .bind(&snapshot.board_event_id)
        .bind(&state_event_id)
        .fetch_one(&pool)
        .await
        .expect("count Meeting V2 outbox rows");
        assert_eq!(outbox_counts, (1, 0, 1));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn stage_one_bootstrap_lazily_initializes_exactly_once() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host_keys = Keys::generate();
        let participant_keys = Keys::generate();
        let relay_keys = Keys::generate();
        let host = host_keys.public_key().to_bytes().to_vec();
        let participant = participant_keys.public_key().to_bytes().to_vec();
        seed_identity(&pool, community_id, &host, "owner").await;
        seed_identity(&pool, community_id, &participant, "member").await;

        let session_id = Uuid::new_v4();
        let host_hex = host_keys.public_key().to_hex();
        let participant_hex = participant_keys.public_key().to_hex();
        let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
            session_id,
            title: "Stage-one lazy upgrade",
            description: None,
            source_channel_id: None,
            author_pubkey: &host_hex,
            participant_pubkeys: &[&participant_hex],
            initial_board: "# Goal\nInitialize this preserved Session once.",
        })
        .expect("build stage-one V2 Create")
        .sign_with_keys(&host_keys)
        .expect("sign stage-one V2 Create");
        let board = buzz_sdk::parse_meeting_v2_board_content(&create.content)
            .expect("parse stage-one V2 Board");
        let roster = vec![host.clone(), participant];

        let mut tx = pool.begin().await.expect("begin stage-one V2 fixture");
        insert_create_event_tx(&mut tx, community_id, session_id, &create).await;
        let base = create_moderated_meeting_base_tx(
            &mut tx,
            CreateModeratedMeetingBaseParams {
                community_id,
                session_id,
                title: "Stage-one lazy upgrade",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                moderator_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                participant_pubkeys: &roster,
                schema_version: SCHEMA_VERSION,
                policy_version: BOARD_POLICY_VERSION,
            },
        )
        .await
        .expect("create stage-one V2 base");
        let board_event = build_board_event(
            &relay_keys,
            session_id,
            &host,
            &board,
            BOARD_POLICY_VERSION,
            base.created_at,
        )
        .expect("build stage-one current Board");
        persist_board_event_tx(
            &mut tx,
            community_id,
            session_id,
            &board_event,
            base.created_at,
        )
        .await
        .expect("persist stage-one current Board");
        sqlx::query(
            "INSERT INTO meeting_current_boards \
                 (community_id, session_id, board_event_id, board_format, board_content, \
                  created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $6)",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(board_event.id.as_bytes().as_slice())
        .bind(&board.format)
        .bind(&board.body)
        .bind(base.created_at)
        .execute(tx.as_mut())
        .await
        .expect("insert stage-one current Board");
        sqlx::query(
            "INSERT INTO meeting_v2_bootstrap_state \
                 (community_id, session_id, runtime_phase, control_epoch, created_at, updated_at) \
             VALUES ($1, $2, 'bootstrap_locked', 1, $3, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(base.created_at)
        .execute(tx.as_mut())
        .await
        .expect("insert stage-one bootstrap runtime");
        crate::meeting::enqueue_meeting_event_tx(
            &mut tx,
            community_id,
            session_id,
            create.id.as_bytes(),
        )
        .await
        .expect("enqueue stage-one Create");
        tx.commit().await.expect("commit stage-one V2 fixture");

        let (first, second) = tokio::join!(
            crate::meeting_baton::recover_meeting_v1(&db, community_id, session_id, &relay_keys,),
            crate::meeting_baton::recover_meeting_v1(&db, community_id, session_id, &relay_keys,),
        );
        assert!(first.expect("first lazy initializer").is_empty());
        assert!(second.expect("second lazy initializer").is_empty());

        let projection: (String, i64, i64, bool, i64, i64, i64, String) = sqlx::query_as(
            "SELECT runtime_phase, control_epoch, board_window, \
                    board_deadline_at IS NOT NULL, \
                    (SELECT count(*) FROM meeting_baton_state state \
                     WHERE state.community_id = runtime.community_id \
                       AND state.session_id = runtime.session_id), \
                    (SELECT count(*) FROM meeting_baton_state_history history \
                     WHERE history.community_id = runtime.community_id \
                       AND history.session_id = runtime.session_id \
                       AND history.transition_primary_type = 'meeting_v2_initialized'), \
                    (SELECT count(*) FROM meeting_v2_config config \
                     WHERE config.community_id = runtime.community_id \
                       AND config.session_id = runtime.session_id), \
                    (SELECT timing_profile_version FROM meeting_baton_config config \
                     WHERE config.community_id = runtime.community_id \
                       AND config.session_id = runtime.session_id) \
             FROM meeting_v2_bootstrap_state runtime \
             WHERE runtime.community_id = $1 AND runtime.session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read lazily initialized V2 runtime");
        assert_eq!(
            projection,
            (
                "board_pending".into(),
                1,
                1,
                true,
                1,
                1,
                1,
                DEFAULT_BATON_TIMING_PROFILE_VERSION.into(),
            )
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn board_update_is_fenced_pull_only_idempotent_and_enables_normal_close() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host_keys = Keys::generate();
        let participant_keys = Keys::generate();
        let relay_keys = Keys::generate();
        let host = host_keys.public_key().to_bytes().to_vec();
        let participant = participant_keys.public_key().to_bytes().to_vec();
        seed_identity(&pool, community_id, &host, "owner").await;
        seed_identity(&pool, community_id, &participant, "member").await;

        let session_id = Uuid::new_v4();
        let host_hex = host_keys.public_key().to_hex();
        let participant_hex = participant_keys.public_key().to_hex();
        let create = buzz_sdk::build_meeting_v2_create(buzz_sdk::MeetingV2CreateParams {
            session_id,
            title: "Stage two lifecycle",
            description: None,
            source_channel_id: None,
            author_pubkey: &host_hex,
            participant_pubkeys: &[participant_hex.as_str()],
            initial_board: "# Goal\nDecide whether to ship.",
        })
        .expect("build V2 Create")
        .sign_with_keys(&host_keys)
        .expect("sign V2 Create");
        let initial_board =
            buzz_sdk::parse_meeting_v2_board_content(&create.content).expect("parse initial Board");
        let roster = vec![host.clone(), participant];
        let mut tx = pool.begin().await.expect("begin V2 Create");
        insert_create_event_tx(&mut tx, community_id, session_id, &create).await;
        let created = create_meeting_v2_tx(
            &mut tx,
            CreateMeetingV2Params {
                community_id,
                session_id,
                policy: MeetingV2Policy::Board,
                title: "Stage two lifecycle",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                participant_pubkeys: &roster,
                initial_board: &initial_board,
                relay_keys: &relay_keys,
                baton_config: BatonConfig::default(),
                board_maintenance_ms: DEFAULT_BOARD_MAINTENANCE_MS,
            },
        )
        .await
        .expect("create V2 lifecycle");
        tx.commit().await.expect("commit V2 Create");

        let updated_body = "# Goal\nShip safely.\n\n## Conclusion\n- Release the API first.";
        let board_command =
            buzz_sdk::build_meeting_v2_board_action(buzz_sdk::MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: 1,
                board_window: 1,
                board: Some(updated_body),
            })
            .expect("build Board update")
            .sign_with_keys(&host_keys)
            .expect("sign Board update");
        let replacement = buzz_sdk::parse_meeting_v2_board_content(&board_command.content)
            .expect("parse Board update");
        let apply = || BoardActionTxParams {
            community_id,
            session_id,
            event: &board_command,
            relay_keys: &relay_keys,
            expected_control_epoch: 1,
            board_window: 1,
            action: BoardAction::Update(replacement.clone()),
        };
        let committed = execute_board_action(&db, apply())
            .await
            .expect("apply Board update");
        let updated_event_id = match committed.outcome {
            BoardActionOutcome::Accepted {
                state_revision,
                board_event_id,
            } => {
                assert_eq!(state_revision, 2);
                board_event_id
            }
            other => panic!("unexpected Board outcome: {other:?}"),
        };
        assert_ne!(updated_event_id, created.board_event_id);
        assert!(matches!(
            execute_board_action(&db, apply())
                .await
                .expect("replay Board update")
                .outcome,
            BoardActionOutcome::Duplicate { accepted: true, .. }
        ));

        let current = get_current_board_for_reader(&db, community_id, session_id, &host)
            .await
            .expect("read updated Board")
            .expect("updated Board exists");
        assert_eq!(current.body, updated_body);
        assert_eq!(current.event_id, updated_event_id);
        let board_counts: (i64, i64, i64) = sqlx::query_as(
            "SELECT \
                 count(*) FILTER (WHERE kind = $3), \
                 count(*) FILTER (WHERE kind = $4), \
                 (SELECT count(*) FROM meeting_event_outbox o \
                  WHERE o.community_id = $1 AND o.session_id = $2 \
                    AND o.event_id = $5) \
             FROM events WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(buzz_core::kind::KIND_MEETING_BOARD as i32)
        .bind(buzz_core::kind::KIND_MEETING_BOARD_COMMAND as i32)
        .bind(&updated_event_id)
        .fetch_one(&pool)
        .await
        .expect("count pull-only Board rows");
        assert_eq!(board_counts, (1, 0, 0));

        let close = buzz_sdk::build_meeting_v2_end(buzz_sdk::MeetingV2EndParams {
            session_id,
            create_event_id: &create.id.to_hex(),
            outcome: buzz_sdk::MeetingV2EndOutcome::Closed,
            reason_code: None,
            reason: None,
        })
        .expect("build V2 close")
        .sign_with_keys(&host_keys)
        .expect("sign V2 close");
        let mut tx = pool.begin().await.expect("begin V2 close");
        insert_create_event_tx(&mut tx, community_id, session_id, &close).await;
        assert!(matches!(
            end_meeting_v2_tx(
                &mut tx,
                EndMeetingV2Params {
                    community_id,
                    session_id,
                    actor_pubkey: &host,
                    create_event_id: create.id.as_bytes(),
                    end_event_id: close.id.as_bytes(),
                    outcome: TerminalOutcome::Closed,
                    reason_code: None,
                    action_fence: None,
                    relay_keys: &relay_keys,
                },
            )
            .await
            .expect("normally close V2"),
            EndMeetingV2Outcome::Ended(_)
        ));
        tx.commit().await.expect("commit V2 close");
        let terminal: (String, String, Option<String>, bool) = sqlx::query_as(
            "SELECT s.status, s.terminal_outcome, s.terminal_reason_code, \
                    c.archived_at IS NOT NULL \
             FROM meeting_sessions s \
             JOIN channels c ON c.community_id = s.community_id AND c.id = s.session_id \
             WHERE s.community_id = $1 AND s.session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read V2 terminal projection");
        assert_eq!(terminal, ("ended".into(), "closed".into(), None, true));
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn action_finalization_gates_close_and_supports_return_and_retry() {
        let (pool, admin, database_name) = setup_isolated_pool("buzz_meeting_actions").await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host_keys = Keys::generate();
        let participant_keys = Keys::generate();
        let relay_keys = Keys::generate();
        let host = host_keys.public_key().to_bytes().to_vec();
        let participant = participant_keys.public_key().to_bytes().to_vec();
        let host_hex = host_keys.public_key().to_hex();
        let participant_hex = participant_keys.public_key().to_hex();
        seed_identity(&pool, community_id, &host, "owner").await;
        seed_identity(&pool, community_id, &participant, "member").await;

        let session_id = Uuid::new_v4();
        let create = buzz_sdk::build_meeting_v2_actions_create(buzz_sdk::MeetingV2CreateParams {
            session_id,
            title: "Action close gate",
            description: None,
            source_channel_id: None,
            author_pubkey: &host_hex,
            participant_pubkeys: &[&participant_hex],
            initial_board: "# Goal\nAgree on an implementation and its owner.",
        })
        .expect("build action-capable V2 Create")
        .sign_with_keys(&host_keys)
        .expect("sign action-capable V2 Create");
        let initial_board = buzz_sdk::parse_meeting_v2_board_content(&create.content)
            .expect("parse action-capable initial Board");
        let roster = vec![host.clone(), participant];
        let mut tx = pool.begin().await.expect("begin action-capable Create");
        insert_create_event_tx(&mut tx, community_id, session_id, &create).await;
        let created = create_meeting_v2_tx(
            &mut tx,
            CreateMeetingV2Params {
                community_id,
                session_id,
                policy: MeetingV2Policy::Actions,
                title: "Action close gate",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                participant_pubkeys: &roster,
                initial_board: &initial_board,
                relay_keys: &relay_keys,
                baton_config: BatonConfig::default(),
                board_maintenance_ms: DEFAULT_BOARD_MAINTENANCE_MS,
            },
        )
        .await
        .expect("create action-capable V2 Meeting");
        tx.commit().await.expect("commit action-capable Create");

        let finish_board = |board_window| {
            buzz_sdk::build_meeting_v2_actions_board_action(buzz_sdk::MeetingV2BoardActionParams {
                session_id,
                expected_control_epoch: 1,
                board_window,
                board: None,
            })
            .expect("build final Board result")
            .sign_with_keys(&host_keys)
            .expect("sign final Board result")
        };
        let board_one = finish_board(1);
        assert!(matches!(
            execute_board_action(
                &db,
                BoardActionTxParams {
                    community_id,
                    session_id,
                    event: &board_one,
                    relay_keys: &relay_keys,
                    expected_control_epoch: 1,
                    board_window: 1,
                    action: BoardAction::Unchanged,
                },
            )
            .await
            .expect("complete first Board window")
            .outcome,
            BoardActionOutcome::Accepted { .. }
        ));

        // Action capability is optional per Meeting: a moderator that declares
        // no closing operations can still close directly from floor_ready.
        // Roll the successful transaction back so this fixture can continue
        // through the action-gated path below.
        let direct_close =
            buzz_sdk::build_meeting_v2_actions_end(buzz_sdk::MeetingV2ActionsEndParams {
                session_id,
                create_event_id: &create.id.to_hex(),
                outcome: buzz_sdk::MeetingV2EndOutcome::Closed,
                reason_code: None,
                reason: None,
                action_fence: None,
            })
            .expect("build no-action close")
            .sign_with_keys(&host_keys)
            .expect("sign no-action close");
        let mut direct_close_tx = pool.begin().await.expect("begin no-action close");
        insert_create_event_tx(
            &mut direct_close_tx,
            community_id,
            session_id,
            &direct_close,
        )
        .await;
        assert!(matches!(
            end_meeting_v2_tx(
                &mut direct_close_tx,
                EndMeetingV2Params {
                    community_id,
                    session_id,
                    actor_pubkey: &host,
                    create_event_id: create.id.as_bytes(),
                    end_event_id: direct_close.id.as_bytes(),
                    outcome: TerminalOutcome::Closed,
                    reason_code: None,
                    action_fence: None,
                    relay_keys: &relay_keys,
                },
            )
            .await
            .expect("close action-capable Meeting without actions"),
            EndMeetingV2Outcome::Ended(_)
        ));
        direct_close_tx
            .rollback()
            .await
            .expect("roll back no-action close probe");

        // An Agent moderator's Candidate Floor binds FINALIZE_ACTIONS to the
        // exact running DecisionAttempt. Begin must consume that attempt and
        // freeze every still-pending discussion source in the same transaction.
        let decision_attempt_id = vec![77_u8; 32];
        let pending_intent_id = vec![78_u8; 32];
        let pending_intent_event_id = vec![79_u8; 32];
        let attempt_started_event_id = vec![80_u8; 32];
        let attempt_started_at = Utc::now();
        let attempt_deadline = attempt_started_at + Duration::minutes(5);
        sqlx::query(
            "INSERT INTO meeting_speech_intents \
                 (community_id, session_id, intent_id, author_pubkey, current_event_id, \
                  basis_speech_revision, summary, state, eligible_decision_epoch) \
             VALUES ($1, $2, $3, $4, $5, 0, 'candidate discussion remains', 'pending', 1)",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&pending_intent_id)
        .bind(&roster[1])
        .bind(&pending_intent_event_id)
        .execute(&pool)
        .await
        .expect("seed pending Candidate Intent");
        let candidate_snapshot = json!({
            "candidate_refs": [{
                "source_type": "intent",
                "source_id": hex::encode(&pending_intent_id),
                "current_event_id": hex::encode(&pending_intent_event_id),
                "author_pubkey": participant_hex.clone(),
            }]
        });
        sqlx::query(
            "INSERT INTO meeting_moderator_decision_attempts \
                 (community_id, session_id, attempt_id, moderator_pubkey, control_epoch, \
                  decision_epoch, attempt_number, speech_revision, snapshot_intent_revision, \
                  snapshot_state_event_id, candidate_snapshot_json, candidate_snapshot_hash, \
                  state, started_by_event_id, started_at, deadline_at) \
             SELECT $1, $2, $3, $4, 1, 1, 1, 0, 1, state_event_id, $5, $6, \
                    'running', $7, $8, $9 \
             FROM meeting_baton_state WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&decision_attempt_id)
        .bind(&host)
        .bind(&candidate_snapshot)
        .bind(vec![81_u8; 32])
        .bind(&attempt_started_event_id)
        .bind(attempt_started_at)
        .bind(attempt_deadline)
        .execute(&pool)
        .await
        .expect("seed running moderator DecisionAttempt");
        sqlx::query(
            "UPDATE meeting_baton_state \
             SET phase = 'moderator_control', decision_epoch = 1, decision_attempt = 1, \
                 intent_revision = 1, active_decision_attempt_id = $3, \
                 moderator_decision_started_at = $4, moderator_decision_deadline = $5, \
                 next_action_at = $5 \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&decision_attempt_id)
        .bind(attempt_started_at)
        .bind(attempt_deadline)
        .execute(&pool)
        .await
        .expect("activate moderator DecisionAttempt");

        let state_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT state_event_id FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read State for first action begin");
        let begin_one =
            buzz_sdk::build_meeting_v2_action_begin(buzz_sdk::MeetingV2ActionBeginParams {
                session_id,
                expected_control_epoch: 1,
                board_window: 1,
                expected_state_event_id: &hex::encode(&state_event_id),
                board_event_id: &hex::encode(&created.board_event_id),
                expected_decision_attempt_id: Some(&hex::encode(&decision_attempt_id)),
            })
            .expect("build first action begin")
            .sign_with_keys(&host_keys)
            .expect("sign first action begin");
        let began_one = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &begin_one,
                command: crate::meeting_v2_actions::ActionCommand::Begin {
                    expected_control_epoch: 1,
                    board_window: 1,
                    expected_state_event_id: state_event_id,
                    board_event_id: created.board_event_id.clone(),
                    expected_decision_attempt_id: Some(decision_attempt_id.clone()),
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("begin first action run");
        assert!(began_one.accepted);
        let consumed_attempt: (String, Option<String>) = sqlx::query_as(
            "SELECT state, terminal_reason FROM meeting_moderator_decision_attempts \
             WHERE community_id = $1 AND session_id = $2 AND attempt_id = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&decision_attempt_id)
        .fetch_one(&pool)
        .await
        .expect("read consumed moderator DecisionAttempt");
        assert_eq!(
            consumed_attempt,
            (
                "completed".to_string(),
                Some("action_finalization".to_string())
            )
        );
        let frozen_intent_state: String = sqlx::query_scalar(
            "SELECT state FROM meeting_speech_intents \
             WHERE community_id = $1 AND session_id = $2 AND intent_id = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&pending_intent_id)
        .fetch_one(&pool)
        .await
        .expect("read frozen Candidate Intent");
        assert_eq!(frozen_intent_state, "ended");
        assert_eq!(began_one.response["details"]["frozen_intent_count"], 1);
        let action_state_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT state_event_id FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read action-capable State event id");
        let action_state_content: String =
            sqlx::query_scalar("SELECT content FROM events WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(action_state_event_id)
                .fetch_one(&pool)
                .await
                .expect("read action-capable State content");
        let action_state_content: Value = serde_json::from_str(&action_state_content)
            .expect("parse action-capable State content");
        assert_eq!(
            action_state_content["board_control"]["phase"],
            "finalizing_actions"
        );
        assert!(action_state_content["board_control"]["action"].is_object());
        let run_one = Uuid::parse_str(
            began_one.response["action_run_id"]
                .as_str()
                .expect("first action run id"),
        )
        .expect("parse first action run id");

        let early_close =
            buzz_sdk::build_meeting_v2_actions_end(buzz_sdk::MeetingV2ActionsEndParams {
                session_id,
                create_event_id: &create.id.to_hex(),
                outcome: buzz_sdk::MeetingV2EndOutcome::Closed,
                reason_code: None,
                reason: None,
                action_fence: None,
            })
            .expect("build premature close")
            .sign_with_keys(&host_keys)
            .expect("sign premature close");
        let mut tx = pool.begin().await.expect("begin premature close");
        insert_create_event_tx(&mut tx, community_id, session_id, &early_close).await;
        let premature = end_meeting_v2_tx(
            &mut tx,
            EndMeetingV2Params {
                community_id,
                session_id,
                actor_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                end_event_id: early_close.id.as_bytes(),
                outcome: TerminalOutcome::Closed,
                reason_code: None,
                action_fence: None,
                relay_keys: &relay_keys,
            },
        )
        .await;
        assert!(matches!(premature, Err(DbError::InvalidData(_))));
        tx.rollback()
            .await
            .expect("rollback rejected premature close");

        let return_event = buzz_sdk::build_meeting_v2_action_return_to_board(
            buzz_sdk::MeetingV2ActionCommandParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_one,
                    action_window: 1,
                    plan_event_id: None,
                },
            },
        )
        .expect("build return to Board")
        .sign_with_keys(&host_keys)
        .expect("sign return to Board");
        let returned = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &return_event,
                command: crate::meeting_v2_actions::ActionCommand::ReturnToBoard {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_one,
                        action_window_epoch: 1,
                        plan_event_id: None,
                    },
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("return first action run to Board");
        assert!(returned.accepted);
        let returned_runtime: (String, i64, String) = sqlx::query_as(
            "SELECT runtime.runtime_phase, runtime.board_window, run.terminal_status \
             FROM meeting_v2_bootstrap_state runtime \
             JOIN meeting_v2_action_runs run \
               ON run.community_id = runtime.community_id \
              AND run.session_id = runtime.session_id \
             WHERE runtime.community_id = $1 AND runtime.session_id = $2 \
               AND run.action_run_id = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(run_one)
        .fetch_one(&pool)
        .await
        .expect("read returned action run");
        assert_eq!(
            returned_runtime,
            (
                "board_pending".to_string(),
                2,
                "returned_to_board".to_string()
            )
        );

        let board_two = finish_board(2);
        execute_board_action(
            &db,
            BoardActionTxParams {
                community_id,
                session_id,
                event: &board_two,
                relay_keys: &relay_keys,
                expected_control_epoch: 1,
                board_window: 2,
                action: BoardAction::Unchanged,
            },
        )
        .await
        .expect("complete second Board window");
        let state_two: Vec<u8> = sqlx::query_scalar(
            "SELECT state_event_id FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read State for second action begin");
        let begin_two =
            buzz_sdk::build_meeting_v2_action_begin(buzz_sdk::MeetingV2ActionBeginParams {
                session_id,
                expected_control_epoch: 1,
                board_window: 2,
                expected_state_event_id: &hex::encode(&state_two),
                board_event_id: &hex::encode(&created.board_event_id),
                expected_decision_attempt_id: None,
            })
            .expect("build second action begin")
            .sign_with_keys(&host_keys)
            .expect("sign second action begin");
        let began_two = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &begin_two,
                command: crate::meeting_v2_actions::ActionCommand::Begin {
                    expected_control_epoch: 1,
                    board_window: 2,
                    expected_state_event_id: state_two,
                    board_event_id: created.board_event_id.clone(),
                    expected_decision_attempt_id: None,
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("begin second action run");
        let run_two = Uuid::parse_str(
            began_two.response["action_run_id"]
                .as_str()
                .expect("second action run id"),
        )
        .expect("parse second action run id");

        let action_id = Uuid::new_v4();
        let requirement_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let plan = buzz_sdk::MeetingV2ActionPlan {
            version: buzz_sdk::MEETING_V2_ACTION_PLAN_VERSION,
            action_run_id: run_two,
            board_event_id: hex::encode(&created.board_event_id),
            items: vec![buzz_sdk::MeetingV2ActionItem {
                action_id,
                summary: "Implement the accepted design".to_string(),
                assignee_pubkey: participant_hex,
            }],
            steps: vec![
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: None,
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateRequirement,
                    target_object_id: requirement_id,
                    payload: json!({"title": "Accepted design"}),
                },
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: Some(action_id),
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewCreateWork,
                    target_object_id: work_id,
                    payload: json!({
                        "title": "Implement the accepted design",
                        "requirement_id": requirement_id
                    }),
                },
                buzz_sdk::MeetingV2ActionStep {
                    step_id: Uuid::new_v4(),
                    action_id: Some(action_id),
                    kind: buzz_sdk::MeetingV2ActionStepKind::ProjectViewSetWorkResponsibility,
                    target_object_id: work_id,
                    payload: json!({}),
                },
            ],
        };
        let plan_event =
            buzz_sdk::build_meeting_v2_action_plan(buzz_sdk::MeetingV2ActionPlanParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_two,
                    action_window: 1,
                    plan_event_id: None,
                },
                plan: &plan,
            })
            .expect("build frozen action plan")
            .sign_with_keys(&host_keys)
            .expect("sign frozen action plan");
        let planned = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &plan_event,
                command: crate::meeting_v2_actions::ActionCommand::Plan {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_two,
                        action_window_epoch: 1,
                        plan_event_id: None,
                    },
                    plan,
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("freeze action plan");
        assert!(planned.accepted);
        let plan_event_id = plan_event.id.as_bytes().to_vec();

        let incomplete_event =
            buzz_sdk::build_meeting_v2_action_complete(buzz_sdk::MeetingV2ActionCommandParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_two,
                    action_window: 1,
                    plan_event_id: Some(&plan_event.id.to_hex()),
                },
            })
            .expect("build incomplete action completion")
            .sign_with_keys(&host_keys)
            .expect("sign incomplete action completion");
        let incomplete = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &incomplete_event,
                command: crate::meeting_v2_actions::ActionCommand::Complete {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_two,
                        action_window_epoch: 1,
                        plan_event_id: Some(plan_event_id.clone()),
                    },
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("reject incomplete action completion");
        assert!(!incomplete.accepted);
        assert_eq!(incomplete.outcome_code, "action_steps_incomplete");

        let blocked_event =
            buzz_sdk::build_meeting_v2_action_block(buzz_sdk::MeetingV2ActionBlockParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_two,
                    action_window: 1,
                    plan_event_id: Some(&plan_event.id.to_hex()),
                },
                reason_code: "provider_failure",
                reason: Some("simulated stage-one failure"),
            })
            .expect("build action block")
            .sign_with_keys(&host_keys)
            .expect("sign action block");
        let blocked = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &blocked_event,
                command: crate::meeting_v2_actions::ActionCommand::Block {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_two,
                        action_window_epoch: 1,
                        plan_event_id: Some(plan_event_id.clone()),
                    },
                    reason_code: "provider_failure".to_string(),
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("block action run");
        assert!(blocked.accepted);

        let retry_event =
            buzz_sdk::build_meeting_v2_action_retry(buzz_sdk::MeetingV2ActionCommandParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_two,
                    action_window: 1,
                    plan_event_id: Some(&plan_event.id.to_hex()),
                },
            })
            .expect("build action retry")
            .sign_with_keys(&host_keys)
            .expect("sign action retry");
        let retried = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &retry_event,
                command: crate::meeting_v2_actions::ActionCommand::Retry {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_two,
                        action_window_epoch: 1,
                        plan_event_id: Some(plan_event_id.clone()),
                    },
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("retry action run");
        assert!(retried.accepted);
        assert_eq!(retried.response["action_window_epoch"].as_i64(), Some(2));

        // Directly changing step status is not sufficient proof of an external
        // effect. Completion must still fail without exact accepted attempts
        // and verified Project View projections.
        sqlx::query(
            "UPDATE meeting_v2_action_steps \
             SET status = 'applied', accepted_project_revision = 10, updated_at = clock_timestamp() \
             WHERE community_id = $1 AND session_id = $2 AND action_run_id = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(run_two)
        .execute(&pool)
        .await
        .expect("simulate unverified Project View statuses");

        let complete_event =
            buzz_sdk::build_meeting_v2_action_complete(buzz_sdk::MeetingV2ActionCommandParams {
                session_id,
                fence: buzz_sdk::MeetingV2ActionRunFence {
                    action_run_id: run_two,
                    action_window: 2,
                    plan_event_id: Some(&plan_event.id.to_hex()),
                },
            })
            .expect("build action completion")
            .sign_with_keys(&host_keys)
            .expect("sign action completion");
        let completed = crate::meeting_v2_actions::execute_action_command(
            &db,
            crate::meeting_v2_actions::ActionCommandTxParams {
                community_id,
                session_id,
                event: &complete_event,
                command: crate::meeting_v2_actions::ActionCommand::Complete {
                    fence: crate::meeting_v2_actions::ActionRunFence {
                        action_run_id: run_two,
                        action_window_epoch: 2,
                        plan_event_id: Some(plan_event_id.clone()),
                    },
                },
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("reject completion without Project evidence");
        assert!(!completed.accepted);
        assert_eq!(completed.outcome_code, "action_projection_mismatch");

        drop(db);
        pool.close().await;
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE {database_name} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop isolated Meeting V2 database");
        admin.close().await;
    }
}
