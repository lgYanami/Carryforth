//! Meeting V0 lifecycle persistence.
//!
//! A meeting reuses a private stream channel, but its identity, frozen roster,
//! and terminal lifecycle are committed as one transaction with the signed
//! command event.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use nostr::Keys;
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, Result};
use buzz_core::CommunityId;

/// Maximum number of identities in a Meeting roster.
pub const MAX_MEETING_PARTICIPANTS: usize = 12;
/// Maximum number of Agent identities in a Meeting roster.
pub const MAX_MEETING_AGENTS: usize = 8;

/// Parameters for atomically creating a Meeting V0 session.
pub struct CreateMeetingParams<'a> {
    /// Community that owns the meeting.
    pub community_id: CommunityId,
    /// Stable meeting identity; also the backing channel UUID.
    pub session_id: Uuid,
    /// Human-readable meeting title.
    pub title: &'a str,
    /// Optional meeting description.
    pub description: Option<&'a str>,
    /// Optional source channel used only as a navigation/context reference.
    pub source_channel_id: Option<Uuid>,
    /// Pubkey of the signed Meeting Create event author.
    pub host_pubkey: &'a [u8],
    /// Event id of the signed Meeting Create command.
    pub create_event_id: &'a [u8],
    /// Complete participant set, including the host exactly once.
    pub participant_pubkeys: &'a [Vec<u8>],
}

/// A participant and their authoritative channel role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingParticipant {
    /// Participant public key bytes.
    pub pubkey: Vec<u8>,
    /// Projected channel role (`owner`, `member`, or `bot`).
    pub role: String,
}

/// Durable Meeting V0 lifecycle projection.
#[derive(Debug, Clone)]
pub struct MeetingRecord {
    /// Stable meeting identity and backing channel UUID.
    pub session_id: Uuid,
    /// Event id of the Meeting Create command.
    pub create_event_id: Vec<u8>,
    /// Host/creator public key.
    pub host_pubkey: Vec<u8>,
    /// Optional source channel reference.
    pub source_channel_id: Option<Uuid>,
    /// Protocol schema version.
    pub schema_version: i32,
    /// Lifecycle status (`active` or `ended`).
    pub status: String,
    /// Time the meeting transaction committed its projection.
    pub created_at: DateTime<Utc>,
    /// End time for a terminal meeting.
    pub ended_at: Option<DateTime<Utc>>,
    /// Identity that ended the meeting.
    pub ended_by: Option<Vec<u8>>,
    /// Event id of the Meeting End command.
    pub end_event_id: Option<Vec<u8>>,
    /// Current speech round, starting at 1.
    pub current_round: i64,
    /// Monotonic session-wide floor revision.
    pub floor_revision: i64,
    /// Persisted winner-selection policy version.
    pub floor_policy_version: String,
}

/// Persisted protocol discriminator used before policy-specific dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingPolicy {
    /// Protocol schema version.
    pub schema_version: i32,
    /// Frozen floor-control policy identifier.
    pub floor_policy_version: String,
    /// V1 moderator, absent for V0.
    pub moderator_pubkey: Option<Vec<u8>>,
}

/// Outcome of an idempotent Meeting End mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndMeetingOutcome {
    /// This command transitioned the meeting from active to ended.
    Ended,
    /// The meeting was already terminal; no state was changed.
    AlreadyEnded,
    /// A real roster revocation won the Session lock and committed a
    /// Relay-authored terminal transition instead of this manual command.
    ParticipantRevoked,
}

/// Normalized terminal outcome exposed to Project Context coordinate checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeetingCoordinateTerminalOutcome {
    /// The verified Meeting ended normally.
    Closed,
    /// The verified Meeting ended abnormally or by administrative action.
    Aborted,
}

/// Security-neutral lifecycle resolution for a prospective Meeting coordinate.
///
/// Callers must authorize the Community actor before mapping these variants to
/// user-visible diagnostics. `MissingOrForeign` intentionally combines absent
/// and cross-Community identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeetingCoordinateResolution {
    /// The Meeting has a verified terminal Create -> State -> End chain.
    Terminal {
        /// Stable Meeting identity.
        meeting_id: Uuid,
        /// Normalized legacy/current terminal outcome.
        normalized_outcome: MeetingCoordinateTerminalOutcome,
        /// Terminal state projection revision.
        state_revision: u64,
        /// Signed Meeting Create event ID.
        create_event_id: Vec<u8>,
        /// Relay-signed terminal Meeting State event ID.
        state_event_id: Vec<u8>,
        /// Signed Meeting End event ID.
        end_event_id: Vec<u8>,
    },
    /// The Meeting is active, but its formal discussion and Board are frozen
    /// while the current action run materializes the decided outputs.
    FinalizingActions {
        /// Stable Meeting identity.
        meeting_id: Uuid,
        /// Current Relay-signed Meeting State revision.
        state_revision: u64,
        /// Current Relay-signed Meeting State event ID.
        state_event_id: Vec<u8>,
        /// Frozen current Board event ID referenced by the action run.
        board_event_id: Vec<u8>,
        /// Current non-terminal action run.
        action_run_id: Uuid,
        /// Runtime/action/state control epoch.
        control_epoch: u64,
        /// Runtime/action frozen Board window.
        board_window: u64,
    },
    /// The Meeting exists but is active outside a verified action-finalization window.
    Active,
    /// The UUID belongs to an ordinary Channel in this Community.
    OrdinaryChannel,
    /// No Meeting or Channel with this identity exists in this Community.
    MissingOrForeign,
    /// A Meeting-shaped row exists but its terminal evidence is incomplete or inconsistent.
    InvalidTerminal,
}

/// Resolve and lock one Meeting coordinate inside a caller-owned transaction.
///
/// This function does not authorize the caller. It is intended for the
/// Project Context attach path after Community write authorization has passed.
pub async fn resolve_meeting_coordinate_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    meeting_id: Uuid,
) -> Result<MeetingCoordinateResolution> {
    // Meeting mutations lock Session before Channel. Keep the same order here
    // so a concurrent End and Project Context attach cannot deadlock.
    let session = sqlx::query(
        "SELECT create_event_id, host_pubkey, schema_version, floor_policy_version, \
                status, ended_by, end_event_id, current_round, terminal_outcome \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(session) = session else {
        let channel = sqlx::query(
            "SELECT room_kind FROM channels \
             WHERE community_id = $1 AND id = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(meeting_id)
        .fetch_optional(tx.as_mut())
        .await?;
        return Ok(match channel {
            Some(channel) if channel.try_get::<String, _>("room_kind")? != "meeting" => {
                MeetingCoordinateResolution::OrdinaryChannel
            }
            Some(_) => MeetingCoordinateResolution::InvalidTerminal,
            None => MeetingCoordinateResolution::MissingOrForeign,
        });
    };

    let channel = sqlx::query(
        "SELECT room_kind, deleted_at FROM channels \
         WHERE community_id = $1 AND id = $2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(channel) = channel else {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    };
    let room_kind: String = channel.try_get("room_kind")?;
    if room_kind != "meeting" {
        return Ok(MeetingCoordinateResolution::OrdinaryChannel);
    }
    if channel
        .try_get::<Option<DateTime<Utc>>, _>("deleted_at")?
        .is_some()
    {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    }

    let status: String = session.try_get("status")?;
    if status == "active" {
        let schema_version: i32 = session.try_get("schema_version")?;
        let floor_policy_version: String = session.try_get("floor_policy_version")?;
        if schema_version == crate::meeting_v2::SCHEMA_VERSION
            && floor_policy_version == crate::meeting_v2::ACTIONS_POLICY_VERSION
        {
            let create_event_id: Vec<u8> = session.try_get("create_event_id")?;
            let host_pubkey: Vec<u8> = session.try_get("host_pubkey")?;
            if let Some(resolution) = resolve_finalizing_meeting_coordinate_tx(
                tx,
                community_id,
                meeting_id,
                &create_event_id,
                &host_pubkey,
            )
            .await?
            {
                return Ok(resolution);
            }
        }
        return Ok(MeetingCoordinateResolution::Active);
    }
    if status != "ended" {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    }

    let create_event_id: Vec<u8> = session.try_get("create_event_id")?;
    let host_pubkey: Vec<u8> = session.try_get("host_pubkey")?;
    let ended_by: Option<Vec<u8>> = session.try_get("ended_by")?;
    let end_event_id: Option<Vec<u8>> = session.try_get("end_event_id")?;
    let schema_version: i32 = session.try_get("schema_version")?;
    let Some(ended_by) = ended_by else {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    };
    let Some(end_event_id) = end_event_id else {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    };
    if create_event_id.len() != 32 || ended_by.len() != 32 || end_event_id.len() != 32 {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    }

    let (state_revision, state_event_id, state_is_terminal) = match schema_version {
        1 => {
            let current_round: i64 = session.try_get("current_round")?;
            let state = sqlx::query(
                "SELECT floor_revision, state_event_id, phase, outcome \
                 FROM meeting_rounds \
                 WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
                 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(meeting_id)
            .bind(current_round)
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(state) = state else {
                return Ok(MeetingCoordinateResolution::InvalidTerminal);
            };
            let revision: i64 = state.try_get("floor_revision")?;
            let event_id: Vec<u8> = state.try_get("state_event_id")?;
            let phase: String = state.try_get("phase")?;
            let outcome: Option<String> = state.try_get("outcome")?;
            (
                revision,
                event_id,
                phase == "closed" && outcome.as_deref() == Some("ended"),
            )
        }
        2 | 3 => {
            let state = sqlx::query(
                "SELECT state_revision, state_event_id, phase FROM meeting_baton_state \
                 WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .bind(meeting_id)
            .fetch_optional(tx.as_mut())
            .await?;
            let Some(state) = state else {
                return Ok(MeetingCoordinateResolution::InvalidTerminal);
            };
            let revision: i64 = state.try_get("state_revision")?;
            let event_id: Vec<u8> = state.try_get("state_event_id")?;
            let phase: String = state.try_get("phase")?;
            (revision, event_id, phase == "ended")
        }
        _ => return Ok(MeetingCoordinateResolution::InvalidTerminal),
    };
    let Ok(state_revision) = u64::try_from(state_revision) else {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    };
    if state_revision == 0 || state_event_id.len() != 32 || !state_is_terminal {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    }

    let create_valid = meeting_coordinate_event_exists_tx(
        tx,
        community_id,
        meeting_id,
        &create_event_id,
        buzz_core::kind::KIND_MEETING_CREATE,
        Some(&host_pubkey),
    )
    .await?;
    let state_valid = meeting_coordinate_event_exists_tx(
        tx,
        community_id,
        meeting_id,
        &state_event_id,
        buzz_core::kind::KIND_MEETING_STATE,
        None,
    )
    .await?;
    let end_valid = meeting_coordinate_event_exists_tx(
        tx,
        community_id,
        meeting_id,
        &end_event_id,
        buzz_core::kind::KIND_MEETING_END,
        Some(&ended_by),
    )
    .await?;
    if !create_valid || !state_valid || !end_valid {
        return Ok(MeetingCoordinateResolution::InvalidTerminal);
    }

    let normalized_outcome = match schema_version {
        3 => match session
            .try_get::<Option<String>, _>("terminal_outcome")?
            .as_deref()
        {
            Some("closed") => MeetingCoordinateTerminalOutcome::Closed,
            Some("aborted") => MeetingCoordinateTerminalOutcome::Aborted,
            _ => return Ok(MeetingCoordinateResolution::InvalidTerminal),
        },
        1 | 2 if ended_by == host_pubkey => MeetingCoordinateTerminalOutcome::Closed,
        1 | 2 => MeetingCoordinateTerminalOutcome::Aborted,
        _ => return Ok(MeetingCoordinateResolution::InvalidTerminal),
    };

    Ok(MeetingCoordinateResolution::Terminal {
        meeting_id,
        normalized_outcome,
        state_revision,
        create_event_id,
        state_event_id,
        end_event_id,
    })
}

async fn resolve_finalizing_meeting_coordinate_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    meeting_id: Uuid,
    create_event_id: &[u8],
    host_pubkey: &[u8],
) -> Result<Option<MeetingCoordinateResolution>> {
    if create_event_id.len() != 32
        || host_pubkey.len() != 32
        || !meeting_coordinate_event_exists_tx(
            tx,
            community_id,
            meeting_id,
            create_event_id,
            buzz_core::kind::KIND_MEETING_CREATE,
            Some(host_pubkey),
        )
        .await?
    {
        return Ok(None);
    }

    let runtime = sqlx::query(
        "SELECT runtime_phase, control_epoch, board_window \
         FROM meeting_v2_bootstrap_state \
         WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    if runtime.try_get::<String, _>("runtime_phase")? != "finalizing_actions" {
        return Ok(None);
    }
    let runtime_control_epoch: i64 = runtime.try_get("control_epoch")?;
    let runtime_board_window: i64 = runtime.try_get("board_window")?;

    let run = sqlx::query(
        "SELECT action_run_id, begin_event_id, board_event_id, control_epoch, board_window, \
                action_condition \
         FROM meeting_v2_action_runs \
         WHERE community_id = $1 AND session_id = $2 AND terminal_status IS NULL \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(run) = run else {
        return Ok(None);
    };
    let action_run_id: Uuid = run.try_get("action_run_id")?;
    let begin_event_id: Vec<u8> = run.try_get("begin_event_id")?;
    let board_event_id: Vec<u8> = run.try_get("board_event_id")?;
    let run_control_epoch: i64 = run.try_get("control_epoch")?;
    let run_board_window: i64 = run.try_get("board_window")?;
    let action_condition: String = run.try_get("action_condition")?;
    if begin_event_id.len() != 32
        || board_event_id.len() != 32
        || run_control_epoch <= 0
        || run_board_window <= 0
        || runtime_control_epoch != run_control_epoch
        || runtime_board_window != run_board_window
        || !matches!(action_condition.as_str(), "runnable" | "blocked")
        || !meeting_coordinate_event_exists_tx(
            tx,
            community_id,
            meeting_id,
            &begin_event_id,
            buzz_core::kind::KIND_MEETING_ACTION_COMMAND,
            Some(host_pubkey),
        )
        .await?
    {
        return Ok(None);
    }

    let current_board_event_id: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT board_event_id FROM meeting_current_boards \
         WHERE community_id = $1 AND session_id = $2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    if current_board_event_id.as_deref() != Some(board_event_id.as_slice())
        || !meeting_coordinate_event_exists_tx(
            tx,
            community_id,
            meeting_id,
            &board_event_id,
            buzz_core::kind::KIND_MEETING_BOARD,
            Some(host_pubkey),
        )
        .await?
    {
        return Ok(None);
    }

    let state = sqlx::query(
        "SELECT current_state.phase, current_state.state_revision, \
                current_state.state_event_id, current_state.control_epoch, \
                current_state.active_offer_id, current_state.active_grant_id, \
                current_state.active_decision_attempt_id, current_state.next_action_at, \
                history.transition_primary_type, history.transition_effects_json \
         FROM meeting_baton_state current_state \
         JOIN meeting_baton_state_history history \
           ON history.community_id = current_state.community_id \
          AND history.session_id = current_state.session_id \
          AND history.state_revision = current_state.state_revision \
          AND history.state_event_id = current_state.state_event_id \
         WHERE current_state.community_id = $1 AND current_state.session_id = $2 \
         FOR UPDATE OF current_state, history",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(state) = state else {
        return Ok(None);
    };
    let phase: String = state.try_get("phase")?;
    let state_revision: i64 = state.try_get("state_revision")?;
    let state_event_id: Vec<u8> = state.try_get("state_event_id")?;
    let state_control_epoch: i64 = state.try_get("control_epoch")?;
    let active_offer_id: Option<Vec<u8>> = state.try_get("active_offer_id")?;
    let active_grant_id: Option<Vec<u8>> = state.try_get("active_grant_id")?;
    let active_decision_attempt_id: Option<Vec<u8>> =
        state.try_get("active_decision_attempt_id")?;
    let next_action_at: Option<DateTime<Utc>> = state.try_get("next_action_at")?;
    let transition_primary_type: String = state.try_get("transition_primary_type")?;
    let transition_effects: serde_json::Value = state.try_get("transition_effects_json")?;
    let Ok(state_revision) = u64::try_from(state_revision) else {
        return Ok(None);
    };
    let Ok(control_epoch) = u64::try_from(run_control_epoch) else {
        return Ok(None);
    };
    let Ok(board_window) = u64::try_from(run_board_window) else {
        return Ok(None);
    };
    if state_revision == 0
        || state_event_id.len() != 32
        || phase != "moderator_idle"
        || state_control_epoch != run_control_epoch
        || active_offer_id.is_some()
        || active_grant_id.is_some()
        || active_decision_attempt_id.is_some()
        || next_action_at.is_some()
        || !action_transition_matches(
            &transition_primary_type,
            &transition_effects,
            action_run_id,
            &action_condition,
        )
        || !meeting_action_state_event_matches_tx(
            tx,
            community_id,
            meeting_id,
            &state_event_id,
            state_revision,
            run_control_epoch,
            run_board_window,
            &board_event_id,
            action_run_id,
            &action_condition,
            &transition_primary_type,
            &transition_effects,
        )
        .await?
    {
        return Ok(None);
    }

    Ok(Some(MeetingCoordinateResolution::FinalizingActions {
        meeting_id,
        state_revision,
        state_event_id,
        board_event_id,
        action_run_id,
        control_epoch,
        board_window,
    }))
}

#[allow(clippy::too_many_arguments)]
async fn meeting_action_state_event_matches_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    meeting_id: Uuid,
    state_event_id: &[u8],
    state_revision: u64,
    control_epoch: i64,
    board_window: i64,
    board_event_id: &[u8],
    action_run_id: Uuid,
    action_condition: &str,
    transition_primary_type: &str,
    transition_effects: &serde_json::Value,
) -> Result<bool> {
    let content: Option<String> = sqlx::query_scalar(
        "SELECT content FROM events \
         WHERE community_id = $1 AND channel_id = $2 AND id = $3 AND kind = $4 \
           AND deleted_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .bind(state_event_id)
    .bind(buzz_core::kind::KIND_MEETING_STATE as i32)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(content) = content else {
        return Ok(false);
    };
    let Ok(content) = serde_json::from_str::<serde_json::Value>(&content) else {
        return Ok(false);
    };
    let expected_run = action_run_id.to_string();
    let expected_run_hex = hex::encode(action_run_id.as_bytes());
    let expected_board_hex = hex::encode(board_event_id);
    let transition = &content["transition"];
    let board_control = &content["board_control"];
    let action = &board_control["action"];
    Ok(
        content.get("phase").and_then(serde_json::Value::as_str) == Some("moderator_idle")
            && content
                .get("state_revision")
                .and_then(serde_json::Value::as_u64)
                == Some(state_revision)
            && content
                .get("control_epoch")
                .and_then(serde_json::Value::as_i64)
                == Some(control_epoch)
            && transition
                .get("primary_type")
                .and_then(serde_json::Value::as_str)
                == Some(transition_primary_type)
            && transition
                .get("primary_object_id")
                .and_then(serde_json::Value::as_str)
                == Some(expected_run_hex.as_str())
            && transition.get("effects") == Some(transition_effects)
            && board_control
                .get("phase")
                .and_then(serde_json::Value::as_str)
                == Some("finalizing_actions")
            && board_control
                .get("control_epoch")
                .and_then(serde_json::Value::as_i64)
                == Some(control_epoch)
            && board_control
                .get("board_window")
                .and_then(serde_json::Value::as_i64)
                == Some(board_window)
            && action
                .get("action_run_id")
                .and_then(serde_json::Value::as_str)
                == Some(expected_run.as_str())
            && action
                .get("board_event_id")
                .and_then(serde_json::Value::as_str)
                == Some(expected_board_hex.as_str())
            && action
                .get("control_epoch")
                .and_then(serde_json::Value::as_i64)
                == Some(control_epoch)
            && action
                .get("board_window")
                .and_then(serde_json::Value::as_i64)
                == Some(board_window)
            && action.get("condition").and_then(serde_json::Value::as_str)
                == Some(action_condition)
            && action
                .get("terminal_status")
                .is_some_and(serde_json::Value::is_null),
    )
}

fn action_transition_matches(
    primary_type: &str,
    effects: &serde_json::Value,
    action_run_id: Uuid,
    action_condition: &str,
) -> bool {
    if !matches!(
        primary_type,
        "action_finalization_began"
            | "action_lease_renewed"
            | "action_blocked"
            | "action_retried"
            | "action_deadline_exceeded"
            | "action_lease_expired"
            | "action_operator_deadline_exceeded"
    ) {
        return false;
    }
    let expected_run = action_run_id.to_string();
    effects.as_array().is_some_and(|effects| {
        effects.iter().any(|effect| {
            effect.get("type").and_then(serde_json::Value::as_str) == Some(primary_type)
                && effect
                    .get("object_type")
                    .and_then(serde_json::Value::as_str)
                    == Some("meeting_action_run")
                && effect.get("object_id").and_then(serde_json::Value::as_str)
                    == Some(expected_run.as_str())
                && effect.get("to").and_then(serde_json::Value::as_str) == Some(action_condition)
        })
    })
}

async fn meeting_coordinate_event_exists_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    meeting_id: Uuid,
    event_id: &[u8],
    kind: u32,
    expected_pubkey: Option<&[u8]>,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events \
         WHERE community_id = $1 AND channel_id = $2 AND id = $3 AND kind = $4 \
           AND deleted_at IS NULL AND ($5::bytea IS NULL OR pubkey = $5))",
    )
    .bind(community_id.as_uuid())
    .bind(meeting_id)
    .bind(event_id)
    .bind(i32::try_from(kind).map_err(|_| DbError::InvalidData("Meeting kind overflow".into()))?)
    .bind(expected_pubkey)
    .fetch_one(tx.as_mut())
    .await?)
}

/// Parameters for atomically ending a Meeting V0 session.
pub struct EndMeetingParams<'a> {
    /// Community that owns the meeting.
    pub community_id: CommunityId,
    /// Meeting/channel UUID.
    pub session_id: Uuid,
    /// Pubkey authoring the Meeting End command.
    pub actor_pubkey: &'a [u8],
    /// Create-event id referenced by the Meeting End command.
    pub create_event_id: &'a [u8],
    /// Event id of the Meeting End command.
    pub end_event_id: &'a [u8],
    /// Relay identity used when lazy roster recovery must win over the manual
    /// End command.
    pub relay_keys: &'a Keys,
}

/// Create a private stream meeting, its complete roster, and lifecycle
/// projection inside the caller's open transaction.
///
/// The caller is responsible for inserting the signed Meeting Create event in
/// the same transaction before committing.
pub async fn create_meeting_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: CreateMeetingParams<'_>,
) -> Result<(MeetingRecord, Vec<MeetingParticipant>)> {
    validate_create_shape(&params)?;

    let title = buzz_core::channel::canonical_channel_name(params.title);
    let mut participants = Vec::with_capacity(params.participant_pubkeys.len());
    let mut agent_count = 0usize;

    crate::meeting_community_read::ensure_meeting_create_allowed_tx(tx, params.community_id)
        .await?;
    validate_community_readable_source_tx(tx, params.community_id, params.source_channel_id)
        .await?;

    for pubkey in params.participant_pubkeys {
        let pubkey_hex = hex::encode(pubkey);
        let relay_membership: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2 \
             FOR KEY SHARE",
        )
        .bind(params.community_id.as_uuid())
        .bind(&pubkey_hex)
        .fetch_optional(&mut **tx)
        .await?;
        if relay_membership.is_none() {
            return Err(DbError::AccessDenied(format!(
                "participant {pubkey_hex} is not a member of this community"
            )));
        }
        if has_active_ban_tx(tx, params.community_id, pubkey).await? {
            return Err(DbError::AccessDenied(format!(
                "participant {pubkey_hex} is banned from this community"
            )));
        }

        let is_archived: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM archived_identities \
                 WHERE community_id = $1 AND pubkey = $2 \
             )",
        )
        .bind(params.community_id.as_uuid())
        .bind(&pubkey_hex)
        .fetch_one(&mut **tx)
        .await?;
        if is_archived {
            return Err(DbError::AccessDenied(format!(
                "participant {pubkey_hex} is archived"
            )));
        }

        let identity = sqlx::query(
            "SELECT agent_owner_pubkey, deactivated_at, \
                    channel_add_policy::text AS channel_add_policy \
             FROM users WHERE community_id = $1 AND pubkey = $2 \
             FOR SHARE",
        )
        .bind(params.community_id.as_uuid())
        .bind(pubkey)
        .fetch_optional(&mut **tx)
        .await?;

        let (is_agent, agent_owner, add_policy) = match identity {
            Some(row) => {
                let owner: Option<Vec<u8>> = row.try_get("agent_owner_pubkey")?;
                let deactivated_at: Option<DateTime<Utc>> = row.try_get("deactivated_at")?;
                if deactivated_at.is_some() {
                    return Err(DbError::AccessDenied(format!(
                        "participant {pubkey_hex} is deactivated"
                    )));
                }
                let policy: String = row.try_get("channel_add_policy")?;
                (owner.is_some(), owner, policy)
            }
            None => (false, None, "anyone".to_string()),
        };

        if is_agent {
            if let Some(owner_pubkey) = agent_owner.as_deref() {
                let owner = sqlx::query(
                    "SELECT deactivated_at FROM users \
                     WHERE community_id = $1 AND pubkey = $2 \
                     FOR SHARE",
                )
                .bind(params.community_id.as_uuid())
                .bind(owner_pubkey)
                .fetch_optional(&mut **tx)
                .await?;
                let owner_is_active = match owner {
                    Some(row) => row
                        .try_get::<Option<DateTime<Utc>>, _>("deactivated_at")?
                        .is_none(),
                    None => false,
                };
                if !owner_is_active {
                    return Err(DbError::AccessDenied(format!(
                        "participant {pubkey_hex} has no active authoritative owner"
                    )));
                }
                if has_active_ban_tx(tx, params.community_id, owner_pubkey).await? {
                    return Err(DbError::AccessDenied(format!(
                        "participant {pubkey_hex} has a banned owner"
                    )));
                }
            }
            agent_count += 1;
            if pubkey.as_slice() != params.host_pubkey {
                match add_policy.as_str() {
                    "anyone" => {}
                    "owner_only" if agent_owner.as_deref() == Some(params.host_pubkey) => {}
                    "owner_only" => {
                        return Err(DbError::AccessDenied(format!(
                            "participant {pubkey_hex} only allows its owner to add it"
                        )));
                    }
                    "nobody" => {
                        return Err(DbError::AccessDenied(format!(
                            "participant {pubkey_hex} does not allow channel additions"
                        )));
                    }
                    other => {
                        return Err(DbError::InvalidData(format!(
                            "participant {pubkey_hex} has unknown channel add policy {other}"
                        )));
                    }
                }
            }
        }

        let role = if pubkey.as_slice() == params.host_pubkey {
            "owner"
        } else if is_agent {
            "bot"
        } else {
            "member"
        };
        participants.push(MeetingParticipant {
            pubkey: pubkey.clone(),
            role: role.to_string(),
        });
    }

    if agent_count > MAX_MEETING_AGENTS {
        return Err(DbError::InvalidData(format!(
            "meeting supports at most {MAX_MEETING_AGENTS} agents"
        )));
    }

    let channel_insert = sqlx::query(
        "INSERT INTO channels \
             (id, community_id, name, channel_type, visibility, description, \
              created_by, max_members, room_kind) \
         VALUES ($1, $2, $3, 'stream', 'private', $4, $5, $6, 'meeting') \
         ON CONFLICT (community_id, id) DO NOTHING",
    )
    .bind(params.session_id)
    .bind(params.community_id.as_uuid())
    .bind(title)
    .bind(params.description)
    .bind(params.host_pubkey)
    .bind(MAX_MEETING_PARTICIPANTS as i32)
    .execute(&mut **tx)
    .await?;
    if channel_insert.rows_affected() == 0 {
        return Err(DbError::InvalidData(format!(
            "meeting session already exists: {}",
            params.session_id
        )));
    }

    for participant in &participants {
        sqlx::query(
            "INSERT INTO channel_members \
                 (community_id, channel_id, pubkey, role, invited_by) \
             VALUES ($1, $2, $3, $4::member_role, $5)",
        )
        .bind(params.community_id.as_uuid())
        .bind(params.session_id)
        .bind(&participant.pubkey)
        .bind(&participant.role)
        .bind(params.host_pubkey)
        .execute(&mut **tx)
        .await?;
    }

    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO meeting_sessions \
             (community_id, session_id, create_event_id, host_pubkey, \
              source_channel_id, schema_version, status, created_at) \
         VALUES ($1, $2, $3, $4, $5, 1, 'active', clock_timestamp()) \
         RETURNING created_at",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.create_event_id)
    .bind(params.host_pubkey)
    .bind(params.source_channel_id)
    .fetch_one(&mut **tx)
    .await?;

    Ok((
        MeetingRecord {
            session_id: params.session_id,
            create_event_id: params.create_event_id.to_vec(),
            host_pubkey: params.host_pubkey.to_vec(),
            source_channel_id: params.source_channel_id,
            schema_version: 1,
            status: "active".to_string(),
            created_at,
            ended_at: None,
            ended_by: None,
            end_event_id: None,
            current_round: 1,
            floor_revision: 0,
            floor_policy_version: "uniform-v0".to_string(),
        },
        participants,
    ))
}

/// Validate and lock the optional source used by a Community-readable Meeting.
///
/// A source is a navigation reference, not an authority bridge. It must be a
/// non-deleted, ordinary, non-DM Channel whose current policy is readable by
/// every current and future Community member. The row lock closes the check /
/// Meeting-insert race inside the caller's transaction.
pub(crate) async fn validate_community_readable_source_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    source_channel_id: Option<Uuid>,
) -> Result<()> {
    let Some(source_id) = source_channel_id else {
        return Ok(());
    };
    let source = sqlx::query(
        "SELECT visibility::text AS visibility, \
                channel_type::text AS channel_type, room_kind \
         FROM channels \
         WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(source_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let source = source
        .ok_or_else(|| DbError::InvalidData(format!("source channel not found: {source_id}")))?;
    let visibility: String = source.try_get("visibility")?;
    let channel_type: String = source.try_get("channel_type")?;
    let room_kind: String = source.try_get("room_kind")?;
    if visibility != "open" || room_kind != "standard" || channel_type == "dm" {
        return Err(DbError::AccessDenied(
            "meeting source is not Community-readable".to_string(),
        ));
    }
    Ok(())
}

async fn has_active_ban_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM community_bans \
             WHERE community_id = $1 AND pubkey = $2 AND banned \
               AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp()) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await?)
}

/// Check whether a Meeting command author is still an active, writable
/// Community principal.
///
/// This gate includes direct membership, account deactivation, active bans
/// and active write timeouts. An owned Agent additionally requires an active,
/// unbanned authoritative owner; owner timeouts and owner relay membership do
/// not cascade to the Agent.
pub async fn is_meeting_actor_security_active(
    db: &Db,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<bool> {
    validate_32_bytes(pubkey, "meeting actor pubkey")?;
    let mut tx = db.begin_transaction().await?;
    let active = actor_security_active_tx(&mut tx, community_id, pubkey).await?;
    tx.commit().await?;
    Ok(active)
}

/// Check both current write authorization and the Session-relative durable
/// revocation fence used for Meeting command replay.
///
/// A principal restored after a real revocation may write in newer Meetings,
/// but cannot submit or create command receipts in a Meeting that predates
/// that revocation. Community-wide historical reads are evaluated separately.
pub async fn is_meeting_actor_session_security_active(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    pubkey: &[u8],
) -> Result<bool> {
    validate_32_bytes(pubkey, "meeting actor pubkey")?;
    let mut tx = db.begin_transaction().await?;
    let current = actor_security_active_tx(&mut tx, community_id, pubkey).await?;
    let durably_revoked = crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        &mut tx,
        community_id,
        session_id,
        pubkey,
    )
    .await?;
    tx.commit().await?;
    Ok(current && !durably_revoked)
}

pub(crate) async fn actor_security_active_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<bool> {
    validate_32_bytes(pubkey, "meeting actor pubkey")?;
    // Keep the same lock order as restriction producers: relay membership,
    // authoritative user, then restriction row. A command that gets these
    // SHARE locks first is linearized before a concurrent revoke; a revoke
    // that gets UPDATE/DELETE locks first is observed here after the wait.
    let member: Option<String> = sqlx::query_scalar(
        "SELECT role FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(hex::encode(pubkey))
    .fetch_optional(tx.as_mut())
    .await?;
    if member.is_none() {
        return Ok(false);
    }

    let identity = sqlx::query(
        "SELECT agent_owner_pubkey, deactivated_at \
         FROM users \
         WHERE community_id = $1 AND pubkey = $2 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(identity) = identity else {
        return Ok(false);
    };
    let deactivated_at: Option<DateTime<Utc>> = identity.try_get("deactivated_at")?;
    if deactivated_at.is_some() {
        return Ok(false);
    }
    let owner: Option<Vec<u8>> = identity.try_get("agent_owner_pubkey")?;

    let direct_restriction = sqlx::query(
        "SELECT banned, ban_expires_at, muted_until \
         FROM community_bans \
         WHERE community_id = $1 AND pubkey = $2 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_optional(tx.as_mut())
    .await?;
    if let Some(restriction) = direct_restriction {
        let banned: bool = restriction.try_get("banned")?;
        let ban_expires_at: Option<DateTime<Utc>> = restriction.try_get("ban_expires_at")?;
        let muted_until: Option<DateTime<Utc>> = restriction.try_get("muted_until")?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(tx.as_mut())
            .await?;
        if (banned && ban_expires_at.is_none_or(|expires_at| expires_at > now))
            || muted_until.is_some_and(|expires_at| expires_at > now)
        {
            return Ok(false);
        }
    }

    if let Some(owner) = owner {
        let owner_deactivated_at: Option<Option<DateTime<Utc>>> = sqlx::query_scalar(
            "SELECT deactivated_at FROM users \
             WHERE community_id = $1 AND pubkey = $2 \
             FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(&owner)
        .fetch_optional(tx.as_mut())
        .await?;
        if !matches!(owner_deactivated_at, Some(None)) {
            return Ok(false);
        }
        let owner_ban = sqlx::query(
            "SELECT banned, ban_expires_at \
             FROM community_bans \
             WHERE community_id = $1 AND pubkey = $2 \
             FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(&owner)
        .fetch_optional(tx.as_mut())
        .await?;
        if let Some(owner_ban) = owner_ban {
            let banned: bool = owner_ban.try_get("banned")?;
            let expires_at: Option<DateTime<Utc>> = owner_ban.try_get("ban_expires_at")?;
            let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(tx.as_mut())
                .await?;
            if banned && expires_at.is_none_or(|expires_at| expires_at > now) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

pub(crate) async fn discard_unenqueued_manual_end_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event_id: &[u8],
    actor_pubkey: &[u8],
) -> Result<()> {
    let deleted = sqlx::query(
        "DELETE FROM events e \
         WHERE e.community_id = $1 AND e.id = $2 AND e.channel_id = $3 \
           AND e.kind = $4 AND e.pubkey = $5 \
           AND NOT EXISTS ( \
               SELECT 1 FROM meeting_event_outbox o \
               WHERE o.community_id = e.community_id AND o.event_id = e.id \
           )",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .bind(session_id)
    .bind(buzz_core::kind::KIND_MEETING_END as i32)
    .bind(actor_pubkey)
    .execute(tx.as_mut())
    .await?;
    if deleted.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "manual Meeting End event is missing or already enqueued".to_string(),
        ));
    }
    Ok(())
}

/// End an active meeting and archive its backing channel inside the caller's
/// open transaction.
///
/// The host may end normally. Community owners/admins may also perform a
/// recovery end. A meeting is terminal: subsequent end commands return
/// [`EndMeetingOutcome::AlreadyEnded`] and must not be committed by the caller.
pub async fn end_meeting_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: EndMeetingParams<'_>,
) -> Result<EndMeetingOutcome> {
    validate_end_shape(&params)?;

    let row = sqlx::query(
        "SELECT host_pubkey, create_event_id, status, schema_version, \
                floor_policy_version \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {}", params.session_id)))?;

    let host_pubkey: Vec<u8> = row.try_get("host_pubkey")?;
    let stored_create_event_id: Vec<u8> = row.try_get("create_event_id")?;
    let status: String = row.try_get("status")?;
    let schema_version: i32 = row.try_get("schema_version")?;
    let floor_policy_version: String = row.try_get("floor_policy_version")?;
    if schema_version != 1 || floor_policy_version != "uniform-v0" {
        return Err(DbError::InvalidData(format!(
            "meeting {} is not a uniform-v0 session",
            params.session_id
        )));
    }

    if stored_create_event_id != params.create_event_id {
        return Err(DbError::InvalidData(
            "meeting end references the wrong create event".to_string(),
        ));
    }

    if status == "active"
        && crate::meeting_revocation::recover_revoked_roster_v0_tx(
            tx,
            params.community_id,
            params.session_id,
            params.relay_keys,
        )
        .await?
    {
        discard_unenqueued_manual_end_event_tx(
            tx,
            params.community_id,
            params.session_id,
            params.end_event_id,
            params.actor_pubkey,
        )
        .await?;
        return Ok(EndMeetingOutcome::ParticipantRevoked);
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
            "meeting End author was durably revoked from this Session".to_string(),
        ));
    }

    if !actor_security_active_tx(tx, params.community_id, params.actor_pubkey).await? {
        if status == "active"
            && crate::meeting_revocation::recover_revoked_roster_v0_tx(
                tx,
                params.community_id,
                params.session_id,
                params.relay_keys,
            )
            .await?
        {
            discard_unenqueued_manual_end_event_tx(
                tx,
                params.community_id,
                params.session_id,
                params.end_event_id,
                params.actor_pubkey,
            )
            .await?;
            return Ok(EndMeetingOutcome::ParticipantRevoked);
        }
        return Err(DbError::AccessDenied(
            "meeting End author is no longer an active writable community principal".to_string(),
        ));
    }

    if params.actor_pubkey != host_pubkey {
        let actor_hex = hex::encode(params.actor_pubkey);
        let recovery_role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2 \
             FOR SHARE",
        )
        .bind(params.community_id.as_uuid())
        .bind(actor_hex)
        .fetch_optional(&mut **tx)
        .await?;
        if !matches!(recovery_role.as_deref(), Some("owner" | "admin")) {
            return Err(DbError::AccessDenied(
                "only the meeting host or a community owner/admin can end this meeting".to_string(),
            ));
        }
    }

    if status == "ended" {
        return Ok(EndMeetingOutcome::AlreadyEnded);
    }
    if status != "active" {
        return Err(DbError::InvalidData(format!(
            "unknown meeting status: {status}"
        )));
    }

    let ended_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE meeting_sessions \
         SET status = 'ended', ended_at = NOW(), ended_by = $3, end_event_id = $4 \
         WHERE community_id = $1 AND session_id = $2 AND status = 'active' \
         RETURNING ended_at",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.actor_pubkey)
    .bind(params.end_event_id)
    .fetch_one(&mut **tx)
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
    .execute(&mut **tx)
    .await?;
    if archived.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "meeting channel is missing or not active".to_string(),
        ));
    }

    Ok(EndMeetingOutcome::Ended)
}

/// Fetch a meeting lifecycle projection by community and session id.
pub async fn get_meeting(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<MeetingRecord> {
    let row = sqlx::query(
        "SELECT session_id, create_event_id, host_pubkey, source_channel_id, \
                schema_version, status, created_at, ended_at, ended_by, end_event_id, \
                current_round, floor_revision, floor_policy_version \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;

    Ok(MeetingRecord {
        session_id: row.try_get("session_id")?,
        create_event_id: row.try_get("create_event_id")?,
        host_pubkey: row.try_get("host_pubkey")?,
        source_channel_id: row.try_get("source_channel_id")?,
        schema_version: row.try_get("schema_version")?,
        status: row.try_get("status")?,
        created_at: row.try_get("created_at")?,
        ended_at: row.try_get("ended_at")?,
        ended_by: row.try_get("ended_by")?,
        end_event_id: row.try_get("end_event_id")?,
        current_round: row.try_get("current_round")?,
        floor_revision: row.try_get("floor_revision")?,
        floor_policy_version: row.try_get("floor_policy_version")?,
    })
}

/// Read the persisted protocol discriminator for policy-aware command routing.
pub async fn get_meeting_policy(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<MeetingPolicy> {
    let row = sqlx::query(
        "SELECT schema_version, floor_policy_version, moderator_pubkey \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(&db.pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    Ok(MeetingPolicy {
        schema_version: row.try_get("schema_version")?,
        floor_policy_version: row.try_get("floor_policy_version")?,
        moderator_pubkey: row.try_get("moderator_pubkey")?,
    })
}

/// Enqueue an already-persisted meeting event for post-commit delivery.
///
/// The event must belong to the same private meeting channel in the caller's
/// open transaction. Callers control causal ordering by invoking this before
/// later relay-signed State events are persisted and enqueued.
pub async fn enqueue_meeting_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event_id: &[u8],
) -> Result<()> {
    validate_32_bytes(event_id, "meeting outbox event id")?;
    let result = sqlx::query(
        "INSERT INTO meeting_event_outbox (community_id, session_id, event_id) \
         SELECT $1, $2, $3 \
         WHERE EXISTS( \
             SELECT 1 FROM events \
             WHERE community_id = $1 AND id = $3 AND channel_id = $2 \
         ) \
         ON CONFLICT (community_id, event_id) DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(event_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "meeting outbox event is missing or already enqueued".to_string(),
        ));
    }
    Ok(())
}

fn validate_create_shape(params: &CreateMeetingParams<'_>) -> Result<()> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting session id must not be nil".to_string(),
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
        return Err(DbError::InvalidData(format!(
            "meeting host {host_hex} must appear exactly once in the complete roster",
            host_hex = hex::encode(params.host_pubkey)
        )));
    }
    Ok(())
}

fn validate_end_shape(params: &EndMeetingParams<'_>) -> Result<()> {
    if params.session_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting session id must not be nil".to_string(),
        ));
    }
    validate_32_bytes(params.actor_pubkey, "actor pubkey")?;
    validate_32_bytes(params.create_event_id, "create event id")?;
    validate_32_bytes(params.end_event_id, "end event id")
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

async fn active_meeting_reader_pubkeys(
    db: &Db,
    community_id: CommunityId,
    pubkeys: &[Vec<u8>],
) -> Result<Vec<Vec<u8>>> {
    for pubkey in pubkeys {
        validate_32_bytes(pubkey, "meeting reader pubkey")?;
    }
    if pubkeys.is_empty() {
        return Ok(Vec::new());
    }

    let authorized = db
        .community_global_authorized_pubkeys(community_id, pubkeys)
        .await?;
    Ok(pubkeys
        .iter()
        .filter(|pubkey| authorized.contains(pubkey.as_slice()))
        .cloned()
        .collect())
}

/// Check current Community-global read authorization for one Meeting principal
/// without using Relay access caches or the immutable Meeting roster.
///
/// This is the same principal predicate used by Project View, Project Document,
/// and Project Context. Write timeouts deliberately do not revoke read access.
pub async fn is_meeting_reader_security_active(
    db: &Db,
    community_id: CommunityId,
    pubkey: &[u8],
) -> Result<bool> {
    validate_32_bytes(pubkey, "meeting reader pubkey")?;
    let active = active_meeting_reader_pubkeys(db, community_id, &[pubkey.to_vec()]).await?;
    Ok(active.iter().any(|candidate| candidate == pubkey))
}

/// Return the candidate Channel IDs that are backed by a Meeting Session.
pub async fn meeting_channel_ids(
    db: &Db,
    community_id: CommunityId,
    channel_ids: &[Uuid],
) -> Result<Vec<Uuid>> {
    if channel_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT session_id FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = ANY($2::uuid[])",
    )
    .bind(community_id.as_uuid())
    .bind(channel_ids)
    .fetch_all(&db.pool)
    .await?)
}

/// Return all non-deleted Meeting Channels in one Community.
pub async fn community_meeting_channel_ids(
    db: &Db,
    community_id: CommunityId,
) -> Result<Vec<Uuid>> {
    Ok(sqlx::query_scalar(
        "SELECT ms.session_id \
         FROM meeting_sessions ms \
         JOIN channels channel \
           ON channel.community_id = ms.community_id \
          AND channel.id = ms.session_id \
          AND channel.room_kind = 'meeting' \
          AND channel.deleted_at IS NULL \
         WHERE ms.community_id = $1 \
         ORDER BY ms.session_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&db.pool)
    .await?)
}

/// Migration-only legacy reader scope for the roster-private contract.
///
/// Ordinary runtime stops calling this once Community-read is durably enabled,
/// but the dark-launch path retains it so deploying new code cannot widen old
/// Meeting visibility before operator approval.
pub async fn meeting_channel_ids_for_frozen_reader(
    db: &Db,
    community_id: CommunityId,
    pubkey: &[u8],
    channel_ids: &[Uuid],
) -> Result<Vec<Uuid>> {
    validate_32_bytes(pubkey, "meeting reader pubkey")?;
    if channel_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT ms.session_id \
         FROM meeting_sessions ms \
         WHERE ms.community_id = $1 \
           AND ms.session_id = ANY($3::uuid[]) \
           AND NOT EXISTS( \
             SELECT 1 FROM meeting_revocation_jobs revocation \
             WHERE revocation.community_id = ms.community_id \
               AND revocation.revoked_pubkey = $2 \
               AND revocation.security_order > ms.security_order \
           ) \
           AND ( \
             (((ms.schema_version = $4 AND ms.floor_policy_version = $5) \
               OR (ms.schema_version = $6 AND ms.floor_policy_version IN ($7, $8))) \
              AND EXISTS( \
                SELECT 1 FROM meeting_participants mp \
                WHERE mp.community_id = ms.community_id \
                  AND mp.session_id = ms.session_id AND mp.pubkey = $2 \
              )) \
             OR \
             (ms.schema_version = 1 AND ms.floor_policy_version = $9 \
              AND EXISTS( \
                SELECT 1 FROM channel_members cm \
                WHERE cm.community_id = ms.community_id \
                  AND cm.channel_id = ms.session_id AND cm.pubkey = $2 \
              )) \
           )",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .bind(channel_ids)
    .bind(crate::meeting_baton::SCHEMA_VERSION)
    .bind(crate::meeting_baton::BATON_POLICY_VERSION)
    .bind(crate::meeting_v2::SCHEMA_VERSION)
    .bind(crate::meeting_v2::BOARD_POLICY_VERSION)
    .bind(crate::meeting_v2::ACTIONS_POLICY_VERSION)
    .bind(crate::meeting_floor::FLOOR_POLICY_VERSION)
    .fetch_all(&db.pool)
    .await?)
}

/// Check whether one Channel UUID is backed by a Meeting Session.
pub async fn is_meeting_channel(
    db: &Db,
    community_id: CommunityId,
    channel_id: Uuid,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2 \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(channel_id)
    .fetch_one(&db.pool)
    .await?)
}

/// Check one Channel against Meeting classification and the currently
/// published read contract.
///
/// `None` denotes an ordinary Channel. A Meeting returns `Some(false)` on any
/// denial. Before durable publication this retains the frozen-roster legacy
/// contract; after publication it uses current Community-global membership.
pub async fn is_meeting_reader_authorized_for_channel(
    db: &Db,
    community_id: CommunityId,
    channel_id: Uuid,
    pubkey: &[u8],
) -> Result<Option<bool>> {
    if !is_meeting_channel(db, community_id, channel_id).await? {
        return Ok(None);
    }
    if !is_meeting_reader_security_active(db, community_id, pubkey).await? {
        return Ok(Some(false));
    }
    if db.meeting_community_read_enabled(community_id).await? {
        return Ok(Some(true));
    }
    let authorized = meeting_channel_ids_for_frozen_reader(
        db,
        community_id,
        pubkey,
        std::slice::from_ref(&channel_id),
    )
    .await?;
    Ok(Some(authorized.contains(&channel_id)))
}

async fn frozen_meeting_reader_pubkeys_for_channel(
    db: &Db,
    community_id: CommunityId,
    channel_id: Uuid,
    pubkeys: &[Vec<u8>],
) -> Result<Vec<Vec<u8>>> {
    for pubkey in pubkeys {
        validate_32_bytes(pubkey, "meeting reader pubkey")?;
    }
    if pubkeys.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_scalar(
        "SELECT mp.pubkey \
         FROM meeting_sessions ms \
         JOIN meeting_participants mp \
           ON mp.community_id = ms.community_id AND mp.session_id = ms.session_id \
         WHERE ms.community_id = $1 AND ms.session_id = $2 \
           AND ((ms.schema_version = $4 AND ms.floor_policy_version = $5) \
                OR (ms.schema_version = $6 AND ms.floor_policy_version IN ($7, $8))) \
           AND NOT EXISTS( \
             SELECT 1 FROM meeting_revocation_jobs revocation \
             WHERE revocation.community_id = ms.community_id \
               AND revocation.revoked_pubkey = mp.pubkey \
               AND revocation.security_order > ms.security_order \
           ) \
           AND mp.pubkey = ANY($3::bytea[]) \
         UNION \
         SELECT cm.pubkey \
         FROM meeting_sessions ms \
         JOIN channel_members cm \
           ON cm.community_id = ms.community_id AND cm.channel_id = ms.session_id \
         WHERE ms.community_id = $1 AND ms.session_id = $2 \
           AND ms.schema_version = 1 AND ms.floor_policy_version = $9 \
           AND NOT EXISTS( \
             SELECT 1 FROM meeting_revocation_jobs revocation \
             WHERE revocation.community_id = ms.community_id \
               AND revocation.revoked_pubkey = cm.pubkey \
               AND revocation.security_order > ms.security_order \
           ) \
           AND cm.pubkey = ANY($3::bytea[])",
    )
    .bind(community_id.as_uuid())
    .bind(channel_id)
    .bind(pubkeys)
    .bind(crate::meeting_baton::SCHEMA_VERSION)
    .bind(crate::meeting_baton::BATON_POLICY_VERSION)
    .bind(crate::meeting_v2::SCHEMA_VERSION)
    .bind(crate::meeting_v2::BOARD_POLICY_VERSION)
    .bind(crate::meeting_v2::ACTIONS_POLICY_VERSION)
    .bind(crate::meeting_floor::FLOOR_POLICY_VERSION)
    .fetch_all(&db.pool)
    .await?)
}

/// Apply the retained roster-private contract during dark launch.
pub async fn legacy_active_meeting_reader_pubkeys_for_channel(
    db: &Db,
    community_id: CommunityId,
    channel_id: Uuid,
    pubkeys: &[Vec<u8>],
) -> Result<Option<Vec<Vec<u8>>>> {
    if !is_meeting_channel(db, community_id, channel_id).await? {
        return Ok(None);
    }
    let active_readers = active_meeting_reader_pubkeys(db, community_id, pubkeys).await?;
    frozen_meeting_reader_pubkeys_for_channel(db, community_id, channel_id, &active_readers)
        .await
        .map(Some)
}

/// Batch-filter live recipients for a Community-readable Meeting fan-out.
///
/// Returns `None` for an ordinary Channel, preserving its existing membership
/// policy. A Meeting applies the uncached shared Community-global principal
/// predicate; the immutable roster remains an action boundary only.
pub async fn active_meeting_reader_pubkeys_for_channel(
    db: &Db,
    community_id: CommunityId,
    channel_id: Uuid,
    pubkeys: &[Vec<u8>],
) -> Result<Option<Vec<Vec<u8>>>> {
    if !is_meeting_channel(db, community_id, channel_id).await? {
        return Ok(None);
    }
    active_meeting_reader_pubkeys(db, community_id, pubkeys)
        .await
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finalizing_coordinate_requires_the_current_action_transition() {
        let action_run_id = Uuid::new_v4();
        let effects = json!([{
            "type": "action_finalization_began",
            "object_type": "meeting_action_run",
            "object_id": action_run_id,
            "from": "floor_ready",
            "to": "runnable",
        }]);
        assert!(action_transition_matches(
            "action_finalization_began",
            &effects,
            action_run_id,
            "runnable",
        ));
        assert!(!action_transition_matches(
            "action_finalization_began",
            &effects,
            Uuid::new_v4(),
            "runnable",
        ));
        assert!(!action_transition_matches(
            "action_returned_to_board",
            &effects,
            action_run_id,
            "runnable",
        ));
        assert!(!action_transition_matches(
            "action_finalization_began",
            &effects,
            action_run_id,
            "blocked",
        ));
    }

    fn create_params<'a>(
        host: &'a [u8],
        event_id: &'a [u8],
        participants: &'a [Vec<u8>],
    ) -> CreateMeetingParams<'a> {
        CreateMeetingParams {
            community_id: CommunityId::from_uuid(Uuid::new_v4()),
            session_id: Uuid::new_v4(),
            title: "stage-one",
            description: None,
            source_channel_id: None,
            host_pubkey: host,
            create_event_id: event_id,
            participant_pubkeys: participants,
        }
    }

    #[test]
    fn create_shape_requires_host_exactly_once() {
        let host = vec![1; 32];
        let other = vec![2; 32];
        let event_id = vec![3; 32];

        let missing = [other.clone(), vec![4; 32]];
        assert!(validate_create_shape(&create_params(&host, &event_id, &missing)).is_err());

        let duplicated = [host.clone(), host.clone(), other];
        assert!(validate_create_shape(&create_params(&host, &event_id, &duplicated)).is_err());
    }

    #[test]
    fn create_shape_enforces_participant_bounds_and_unique_pubkeys() {
        let host = vec![1; 32];
        let event_id = vec![3; 32];

        let one = [host.clone()];
        assert!(validate_create_shape(&create_params(&host, &event_id, &one)).is_err());

        let duplicate = [host.clone(), vec![2; 32], vec![2; 32]];
        assert!(validate_create_shape(&create_params(&host, &event_id, &duplicate)).is_err());

        let valid = [host.clone(), vec![2; 32]];
        assert!(validate_create_shape(&create_params(&host, &event_id, &valid)).is_ok());
    }

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to Meeting V0 test database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V0 migrations");
        pool
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("meeting-test-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert test community");
        let community_id = CommunityId::from_uuid(id);
        let db = Db::from_pool(pool.clone());
        db.set_meeting_community_read_create_paused(community_id, true)
            .await
            .expect("pause empty Meeting corpus");
        let audit = db
            .audit_legacy_meeting_visibility(community_id)
            .await
            .expect("audit empty Meeting corpus");
        db.approve_legacy_meeting_visibility(
            community_id,
            audit.watermark,
            &audit.digest,
            "meeting-test",
        )
        .await
        .expect("approve empty Meeting corpus");
        db.enable_meeting_community_read(community_id)
            .await
            .expect("publish test Meeting reads");
        community_id
    }

    async fn seed_identity(
        pool: &PgPool,
        community_id: CommunityId,
        pubkey: &[u8],
        relay_role: &str,
        agent_owner_pubkey: Option<&[u8]>,
        add_policy: &str,
    ) {
        sqlx::query(
            "INSERT INTO users \
                 (community_id, pubkey, agent_owner_pubkey, channel_add_policy) \
             VALUES ($1, $2, $3, $4::channel_add_policy)",
        )
        .bind(community_id.as_uuid())
        .bind(pubkey)
        .bind(agent_owner_pubkey)
        .bind(add_policy)
        .execute(pool)
        .await
        .expect("insert test identity");

        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(pubkey))
        .bind(relay_role)
        .execute(pool)
        .await
        .expect("insert test relay membership");
    }

    #[tokio::test]
    #[ignore = "requires isolated Postgres"]
    async fn meeting_source_requires_open_standard_non_dm_channel() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let creator = vec![0x45_u8; 32];
        let open_standard = Uuid::new_v4();
        let private_standard = Uuid::new_v4();
        let open_dm = Uuid::new_v4();
        let meeting_room = Uuid::new_v4();
        for (id, channel_type, visibility, room_kind) in [
            (open_standard, "stream", "open", "standard"),
            (private_standard, "stream", "private", "standard"),
            (open_dm, "dm", "open", "standard"),
            (meeting_room, "stream", "private", "meeting"),
        ] {
            sqlx::query(
                "INSERT INTO channels \
                     (community_id, id, name, channel_type, visibility, created_by, room_kind) \
                 VALUES ($1, $2, 'source', $3::channel_type, \
                         $4::channel_visibility, $5, $6)",
            )
            .bind(community_id.as_uuid())
            .bind(id)
            .bind(channel_type)
            .bind(visibility)
            .bind(&creator)
            .bind(room_kind)
            .execute(&pool)
            .await
            .expect("insert source candidate");
        }

        let mut tx = pool.begin().await.expect("begin source validation");
        validate_community_readable_source_tx(&mut tx, community_id, Some(open_standard))
            .await
            .expect("open ordinary source");
        for blocked in [private_standard, open_dm, meeting_room] {
            assert!(matches!(
                validate_community_readable_source_tx(&mut tx, community_id, Some(blocked)).await,
                Err(DbError::AccessDenied(_))
            ));
        }
        assert!(matches!(
            validate_community_readable_source_tx(&mut tx, community_id, Some(Uuid::new_v4()),)
                .await,
            Err(DbError::InvalidData(_))
        ));
        tx.rollback().await.expect("rollback source validation");
    }

    async fn insert_command_event_tx(
        tx: &mut Transaction<'_, Postgres>,
        community_id: CommunityId,
        event_id: &[u8],
        pubkey: &[u8],
        kind: i32,
        channel_id: Uuid,
    ) {
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO events \
                 (community_id, id, pubkey, created_at, kind, tags, content, sig, \
                  received_at, channel_id) \
             VALUES ($1, $2, $3, $4, $5, $6, '', $7, $4, $8)",
        )
        .bind(community_id.as_uuid())
        .bind(event_id)
        .bind(pubkey)
        .bind(now)
        .bind(kind)
        .bind(json!([["h", channel_id.to_string()]]))
        .bind(vec![0_u8; 64])
        .bind(channel_id)
        .execute(&mut **tx)
        .await
        .expect("insert command event in lifecycle transaction");
    }

    async fn create_active_v0_meeting(
        pool: &PgPool,
        community_id: CommunityId,
        host: &[u8],
        other: &[u8],
    ) -> (Uuid, Vec<u8>) {
        let session_id = Uuid::new_v4();
        let create_event_id = rand::random::<[u8; 32]>().to_vec();
        let roster = vec![host.to_vec(), other.to_vec()];
        let mut tx = pool.begin().await.expect("begin security Meeting create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &create_event_id,
            host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        create_meeting_tx(
            &mut tx,
            CreateMeetingParams {
                community_id,
                session_id,
                title: "Manual End security",
                description: None,
                source_channel_id: None,
                host_pubkey: host,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
            },
        )
        .await
        .expect("create active security Meeting");
        tx.commit().await.expect("commit active security Meeting");
        (session_id, create_event_id)
    }

    async fn try_manual_end_v0(
        pool: &PgPool,
        community_id: CommunityId,
        session_id: Uuid,
        actor: &[u8],
        create_event_id: &[u8],
        relay_keys: &Keys,
    ) -> (Vec<u8>, Result<EndMeetingOutcome>) {
        let end_event_id = rand::random::<[u8; 32]>().to_vec();
        let mut tx = pool.begin().await.expect("begin manual security End");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &end_event_id,
            actor,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let outcome = end_meeting_tx(
            &mut tx,
            EndMeetingParams {
                community_id,
                session_id,
                actor_pubkey: actor,
                create_event_id,
                end_event_id: &end_event_id,
                relay_keys,
            },
        )
        .await;
        if outcome.is_ok() {
            tx.commit().await.expect("commit manual security End");
        } else {
            tx.rollback().await.expect("rollback rejected manual End");
        }
        (end_event_id, outcome)
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn create_and_end_are_atomic_private_and_terminal() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let host = vec![11_u8; 32];
        let human = vec![12_u8; 32];
        let agent = vec![13_u8; 32];
        let outsider = vec![14_u8; 32];
        let create_event_id = vec![21_u8; 32];
        let end_event_id = vec![22_u8; 32];
        let session_id = Uuid::new_v4();
        let relay_keys = Keys::generate();

        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_identity(&pool, community_id, &human, "member", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &agent,
            "member",
            Some(&host),
            "owner_only",
        )
        .await;
        seed_identity(&pool, community_id, &outsider, "member", None, "anyone").await;

        let roster = vec![host.clone(), human.clone(), agent.clone()];
        let mut create_tx = pool.begin().await.expect("begin meeting create");
        insert_command_event_tx(
            &mut create_tx,
            community_id,
            &create_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        let (record, mut projected_roster) = create_meeting_tx(
            &mut create_tx,
            CreateMeetingParams {
                community_id,
                session_id,
                title: "# Stage One",
                description: Some("lifecycle proof"),
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
            },
        )
        .await
        .expect("create meeting atomically");
        create_tx.commit().await.expect("commit meeting create");

        assert_eq!(record.status, "active");
        let channel = crate::channel::get_channel(&pool, community_id, session_id)
            .await
            .expect("meeting channel");
        assert_eq!(channel.name, "Stage One");
        assert_eq!(channel.channel_type, "stream");
        assert_eq!(channel.visibility, "private");
        assert_eq!(channel.room_kind, "meeting");
        assert!(channel.archived_at.is_none());

        projected_roster.sort_by(|left, right| left.pubkey.cmp(&right.pubkey));
        assert_eq!(
            projected_roster,
            vec![
                MeetingParticipant {
                    pubkey: host.clone(),
                    role: "owner".to_string(),
                },
                MeetingParticipant {
                    pubkey: human.clone(),
                    role: "member".to_string(),
                },
                MeetingParticipant {
                    pubkey: agent.clone(),
                    role: "bot".to_string(),
                },
            ]
        );

        for participant in [&host, &human, &agent] {
            let accessible =
                crate::channel::get_accessible_channel_ids(&pool, community_id, participant)
                    .await
                    .expect("participant access");
            assert!(accessible.contains(&session_id));
        }
        let outsider_access =
            crate::channel::get_accessible_channel_ids(&pool, community_id, &outsider)
                .await
                .expect("outsider access");
        assert!(!outsider_access.contains(&session_id));

        let mut end_tx = pool.begin().await.expect("begin meeting end");
        insert_command_event_tx(
            &mut end_tx,
            community_id,
            &end_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        assert_eq!(
            end_meeting_tx(
                &mut end_tx,
                EndMeetingParams {
                    community_id,
                    session_id,
                    actor_pubkey: &host,
                    create_event_id: &create_event_id,
                    end_event_id: &end_event_id,
                    relay_keys: &relay_keys,
                },
            )
            .await
            .expect("end active meeting"),
            EndMeetingOutcome::Ended
        );
        end_tx.commit().await.expect("commit meeting end");

        let ended = get_meeting(&pool, community_id, session_id)
            .await
            .expect("ended meeting projection");
        assert_eq!(ended.status, "ended");
        assert_eq!(ended.end_event_id.as_deref(), Some(end_event_id.as_slice()));
        assert_eq!(ended.ended_by.as_deref(), Some(host.as_slice()));
        assert!(ended.ended_at.is_some());
        let archived = crate::channel::get_channel(&pool, community_id, session_id)
            .await
            .expect("archived meeting channel");
        assert!(archived.archived_at.is_some());

        // Archiving is terminal but does not remove the frozen roster or its
        // read access to history.
        let members = crate::channel::get_members(&pool, community_id, session_id)
            .await
            .expect("archived meeting roster");
        assert_eq!(members.len(), 3);
        for participant in [&host, &human, &agent] {
            let accessible =
                crate::channel::get_accessible_channel_ids(&pool, community_id, participant)
                    .await
                    .expect("archived participant access");
            assert!(accessible.contains(&session_id));
        }

        let retry_event_id = vec![23_u8; 32];
        let mut retry_tx = pool.begin().await.expect("begin duplicate end");
        insert_command_event_tx(
            &mut retry_tx,
            community_id,
            &retry_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        assert_eq!(
            end_meeting_tx(
                &mut retry_tx,
                EndMeetingParams {
                    community_id,
                    session_id,
                    actor_pubkey: &host,
                    create_event_id: &create_event_id,
                    end_event_id: &retry_event_id,
                    relay_keys: &relay_keys,
                },
            )
            .await
            .expect("idempotent duplicate end"),
            EndMeetingOutcome::AlreadyEnded
        );
        retry_tx
            .rollback()
            .await
            .expect("discard duplicate end event");
        let end_event_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND channel_id = $2 AND kind = $3",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(buzz_core::kind::KIND_MEETING_END as i32)
        .fetch_one(&pool)
        .await
        .expect("count committed end events");
        assert_eq!(end_event_count, 1);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn manual_end_security_rejects_inactive_actors_and_prioritizes_v0_roster_recovery() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let relay_keys = Keys::generate();

        let banned_host = vec![0x51_u8; 32];
        let banned_host_peer = vec![0x52_u8; 32];
        seed_identity(&pool, community_id, &banned_host, "owner", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &banned_host_peer,
            "member",
            None,
            "anyone",
        )
        .await;
        let (session_id, create_event_id) =
            create_active_v0_meeting(&pool, community_id, &banned_host, &banned_host_peer).await;
        crate::moderation::ban_member_with_revocation(
            &pool,
            community_id,
            &banned_host,
            &banned_host_peer,
            Some("manual End race"),
            None,
            &[0xa1; 32],
        )
        .await
        .expect("ban active V0 host");
        let (manual_event_id, outcome) = try_manual_end_v0(
            &pool,
            community_id,
            session_id,
            &banned_host,
            &create_event_id,
            &relay_keys,
        )
        .await;
        assert_eq!(
            outcome.expect("revocation recovery wins over manual End"),
            EndMeetingOutcome::ParticipantRevoked
        );
        let (status, canonical_end_id): (String, Vec<u8>) = sqlx::query_as(
            "SELECT status, end_event_id FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("load V0 revocation terminal session");
        assert_eq!(status, "ended");
        assert_ne!(canonical_end_id, manual_event_id);
        let manual_persistence: (bool, bool) = sqlx::query_as(
            "SELECT \
                 EXISTS(SELECT 1 FROM events \
                        WHERE community_id = $1 AND id = $2), \
                 EXISTS(SELECT 1 FROM meeting_event_outbox \
                        WHERE community_id = $1 AND event_id = $2)",
        )
        .bind(community_id.as_uuid())
        .bind(&manual_event_id)
        .fetch_one(&pool)
        .await
        .expect("check discarded V0 manual End");
        assert_eq!(manual_persistence, (false, false));

        let active_host = vec![0x53_u8; 32];
        let active_peer = vec![0x54_u8; 32];
        let banned_admin = vec![0x55_u8; 32];
        seed_identity(&pool, community_id, &active_host, "owner", None, "anyone").await;
        seed_identity(&pool, community_id, &active_peer, "member", None, "anyone").await;
        seed_identity(&pool, community_id, &banned_admin, "admin", None, "anyone").await;
        let (session_id, create_event_id) =
            create_active_v0_meeting(&pool, community_id, &active_host, &active_peer).await;
        crate::moderation::ban_member_with_revocation(
            &pool,
            community_id,
            &banned_admin,
            &active_host,
            Some("inactive recovery admin"),
            None,
            &[0xa2; 32],
        )
        .await
        .expect("ban non-roster admin");
        let (_, error) = try_manual_end_v0(
            &pool,
            community_id,
            session_id,
            &banned_admin,
            &create_event_id,
            &relay_keys,
        )
        .await;
        assert!(matches!(error, Err(DbError::AccessDenied(_))));
        assert_eq!(
            get_meeting(&pool, community_id, session_id)
                .await
                .expect("load Meeting after banned admin End")
                .status,
            "active"
        );

        let deactivated_host = vec![0x56_u8; 32];
        let deactivated_peer = vec![0x57_u8; 32];
        seed_identity(
            &pool,
            community_id,
            &deactivated_host,
            "owner",
            None,
            "anyone",
        )
        .await;
        seed_identity(
            &pool,
            community_id,
            &deactivated_peer,
            "member",
            None,
            "anyone",
        )
        .await;
        let (session_id, create_event_id) =
            create_active_v0_meeting(&pool, community_id, &deactivated_host, &deactivated_peer)
                .await;
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&deactivated_host)
        .execute(&pool)
        .await
        .expect("deactivate active V0 host");
        let (_, outcome) = try_manual_end_v0(
            &pool,
            community_id,
            session_id,
            &deactivated_host,
            &create_event_id,
            &relay_keys,
        )
        .await;
        assert_eq!(
            outcome.expect("deactivated host triggers lazy recovery"),
            EndMeetingOutcome::ParticipantRevoked
        );

        let agent_owner = vec![0x58_u8; 32];
        let agent_host = vec![0x59_u8; 32];
        let agent_peer = vec![0x5a_u8; 32];
        seed_identity(&pool, community_id, &agent_owner, "member", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &agent_host,
            "member",
            Some(&agent_owner),
            "owner_only",
        )
        .await;
        seed_identity(&pool, community_id, &agent_peer, "member", None, "anyone").await;
        let (session_id, create_event_id) =
            create_active_v0_meeting(&pool, community_id, &agent_host, &agent_peer).await;
        crate::moderation::ban_member_with_revocation(
            &pool,
            community_id,
            &agent_owner,
            &agent_peer,
            Some("owned Agent host revoked"),
            None,
            &[0xa3; 32],
        )
        .await
        .expect("ban authoritative Agent owner");
        let (_, outcome) = try_manual_end_v0(
            &pool,
            community_id,
            session_id,
            &agent_host,
            &create_event_id,
            &relay_keys,
        )
        .await;
        assert_eq!(
            outcome.expect("owner ban revokes Agent host before manual End"),
            EndMeetingOutcome::ParticipantRevoked
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn invalid_roster_rolls_back_event_room_members_and_projection() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let host = vec![31_u8; 32];
        let missing_member = vec![32_u8; 32];
        let create_event_id = vec![33_u8; 32];
        let session_id = Uuid::new_v4();
        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;

        let roster = vec![host.clone(), missing_member];
        let mut tx = pool.begin().await.expect("begin invalid meeting create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &create_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        let error = create_meeting_tx(
            &mut tx,
            CreateMeetingParams {
                community_id,
                session_id,
                title: "must rollback",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
            },
        )
        .await
        .expect_err("non-member participant must reject the whole create");
        assert!(matches!(error, DbError::AccessDenied(_)));
        tx.rollback()
            .await
            .expect("rollback invalid meeting create");

        let event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(&create_event_id)
                .fetch_one(&pool)
                .await
                .expect("count rolled-back create event");
        let channel_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM channels WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .expect("count rolled-back meeting channel");
        let member_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count rolled-back meeting members");
        let session_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count rolled-back meeting projection");

        assert_eq!(
            (event_count, channel_count, member_count, session_count),
            (0, 0, 0, 0)
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn create_rejects_direct_and_nip_oa_owner_bans() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let host = vec![41_u8; 32];
        let human = vec![42_u8; 32];
        let agent_owner = vec![43_u8; 32];
        let agent = vec![44_u8; 32];
        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_identity(&pool, community_id, &human, "member", None, "anyone").await;
        seed_identity(&pool, community_id, &agent_owner, "member", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &agent,
            "member",
            Some(&agent_owner),
            "anyone",
        )
        .await;

        sqlx::query(
            "INSERT INTO community_bans \
                 (community_id, pubkey, banned, actor_pubkey) \
             VALUES ($1, $2, true, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(&human)
        .bind(&host)
        .execute(&pool)
        .await
        .expect("ban direct participant");
        let direct_event_id = vec![45_u8; 32];
        let direct_session = Uuid::new_v4();
        let direct_roster = vec![host.clone(), human.clone()];
        let mut tx = pool.begin().await.expect("begin direct-ban create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &direct_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            direct_session,
        )
        .await;
        let direct_error = create_meeting_tx(
            &mut tx,
            CreateMeetingParams {
                community_id,
                session_id: direct_session,
                title: "direct ban",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: &direct_event_id,
                participant_pubkeys: &direct_roster,
            },
        )
        .await
        .expect_err("directly banned participant must reject create");
        assert!(direct_error.to_string().contains("is banned"));
        tx.rollback().await.expect("rollback direct-ban create");

        sqlx::query(
            "INSERT INTO community_bans \
                 (community_id, pubkey, banned, actor_pubkey) \
             VALUES ($1, $2, true, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(&agent_owner)
        .bind(&host)
        .execute(&pool)
        .await
        .expect("ban Agent owner");
        let owner_event_id = vec![46_u8; 32];
        let owner_session = Uuid::new_v4();
        let owner_roster = vec![host.clone(), agent.clone()];
        let mut tx = pool.begin().await.expect("begin owner-ban create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &owner_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            owner_session,
        )
        .await;
        let owner_error = create_meeting_tx(
            &mut tx,
            CreateMeetingParams {
                community_id,
                session_id: owner_session,
                title: "owner ban",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: &owner_event_id,
                participant_pubkeys: &owner_roster,
            },
        )
        .await
        .expect_err("Agent with banned owner must reject create");
        assert!(owner_error.to_string().contains("has a banned owner"));
        tx.rollback().await.expect("rollback owner-ban create");

        sqlx::query(
            "UPDATE community_bans SET banned = false \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&agent_owner)
        .execute(&pool)
        .await
        .expect("unban Agent owner");
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&agent_owner)
        .execute(&pool)
        .await
        .expect("deactivate Agent owner");
        let deactivated_event_id = vec![47_u8; 32];
        let deactivated_session = Uuid::new_v4();
        let deactivated_roster = vec![host.clone(), agent];
        let mut tx = pool.begin().await.expect("begin deactivated-owner create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &deactivated_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            deactivated_session,
        )
        .await;
        let deactivated_error = create_meeting_tx(
            &mut tx,
            CreateMeetingParams {
                community_id,
                session_id: deactivated_session,
                title: "deactivated owner",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: &deactivated_event_id,
                participant_pubkeys: &deactivated_roster,
            },
        )
        .await
        .expect_err("Agent with deactivated owner must reject create");
        assert!(deactivated_error
            .to_string()
            .contains("has no active authoritative owner"));
        tx.rollback()
            .await
            .expect("rollback deactivated-owner create");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn community_read_restores_history_after_reactivation_without_erasing_legacy_fence() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host = vec![0x71_u8; 32];
        let restored_participant = vec![0x72_u8; 32];
        let observer = vec![0x74_u8; 32];

        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &restored_participant,
            "member",
            None,
            "anyone",
        )
        .await;
        seed_identity(&pool, community_id, &observer, "member", None, "anyone").await;

        let (old_session, _) =
            create_active_v0_meeting(&pool, community_id, &host, &restored_participant).await;
        assert_eq!(
            is_meeting_reader_authorized_for_channel(&db, community_id, old_session, &observer)
                .await
                .expect("authorize non-roster Community observer"),
            Some(true),
            "the frozen roster is not a Community Meeting read ACL"
        );
        assert_eq!(
            active_meeting_reader_pubkeys_for_channel(
                &db,
                community_id,
                old_session,
                std::slice::from_ref(&observer),
            )
            .await
            .expect("filter observer fan-out"),
            Some(vec![observer.clone()])
        );
        assert_eq!(
            is_meeting_reader_authorized_for_channel(
                &db,
                community_id,
                old_session,
                &restored_participant,
            )
            .await
            .expect("authorize original participant"),
            Some(true)
        );
        let candidates_read_before_revocation = active_meeting_reader_pubkeys(
            &db,
            community_id,
            std::slice::from_ref(&restored_participant),
        )
        .await
        .expect("read current-principal candidates before concurrent revocation");
        assert_eq!(
            candidates_read_before_revocation,
            vec![restored_participant.clone()]
        );

        // Model a completed removal workflow followed by a rapid re-add. The
        // current-principal checks pass after the re-add, while the durable job
        // remains as the permanent per-Session revocation fence.
        sqlx::query(
            "DELETE FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(&restored_participant))
        .execute(&pool)
        .await
        .expect("remove participant from Relay");
        sqlx::query(
            "INSERT INTO meeting_revocation_jobs \
                 (community_id, job_id, revoked_pubkey, revocation_event_id, \
                  state, completed_at) \
             VALUES ($1, $2, $3, $4, 'completed', clock_timestamp())",
        )
        .bind(community_id.as_uuid())
        .bind(Uuid::new_v4())
        .bind(&restored_participant)
        .bind([0x73_u8; 32].as_slice())
        .execute(&pool)
        .await
        .expect("persist completed revocation");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, 'member')",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(&restored_participant))
        .execute(&pool)
        .await
        .expect("restore participant Relay membership");

        assert!(
            is_meeting_reader_security_active(&db, community_id, &restored_participant)
                .await
                .expect("current security after restore"),
            "reactivated principal should pass the current-principal gate"
        );
        assert_eq!(
            frozen_meeting_reader_pubkeys_for_channel(
                &db,
                community_id,
                old_session,
                &candidates_read_before_revocation,
            )
            .await
            .expect("apply final durable fence after concurrent revocation"),
            Vec::<Vec<u8>>::new(),
            "a revocation committed after the current-principal read must win \
             at the final frozen-roster read"
        );
        assert_eq!(
            is_meeting_reader_authorized_for_channel(
                &db,
                community_id,
                old_session,
                &restored_participant,
            )
            .await
            .expect("authorize restored participant for old Meeting"),
            Some(true),
            "current Community membership restores Community-readable history"
        );
        assert_eq!(
            active_meeting_reader_pubkeys_for_channel(
                &db,
                community_id,
                old_session,
                std::slice::from_ref(&restored_participant),
            )
            .await
            .expect("filter old Meeting fan-out"),
            Some(vec![restored_participant.clone()]),
            "Community-wide live fan-out must not reuse the legacy roster fence"
        );

        let (new_session, _) =
            create_active_v0_meeting(&pool, community_id, &host, &restored_participant).await;
        assert_eq!(
            is_meeting_reader_authorized_for_channel(
                &db,
                community_id,
                new_session,
                &restored_participant,
            )
            .await
            .expect("authorize restored participant for new Meeting"),
            Some(true),
            "a revocation must not block Meetings created after the job"
        );
        assert_eq!(
            meeting_channel_ids_for_frozen_reader(
                &db,
                community_id,
                &restored_participant,
                &[old_session, new_session],
            )
            .await
            .expect("filter old and new Meeting reads"),
            vec![new_session],
            "the frozen helper remains available only for dark-launch compatibility"
        );
        let mut expected_meetings = vec![old_session, new_session];
        expected_meetings.sort();
        assert_eq!(
            community_meeting_channel_ids(&db, community_id)
                .await
                .expect("list Community-readable Meetings"),
            expected_meetings,
            "the Community reader catalog includes history across membership epochs"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn meeting_reader_uses_project_asset_membership_and_ignores_write_timeouts() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let owner = vec![0xa1_u8; 32];
        let agent = vec![0xa2_u8; 32];
        let direct_human = vec![0xa3_u8; 32];
        let unmembered_owner = vec![0xa4_u8; 32];
        let unmembered_agent = vec![0xa5_u8; 32];

        seed_identity(&pool, community_id, &owner, "owner", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &agent,
            "member",
            Some(&owner),
            "owner_only",
        )
        .await;
        seed_identity(&pool, community_id, &direct_human, "member", None, "anyone").await;
        sqlx::query(
            "INSERT INTO users \
                 (community_id, pubkey, agent_owner_pubkey, channel_add_policy) \
             VALUES ($1, $2, NULL, 'anyone'::channel_add_policy), \
                    ($1, $3, $2, 'owner_only'::channel_add_policy)",
        )
        .bind(community_id.as_uuid())
        .bind(&unmembered_owner)
        .bind(&unmembered_agent)
        .execute(&pool)
        .await
        .expect("seed Agent whose owner is not a Relay member");

        sqlx::query(
            "INSERT INTO community_bans \
                 (community_id, pubkey, banned, muted_until, actor_pubkey) \
             VALUES ($1, $2, false, clock_timestamp() + INTERVAL '10 minutes', $3), \
                    ($1, $3, false, clock_timestamp() + INTERVAL '10 minutes', $3)",
        )
        .bind(community_id.as_uuid())
        .bind(&owner)
        .bind(&direct_human)
        .execute(&pool)
        .await
        .expect("apply write-only timeouts");
        assert!(
            is_meeting_reader_security_active(&db, community_id, &direct_human)
                .await
                .expect("check timed-out Human reader"),
            "a write-only timeout must not revoke Meeting reads"
        );
        assert!(
            is_meeting_reader_security_active(&db, community_id, &agent)
                .await
                .expect("check Agent with timed-out owner"),
            "an owner write-only timeout must not cascade to Agent reads"
        );

        assert!(
            !is_meeting_reader_security_active(&db, community_id, &unmembered_agent)
                .await
                .expect("check Agent without an owner Relay membership"),
            "managed Agent Community reads require its authoritative owner to remain a member"
        );
    }
}
