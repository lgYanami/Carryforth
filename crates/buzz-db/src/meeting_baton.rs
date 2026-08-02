//! Meeting V1 moderated-baton persistence.
//!
//! V1 is a separate protocol from the V0 uniform floor. Creation freezes the
//! authoritative roster, moderator, and timing policy and commits the initial
//! relay-signed state through the durable meeting outbox in one transaction.

use std::collections::HashSet;

use buzz_core::CommunityId;
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::meeting::{MAX_MEETING_AGENTS, MAX_MEETING_PARTICIPANTS};
use crate::{Db, DbError, Result};

mod commands;

pub use commands::*;

/// Meeting V1 wire schema version.
pub const SCHEMA_VERSION: i32 = 2;
/// Persisted moderated-baton policy identifier.
pub const BATON_POLICY_VERSION: &str = "moderated-baton-v1";
/// Default immutable timing profile.
pub const DEFAULT_TIMING_PROFILE_VERSION: &str = "moderated-baton-v1-default";
/// Default deterministic fallback policy.
pub const DEFAULT_FALLBACK_POLICY_VERSION: &str = "fallback-v1";
/// Largest accepted persisted Meeting V1 duration (24 hours).
pub const MAX_BATON_DURATION_MS: i64 = 86_400_000;

/// Frozen Meeting V1 protocol configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonConfig {
    /// Versioned collection of timing defaults.
    pub timing_profile_version: String,
    /// Offer acknowledgement deadline for an Agent target.
    pub agent_offer_ack_ms: i64,
    /// Offer acknowledgement deadline for a Human target.
    pub human_offer_ack_ms: i64,
    /// Maximum moderator decision window.
    pub moderator_decision_ms: i64,
    /// Initial soft Grant lease.
    pub grant_soft_lease_ms: i64,
    /// Recommended accepted Progress interval.
    pub progress_interval_ms: i64,
    /// Absolute Grant hard deadline.
    pub grant_hard_deadline_ms: i64,
    /// Local Agent budget reserved before the hard deadline.
    pub agent_safety_margin_ms: i64,
    /// Maximum consecutive direct-handoff depth.
    pub max_handoff_depth: i32,
    /// Maximum unresolved directed handoffs.
    pub max_open_handoffs: i32,
    /// Maximum semantic rejudgments after the initial moderator attempt.
    pub moderator_max_rejudgments: i32,
    /// Maximum compare-and-swap rebases within one moderator attempt.
    pub moderator_max_cas_rebases_per_attempt: i32,
    /// Versioned deterministic fallback algorithm.
    pub fallback_policy_version: String,
}

impl Default for BatonConfig {
    fn default() -> Self {
        Self {
            timing_profile_version: DEFAULT_TIMING_PROFILE_VERSION.to_string(),
            agent_offer_ack_ms: 5_000,
            human_offer_ack_ms: 15_000,
            moderator_decision_ms: 180_000,
            grant_soft_lease_ms: 30_000,
            progress_interval_ms: 10_000,
            grant_hard_deadline_ms: 300_000,
            agent_safety_margin_ms: 30_000,
            max_handoff_depth: 5,
            max_open_handoffs: 32,
            moderator_max_rejudgments: 2,
            moderator_max_cas_rebases_per_attempt: 8,
            fallback_policy_version: DEFAULT_FALLBACK_POLICY_VERSION.to_string(),
        }
    }
}

/// Frozen participant classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParticipantType {
    /// A human-controlled identity.
    Human,
    /// A managed Agent identity.
    Agent,
}

impl ParticipantType {
    fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Agent => "agent",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "human" => Ok(Self::Human),
            "agent" => Ok(Self::Agent),
            other => Err(DbError::InvalidData(format!(
                "unknown meeting participant type: {other}"
            ))),
        }
    }
}

/// Participant projected into a Meeting V1 state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatonParticipant {
    /// Participant public key bytes.
    #[serde(with = "hex_bytes")]
    pub pubkey: Vec<u8>,
    /// Frozen authoritative identity type.
    pub participant_type: ParticipantType,
    /// Backing private-channel role.
    pub channel_role: String,
}

/// Durable phase of the moderated baton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatonPhase {
    /// The moderator owns control and no deterministic work is pending.
    ModeratorIdle,
    /// The moderator owns control while a decision deadline is active.
    ModeratorControl,
    /// A participant has an unresolved Offer.
    Offered,
    /// A participant owns the active Grant.
    Granted,
    /// The meeting is terminal.
    Ended,
}

impl BatonPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::ModeratorIdle => "moderator_idle",
            Self::ModeratorControl => "moderator_control",
            Self::Offered => "offered",
            Self::Granted => "granted",
            Self::Ended => "ended",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "moderator_idle" => Ok(Self::ModeratorIdle),
            "moderator_control" => Ok(Self::ModeratorControl),
            "offered" => Ok(Self::Offered),
            "granted" => Ok(Self::Granted),
            "ended" => Ok(Self::Ended),
            other => Err(DbError::InvalidData(format!(
                "unknown meeting baton phase: {other}"
            ))),
        }
    }
}

/// Current durable Meeting V1 state and its frozen public context.
#[derive(Debug, Clone)]
pub struct BatonSnapshot {
    /// Meeting/channel identity.
    pub session_id: Uuid,
    /// Frozen moderator public key.
    pub moderator_pubkey: Vec<u8>,
    /// Current baton phase.
    pub phase: BatonPhase,
    /// Monotonic floor/control revision.
    pub floor_revision: i64,
    /// Monotonic Intent-pool revision.
    pub intent_revision: i64,
    /// Monotonic canonical-speech revision.
    pub speech_revision: i64,
    /// Total order of relay State snapshots.
    pub state_revision: i64,
    /// Epoch of the current direct-control chain.
    pub control_epoch: i64,
    /// Epoch of the current moderator decision window.
    pub decision_epoch: i64,
    /// Monotonic attempt number within the current decision window.
    pub decision_attempt: i32,
    /// Currently running authoritative moderator attempt.
    pub active_decision_attempt_id: Option<Vec<u8>>,
    /// Relay-signed State event ID.
    pub state_event_id: Vec<u8>,
    /// Active Offer object ID.
    pub active_offer_id: Option<Vec<u8>>,
    /// Active Grant object ID.
    pub active_grant_id: Option<Vec<u8>>,
    /// Current directed-handoff depth.
    pub handoff_depth: i32,
    /// Consecutive moderator-self speeches.
    pub consecutive_moderator_speeches: i32,
    /// Whether control must return to the moderator.
    pub forced_return_to_moderator: bool,
    /// Active moderator decision deadline.
    pub moderator_decision_deadline: Option<DateTime<Utc>>,
    /// Earliest protocol deadline.
    pub next_action_at: Option<DateTime<Utc>>,
    /// Frozen timing and capacity policy.
    pub config: BatonConfig,
    /// Frozen participant roster sorted by pubkey.
    pub participants: Vec<BatonParticipant>,
    /// Projection creation time.
    pub created_at: DateTime<Utc>,
    /// Projection update time.
    pub updated_at: DateTime<Utc>,
}

/// Parameters for atomically creating a Meeting V1 session.
pub struct CreateMeetingV1Params<'a> {
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
    /// Pubkey of the signed Meeting Create author and Channel owner.
    pub host_pubkey: &'a [u8],
    /// Frozen moderator, which must occur exactly once in the roster.
    pub moderator_pubkey: &'a [u8],
    /// Event ID of the already-persisted signed Meeting Create command.
    pub create_event_id: &'a [u8],
    /// Complete participant set, including the host exactly once.
    pub participant_pubkeys: &'a [Vec<u8>],
    /// Relay identity used to sign the initial State.
    pub relay_keys: &'a Keys,
    /// Validated configuration to freeze for this session.
    pub config: BatonConfig,
}

pub(crate) struct CreateModeratedMeetingBaseParams<'a> {
    pub community_id: CommunityId,
    pub session_id: Uuid,
    pub title: &'a str,
    pub description: Option<&'a str>,
    pub source_channel_id: Option<Uuid>,
    pub host_pubkey: &'a [u8],
    pub moderator_pubkey: &'a [u8],
    pub create_event_id: &'a [u8],
    pub participant_pubkeys: &'a [Vec<u8>],
    pub schema_version: i32,
    pub policy_version: &'a str,
}

pub(crate) struct ModeratedMeetingBase {
    pub participants: Vec<BatonParticipant>,
    pub created_at: DateTime<Utc>,
}

/// Parameters for atomically ending a Meeting V1 session.
pub struct EndMeetingV1Params<'a> {
    /// Community that owns the meeting.
    pub community_id: CommunityId,
    /// Meeting/channel identity.
    pub session_id: Uuid,
    /// Author of the signed Meeting End command.
    pub actor_pubkey: &'a [u8],
    /// Referenced Meeting Create event ID.
    pub create_event_id: &'a [u8],
    /// Already-persisted Meeting End command event ID.
    pub end_event_id: &'a [u8],
    /// Relay identity used to sign the terminal State.
    pub relay_keys: &'a Keys,
}

/// Outcome of a Meeting V1 End command.
#[derive(Debug, Clone)]
pub enum EndMeetingV1Outcome {
    /// This command ended the meeting and produced a terminal State.
    Ended(Box<BatonSnapshot>),
    /// The meeting was already terminal; no command or State was committed.
    AlreadyEnded,
    /// Roster security recovery committed a Relay-authored End instead of the
    /// uncommitted manual command.
    ParticipantRevoked(Box<BatonSnapshot>),
}

/// Opaque lease token for one claim of a durable security-revocation job.
///
/// Reclaiming an expired job produces a different token. Workers must present
/// the token returned by [`claim_revocation_jobs`] when advancing, completing,
/// or releasing that claim so a stale worker cannot mutate a newer lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeetingRevocationClaimToken(i32);

impl MeetingRevocationClaimToken {
    /// Monotonic claim-attempt number, exposed for bounded observability only.
    pub fn attempt(self) -> i32 {
        self.0
    }
}

/// One claimed durable security-revocation job.
#[derive(Debug, Clone)]
pub struct MeetingRevocationJob {
    /// Community in which the identity was revoked.
    pub community_id: CommunityId,
    /// Stable job identity.
    pub job_id: Uuid,
    /// Revoked participant public key.
    pub revoked_pubkey: Vec<u8>,
    /// Event or audit identifier that caused the security revocation.
    pub revocation_event_id: Vec<u8>,
    /// Database time at which access was revoked and this job became durable.
    pub created_at: DateTime<Utc>,
    /// Database-monotonic security order allocated after the producer locks
    /// the affected authoritative identity rows.
    pub security_order: i64,
    /// Optional cursor used by a bounded worker.
    pub cursor_session_id: Option<Uuid>,
    /// Token fencing every mutation performed under this claim.
    pub claim_token: MeetingRevocationClaimToken,
}

/// Create a Meeting V1 room, strict frozen identity projection, immutable
/// configuration, and initial `moderator_idle` State in one transaction.
///
/// The signed Create event must already exist in `events` inside the same
/// transaction. Both it and the relay-signed State are enqueued in causal order.
pub async fn create_meeting_v1_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: CreateMeetingV1Params<'_>,
) -> Result<BatonSnapshot> {
    validate_create_shape(&params)?;
    validate_config(&params.config)?;
    let base = create_moderated_meeting_base_tx(
        tx,
        CreateModeratedMeetingBaseParams {
            community_id: params.community_id,
            session_id: params.session_id,
            title: params.title,
            description: params.description,
            source_channel_id: params.source_channel_id,
            host_pubkey: params.host_pubkey,
            moderator_pubkey: params.moderator_pubkey,
            create_event_id: params.create_event_id,
            participant_pubkeys: params.participant_pubkeys,
            schema_version: SCHEMA_VERSION,
            policy_version: BATON_POLICY_VERSION,
        },
    )
    .await?;
    let participants = base.participants;
    let now = base.created_at;
    insert_config_tx(tx, params.community_id, params.session_id, &params.config).await?;

    let transition = meeting_transition(
        "meeting_created",
        "accepted",
        params.session_id,
        Some(params.create_event_id),
        now,
        "meeting_created",
        None,
        Some("active"),
        None,
        Some(BatonPhase::ModeratorIdle.as_str()),
    );
    let state_event = build_state_event(
        params.relay_keys,
        params.session_id,
        params.moderator_pubkey,
        BatonPhase::ModeratorIdle,
        1,
        0,
        0,
        1,
        1,
        0,
        &params.config,
        &participants,
        &transition,
        now,
    )?;
    persist_state_event_tx(
        tx,
        params.community_id,
        params.session_id,
        &state_event,
        now,
    )
    .await?;
    insert_history_tx(
        tx,
        params.community_id,
        params.session_id,
        &state_event,
        1,
        1,
        0,
        0,
        1,
        0,
        "meeting_created",
        transition
            .get("effects")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        now,
    )
    .await?;
    sqlx::query(
        "INSERT INTO meeting_baton_state \
             (community_id, session_id, phase, floor_revision, intent_revision, \
              speech_revision, state_revision, control_epoch, decision_epoch, \
              state_event_id, created_at, updated_at) \
         VALUES ($1, $2, 'moderator_idle', 1, 0, 0, 1, 1, 0, $3, $4, $4)",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(state_event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.create_event_id,
    )
    .await?;
    crate::meeting::enqueue_meeting_event_tx(
        tx,
        params.community_id,
        params.session_id,
        state_event.id.as_bytes().as_slice(),
    )
    .await?;

    Ok(BatonSnapshot {
        session_id: params.session_id,
        moderator_pubkey: params.moderator_pubkey.to_vec(),
        phase: BatonPhase::ModeratorIdle,
        floor_revision: 1,
        intent_revision: 0,
        speech_revision: 0,
        state_revision: 1,
        control_epoch: 1,
        decision_epoch: 0,
        decision_attempt: 0,
        active_decision_attempt_id: None,
        state_event_id: state_event.id.as_bytes().to_vec(),
        active_offer_id: None,
        active_grant_id: None,
        handoff_depth: 0,
        consecutive_moderator_speeches: 0,
        forced_return_to_moderator: false,
        moderator_decision_deadline: None,
        next_action_at: None,
        config: params.config,
        participants,
        created_at: now,
        updated_at: now,
    })
}

pub(crate) async fn create_moderated_meeting_base_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: CreateModeratedMeetingBaseParams<'_>,
) -> Result<ModeratedMeetingBase> {
    ensure_existing_command_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.create_event_id,
        buzz_core::kind::KIND_MEETING_CREATE as i32,
        params.host_pubkey,
    )
    .await?;
    validate_source_access_tx(
        tx,
        params.community_id,
        params.source_channel_id,
        params.participant_pubkeys,
    )
    .await?;

    let mut participants = resolve_participants_tx(
        tx,
        params.community_id,
        params.host_pubkey,
        params.participant_pubkeys,
    )
    .await?;
    participants.sort_by(|left, right| left.pubkey.cmp(&right.pubkey));

    let title = buzz_core::channel::canonical_channel_name(params.title);
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
    .execute(tx.as_mut())
    .await?;
    if channel_insert.rows_affected() != 1 {
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
        .bind(&participant.channel_role)
        .bind(params.host_pubkey)
        .execute(tx.as_mut())
        .await?;
    }

    let now: DateTime<Utc> = sqlx::query_scalar(
        "INSERT INTO meeting_sessions \
             (community_id, session_id, create_event_id, host_pubkey, \
              source_channel_id, schema_version, status, floor_policy_version, \
              moderator_pubkey, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, 'active', $7, $8, clock_timestamp()) \
         RETURNING created_at",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.create_event_id)
    .bind(params.host_pubkey)
    .bind(params.source_channel_id)
    .bind(params.schema_version)
    .bind(params.policy_version)
    .bind(params.moderator_pubkey)
    .fetch_one(tx.as_mut())
    .await?;

    for participant in &participants {
        sqlx::query(
            "INSERT INTO meeting_participants \
                 (community_id, session_id, pubkey, participant_type, channel_role) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(params.community_id.as_uuid())
        .bind(params.session_id)
        .bind(&participant.pubkey)
        .bind(participant.participant_type.as_str())
        .bind(&participant.channel_role)
        .execute(tx.as_mut())
        .await?;
    }

    Ok(ModeratedMeetingBase {
        participants,
        created_at: now,
    })
}

/// End an active Meeting V1 and commit its terminal State through the meeting
/// outbox in the same transaction.
pub async fn end_meeting_v1_tx(
    tx: &mut Transaction<'_, Postgres>,
    params: EndMeetingV1Params<'_>,
) -> Result<EndMeetingV1Outcome> {
    validate_32_bytes(params.actor_pubkey, "actor pubkey")?;
    validate_32_bytes(params.create_event_id, "create event id")?;
    validate_32_bytes(params.end_event_id, "end event id")?;
    ensure_existing_command_event_tx(
        tx,
        params.community_id,
        params.session_id,
        params.end_event_id,
        buzz_core::kind::KIND_MEETING_END as i32,
        params.actor_pubkey,
    )
    .await?;

    let session = lock_v1_session_tx(tx, params.community_id, params.session_id).await?;
    if session.create_event_id != params.create_event_id {
        return Err(DbError::InvalidData(
            "meeting end references the wrong create event".to_string(),
        ));
    }
    if session.status == "active" {
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
            return Ok(EndMeetingV1Outcome::ParticipantRevoked(Box::new(snapshot)));
        }
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
    if !crate::meeting::actor_security_active_tx(tx, params.community_id, params.actor_pubkey)
        .await?
    {
        if session.status == "active" {
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
                return Ok(EndMeetingV1Outcome::ParticipantRevoked(Box::new(snapshot)));
            }
        }
        return Err(DbError::AccessDenied(
            "meeting End author is no longer an active writable community principal".to_string(),
        ));
    }
    authorize_end_tx(
        tx,
        params.community_id,
        params.session_id,
        params.actor_pubkey,
        &session.host_pubkey,
    )
    .await?;
    if session.status == "ended" {
        return Ok(EndMeetingV1Outcome::AlreadyEnded);
    }
    if session.status != "active" {
        return Err(DbError::InvalidData(format!(
            "unknown meeting status: {}",
            session.status
        )));
    }

    let ended_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE meeting_sessions \
         SET status = 'ended', ended_at = clock_timestamp(), ended_by = $3, \
             end_event_id = $4 \
         WHERE community_id = $1 AND session_id = $2 AND status = 'active' \
           AND schema_version = 2 AND floor_policy_version = $5 \
         RETURNING ended_at",
    )
    .bind(params.community_id.as_uuid())
    .bind(params.session_id)
    .bind(params.actor_pubkey)
    .bind(params.end_event_id)
    .bind(BATON_POLICY_VERSION)
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
            "meeting channel is missing or not active".to_string(),
        ));
    }

    let snapshot = close_baton_locked_tx(
        tx,
        params.community_id,
        params.session_id,
        params.end_event_id,
        "meeting_ended",
        params.relay_keys,
        ended_at,
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
    Ok(EndMeetingV1Outcome::Ended(Box::new(snapshot)))
}

/// Fetch the current Meeting V1 baton state and frozen public context.
pub async fn get_baton_snapshot(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<BatonSnapshot> {
    load_snapshot_pool(&db.pool, community_id, session_id).await
}

/// Enqueue a real security revocation inside the producer's authorization
/// transaction.
///
/// Producers must use this form when removing membership, banning an identity,
/// or deactivating its authoritative user row so the authorization change and
/// cleanup job cannot commit independently.
///
/// An authoritative owner ban/deactivation also revokes every Agent whose
/// `users.agent_owner_pubkey` names that owner. Such a producer must enqueue
/// one child job per owned Agent in this same transaction. The repository has
/// no production account-deactivation mutation today, so Meeting command
/// lazy-recovery checks owner liveness as the immediate backstop until that
/// producer is introduced. NIP-IA archival must never call this API.
pub async fn enqueue_revocation_job_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    job_id: Uuid,
    revoked_pubkey: &[u8],
    revocation_event_id: &[u8],
) -> Result<bool> {
    if job_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting revocation job id must not be nil".to_string(),
        ));
    }
    validate_32_bytes(revoked_pubkey, "revoked pubkey")?;
    validate_32_bytes(revocation_event_id, "revocation event id")?;
    let result = sqlx::query(
        "INSERT INTO meeting_revocation_jobs \
             (community_id, job_id, revoked_pubkey, revocation_event_id) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (community_id, revocation_event_id) DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(job_id)
    .bind(revoked_pubkey)
    .bind(revocation_event_id)
    .execute(tx.as_mut())
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Claim due security-revocation jobs with a bounded database lease.
pub async fn claim_revocation_jobs(
    db: &Db,
    limit: i64,
    lease_ms: i64,
) -> Result<Vec<MeetingRevocationJob>> {
    if limit <= 0 {
        return Ok(Vec::new());
    }
    if lease_ms <= 0 {
        return Err(DbError::InvalidData(
            "meeting revocation lease must be positive".to_string(),
        ));
    }
    let rows = sqlx::query(
        "WITH candidates AS ( \
             SELECT community_id, job_id \
             FROM meeting_revocation_jobs \
             WHERE state IN ('pending', 'running') \
               AND next_attempt_at <= clock_timestamp() \
             ORDER BY next_attempt_at, community_id, job_id \
             FOR UPDATE SKIP LOCKED \
             LIMIT $1 \
         ) \
         UPDATE meeting_revocation_jobs jobs \
         SET state = 'running', attempts = attempts + 1, \
             next_attempt_at = clock_timestamp() + ($2 * interval '1 millisecond'), \
             last_error = NULL \
         FROM candidates \
         WHERE jobs.community_id = candidates.community_id \
           AND jobs.job_id = candidates.job_id \
         RETURNING jobs.community_id, jobs.job_id, jobs.revoked_pubkey, \
                   jobs.revocation_event_id, jobs.created_at, jobs.security_order, \
                   jobs.cursor_session_id, jobs.attempts",
    )
    .bind(limit)
    .bind(lease_ms)
    .fetch_all(&db.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(MeetingRevocationJob {
                community_id: CommunityId::from_uuid(row.try_get("community_id")?),
                job_id: row.try_get("job_id")?,
                revoked_pubkey: row.try_get("revoked_pubkey")?,
                revocation_event_id: row.try_get("revocation_event_id")?,
                created_at: row.try_get("created_at")?,
                security_order: row.try_get("security_order")?,
                cursor_session_id: row.try_get("cursor_session_id")?,
                claim_token: MeetingRevocationClaimToken(row.try_get("attempts")?),
            })
        })
        .collect()
}

/// Advance a security-revocation job cursor after one bounded worker batch.
///
/// Returns `false` when the claim token no longer owns the running lease.
pub async fn advance_revocation_job(
    db: &Db,
    community_id: CommunityId,
    job_id: Uuid,
    claim_token: MeetingRevocationClaimToken,
    cursor_session_id: Uuid,
    retry_at: DateTime<Utc>,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_revocation_jobs \
         SET cursor_session_id = $4, state = 'pending', next_attempt_at = $5 \
         WHERE community_id = $1 AND job_id = $2 AND state = 'running' \
           AND attempts = $3",
    )
    .bind(community_id.as_uuid())
    .bind(job_id)
    .bind(claim_token.attempt())
    .bind(cursor_session_id)
    .bind(retry_at)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Mark a security-revocation job complete.
///
/// Returns `false` when the claim token no longer owns the running lease.
pub async fn complete_revocation_job(
    db: &Db,
    community_id: CommunityId,
    job_id: Uuid,
    claim_token: MeetingRevocationClaimToken,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_revocation_jobs \
         SET state = 'completed', completed_at = clock_timestamp() \
         WHERE community_id = $1 AND job_id = $2 AND state = 'running' \
           AND attempts = $3",
    )
    .bind(community_id.as_uuid())
    .bind(job_id)
    .bind(claim_token.attempt())
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Release a claimed security-revocation job for retry.
///
/// Returns `false` when the claim token no longer owns the running lease.
pub async fn release_revocation_job(
    db: &Db,
    community_id: CommunityId,
    job_id: Uuid,
    claim_token: MeetingRevocationClaimToken,
    error: &str,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_revocation_jobs \
         SET state = 'pending', next_attempt_at = clock_timestamp() + interval '1 second', \
             last_error = $4 \
         WHERE community_id = $1 AND job_id = $2 AND state = 'running' \
           AND attempts = $3",
    )
    .bind(community_id.as_uuid())
    .bind(job_id)
    .bind(claim_token.attempt())
    .bind(error)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[derive(Debug)]
struct V1SessionLock {
    create_event_id: Vec<u8>,
    host_pubkey: Vec<u8>,
    status: String,
}

async fn lock_v1_session_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<V1SessionLock> {
    let row = sqlx::query(
        "SELECT create_event_id, host_pubkey, moderator_pubkey, status, \
                schema_version, floor_policy_version \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let schema_version: i32 = row.try_get("schema_version")?;
    let policy: String = row.try_get("floor_policy_version")?;
    if schema_version != SCHEMA_VERSION || policy != BATON_POLICY_VERSION {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a {BATON_POLICY_VERSION} session"
        )));
    }
    let moderator_pubkey: Option<Vec<u8>> = row.try_get("moderator_pubkey")?;
    let moderator_pubkey = moderator_pubkey.ok_or_else(|| {
        DbError::InvalidData(format!("meeting {session_id} is missing its moderator"))
    })?;
    validate_32_bytes(&moderator_pubkey, "meeting moderator pubkey")?;
    Ok(V1SessionLock {
        create_event_id: row.try_get("create_event_id")?,
        host_pubkey: row.try_get("host_pubkey")?,
        status: row.try_get("status")?,
    })
}

async fn close_baton_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    caused_by_event_id: &[u8],
    primary_type: &str,
    relay_keys: &Keys,
    now: DateTime<Utc>,
) -> Result<BatonSnapshot> {
    let state = load_state_tx(tx, community_id, session_id, true).await?;
    if state.phase == BatonPhase::Ended {
        return load_snapshot_tx(tx, community_id, session_id).await;
    }
    let config = load_config_tx(tx, community_id, session_id).await?;
    let participants = load_participants_tx(tx, community_id, session_id).await?;
    let moderator_pubkey = load_moderator_tx(tx, community_id, session_id).await?;

    // End every live protocol object before publishing the terminal snapshot.
    // Historical terminal rows remain untouched. These updates deliberately
    // share the Meeting Session lock held by both manual End and revocation End.
    let mut ended_intents = sqlx::query(
        "SELECT intent_id, state \
         FROM meeting_speech_intents \
         WHERE community_id = $1 AND session_id = $2 \
           AND state IN ('pending', 'selected') \
         ORDER BY intent_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| Ok((row.try_get("intent_id")?, row.try_get("state")?)))
    .collect::<std::result::Result<Vec<(Vec<u8>, String)>, sqlx::Error>>()?;
    ended_intents.sort_by(|left, right| left.0.cmp(&right.0));
    let ended_intent_count = sqlx::query(
        "UPDATE meeting_speech_intents \
         SET state = 'ended', terminal_event_id = $3, terminal_at = $4, \
             updated_at = $4, last_attempt_outcome = 'ended', \
             deferred_by_offer_id = NULL, defer_event_id = NULL, defer_reason = NULL \
         WHERE community_id = $1 AND session_id = $2 \
           AND state IN ('pending', 'selected')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(caused_by_event_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut ended_requests = sqlx::query(
        "SELECT request_id, state \
         FROM meeting_human_floor_requests \
         WHERE community_id = $1 AND session_id = $2 \
           AND state IN ('queued', 'offered') \
         ORDER BY request_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| Ok((row.try_get("request_id")?, row.try_get("state")?)))
    .collect::<std::result::Result<Vec<(Vec<u8>, String)>, sqlx::Error>>()?;
    ended_requests.sort_by(|left, right| left.0.cmp(&right.0));
    let ended_request_count = sqlx::query(
        "UPDATE meeting_human_floor_requests \
         SET state = 'ended', terminal_event_id = $3, terminal_at = $4 \
         WHERE community_id = $1 AND session_id = $2 \
           AND state IN ('queued', 'offered')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(caused_by_event_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut ended_offer_ids: Vec<Vec<u8>> = sqlx::query(
        "SELECT offer_id \
         FROM meeting_baton_offers \
         WHERE community_id = $1 AND session_id = $2 AND state = 'pending' \
         ORDER BY offer_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| row.try_get("offer_id"))
    .collect::<std::result::Result<_, sqlx::Error>>()?;
    ended_offer_ids.sort();
    let ended_offer_count = sqlx::query(
        "UPDATE meeting_baton_offers \
         SET state = 'ended', resolved_at = $3 \
         WHERE community_id = $1 AND session_id = $2 AND state = 'pending'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut ended_grant_ids: Vec<Vec<u8>> = sqlx::query(
        "SELECT grant_id \
         FROM meeting_baton_grants \
         WHERE community_id = $1 AND session_id = $2 AND state = 'active' \
         ORDER BY grant_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| row.try_get("grant_id"))
    .collect::<std::result::Result<_, sqlx::Error>>()?;
    ended_grant_ids.sort();
    let ended_grant_count = sqlx::query(
        "UPDATE meeting_baton_grants \
         SET state = 'ended', terminal_event_id = $3, \
             terminal_reason = $4, terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND state = 'active'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(caused_by_event_id)
    .bind(primary_type)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let mut ended_handoff_ids: Vec<Vec<u8>> = sqlx::query(
        "SELECT handoff_id \
         FROM meeting_directed_handoffs \
         WHERE community_id = $1 AND session_id = $2 AND question_state = 'open' \
         ORDER BY handoff_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| row.try_get("handoff_id"))
    .collect::<std::result::Result<_, sqlx::Error>>()?;
    ended_handoff_ids.sort();
    let ended_handoff_count = sqlx::query(
        "UPDATE meeting_directed_handoffs \
         SET question_state = 'ended', last_attempt_outcome = 'ended', terminal_at = $3 \
         WHERE community_id = $1 AND session_id = $2 AND question_state = 'open'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    let ended_attempt_ids: Vec<Vec<u8>> = sqlx::query(
        "UPDATE meeting_moderator_decision_attempts \
         SET state = 'discarded', terminal_event_id = $3, \
             terminal_reason = $4, terminal_at = $5 \
         WHERE community_id = $1 AND session_id = $2 AND state = 'running' \
         RETURNING attempt_id",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(caused_by_event_id)
    .bind(primary_type)
    .bind(now)
    .fetch_all(tx.as_mut())
    .await?
    .into_iter()
    .map(|row| row.try_get("attempt_id"))
    .collect::<std::result::Result<_, sqlx::Error>>()?;

    for (label, actual, expected) in [
        (
            "Intent",
            ended_intent_count.rows_affected(),
            ended_intents.len(),
        ),
        (
            "Human Request",
            ended_request_count.rows_affected(),
            ended_requests.len(),
        ),
        (
            "Offer",
            ended_offer_count.rows_affected(),
            ended_offer_ids.len(),
        ),
        (
            "Grant",
            ended_grant_count.rows_affected(),
            ended_grant_ids.len(),
        ),
        (
            "Directed Handoff",
            ended_handoff_count.rows_affected(),
            ended_handoff_ids.len(),
        ),
    ] {
        if actual
            != u64::try_from(expected).map_err(|_| {
                DbError::InvalidData(format!("{label} end projection count does not fit u64"))
            })?
        {
            return Err(DbError::InvalidData(format!(
                "{label} projection changed while ending meeting {session_id}"
            )));
        }
    }

    let next_state_revision = state.state_revision + 1;
    let next_floor_revision = state.floor_revision + 1;
    let next_intent_revision =
        state.intent_revision + i64::from(!ended_intents.is_empty() || !ended_requests.is_empty());
    let mut effects = vec![serde_json::json!({
        "type": primary_type,
        "object_type": "meeting",
        "object_id": session_id.to_string(),
        "from": "active",
        "to": "ended",
    })];
    effects.extend(ended_offer_ids.iter().map(|offer_id| {
        serde_json::json!({
            "type": "offer_ended",
            "object_type": "offer",
            "object_id": hex::encode(offer_id),
            "from": "pending",
            "to": "ended",
        })
    }));
    effects.extend(ended_grant_ids.iter().map(|grant_id| {
        serde_json::json!({
            "type": "grant_ended",
            "object_type": "grant",
            "object_id": hex::encode(grant_id),
            "from": "active",
            "to": "ended",
        })
    }));
    effects.extend(ended_intents.iter().map(|(intent_id, from)| {
        serde_json::json!({
            "type": "intent_ended",
            "object_type": "intent",
            "object_id": hex::encode(intent_id),
            "from": from,
            "to": "ended",
        })
    }));
    effects.extend(ended_requests.iter().map(|(request_id, from)| {
        serde_json::json!({
            "type": "human_ended",
            "object_type": "human_request",
            "object_id": hex::encode(request_id),
            "from": from,
            "to": "ended",
        })
    }));
    effects.extend(ended_handoff_ids.iter().map(|handoff_id| {
        serde_json::json!({
            "type": "handoff_ended",
            "object_type": "handoff",
            "object_id": hex::encode(handoff_id),
            "from": "open",
            "to": "ended",
        })
    }));
    effects.extend(ended_attempt_ids.iter().map(|attempt_id| {
        serde_json::json!({
            "type": "moderator_decision_attempt_discarded",
            "object_type": "moderator_decision_attempt",
            "object_id": hex::encode(attempt_id),
            "from": "running",
            "to": "discarded",
        })
    }));
    if let Some(recall_event_id) = state.recall_event_id.as_deref() {
        effects.push(serde_json::json!({
            "type": "recall_cleared",
            "object_type": "recall",
            "object_id": hex::encode(recall_event_id),
            "from": "latched",
            "to": "cleared",
        }));
    }
    effects.push(serde_json::json!({
        "type": "phase_changed",
        "object_type": "phase",
        "object_id": session_id.to_string(),
        "from": state.phase.as_str(),
        "to": BatonPhase::Ended.as_str(),
    }));
    let transition = serde_json::json!({
        "primary_type": primary_type,
        "outcome": "accepted",
        "primary_object_id": session_id.to_string(),
        "caused_by_event_id": hex::encode(caused_by_event_id),
        "deadline_type": null,
        "blocked_by": null,
        "at_ms": now.timestamp_millis(),
        "effects": effects,
    });
    let state_event = build_state_event(
        relay_keys,
        session_id,
        &moderator_pubkey,
        BatonPhase::Ended,
        next_floor_revision,
        next_intent_revision,
        state.speech_revision,
        next_state_revision,
        state.control_epoch,
        state.decision_epoch,
        &config,
        &participants,
        &transition,
        now,
    )?;
    persist_state_event_tx(tx, community_id, session_id, &state_event, now).await?;
    insert_history_tx(
        tx,
        community_id,
        session_id,
        &state_event,
        next_state_revision,
        next_floor_revision,
        next_intent_revision,
        state.speech_revision,
        state.control_epoch,
        state.decision_epoch,
        primary_type,
        transition
            .get("effects")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        now,
    )
    .await?;
    sqlx::query(
        "UPDATE meeting_baton_state \
         SET phase = 'ended', floor_revision = $3, intent_revision = $4, \
             state_revision = $5, state_event_id = $6, \
             active_offer_id = NULL, active_grant_id = NULL, handoff_depth = 0, \
             active_decision_attempt_id = NULL, \
             consecutive_moderator_speeches = 0, forced_return_to_moderator = FALSE, \
             recall_event_id = NULL, \
             moderator_decision_started_at = NULL, moderator_decision_deadline = NULL, \
             next_action_at = NULL, recovery_retry_at = '-infinity', \
             recovery_attempts = 0, updated_at = $7 \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(next_floor_revision)
    .bind(next_intent_revision)
    .bind(next_state_revision)
    .bind(state_event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    Ok(BatonSnapshot {
        session_id,
        moderator_pubkey,
        phase: BatonPhase::Ended,
        floor_revision: next_floor_revision,
        intent_revision: next_intent_revision,
        speech_revision: state.speech_revision,
        state_revision: next_state_revision,
        control_epoch: state.control_epoch,
        decision_epoch: state.decision_epoch,
        decision_attempt: state.decision_attempt,
        active_decision_attempt_id: None,
        state_event_id: state_event.id.as_bytes().to_vec(),
        active_offer_id: None,
        active_grant_id: None,
        handoff_depth: 0,
        consecutive_moderator_speeches: 0,
        forced_return_to_moderator: false,
        moderator_decision_deadline: None,
        next_action_at: None,
        config,
        participants,
        created_at: state.created_at,
        updated_at: now,
    })
}

/// Close a locked, terminal Meeting V1 after a real participant security
/// revocation.
///
/// The caller owns the `meeting_sessions` row lock, has already persisted the
/// Relay-authored End, and remains responsible for enqueuing the returned State.
pub(crate) async fn close_baton_for_security_revocation_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    end_event_id: &[u8],
    relay_keys: &Keys,
    ended_at: DateTime<Utc>,
) -> Result<BatonSnapshot> {
    close_baton_locked_tx(
        tx,
        community_id,
        session_id,
        end_event_id,
        "participant_revoked",
        relay_keys,
        ended_at,
    )
    .await
}

#[derive(Debug, Clone)]
struct StateRow {
    phase: BatonPhase,
    floor_revision: i64,
    intent_revision: i64,
    speech_revision: i64,
    state_revision: i64,
    control_epoch: i64,
    decision_epoch: i64,
    decision_attempt: i32,
    active_decision_attempt_id: Option<Vec<u8>>,
    state_event_id: Vec<u8>,
    active_offer_id: Option<Vec<u8>>,
    active_grant_id: Option<Vec<u8>>,
    handoff_depth: i32,
    consecutive_moderator_speeches: i32,
    forced_return_to_moderator: bool,
    recall_event_id: Option<Vec<u8>>,
    moderator_decision_started_at: Option<DateTime<Utc>>,
    moderator_decision_deadline: Option<DateTime<Utc>>,
    next_action_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

async fn load_state_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    for_update: bool,
) -> Result<StateRow> {
    let row = if for_update {
        sqlx::query(
            "SELECT phase, floor_revision, intent_revision, speech_revision, \
                    state_revision, control_epoch, decision_epoch, decision_attempt, \
                    active_decision_attempt_id, state_event_id, \
                    active_offer_id, active_grant_id, handoff_depth, \
                    consecutive_moderator_speeches, forced_return_to_moderator, \
                    recall_event_id, moderator_decision_started_at, \
                    moderator_decision_deadline, next_action_at, created_at, updated_at \
             FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2 \
             FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?
    } else {
        sqlx::query(
            "SELECT phase, floor_revision, intent_revision, speech_revision, \
                    state_revision, control_epoch, decision_epoch, decision_attempt, \
                    active_decision_attempt_id, state_event_id, \
                    active_offer_id, active_grant_id, handoff_depth, \
                    consecutive_moderator_speeches, forced_return_to_moderator, \
                    recall_event_id, moderator_decision_started_at, \
                    moderator_decision_deadline, next_action_at, created_at, updated_at \
             FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_optional(tx.as_mut())
        .await?
    };
    let row = row.ok_or_else(|| DbError::NotFound(format!("meeting baton {session_id}")))?;
    state_row_from_pg(row)
}

fn state_row_from_pg(row: sqlx::postgres::PgRow) -> Result<StateRow> {
    let phase: String = row.try_get("phase")?;
    Ok(StateRow {
        phase: BatonPhase::parse(&phase)?,
        floor_revision: row.try_get("floor_revision")?,
        intent_revision: row.try_get("intent_revision")?,
        speech_revision: row.try_get("speech_revision")?,
        state_revision: row.try_get("state_revision")?,
        control_epoch: row.try_get("control_epoch")?,
        decision_epoch: row.try_get("decision_epoch")?,
        decision_attempt: row.try_get("decision_attempt")?,
        active_decision_attempt_id: row.try_get("active_decision_attempt_id")?,
        state_event_id: row.try_get("state_event_id")?,
        active_offer_id: row.try_get("active_offer_id")?,
        active_grant_id: row.try_get("active_grant_id")?,
        handoff_depth: row.try_get("handoff_depth")?,
        consecutive_moderator_speeches: row.try_get("consecutive_moderator_speeches")?,
        forced_return_to_moderator: row.try_get("forced_return_to_moderator")?,
        recall_event_id: row.try_get("recall_event_id")?,
        moderator_decision_started_at: row.try_get("moderator_decision_started_at")?,
        moderator_decision_deadline: row.try_get("moderator_decision_deadline")?,
        next_action_at: row.try_get("next_action_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn load_snapshot_pool(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<BatonSnapshot> {
    assert_v1_session_pool(pool, community_id, session_id).await?;
    let row = sqlx::query(
        "SELECT phase, floor_revision, intent_revision, speech_revision, \
                state_revision, control_epoch, decision_epoch, decision_attempt, \
                active_decision_attempt_id, state_event_id, \
                active_offer_id, active_grant_id, handoff_depth, \
                consecutive_moderator_speeches, forced_return_to_moderator, \
                recall_event_id, moderator_decision_started_at, \
                moderator_decision_deadline, next_action_at, created_at, updated_at \
         FROM meeting_baton_state \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting baton {session_id}")))?;
    let state = state_row_from_pg(row)?;
    let config = load_config_pool(pool, community_id, session_id).await?;
    let participants = load_participants_pool(pool, community_id, session_id).await?;
    let moderator_pubkey = load_moderator_pool(pool, community_id, session_id).await?;
    Ok(snapshot_from_parts(
        session_id,
        moderator_pubkey,
        state,
        config,
        participants,
    ))
}

async fn load_snapshot_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<BatonSnapshot> {
    let state = load_state_tx(tx, community_id, session_id, false).await?;
    let config = load_config_tx(tx, community_id, session_id).await?;
    let participants = load_participants_tx(tx, community_id, session_id).await?;
    let moderator_pubkey = load_moderator_tx(tx, community_id, session_id).await?;
    Ok(snapshot_from_parts(
        session_id,
        moderator_pubkey,
        state,
        config,
        participants,
    ))
}

fn snapshot_from_parts(
    session_id: Uuid,
    moderator_pubkey: Vec<u8>,
    state: StateRow,
    config: BatonConfig,
    participants: Vec<BatonParticipant>,
) -> BatonSnapshot {
    BatonSnapshot {
        session_id,
        moderator_pubkey,
        phase: state.phase,
        floor_revision: state.floor_revision,
        intent_revision: state.intent_revision,
        speech_revision: state.speech_revision,
        state_revision: state.state_revision,
        control_epoch: state.control_epoch,
        decision_epoch: state.decision_epoch,
        decision_attempt: state.decision_attempt,
        active_decision_attempt_id: state.active_decision_attempt_id,
        state_event_id: state.state_event_id,
        active_offer_id: state.active_offer_id,
        active_grant_id: state.active_grant_id,
        handoff_depth: state.handoff_depth,
        consecutive_moderator_speeches: state.consecutive_moderator_speeches,
        forced_return_to_moderator: state.forced_return_to_moderator,
        moderator_decision_deadline: state.moderator_decision_deadline,
        next_action_at: state.next_action_at,
        config,
        participants,
        created_at: state.created_at,
        updated_at: state.updated_at,
    }
}

async fn assert_v1_session_pool(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<()> {
    let row = sqlx::query(
        "SELECT schema_version, floor_policy_version, moderator_pubkey \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let schema_version: i32 = row.try_get("schema_version")?;
    let policy: String = row.try_get("floor_policy_version")?;
    let moderator: Option<Vec<u8>> = row.try_get("moderator_pubkey")?;
    if schema_version != SCHEMA_VERSION
        || policy != BATON_POLICY_VERSION
        || moderator.as_ref().is_none_or(|value| value.len() != 32)
    {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a valid {BATON_POLICY_VERSION} session"
        )));
    }
    Ok(())
}

async fn load_moderator_pool(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<u8>> {
    let moderator: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT moderator_pubkey FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
           AND schema_version = 2 AND floor_policy_version = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(BATON_POLICY_VERSION)
    .fetch_optional(pool)
    .await?
    .flatten();
    moderator.ok_or_else(|| DbError::NotFound(format!("meeting moderator {session_id}")))
}

async fn load_moderator_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<u8>> {
    let moderator: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT moderator_pubkey FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
           AND schema_version = 2 AND floor_policy_version = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(BATON_POLICY_VERSION)
    .fetch_optional(tx.as_mut())
    .await?
    .flatten();
    moderator.ok_or_else(|| DbError::NotFound(format!("meeting moderator {session_id}")))
}

async fn load_participants_pool(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<BatonParticipant>> {
    let rows = sqlx::query(
        "SELECT pubkey, participant_type, channel_role \
         FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2 \
         ORDER BY pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    participants_from_rows(rows)
}

async fn load_participants_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Vec<BatonParticipant>> {
    let rows = sqlx::query(
        "SELECT pubkey, participant_type, channel_role \
         FROM meeting_participants \
         WHERE community_id = $1 AND session_id = $2 \
         ORDER BY pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_all(tx.as_mut())
    .await?;
    participants_from_rows(rows)
}

fn participants_from_rows(rows: Vec<sqlx::postgres::PgRow>) -> Result<Vec<BatonParticipant>> {
    rows.into_iter()
        .map(|row| {
            let participant_type: String = row.try_get("participant_type")?;
            Ok(BatonParticipant {
                pubkey: row.try_get("pubkey")?,
                participant_type: ParticipantType::parse(&participant_type)?,
                channel_role: row.try_get("channel_role")?,
            })
        })
        .collect()
}

async fn load_config_pool(
    pool: &PgPool,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<BatonConfig> {
    let row = sqlx::query(
        "SELECT timing_profile_version, agent_offer_ack_ms, human_offer_ack_ms, \
                moderator_decision_ms, grant_soft_lease_ms, progress_interval_ms, \
                grant_hard_deadline_ms, agent_safety_margin_ms, max_handoff_depth, \
                max_open_handoffs, moderator_max_rejudgments, \
                moderator_max_cas_rebases_per_attempt, fallback_policy_version \
         FROM meeting_baton_config \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting baton config {session_id}")))?;
    config_from_row(row)
}

async fn load_config_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<BatonConfig> {
    let row = sqlx::query(
        "SELECT timing_profile_version, agent_offer_ack_ms, human_offer_ack_ms, \
                moderator_decision_ms, grant_soft_lease_ms, progress_interval_ms, \
                grant_hard_deadline_ms, agent_safety_margin_ms, max_handoff_depth, \
                max_open_handoffs, moderator_max_rejudgments, \
                moderator_max_cas_rebases_per_attempt, fallback_policy_version \
         FROM meeting_baton_config \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting baton config {session_id}")))?;
    config_from_row(row)
}

fn config_from_row(row: sqlx::postgres::PgRow) -> Result<BatonConfig> {
    Ok(BatonConfig {
        timing_profile_version: row.try_get("timing_profile_version")?,
        agent_offer_ack_ms: row.try_get("agent_offer_ack_ms")?,
        human_offer_ack_ms: row.try_get("human_offer_ack_ms")?,
        moderator_decision_ms: row.try_get("moderator_decision_ms")?,
        grant_soft_lease_ms: row.try_get("grant_soft_lease_ms")?,
        progress_interval_ms: row.try_get("progress_interval_ms")?,
        grant_hard_deadline_ms: row.try_get("grant_hard_deadline_ms")?,
        agent_safety_margin_ms: row.try_get("agent_safety_margin_ms")?,
        max_handoff_depth: row.try_get("max_handoff_depth")?,
        max_open_handoffs: row.try_get("max_open_handoffs")?,
        moderator_max_rejudgments: row.try_get("moderator_max_rejudgments")?,
        moderator_max_cas_rebases_per_attempt: row
            .try_get("moderator_max_cas_rebases_per_attempt")?,
        fallback_policy_version: row.try_get("fallback_policy_version")?,
    })
}

async fn insert_config_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    config: &BatonConfig,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_baton_config \
             (community_id, session_id, timing_profile_version, agent_offer_ack_ms, \
              human_offer_ack_ms, moderator_decision_ms, grant_soft_lease_ms, \
              progress_interval_ms, grant_hard_deadline_ms, agent_safety_margin_ms, \
              max_handoff_depth, max_open_handoffs, moderator_max_rejudgments, \
              moderator_max_cas_rebases_per_attempt, fallback_policy_version) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(&config.timing_profile_version)
    .bind(config.agent_offer_ack_ms)
    .bind(config.human_offer_ack_ms)
    .bind(config.moderator_decision_ms)
    .bind(config.grant_soft_lease_ms)
    .bind(config.progress_interval_ms)
    .bind(config.grant_hard_deadline_ms)
    .bind(config.agent_safety_margin_ms)
    .bind(config.max_handoff_depth)
    .bind(config.max_open_handoffs)
    .bind(config.moderator_max_rejudgments)
    .bind(config.moderator_max_cas_rebases_per_attempt)
    .bind(&config.fallback_policy_version)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn resolve_participants_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    host_pubkey: &[u8],
    participant_pubkeys: &[Vec<u8>],
) -> Result<Vec<BatonParticipant>> {
    let mut participants = Vec::with_capacity(participant_pubkeys.len());
    let mut agent_count = 0usize;
    for pubkey in participant_pubkeys {
        let pubkey_hex = hex::encode(pubkey);
        let relay_membership: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2 \
             FOR KEY SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(&pubkey_hex)
        .fetch_optional(tx.as_mut())
        .await?;
        if relay_membership.is_none() {
            return Err(DbError::AccessDenied(format!(
                "participant {pubkey_hex} is not a member of this community"
            )));
        }
        let participant_banned: bool = sqlx::query_scalar(
            "SELECT EXISTS ( \
                 SELECT 1 FROM community_bans \
                 WHERE community_id = $1 AND pubkey = $2 AND banned \
                   AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp()) \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(pubkey)
        .fetch_one(tx.as_mut())
        .await?;
        if participant_banned {
            return Err(DbError::AccessDenied(format!(
                "participant {pubkey_hex} is banned from this community"
            )));
        }
        let identity = sqlx::query(
            "SELECT agent_owner_pubkey, channel_add_policy::text AS channel_add_policy \
             FROM users \
             WHERE community_id = $1 AND pubkey = $2 AND deactivated_at IS NULL \
             FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(pubkey)
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| {
            DbError::InvalidData(format!(
                "participant {pubkey_hex} has no authoritative identity type"
            ))
        })?;
        let agent_owner: Option<Vec<u8>> = identity.try_get("agent_owner_pubkey")?;
        let add_policy: String = identity.try_get("channel_add_policy")?;
        let participant_type = if agent_owner.is_some() {
            ParticipantType::Agent
        } else {
            ParticipantType::Human
        };
        if participant_type == ParticipantType::Agent {
            if let Some(owner_pubkey) = agent_owner.as_deref() {
                let owner_active: Option<i32> = sqlx::query_scalar(
                    "SELECT 1 FROM users \
                     WHERE community_id = $1 AND pubkey = $2 AND deactivated_at IS NULL \
                     FOR SHARE",
                )
                .bind(community_id.as_uuid())
                .bind(owner_pubkey)
                .fetch_optional(tx.as_mut())
                .await?;
                if owner_active.is_none() {
                    return Err(DbError::AccessDenied(format!(
                        "participant {pubkey_hex} has no active authoritative owner"
                    )));
                }
                let owner_banned: bool = sqlx::query_scalar(
                    "SELECT EXISTS ( \
                         SELECT 1 FROM community_bans \
                         WHERE community_id = $1 AND pubkey = $2 AND banned \
                           AND (ban_expires_at IS NULL \
                                OR ban_expires_at > clock_timestamp()) \
                     )",
                )
                .bind(community_id.as_uuid())
                .bind(owner_pubkey)
                .fetch_one(tx.as_mut())
                .await?;
                if owner_banned {
                    return Err(DbError::AccessDenied(format!(
                        "participant {pubkey_hex} has a banned authoritative owner"
                    )));
                }
            }
            agent_count += 1;
            if pubkey.as_slice() != host_pubkey {
                match add_policy.as_str() {
                    "anyone" => {}
                    "owner_only" if agent_owner.as_deref() == Some(host_pubkey) => {}
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
        let channel_role = if pubkey.as_slice() == host_pubkey {
            "owner"
        } else if participant_type == ParticipantType::Agent {
            "bot"
        } else {
            "member"
        };
        participants.push(BatonParticipant {
            pubkey: pubkey.clone(),
            participant_type,
            channel_role: channel_role.to_string(),
        });
    }
    if agent_count > MAX_MEETING_AGENTS {
        return Err(DbError::InvalidData(format!(
            "meeting supports at most {MAX_MEETING_AGENTS} agents"
        )));
    }
    Ok(participants)
}

async fn validate_source_access_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    source_channel_id: Option<Uuid>,
    participant_pubkeys: &[Vec<u8>],
) -> Result<()> {
    let Some(source_id) = source_channel_id else {
        return Ok(());
    };
    let visibility: Option<String> = sqlx::query_scalar(
        "SELECT visibility::text FROM channels \
         WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(source_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let visibility = visibility
        .ok_or_else(|| DbError::InvalidData(format!("source channel not found: {source_id}")))?;
    if visibility != "private" {
        return Ok(());
    }
    for pubkey in participant_pubkeys {
        let membership: Option<i32> = sqlx::query_scalar(
            "SELECT 1 FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 \
               AND pubkey = $3 AND removed_at IS NULL \
             FOR SHARE",
        )
        .bind(community_id.as_uuid())
        .bind(source_id)
        .bind(pubkey)
        .fetch_optional(tx.as_mut())
        .await?;
        if membership.is_none() {
            return Err(DbError::AccessDenied(format!(
                "participant {} cannot read source channel {source_id}",
                hex::encode(pubkey)
            )));
        }
    }
    Ok(())
}

async fn authorize_end_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    actor_pubkey: &[u8],
    host_pubkey: &[u8],
) -> Result<()> {
    if actor_pubkey == host_pubkey {
        let active_host: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 \
                 FROM meeting_participants participants \
                 JOIN channel_members members \
                   ON members.community_id = participants.community_id \
                  AND members.channel_id = participants.session_id \
                  AND members.pubkey = participants.pubkey \
                 JOIN relay_members relay \
                   ON relay.community_id = participants.community_id \
                  AND relay.pubkey = $3 \
                 WHERE participants.community_id = $1 \
                   AND participants.session_id = $2 \
                   AND participants.pubkey = $4 \
                   AND members.removed_at IS NULL \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(hex::encode(actor_pubkey))
        .bind(actor_pubkey)
        .fetch_one(tx.as_mut())
        .await?;
        if active_host {
            return Ok(());
        }
    }
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2 \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(hex::encode(actor_pubkey))
    .fetch_optional(tx.as_mut())
    .await?;
    if matches!(role.as_deref(), Some("owner" | "admin")) {
        return Ok(());
    }
    Err(DbError::AccessDenied(
        "only the meeting owner or a community owner/admin can end this meeting".to_string(),
    ))
}

async fn ensure_existing_command_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event_id: &[u8],
    expected_kind: i32,
    expected_author: &[u8],
) -> Result<()> {
    validate_32_bytes(event_id, "meeting command event id")?;
    validate_32_bytes(expected_author, "meeting command author")?;
    let event = sqlx::query(
        "SELECT channel_id, kind, pubkey \
         FROM events \
         WHERE community_id = $1 AND id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .fetch_optional(tx.as_mut())
    .await?;
    let Some(event) = event else {
        return Err(DbError::InvalidData(
            "meeting command event must be persisted in the same transaction".to_string(),
        ));
    };
    let channel_id: Option<Uuid> = event.try_get("channel_id")?;
    let kind: i32 = event.try_get("kind")?;
    let author: Vec<u8> = event.try_get("pubkey")?;
    if channel_id != Some(session_id) {
        return Err(DbError::InvalidData(
            "meeting command event is scoped to the wrong session".to_string(),
        ));
    }
    if kind != expected_kind {
        return Err(DbError::InvalidData(format!(
            "meeting command event has kind {kind}, expected {expected_kind}"
        )));
    }
    if author != expected_author {
        return Err(DbError::InvalidData(
            "meeting command event author does not match the command actor".to_string(),
        ));
    }
    Ok(())
}

async fn persist_state_event_tx(
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
            "meeting State event {} already exists without its projection",
            event.id
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_history_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    state_revision: i64,
    floor_revision: i64,
    intent_revision: i64,
    speech_revision: i64,
    control_epoch: i64,
    decision_epoch: i64,
    transition_primary_type: &str,
    transition_effects_json: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_baton_state_history \
             (community_id, session_id, state_revision, state_event_id, \
              floor_revision, intent_revision, speech_revision, control_epoch, \
              decision_epoch, transition_primary_type, transition_effects_json, \
              created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(state_revision)
    .bind(event.id.as_bytes().as_slice())
    .bind(floor_revision)
    .bind(intent_revision)
    .bind(speech_revision)
    .bind(control_epoch)
    .bind(decision_epoch)
    .bind(transition_primary_type)
    .bind(transition_effects_json)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_state_event(
    relay_keys: &Keys,
    session_id: Uuid,
    moderator_pubkey: &[u8],
    phase: BatonPhase,
    floor_revision: i64,
    intent_revision: i64,
    speech_revision: i64,
    state_revision: i64,
    control_epoch: i64,
    decision_epoch: i64,
    config: &BatonConfig,
    participants: &[BatonParticipant],
    transition: &serde_json::Value,
    now: DateTime<Utc>,
) -> Result<Event> {
    let session = session_id.to_string();
    let moderator = hex::encode(moderator_pubkey);
    let floor_revision_string = floor_revision.to_string();
    let intent_revision_string = intent_revision.to_string();
    let speech_revision_string = speech_revision.to_string();
    let state_revision_string = state_revision.to_string();
    let tags = vec![
        parse_tag(["h", session.as_str()])?,
        parse_tag(["v", "2"])?,
        parse_tag(["policy", BATON_POLICY_VERSION])?,
        parse_tag(["phase", phase.as_str()])?,
        parse_tag(["floor-revision", floor_revision_string.as_str()])?,
        parse_tag(["intent-revision", intent_revision_string.as_str()])?,
        parse_tag(["speech-revision", speech_revision_string.as_str()])?,
        parse_tag(["state-revision", state_revision_string.as_str()])?,
        parse_tag(["moderator", moderator.as_str()])?,
    ];
    let content = serde_json::json!({
        "phase": phase,
        "state_revision": state_revision,
        "floor_revision": floor_revision,
        "intent_revision": intent_revision,
        "speech_revision": speech_revision,
        "control_epoch": control_epoch,
        "decision_epoch": decision_epoch,
        "decision_attempt": 0,
        "active_decision_attempt": null,
        "baton_config": config,
        "moderator_pubkey": moderator,
        "participants": participants,
        "pending_intents": [],
        "human_queue": [],
        "unresolved_handoffs": [],
        "handoff_depth": 0,
        "consecutive_moderator_speeches": 0,
        "forced_return_to_moderator": false,
        "moderator_decision_deadline_ms": null,
        "next_action_at_ms": null,
        "offer": null,
        "grant": null,
        "transition": transition,
    });
    let timestamp =
        u64::try_from(now.timestamp()).map_err(|_| DbError::InvalidTimestamp(now.timestamp()))?;
    EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_MEETING_STATE as u16),
        serde_json::to_string(&content)?,
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(timestamp))
    .sign_with_keys(relay_keys)
    .map_err(|error| DbError::InvalidData(format!("sign meeting V1 State: {error}")))
}

#[allow(clippy::too_many_arguments)]
fn meeting_transition(
    primary_type: &str,
    outcome: &str,
    session_id: Uuid,
    caused_by_event_id: Option<&[u8]>,
    now: DateTime<Utc>,
    effect_type: &str,
    from: Option<&str>,
    to: Option<&str>,
    phase_from: Option<&str>,
    phase_to: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "primary_type": primary_type,
        "outcome": outcome,
        "primary_object_id": session_id.to_string(),
        "caused_by_event_id": caused_by_event_id.map(hex::encode),
        "deadline_type": null,
        "blocked_by": null,
        "at_ms": now.timestamp_millis(),
        "effects": [
            {
                "type": effect_type,
                "object_type": "meeting",
                "object_id": session_id.to_string(),
                "from": from,
                "to": to,
            },
            {
                "type": "phase_changed",
                "object_type": "phase",
                "object_id": session_id.to_string(),
                "from": phase_from,
                "to": phase_to,
            }
        ],
    })
}

fn parse_tag<const N: usize>(parts: [&str; N]) -> Result<Tag> {
    Tag::parse(parts).map_err(|error| DbError::InvalidData(format!("build meeting tag: {error}")))
}

fn validate_create_shape(params: &CreateMeetingV1Params<'_>) -> Result<()> {
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
    validate_32_bytes(params.moderator_pubkey, "moderator pubkey")?;
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
    let mut moderator_count = 0usize;
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
        if pubkey.as_slice() == params.moderator_pubkey {
            moderator_count += 1;
        }
    }
    if host_count != 1 {
        return Err(DbError::InvalidData(
            "meeting host must appear exactly once in the complete roster".to_string(),
        ));
    }
    if moderator_count != 1 {
        return Err(DbError::InvalidData(
            "meeting moderator must appear exactly once in the complete roster".to_string(),
        ));
    }
    Ok(())
}

fn validate_config(config: &BatonConfig) -> Result<()> {
    if config.timing_profile_version.is_empty()
        || config.timing_profile_version.len() > 128
        || config.fallback_policy_version.is_empty()
        || config.fallback_policy_version.len() > 128
    {
        return Err(DbError::InvalidData(
            "meeting V1 policy versions must contain 1-128 bytes".to_string(),
        ));
    }
    let durations = [
        config.agent_offer_ack_ms,
        config.human_offer_ack_ms,
        config.moderator_decision_ms,
        config.grant_soft_lease_ms,
        config.progress_interval_ms,
        config.grant_hard_deadline_ms,
        config.agent_safety_margin_ms,
    ];
    if durations
        .iter()
        .any(|duration| !(1..=MAX_BATON_DURATION_MS).contains(duration))
        || config.progress_interval_ms > config.grant_soft_lease_ms
        || config.grant_soft_lease_ms > config.grant_hard_deadline_ms
        || config.agent_safety_margin_ms >= config.grant_hard_deadline_ms
    {
        return Err(DbError::InvalidData(
            "invalid Meeting V1 timing profile".to_string(),
        ));
    }
    if !(0..=255).contains(&config.max_handoff_depth)
        || !(1..=32).contains(&config.max_open_handoffs)
        || !(0..=8).contains(&config.moderator_max_rejudgments)
        || !(1..=64).contains(&config.moderator_max_cas_rebases_per_attempt)
    {
        return Err(DbError::InvalidData(
            "invalid Meeting V1 handoff or moderator attempt limits".to_string(),
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

mod hex_bytes {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        hex::decode(&value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to Meeting V1 test database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V1 migrations");
        pool
    }

    async fn make_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(id)
            .bind(format!("meeting-v1-test-{}.example", id.simple()))
            .execute(pool)
            .await
            .expect("insert Meeting V1 test community");
        CommunityId::from_uuid(id)
    }

    async fn seed_relay_member(
        pool: &PgPool,
        community_id: CommunityId,
        pubkey: &[u8],
        relay_role: &str,
    ) {
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(pubkey))
        .bind(relay_role)
        .execute(pool)
        .await
        .expect("insert Meeting V1 relay membership");
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
        .expect("insert Meeting V1 identity");
        seed_relay_member(pool, community_id, pubkey, relay_role).await;
    }

    async fn insert_command_event_tx(
        tx: &mut Transaction<'_, Postgres>,
        community_id: CommunityId,
        event_id: &[u8],
        pubkey: &[u8],
        kind: i32,
        session_id: Uuid,
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
        .bind(json!([["h", session_id.to_string()]]))
        .bind(vec![0_u8; 64])
        .bind(session_id)
        .execute(tx.as_mut())
        .await
        .expect("insert Meeting V1 command event");
    }

    #[test]
    fn default_config_matches_protocol_defaults() {
        let config = BatonConfig::default();
        assert!(validate_config(&config).is_ok());
        assert_eq!(config.agent_offer_ack_ms, 5_000);
        assert_eq!(config.human_offer_ack_ms, 15_000);
        assert_eq!(config.moderator_decision_ms, 180_000);
        assert_eq!(config.grant_hard_deadline_ms, 300_000);
        assert_eq!(config.max_handoff_depth, 5);
    }

    #[test]
    fn config_rejects_durations_beyond_safe_datetime_arithmetic() {
        for mutate in [
            |config: &mut BatonConfig| config.agent_offer_ack_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.human_offer_ack_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.moderator_decision_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.grant_soft_lease_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.progress_interval_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.grant_hard_deadline_ms = MAX_BATON_DURATION_MS + 1,
            |config: &mut BatonConfig| config.agent_safety_margin_ms = MAX_BATON_DURATION_MS + 1,
        ] {
            let mut config = BatonConfig::default();
            mutate(&mut config);
            assert!(validate_config(&config).is_err());
        }
    }

    #[test]
    fn v1_create_requires_host_and_moderator_in_roster() {
        let host = vec![1; 32];
        let moderator = vec![2; 32];
        let other = vec![3; 32];
        let create_event_id = vec![4; 32];
        let relay_keys = Keys::generate();

        let missing_moderator = vec![host.clone(), other.clone()];
        let params = CreateMeetingV1Params {
            community_id: CommunityId::from_uuid(Uuid::new_v4()),
            session_id: Uuid::new_v4(),
            title: "V1",
            description: None,
            source_channel_id: None,
            host_pubkey: &host,
            moderator_pubkey: &moderator,
            create_event_id: &create_event_id,
            participant_pubkeys: &missing_moderator,
            relay_keys: &relay_keys,
            config: BatonConfig::default(),
        };
        assert!(validate_create_shape(&params).is_err());

        let complete = vec![host.clone(), moderator.clone(), other];
        let params = CreateMeetingV1Params {
            participant_pubkeys: &complete,
            ..params
        };
        assert!(validate_create_shape(&params).is_ok());
    }

    #[test]
    fn initial_state_is_complete_and_relay_signed() {
        let relay_keys = Keys::generate();
        let moderator = vec![7; 32];
        let participant = BatonParticipant {
            pubkey: moderator.clone(),
            participant_type: ParticipantType::Human,
            channel_role: "owner".to_string(),
        };
        let now = Utc::now();
        let session_id = Uuid::new_v4();
        let transition = meeting_transition(
            "meeting_created",
            "accepted",
            session_id,
            Some(&[8; 32]),
            now,
            "meeting_created",
            None,
            Some("active"),
            None,
            Some(BatonPhase::ModeratorIdle.as_str()),
        );
        let event = build_state_event(
            &relay_keys,
            session_id,
            &moderator,
            BatonPhase::ModeratorIdle,
            1,
            0,
            0,
            1,
            1,
            0,
            &BatonConfig::default(),
            &[participant],
            &transition,
            now,
        )
        .expect("build initial V1 state");
        assert!(event.verify().is_ok());
        let content: serde_json::Value =
            serde_json::from_str(&event.content).expect("parse state content");
        assert_eq!(content["phase"], "moderator_idle");
        assert_eq!(content["state_revision"], 1);
        assert_eq!(content["floor_revision"], 1);
        assert_eq!(content["participants"].as_array().map(Vec::len), Some(1));
        assert_eq!(content["transition"]["primary_type"], "meeting_created");
        assert_eq!(
            content["transition"]["effects"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(content["transition"]["effects"][1]["type"], "phase_changed");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn create_and_end_commit_complete_v1_projection_and_outbox() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let host = vec![21_u8; 32];
        let moderator = vec![22_u8; 32];
        let agent = vec![23_u8; 32];
        let create_event_id = vec![24_u8; 32];
        let wrong_end_event_id = vec![25_u8; 32];
        let end_event_id = vec![26_u8; 32];
        let wrong_create_kind_event_id = vec![27_u8; 32];
        let wrong_end_author_event_id = vec![28_u8; 32];
        let session_id = Uuid::new_v4();
        let relay_keys = Keys::generate();

        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_identity(&pool, community_id, &moderator, "member", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &agent,
            "member",
            Some(&host),
            "owner_only",
        )
        .await;
        // NIP-IA archive affects presentation only; it is not a security
        // revocation and must not block an otherwise-authorized V1 roster.
        sqlx::query(
            "INSERT INTO archived_identities \
                 (community_id, pubkey, consent_path, actor, request_event_id) \
             VALUES ($1, $2, 'self', $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(hex::encode(&moderator))
        .bind(Uuid::new_v4().to_string())
        .execute(&pool)
        .await
        .expect("archive moderator identity for presentation-only coverage");

        let roster = vec![host.clone(), moderator.clone(), agent.clone()];
        let mut wrong_create_tx = pool.begin().await.expect("begin wrong-kind V1 create");
        insert_command_event_tx(
            &mut wrong_create_tx,
            community_id,
            &wrong_create_kind_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let wrong_create = create_meeting_v1_tx(
            &mut wrong_create_tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Moderated Baton",
                description: Some("stage one"),
                source_channel_id: None,
                host_pubkey: &host,
                moderator_pubkey: &moderator,
                create_event_id: &wrong_create_kind_event_id,
                participant_pubkeys: &roster,
                relay_keys: &relay_keys,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect_err("Create must reject a persisted command with the wrong kind");
        assert!(matches!(&wrong_create, DbError::InvalidData(message) if message.contains("kind")));
        wrong_create_tx
            .rollback()
            .await
            .expect("rollback wrong-kind V1 create");

        let mut create_tx = pool.begin().await.expect("begin V1 create");
        insert_command_event_tx(
            &mut create_tx,
            community_id,
            &create_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        let created = create_meeting_v1_tx(
            &mut create_tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Moderated Baton",
                description: Some("stage one"),
                source_channel_id: None,
                host_pubkey: &host,
                moderator_pubkey: &moderator,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
                relay_keys: &relay_keys,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect("create Meeting V1 atomically");
        create_tx.commit().await.expect("commit Meeting V1 create");

        assert_eq!(created.phase, BatonPhase::ModeratorIdle);
        assert_eq!(
            (
                created.floor_revision,
                created.intent_revision,
                created.speech_revision,
                created.state_revision,
                created.control_epoch,
                created.decision_epoch,
            ),
            (1, 0, 0, 1, 1, 0)
        );
        assert_eq!(created.moderator_pubkey, moderator);
        assert_eq!(created.participants.len(), 3);
        assert_eq!(
            created
                .participants
                .iter()
                .find(|participant| participant.pubkey == agent)
                .map(|participant| participant.participant_type),
            Some(ParticipantType::Agent)
        );

        let policy = crate::meeting::get_meeting_policy(&db, community_id, session_id)
            .await
            .expect("read V1 policy");
        assert_eq!(policy.schema_version, SCHEMA_VERSION);
        assert_eq!(policy.floor_policy_version, BATON_POLICY_VERSION);
        assert_eq!(
            policy.moderator_pubkey.as_deref(),
            Some(moderator.as_slice())
        );
        sqlx::query(
            "UPDATE meeting_sessions SET moderator_pubkey = $3 \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(&host)
        .execute(&pool)
        .await
        .expect_err("meeting protocol and moderator must be immutable");
        let snapshot = get_baton_snapshot(&db, community_id, session_id)
            .await
            .expect("read initial Baton State");
        assert_eq!(snapshot.state_event_id, created.state_event_id);

        let state_event_content: String = sqlx::query_scalar(
            "SELECT content FROM events \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&created.state_event_id)
        .fetch_one(&pool)
        .await
        .expect("read initial State event");
        let state_json: serde_json::Value =
            serde_json::from_str(&state_event_content).expect("decode initial State");
        assert_eq!(state_json["baton_config"]["max_handoff_depth"], 5);
        assert_eq!(state_json["participants"].as_array().map(Vec::len), Some(3));
        assert_eq!(
            state_json["transition"]["effects"][1]["to"],
            "moderator_idle"
        );
        let history_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_state_history \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count initial State history");
        let round_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_rounds \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count V0 rounds for V1 meeting");
        let outbox_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count initial outbox");
        assert_eq!((history_count, round_count, outbox_count), (1, 0, 2));

        let floor_error = crate::meeting_floor::get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect_err("V0 floor query must reject a V1 session");
        assert!(matches!(floor_error, DbError::InvalidData(_)));

        let mut wrong_end_tx = pool.begin().await.expect("begin V0-shaped V1 end");
        insert_command_event_tx(
            &mut wrong_end_tx,
            community_id,
            &wrong_end_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let wrong_end = crate::meeting::end_meeting_tx(
            &mut wrong_end_tx,
            crate::meeting::EndMeetingParams {
                community_id,
                session_id,
                actor_pubkey: &host,
                create_event_id: &create_event_id,
                end_event_id: &wrong_end_event_id,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect_err("V0 lifecycle End must reject a V1 session");
        assert!(matches!(wrong_end, DbError::InvalidData(_)));
        wrong_end_tx
            .rollback()
            .await
            .expect("rollback V0-shaped V1 End");

        let mut wrong_author_tx = pool.begin().await.expect("begin wrong-author V1 End");
        insert_command_event_tx(
            &mut wrong_author_tx,
            community_id,
            &wrong_end_author_event_id,
            &moderator,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let wrong_author = end_meeting_v1_tx(
            &mut wrong_author_tx,
            EndMeetingV1Params {
                community_id,
                session_id,
                actor_pubkey: &host,
                create_event_id: &create_event_id,
                end_event_id: &wrong_end_author_event_id,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect_err("End must reject a persisted command authored by another participant");
        assert!(
            matches!(&wrong_author, DbError::InvalidData(message) if message.contains("author"))
        );
        wrong_author_tx
            .rollback()
            .await
            .expect("rollback wrong-author V1 End");

        let mut end_tx = pool.begin().await.expect("begin V1 End");
        insert_command_event_tx(
            &mut end_tx,
            community_id,
            &end_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let ended = end_meeting_v1_tx(
            &mut end_tx,
            EndMeetingV1Params {
                community_id,
                session_id,
                actor_pubkey: &host,
                create_event_id: &create_event_id,
                end_event_id: &end_event_id,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("end Meeting V1");
        let EndMeetingV1Outcome::Ended(ended) = ended else {
            panic!("active Meeting V1 must transition to ended");
        };
        end_tx.commit().await.expect("commit Meeting V1 End");
        assert_eq!(ended.phase, BatonPhase::Ended);
        assert_eq!(
            (
                ended.floor_revision,
                ended.intent_revision,
                ended.speech_revision,
                ended.state_revision,
            ),
            (2, 0, 0, 2)
        );
        let final_snapshot = get_baton_snapshot(&db, community_id, session_id)
            .await
            .expect("read terminal Baton State");
        assert_eq!(final_snapshot.state_event_id, ended.state_event_id);
        let history_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_state_history \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count terminal State history");
        let outbox_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count terminal outbox");
        assert_eq!((history_count, outbox_count), (2, 4));
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn manual_end_prioritizes_v1_roster_revocation_and_discards_the_command() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let host = vec![0x91_u8; 32];
        let participant = vec![0x92_u8; 32];
        let session_id = Uuid::new_v4();
        let create_event_id = vec![0x93_u8; 32];
        let manual_end_event_id = vec![0x94_u8; 32];
        let relay_keys = Keys::generate();
        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_identity(&pool, community_id, &participant, "member", None, "anyone").await;
        let roster = vec![host.clone(), participant.clone()];
        let mut create_tx = pool.begin().await.expect("begin V1 security create");
        insert_command_event_tx(
            &mut create_tx,
            community_id,
            &create_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        create_meeting_v1_tx(
            &mut create_tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "V1 manual End security",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                moderator_pubkey: &host,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
                relay_keys: &relay_keys,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect("create V1 security Meeting");
        create_tx.commit().await.expect("commit V1 security create");

        crate::moderation::ban_member_with_revocation(
            &pool,
            community_id,
            &host,
            &participant,
            Some("manual End must lose to revocation"),
            None,
            &[0x95; 32],
        )
        .await
        .expect("ban V1 host");
        let mut end_tx = pool.begin().await.expect("begin V1 security End");
        insert_command_event_tx(
            &mut end_tx,
            community_id,
            &manual_end_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_END as i32,
            session_id,
        )
        .await;
        let outcome = end_meeting_v1_tx(
            &mut end_tx,
            EndMeetingV1Params {
                community_id,
                session_id,
                actor_pubkey: &host,
                create_event_id: &create_event_id,
                end_event_id: &manual_end_event_id,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("recover revoked V1 roster before manual End");
        let EndMeetingV1Outcome::ParticipantRevoked(snapshot) = outcome else {
            panic!("V1 roster revocation must win over manual End");
        };
        end_tx.commit().await.expect("commit V1 roster recovery");
        assert_eq!(snapshot.phase, BatonPhase::Ended);
        let state_content: String = sqlx::query_scalar(
            "SELECT content FROM events \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&snapshot.state_event_id)
        .fetch_one(&pool)
        .await
        .expect("load V1 revocation State");
        let state_json: serde_json::Value =
            serde_json::from_str(&state_content).expect("parse V1 revocation State");
        assert_eq!(
            state_json["transition"]["primary_type"],
            "participant_revoked"
        );
        let manual_persistence: (bool, bool) = sqlx::query_as(
            "SELECT \
                 EXISTS(SELECT 1 FROM events \
                        WHERE community_id = $1 AND id = $2), \
                 EXISTS(SELECT 1 FROM meeting_event_outbox \
                        WHERE community_id = $1 AND event_id = $2)",
        )
        .bind(community_id.as_uuid())
        .bind(&manual_end_event_id)
        .fetch_one(&pool)
        .await
        .expect("check discarded V1 manual End");
        assert_eq!(manual_persistence, (false, false));

        let active_host = vec![0x96_u8; 32];
        let active_participant = vec![0x97_u8; 32];
        let timed_out_admin = vec![0x98_u8; 32];
        let active_session_id = Uuid::new_v4();
        let active_create_event_id = vec![0x99_u8; 32];
        let rejected_end_event_id = vec![0x9a_u8; 32];
        seed_identity(&pool, community_id, &active_host, "owner", None, "anyone").await;
        seed_identity(
            &pool,
            community_id,
            &active_participant,
            "member",
            None,
            "anyone",
        )
        .await;
        seed_identity(
            &pool,
            community_id,
            &timed_out_admin,
            "admin",
            None,
            "anyone",
        )
        .await;
        let roster = vec![active_host.clone(), active_participant];
        let mut create_tx = pool.begin().await.expect("begin timeout-only V1 create");
        insert_command_event_tx(
            &mut create_tx,
            community_id,
            &active_create_event_id,
            &active_host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            active_session_id,
        )
        .await;
        create_meeting_v1_tx(
            &mut create_tx,
            CreateMeetingV1Params {
                community_id,
                session_id: active_session_id,
                title: "V1 timeout-only End rejection",
                description: None,
                source_channel_id: None,
                host_pubkey: &active_host,
                moderator_pubkey: &active_host,
                create_event_id: &active_create_event_id,
                participant_pubkeys: &roster,
                relay_keys: &relay_keys,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect("create timeout-only V1 Meeting");
        create_tx
            .commit()
            .await
            .expect("commit timeout-only V1 Meeting");
        crate::moderation::timeout_member(
            &pool,
            community_id,
            &timed_out_admin,
            &active_host,
            Utc::now() + chrono::Duration::hours(1),
            Some("cannot issue recovery End"),
        )
        .await
        .expect("timeout non-roster admin");
        let mut end_tx = pool.begin().await.expect("begin timed-out admin End");
        insert_command_event_tx(
            &mut end_tx,
            community_id,
            &rejected_end_event_id,
            &timed_out_admin,
            buzz_core::kind::KIND_MEETING_END as i32,
            active_session_id,
        )
        .await;
        let error = end_meeting_v1_tx(
            &mut end_tx,
            EndMeetingV1Params {
                community_id,
                session_id: active_session_id,
                actor_pubkey: &timed_out_admin,
                create_event_id: &active_create_event_id,
                end_event_id: &rejected_end_event_id,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect_err("timeout-only admin cannot end a Meeting");
        assert!(matches!(error, DbError::AccessDenied(_)));
        end_tx
            .rollback()
            .await
            .expect("discard timed-out admin End event");
        let status: String = sqlx::query_scalar(
            "SELECT status FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(active_session_id)
        .fetch_one(&pool)
        .await
        .expect("load Meeting after timeout-only rejection");
        assert_eq!(status, "active");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn missing_authoritative_identity_rolls_back_v1_create() {
        let pool = setup_pool().await;
        let community_id = make_community(&pool).await;
        let host = vec![41_u8; 32];
        let missing_identity = vec![42_u8; 32];
        let create_event_id = vec![43_u8; 32];
        let session_id = Uuid::new_v4();
        let relay_keys = Keys::generate();
        seed_identity(&pool, community_id, &host, "owner", None, "anyone").await;
        seed_relay_member(&pool, community_id, &missing_identity, "member").await;

        let roster = vec![host.clone(), missing_identity];
        let mut tx = pool.begin().await.expect("begin invalid V1 create");
        insert_command_event_tx(
            &mut tx,
            community_id,
            &create_event_id,
            &host,
            buzz_core::kind::KIND_MEETING_CREATE as i32,
            session_id,
        )
        .await;
        let error = create_meeting_v1_tx(
            &mut tx,
            CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Missing identity",
                description: None,
                source_channel_id: None,
                host_pubkey: &host,
                moderator_pubkey: &host,
                create_event_id: &create_event_id,
                participant_pubkeys: &roster,
                relay_keys: &relay_keys,
                config: BatonConfig::default(),
            },
        )
        .await
        .expect_err("missing users row must fail closed");
        assert!(matches!(error, DbError::InvalidData(_)));
        tx.rollback().await.expect("rollback invalid V1 create");

        let event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(&create_event_id)
                .fetch_one(&pool)
                .await
                .expect("count rolled-back command");
        let channel_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM channels WHERE community_id = $1 AND id = $2")
                .bind(community_id.as_uuid())
                .bind(session_id)
                .fetch_one(&pool)
                .await
                .expect("count rolled-back channel");
        let session_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count rolled-back session");
        let state_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM meeting_baton_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("count rolled-back State");
        assert_eq!(
            (event_count, channel_count, session_count, state_count),
            (0, 0, 0, 0)
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn revocation_claim_token_fences_every_stale_worker_mutation() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = make_community(&pool).await;
        let advance_job_id = Uuid::new_v4();
        let complete_job_id = Uuid::new_v4();
        let release_job_id = Uuid::new_v4();
        let job_ids = [advance_job_id, complete_job_id, release_job_id];

        let mut tx = pool.begin().await.expect("begin revocation job seeds");
        for (index, job_id) in job_ids.iter().copied().enumerate() {
            let revoked_pubkey = vec![0x70 + index as u8; 32];
            let revocation_event_id = vec![0x80 + index as u8; 32];
            assert!(enqueue_revocation_job_tx(
                &mut tx,
                community_id,
                job_id,
                &revoked_pubkey,
                &revocation_event_id,
            )
            .await
            .expect("enqueue revocation job"));
        }
        tx.commit().await.expect("commit revocation job seeds");

        // Give these uniquely-scoped jobs deterministic priority over unrelated
        // rows that may exist in a shared integration-test database.
        sqlx::query(
            "UPDATE meeting_revocation_jobs \
             SET next_attempt_at = '-infinity' \
             WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .execute(&pool)
        .await
        .expect("make initial claims due");

        let first_claims = claim_revocation_jobs(&db, 3, 60_000)
            .await
            .expect("claim first leases");
        assert_eq!(first_claims.len(), 3);
        assert!(first_claims
            .iter()
            .all(|job| job.claim_token.attempt() == 1));

        // Simulate all three workers exceeding their lease, then let another
        // worker reclaim each row. Every returned token must now fence attempt 1.
        sqlx::query(
            "UPDATE meeting_revocation_jobs \
             SET next_attempt_at = '-infinity' \
             WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .execute(&pool)
        .await
        .expect("expire first claims");
        let second_claims = claim_revocation_jobs(&db, 3, 60_000)
            .await
            .expect("claim replacement leases");
        assert_eq!(second_claims.len(), 3);
        assert!(second_claims
            .iter()
            .all(|job| job.claim_token.attempt() == 2));

        let claimed = |claims: &[MeetingRevocationJob], job_id: Uuid| {
            claims
                .iter()
                .find(|job| job.job_id == job_id)
                .cloned()
                .expect("find claimed revocation job")
        };
        let stale_advance = claimed(&first_claims, advance_job_id);
        let current_advance = claimed(&second_claims, advance_job_id);
        let stale_complete = claimed(&first_claims, complete_job_id);
        let current_complete = claimed(&second_claims, complete_job_id);
        let stale_release = claimed(&first_claims, release_job_id);
        let current_release = claimed(&second_claims, release_job_id);
        let cursor = Uuid::new_v4();

        assert!(!advance_revocation_job(
            &db,
            community_id,
            advance_job_id,
            stale_advance.claim_token,
            Uuid::new_v4(),
            Utc::now(),
        )
        .await
        .expect("reject stale cursor advancement"));
        assert!(!complete_revocation_job(
            &db,
            community_id,
            complete_job_id,
            stale_complete.claim_token,
        )
        .await
        .expect("reject stale completion"));
        assert!(!release_revocation_job(
            &db,
            community_id,
            release_job_id,
            stale_release.claim_token,
            "stale release",
        )
        .await
        .expect("reject stale release"));

        // The current claim remains authoritative after all three stale calls.
        assert!(advance_revocation_job(
            &db,
            community_id,
            advance_job_id,
            current_advance.claim_token,
            cursor,
            Utc::now(),
        )
        .await
        .expect("advance with current claim"));
        assert!(complete_revocation_job(
            &db,
            community_id,
            complete_job_id,
            current_complete.claim_token,
        )
        .await
        .expect("complete with current claim"));
        assert!(release_revocation_job(
            &db,
            community_id,
            release_job_id,
            current_release.claim_token,
            "current release",
        )
        .await
        .expect("release with current claim"));

        let advance_shape: (String, Option<Uuid>, i32, Option<String>) = sqlx::query_as(
            "SELECT state, cursor_session_id, attempts, last_error \
             FROM meeting_revocation_jobs \
             WHERE community_id = $1 AND job_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(advance_job_id)
        .fetch_one(&pool)
        .await
        .expect("load advanced job");
        assert_eq!(
            advance_shape,
            ("pending".to_string(), Some(cursor), 2, None)
        );

        let complete_shape: (String, i32, bool) = sqlx::query_as(
            "SELECT state, attempts, completed_at IS NOT NULL \
             FROM meeting_revocation_jobs \
             WHERE community_id = $1 AND job_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(complete_job_id)
        .fetch_one(&pool)
        .await
        .expect("load completed job");
        assert_eq!(complete_shape, ("completed".to_string(), 2, true));

        let release_shape: (String, Option<Uuid>, i32, Option<String>) = sqlx::query_as(
            "SELECT state, cursor_session_id, attempts, last_error \
             FROM meeting_revocation_jobs \
             WHERE community_id = $1 AND job_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(release_job_id)
        .fetch_one(&pool)
        .await
        .expect("load released job");
        assert_eq!(
            release_shape,
            (
                "pending".to_string(),
                None,
                2,
                Some("current release".to_string()),
            )
        );
    }
}
