//! Durable Meeting V0 speech-floor state machine.
//!
//! Every mutation locks the owning `meeting_sessions` row. This gives Claim,
//! Grant, Say, expiry, and End one serialization point per meeting while still
//! allowing unrelated meetings to advance concurrently.

use std::time::Duration as StdDuration;

use buzz_core::{CommunityId, StoredEvent};
use chrono::{DateTime, Duration, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// The arbitration policy persisted on every Meeting V0 round.
pub const FLOOR_POLICY_VERSION: &str = "uniform-v0";

/// Default duration of the Claim competition window.
pub const DEFAULT_CLAIM_WINDOW: StdDuration = StdDuration::from_secs(3);
/// Default duration of a granted speech lease.
pub const DEFAULT_GRANT_LEASE: StdDuration = StdDuration::from_secs(10);

/// Runtime timing configuration for the Meeting V0 floor.
#[derive(Debug, Clone, Copy)]
pub struct FloorConfig {
    /// Time from the first Claim until winner selection.
    pub claim_window: StdDuration,
    /// Time a winner has to publish one valid speech.
    pub grant_lease: StdDuration,
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            claim_window: DEFAULT_CLAIM_WINDOW,
            grant_lease: DEFAULT_GRANT_LEASE,
        }
    }
}

/// Winner selection mode.
///
/// Production uses [`Self::UniformRandom`]. Tests can inject a stable claim
/// index without changing the persisted protocol or event shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinnerSelector {
    /// Choose each canonical Claim with equal probability.
    UniformRandom,
    /// Choose the zero-based Claim index after sorting by event ID.
    FixedIndex(usize),
}

/// Durable phase of one speech round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorPhase {
    /// No Claim has arrived; the round can remain quiet indefinitely.
    Open,
    /// At least one Claim exists and the competition deadline is running.
    Claiming,
    /// Exactly one holder owns a single-use, time-limited Grant.
    Granted,
    /// The round is terminal.
    Closed,
}

impl FloorPhase {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "open" => Ok(Self::Open),
            "claiming" => Ok(Self::Claiming),
            "granted" => Ok(Self::Granted),
            "closed" => Ok(Self::Closed),
            other => Err(DbError::InvalidData(format!(
                "unknown meeting floor phase: {other}"
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Claiming => "claiming",
            Self::Granted => "granted",
            Self::Closed => "closed",
        }
    }
}

/// Current durable floor snapshot for a meeting.
#[derive(Debug, Clone)]
pub struct FloorSnapshot {
    /// Meeting/channel UUID.
    pub session_id: Uuid,
    /// Current speech round, starting at 1.
    pub round_number: i64,
    /// Monotonic session-wide floor revision.
    pub floor_revision: i64,
    /// Current round phase.
    pub phase: FloorPhase,
    /// Relay-signed state event representing this snapshot.
    pub state_event_id: Vec<u8>,
    /// Claim deadline, when in `claiming`.
    pub claim_deadline: Option<DateTime<Utc>>,
    /// Current holder, when in `granted`.
    pub holder_pubkey: Option<Vec<u8>>,
    /// Relay-signed Grant event ID, when in `granted`.
    pub grant_event_id: Option<Vec<u8>>,
    /// Grant lease deadline, when in `granted`.
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Terminal outcome for a closed round.
    pub outcome: Option<String>,
    /// Canonical speech event for a `spoken` round.
    pub speech_event_id: Option<Vec<u8>>,
    /// Canonical Claim event IDs, sorted by event ID.
    pub claim_event_ids: Vec<Vec<u8>>,
    /// Claimant pubkeys ordered alongside [`Self::claim_event_ids`].
    pub claimant_pubkeys: Vec<Vec<u8>>,
}

/// Result of submitting a Meeting V0 floor Claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimFloorOutcome {
    /// A new canonical Claim was committed.
    Accepted {
        /// Round claimed.
        round_number: i64,
        /// Revision of the resulting `claiming` state.
        floor_revision: i64,
        /// Canonical Claim event ID.
        claim_event_id: Vec<u8>,
    },
    /// The same signed event was already committed.
    Duplicate {
        /// Round originally claimed.
        round_number: i64,
        /// Revision assigned to the original Claim.
        floor_revision: i64,
        /// Canonical Claim event ID.
        claim_event_id: Vec<u8>,
    },
    /// This identity already used its one Claim slot for the round.
    Conflict {
        /// Existing canonical Claim event ID.
        canonical_claim_event_id: Vec<u8>,
    },
}

/// Result of submitting a Grant-bound Meeting V0 speech.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SayOutcome {
    /// The speech was committed and the next round was opened.
    Accepted {
        /// Consumed round.
        round_number: i64,
        /// Canonical speech event ID.
        speech_event_id: Vec<u8>,
        /// Newly opened round.
        next_round_number: i64,
        /// Revision of the newly opened round.
        floor_revision: i64,
    },
    /// The same signed speech event was already committed.
    Duplicate {
        /// Round originally consumed.
        round_number: i64,
        /// Canonical speech event ID.
        speech_event_id: Vec<u8>,
    },
    /// A different speech already consumed the referenced Grant.
    GrantConsumed {
        /// Canonical accepted speech event ID.
        accepted_speech_event_id: Vec<u8>,
    },
}

/// One outbox event claimed for post-commit dispatch.
#[derive(Debug, Clone)]
pub struct MeetingOutboxEvent {
    /// Server-trusted community owning the event.
    pub community_id: CommunityId,
    /// Normalized host loaded from the owning community row.
    pub host: String,
    /// Meeting/channel UUID.
    pub session_id: Uuid,
    /// Monotonic outbox sequence within the community.
    pub sequence: i64,
    /// Persisted and verified event.
    pub stored_event: StoredEvent,
}

#[derive(Debug)]
struct SessionLock {
    status: String,
    current_round: i64,
    floor_revision: i64,
}

#[derive(Debug, Clone)]
struct RoundRow {
    round_number: i64,
    floor_revision: i64,
    phase: FloorPhase,
    state_event_id: Vec<u8>,
    claim_deadline: Option<DateTime<Utc>>,
    holder_pubkey: Option<Vec<u8>>,
    grant_event_id: Option<Vec<u8>>,
    lease_expires_at: Option<DateTime<Utc>>,
    outcome: Option<String>,
    speech_event_id: Option<Vec<u8>>,
}

#[derive(Debug)]
struct CanonicalClaim {
    event_id: Vec<u8>,
    claimant_pubkey: Vec<u8>,
}

/// Create Round 1 and its relay-signed `open` event inside an existing Meeting
/// Create transaction.
///
/// Calling this for an already initialized meeting is idempotent.
pub async fn initialize_floor_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
) -> Result<FloorSnapshot> {
    let mut session = lock_session(tx, community_id, session_id).await?;
    if let Some(round) = load_round_tx(tx, community_id, session_id, session.current_round).await? {
        return snapshot_from_round(tx, community_id, session_id, round).await;
    }
    if session.status != "active" {
        return Err(DbError::InvalidData(
            "cannot initialize the floor of an ended meeting".to_string(),
        ));
    }
    let now = database_now(tx).await?;
    let round = create_open_round_locked(
        tx,
        community_id,
        session_id,
        &mut session,
        now,
        relay_keys,
        None,
    )
    .await?;
    snapshot_from_round(tx, community_id, session_id, round).await
}

/// Submit one participant-signed Claim.
///
/// The function is idempotent by signed event ID and rejects a second distinct
/// Claim from the same participant in one round.
#[allow(clippy::too_many_arguments)]
pub async fn claim_floor(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    event: &Event,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<ClaimFloorOutcome> {
    validate_duration(config.claim_window, "meeting claim window")?;
    validate_duration(config.grant_lease, "meeting grant lease")?;
    validate_event_identity(event, session_id, round_number)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;

    if let Some(row) = sqlx::query(
        "SELECT round_number, floor_revision, claim_event_id \
         FROM meeting_floor_claims \
         WHERE community_id = $1 AND claim_event_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .fetch_optional(tx.as_mut())
    .await?
    {
        let outcome = ClaimFloorOutcome::Duplicate {
            round_number: row.try_get("round_number")?,
            floor_revision: row.try_get("floor_revision")?,
            claim_event_id: row.try_get("claim_event_id")?,
        };
        tx.rollback().await?;
        return Ok(outcome);
    }

    if session.status != "active" {
        return Err(DbError::InvalidData("meeting has ended".to_string()));
    }

    ensure_active_participant(&mut tx, community_id, session_id, event.pubkey.as_bytes()).await?;

    if let Some(canonical_id) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT claim_event_id FROM meeting_floor_claims \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND claimant_pubkey = $4",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(event.pubkey.as_bytes())
    .fetch_optional(tx.as_mut())
    .await?
    {
        tx.rollback().await?;
        return Ok(ClaimFloorOutcome::Conflict {
            canonical_claim_event_id: canonical_id,
        });
    }

    let now = database_now(&mut tx).await?;
    let mut round = ensure_current_round_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        now,
        relay_keys,
    )
    .await?;
    if advance_due_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        &round,
        now,
        relay_keys,
        config,
        selector,
    )
    .await?
    {
        round = load_current_round_required(&mut tx, community_id, session_id, &session).await?;
    }

    if round_number != session.current_round {
        return Err(DbError::InvalidData(format!(
            "claim targets round {round_number}, current round is {}",
            session.current_round
        )));
    }
    match round.phase {
        FloorPhase::Open | FloorPhase::Claiming => {}
        FloorPhase::Granted => {
            return Err(DbError::InvalidData(
                "claim window is already settled".to_string(),
            ));
        }
        FloorPhase::Closed => {
            return Err(DbError::InvalidData("meeting round is closed".to_string()));
        }
    }
    if round.claim_deadline.is_some_and(|deadline| now >= deadline) {
        return Err(DbError::InvalidData(
            "claim arrived at or after the competition deadline".to_string(),
        ));
    }

    let claim_deadline = match round.phase {
        FloorPhase::Open => Some(now + chrono_duration(config.claim_window)?),
        FloorPhase::Claiming => round.claim_deadline,
        _ => None,
    }
    .ok_or_else(|| DbError::InvalidData("claiming round is missing its deadline".to_string()))?;

    persist_meeting_event_tx(&mut tx, community_id, session_id, event, now).await?;

    let next_revision = session.floor_revision + 1;
    sqlx::query(
        "INSERT INTO meeting_floor_claims \
             (community_id, session_id, round_number, claimant_pubkey, \
              claim_event_id, floor_revision, received_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(event.pubkey.as_bytes())
    .bind(event.id.as_bytes().as_slice())
    .bind(next_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    let claims = load_claims_tx(&mut tx, community_id, session_id, round_number).await?;
    let state_event = build_round_state_event(
        relay_keys,
        session_id,
        round_number,
        next_revision,
        FloorPhase::Claiming,
        None,
        serde_json::json!({
            "claim_deadline_ms": claim_deadline.timestamp_millis(),
            "claim_event_ids": claim_ids_hex(&claims),
            "claimants": claimant_hex(&claims),
        }),
        now,
    )?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, &state_event, now).await?;

    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'claiming', floor_revision = $4, state_event_id = $5, \
             claim_deadline = $6, updated_at = $7 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(next_revision)
    .bind(state_event.id.as_bytes().as_slice())
    .bind(claim_deadline)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    set_session_floor(
        &mut tx,
        community_id,
        session_id,
        round_number,
        next_revision,
    )
    .await?;

    tx.commit().await?;
    Ok(ClaimFloorOutcome::Accepted {
        round_number,
        floor_revision: next_revision,
        claim_event_id: event.id.as_bytes().to_vec(),
    })
}

/// Submit one holder-authored, Grant-bound speech and atomically open the next
/// round.
#[allow(clippy::too_many_arguments)]
pub async fn say(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    grant_event_id: &[u8],
    event: &Event,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<SayOutcome> {
    validate_duration(config.claim_window, "meeting claim window")?;
    validate_duration(config.grant_lease, "meeting grant lease")?;
    validate_32(grant_event_id, "meeting grant event id")?;
    validate_event_identity(event, session_id, round_number)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;

    if let Some(row) = sqlx::query(
        "SELECT round_number, speech_event_id FROM meeting_rounds \
         WHERE community_id = $1 AND session_id = $2 AND speech_event_id = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(event.id.as_bytes().as_slice())
    .fetch_optional(tx.as_mut())
    .await?
    {
        let outcome = SayOutcome::Duplicate {
            round_number: row.try_get("round_number")?,
            speech_event_id: row.try_get("speech_event_id")?,
        };
        tx.rollback().await?;
        return Ok(outcome);
    }

    if let Some(accepted_speech_event_id) = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT speech_event_id FROM meeting_rounds \
         WHERE community_id = $1 AND session_id = $2 AND grant_event_id = $3 \
           AND speech_event_id IS NOT NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(grant_event_id)
    .fetch_optional(tx.as_mut())
    .await?
    {
        tx.rollback().await?;
        return Ok(SayOutcome::GrantConsumed {
            accepted_speech_event_id,
        });
    }

    if session.status != "active" {
        return Err(DbError::InvalidData("meeting has ended".to_string()));
    }
    let now = database_now(&mut tx).await?;
    let mut round = ensure_current_round_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        now,
        relay_keys,
    )
    .await?;
    if advance_due_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        &round,
        now,
        relay_keys,
        config,
        selector,
    )
    .await?
    {
        round = load_current_round_required(&mut tx, community_id, session_id, &session).await?;
    }

    if round_number != session.current_round {
        return Err(DbError::InvalidData(format!(
            "speech targets round {round_number}, current round is {}",
            session.current_round
        )));
    }
    if round.phase != FloorPhase::Granted {
        return Err(DbError::InvalidData(
            "meeting round does not have an active Grant".to_string(),
        ));
    }
    if round.grant_event_id.as_deref() != Some(grant_event_id) {
        return Err(DbError::InvalidData(
            "speech references the wrong meeting Grant".to_string(),
        ));
    }
    if round.holder_pubkey.as_deref() != Some(event.pubkey.as_bytes()) {
        return Err(DbError::AccessDenied(
            "only the current floor holder may consume this Grant".to_string(),
        ));
    }
    if round
        .lease_expires_at
        .is_none_or(|lease_expires_at| now >= lease_expires_at)
    {
        return Err(DbError::InvalidData(
            "meeting Grant lease has expired".to_string(),
        ));
    }

    ensure_active_participant(&mut tx, community_id, session_id, event.pubkey.as_bytes()).await?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, event, now).await?;

    let closed_revision = session.floor_revision + 1;
    let closed_event = build_round_state_event(
        relay_keys,
        session_id,
        round_number,
        closed_revision,
        FloorPhase::Closed,
        None,
        serde_json::json!({
            "outcome": "spoken",
            "speech_event_id": event.id.to_hex(),
        }),
        now,
    )?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, &closed_event, now).await?;
    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'closed', floor_revision = $4, state_event_id = $5, \
             outcome = 'spoken', speech_event_id = $6, updated_at = $7 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(closed_revision)
    .bind(closed_event.id.as_bytes().as_slice())
    .bind(event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    session.floor_revision = closed_revision;

    let next_round_number = round_number + 1;
    session.current_round = next_round_number;
    let opened = create_open_round_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        now,
        relay_keys,
        Some(PreviousRound {
            round_number,
            outcome: "spoken",
            speech_event_id: Some(event.id.as_bytes()),
        }),
    )
    .await?;

    tx.commit().await?;
    Ok(SayOutcome::Accepted {
        round_number,
        speech_event_id: event.id.as_bytes().to_vec(),
        next_round_number,
        floor_revision: opened.floor_revision,
    })
}

/// Close the current floor as `ended` inside the Meeting End transaction.
///
/// No next round is created. If a stage-1 meeting has no round yet, a terminal
/// Round 1 projection is created directly.
pub async fn close_floor_for_end_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
) -> Result<FloorSnapshot> {
    let mut session = lock_session(tx, community_id, session_id).await?;
    if session.status != "ended" {
        return Err(DbError::InvalidData(
            "meeting must be terminal before closing its floor".to_string(),
        ));
    }
    let now = database_now(tx).await?;
    let next_revision = session.floor_revision + 1;
    let closed_event = build_round_state_event(
        relay_keys,
        session_id,
        session.current_round,
        next_revision,
        FloorPhase::Closed,
        None,
        serde_json::json!({ "outcome": "ended" }),
        now,
    )?;
    persist_meeting_event_tx(tx, community_id, session_id, &closed_event, now).await?;

    let existing = load_round_tx(tx, community_id, session_id, session.current_round).await?;
    if existing.is_some_and(|round| {
        round.phase == FloorPhase::Closed && round.outcome.as_deref() == Some("ended")
    }) {
        return Err(DbError::InvalidData(
            "meeting floor was already closed as ended".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO meeting_rounds \
             (community_id, session_id, round_number, floor_revision, phase, \
              state_event_id, outcome, policy_version, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'closed', $5, 'ended', $6, $7, $7) \
         ON CONFLICT (community_id, session_id, round_number) DO UPDATE \
         SET phase = 'closed', floor_revision = EXCLUDED.floor_revision, \
             state_event_id = EXCLUDED.state_event_id, outcome = 'ended', \
             speech_event_id = NULL, updated_at = EXCLUDED.updated_at",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(session.current_round)
    .bind(next_revision)
    .bind(closed_event.id.as_bytes().as_slice())
    .bind(FLOOR_POLICY_VERSION)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    set_session_floor(
        tx,
        community_id,
        session_id,
        session.current_round,
        next_revision,
    )
    .await?;
    session.floor_revision = next_revision;

    let round = load_current_round_required(tx, community_id, session_id, &session).await?;
    snapshot_from_round(tx, community_id, session_id, round).await
}

/// Recover missing initial rounds and advance due Claim/Grant deadlines.
///
/// This is safe to run concurrently on multiple relay processes. Each candidate
/// is rechecked after locking its session row, so only one process can commit a
/// transition.
pub async fn recover_due_floors(
    db: &Db,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
    limit: i64,
) -> Result<usize> {
    validate_duration(config.claim_window, "meeting claim window")?;
    validate_duration(config.grant_lease, "meeting grant lease")?;
    if limit <= 0 {
        return Ok(0);
    }

    let candidates = sqlx::query(
        "SELECT ms.community_id, ms.session_id \
         FROM meeting_sessions ms \
         LEFT JOIN meeting_rounds mr \
           ON mr.community_id = ms.community_id \
          AND mr.session_id = ms.session_id \
          AND mr.round_number = ms.current_round \
         WHERE ms.status = 'active' \
           AND ( \
             mr.session_id IS NULL \
             OR (mr.phase = 'claiming' AND mr.claim_deadline <= clock_timestamp()) \
             OR (mr.phase = 'granted' AND mr.lease_expires_at <= clock_timestamp()) \
           ) \
         ORDER BY COALESCE(mr.claim_deadline, mr.lease_expires_at, ms.created_at), \
                  ms.community_id, ms.session_id \
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(&db.pool)
    .await?;

    let mut advanced = 0usize;
    for candidate in candidates {
        let community_id = CommunityId::from_uuid(candidate.try_get::<Uuid, _>("community_id")?);
        let session_id: Uuid = candidate.try_get("session_id")?;
        let mut tx = db.begin_transaction().await?;
        let mut session = lock_session(&mut tx, community_id, session_id).await?;
        if session.status != "active" {
            tx.rollback().await?;
            continue;
        }
        let now = database_now(&mut tx).await?;
        let round = ensure_current_round_locked(
            &mut tx,
            community_id,
            session_id,
            &mut session,
            now,
            relay_keys,
        )
        .await?;
        let did_advance = advance_due_locked(
            &mut tx,
            community_id,
            session_id,
            &mut session,
            &round,
            now,
            relay_keys,
            config,
            selector,
        )
        .await?;
        tx.commit().await?;
        if did_advance || round.floor_revision == 1 {
            advanced += 1;
        }
    }
    Ok(advanced)
}

/// Read the current durable floor snapshot.
pub async fn get_floor_snapshot(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<FloorSnapshot> {
    let row = sqlx::query(
        "SELECT current_round FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(&db.pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let current_round: i64 = row.try_get("current_round")?;
    let round = load_round_pool(db, community_id, session_id, current_round)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("meeting floor {session_id}/{current_round}")))?;
    snapshot_from_round_pool(db, community_id, session_id, round).await
}

/// Claim a batch of transactional meeting events for post-commit delivery.
pub async fn claim_outbox_batch(
    db: &Db,
    worker_id: Uuid,
    lease: StdDuration,
    limit: i64,
) -> Result<Vec<MeetingOutboxEvent>> {
    validate_duration(lease, "meeting outbox lease")?;
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let lease_ms = i64::try_from(lease.as_millis())
        .map_err(|_| DbError::InvalidData("meeting outbox lease is too large".to_string()))?;
    let rows = sqlx::query(
        "WITH candidates AS ( \
             SELECT community_id, sequence \
             FROM meeting_event_outbox \
             WHERE delivered_at IS NULL \
               AND available_at <= clock_timestamp() \
               AND (claimed_until IS NULL OR claimed_until <= clock_timestamp()) \
             ORDER BY available_at, sequence \
             FOR UPDATE SKIP LOCKED \
             LIMIT $1 \
         ), claimed AS ( \
             UPDATE meeting_event_outbox o \
             SET claimed_by = $2, \
                 claimed_until = clock_timestamp() + ($3 * interval '1 millisecond'), \
                 attempts = attempts + 1, \
                 last_error = NULL \
             FROM candidates c \
             WHERE o.community_id = c.community_id AND o.sequence = c.sequence \
             RETURNING o.community_id, o.sequence, o.session_id, o.event_id \
         ) \
         SELECT cl.community_id, cl.sequence, cl.session_id, c.host, \
                e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content, \
                e.sig, e.received_at, e.channel_id \
         FROM claimed cl \
         JOIN communities c ON c.id = cl.community_id \
         JOIN LATERAL ( \
             SELECT id, pubkey, created_at, kind, tags, content, sig, \
                    received_at, channel_id \
             FROM events \
             WHERE community_id = cl.community_id AND id = cl.event_id \
               AND deleted_at IS NULL \
             ORDER BY created_at DESC \
             LIMIT 1 \
         ) e ON TRUE \
         ORDER BY cl.sequence",
    )
    .bind(limit)
    .bind(worker_id)
    .bind(lease_ms)
    .fetch_all(&db.pool)
    .await?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        let community_id = CommunityId::from_uuid(row.try_get::<Uuid, _>("community_id")?);
        let host: String = row.try_get("host")?;
        let session_id: Uuid = row.try_get("session_id")?;
        let sequence: i64 = row.try_get("sequence")?;
        if let Some(stored_event) = crate::event::row_to_stored_event(row)? {
            events.push(MeetingOutboxEvent {
                community_id,
                host,
                session_id,
                sequence,
                stored_event,
            });
        }
    }
    Ok(events)
}

/// Mark one claimed outbox event as delivered.
pub async fn mark_outbox_delivered(
    db: &Db,
    community_id: CommunityId,
    sequence: i64,
    worker_id: Uuid,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_event_outbox \
         SET delivered_at = clock_timestamp(), claimed_until = NULL, last_error = NULL \
         WHERE community_id = $1 AND sequence = $2 AND claimed_by = $3 \
           AND delivered_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(sequence)
    .bind(worker_id)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// Release one claimed outbox event for retry after a delivery failure.
pub async fn release_outbox(
    db: &Db,
    community_id: CommunityId,
    sequence: i64,
    worker_id: Uuid,
    error: &str,
) -> Result<bool> {
    let result = sqlx::query(
        "UPDATE meeting_event_outbox \
         SET claimed_by = NULL, claimed_until = NULL, \
             available_at = clock_timestamp() + interval '1 second', last_error = $4 \
         WHERE community_id = $1 AND sequence = $2 AND claimed_by = $3 \
           AND delivered_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(sequence)
    .bind(worker_id)
    .bind(error)
    .execute(&db.pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

async fn lock_session(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<SessionLock> {
    let row = sqlx::query(
        "SELECT status, current_round, floor_revision \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    Ok(SessionLock {
        status: row.try_get("status")?,
        current_round: row.try_get("current_round")?,
        floor_revision: row.try_get("floor_revision")?,
    })
}

async fn database_now(tx: &mut Transaction<'_, Postgres>) -> Result<DateTime<Utc>> {
    sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await
        .map_err(Into::into)
}

async fn ensure_current_round_locked(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &mut SessionLock,
    now: DateTime<Utc>,
    relay_keys: &Keys,
) -> Result<RoundRow> {
    if let Some(round) = load_round_tx(tx, community_id, session_id, session.current_round).await? {
        return Ok(round);
    }
    create_open_round_locked(tx, community_id, session_id, session, now, relay_keys, None).await
}

async fn load_current_round_required(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &SessionLock,
) -> Result<RoundRow> {
    load_round_tx(tx, community_id, session_id, session.current_round)
        .await?
        .ok_or_else(|| {
            DbError::InvalidData(format!(
                "meeting {session_id} is missing current round {}",
                session.current_round
            ))
        })
}

async fn load_round_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<Option<RoundRow>> {
    let row = sqlx::query(
        "SELECT round_number, floor_revision, phase, state_event_id, \
                claim_deadline, holder_pubkey, grant_event_id, lease_expires_at, \
                outcome, speech_event_id \
         FROM meeting_rounds \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_optional(tx.as_mut())
    .await?;
    row.map(round_from_row).transpose()
}

async fn load_round_pool(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<Option<RoundRow>> {
    let row = sqlx::query(
        "SELECT round_number, floor_revision, phase, state_event_id, \
                claim_deadline, holder_pubkey, grant_event_id, lease_expires_at, \
                outcome, speech_event_id \
         FROM meeting_rounds \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_optional(&db.pool)
    .await?;
    row.map(round_from_row).transpose()
}

fn round_from_row(row: sqlx::postgres::PgRow) -> Result<RoundRow> {
    let phase: String = row.try_get("phase")?;
    Ok(RoundRow {
        round_number: row.try_get("round_number")?,
        floor_revision: row.try_get("floor_revision")?,
        phase: FloorPhase::parse(&phase)?,
        state_event_id: row.try_get("state_event_id")?,
        claim_deadline: row.try_get("claim_deadline")?,
        holder_pubkey: row.try_get("holder_pubkey")?,
        grant_event_id: row.try_get("grant_event_id")?,
        lease_expires_at: row.try_get("lease_expires_at")?,
        outcome: row.try_get("outcome")?,
        speech_event_id: row.try_get("speech_event_id")?,
    })
}

async fn load_claims_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<Vec<CanonicalClaim>> {
    let rows = sqlx::query(
        "SELECT claim_event_id, claimant_pubkey \
         FROM meeting_floor_claims \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
         ORDER BY claim_event_id ASC",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(tx.as_mut())
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(CanonicalClaim {
                event_id: row.try_get("claim_event_id")?,
                claimant_pubkey: row.try_get("claimant_pubkey")?,
            })
        })
        .collect()
}

async fn load_claims_pool(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<Vec<CanonicalClaim>> {
    let rows = sqlx::query(
        "SELECT claim_event_id, claimant_pubkey \
         FROM meeting_floor_claims \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
         ORDER BY claim_event_id ASC",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(&db.pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(CanonicalClaim {
                event_id: row.try_get("claim_event_id")?,
                claimant_pubkey: row.try_get("claimant_pubkey")?,
            })
        })
        .collect()
}

struct PreviousRound<'a> {
    round_number: i64,
    outcome: &'a str,
    speech_event_id: Option<&'a [u8]>,
}

#[allow(clippy::too_many_arguments)]
async fn create_open_round_locked(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &mut SessionLock,
    now: DateTime<Utc>,
    relay_keys: &Keys,
    previous: Option<PreviousRound<'_>>,
) -> Result<RoundRow> {
    let next_revision = session.floor_revision + 1;
    let mut content = serde_json::json!({ "claim_event_ids": [] });
    if let Some(previous) = previous {
        content["previous_round"] = serde_json::json!(previous.round_number);
        content["previous_outcome"] = serde_json::json!(previous.outcome);
        if let Some(speech_event_id) = previous.speech_event_id {
            content["previous_speech_event_id"] = serde_json::json!(hex::encode(speech_event_id));
        }
    }
    let state_event = build_round_state_event(
        relay_keys,
        session_id,
        session.current_round,
        next_revision,
        FloorPhase::Open,
        None,
        content,
        now,
    )?;
    persist_meeting_event_tx(tx, community_id, session_id, &state_event, now).await?;
    sqlx::query(
        "INSERT INTO meeting_rounds \
             (community_id, session_id, round_number, floor_revision, phase, \
              state_event_id, policy_version, created_at, updated_at) \
         VALUES ($1, $2, $3, $4, 'open', $5, $6, $7, $7)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(session.current_round)
    .bind(next_revision)
    .bind(state_event.id.as_bytes().as_slice())
    .bind(FLOOR_POLICY_VERSION)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    set_session_floor(
        tx,
        community_id,
        session_id,
        session.current_round,
        next_revision,
    )
    .await?;
    session.floor_revision = next_revision;
    Ok(RoundRow {
        round_number: session.current_round,
        floor_revision: next_revision,
        phase: FloorPhase::Open,
        state_event_id: state_event.id.as_bytes().to_vec(),
        claim_deadline: None,
        holder_pubkey: None,
        grant_event_id: None,
        lease_expires_at: None,
        outcome: None,
        speech_event_id: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn advance_due_locked(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &mut SessionLock,
    round: &RoundRow,
    now: DateTime<Utc>,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<bool> {
    if session.status != "active" || round.round_number != session.current_round {
        return Ok(false);
    }
    match round.phase {
        FloorPhase::Claiming if round.claim_deadline.is_some_and(|deadline| now >= deadline) => {
            grant_round_locked(
                tx,
                community_id,
                session_id,
                session,
                round,
                now,
                relay_keys,
                config,
                selector,
            )
            .await?;
            Ok(true)
        }
        FloorPhase::Granted
            if round
                .lease_expires_at
                .is_some_and(|deadline| now >= deadline) =>
        {
            expire_round_locked(
                tx,
                community_id,
                session_id,
                session,
                round,
                now,
                relay_keys,
            )
            .await?;
            Ok(true)
        }
        _ => Ok(false),
    }
}

#[allow(clippy::too_many_arguments)]
async fn grant_round_locked(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &mut SessionLock,
    round: &RoundRow,
    now: DateTime<Utc>,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<RoundRow> {
    let claims = load_claims_tx(tx, community_id, session_id, round.round_number).await?;
    if claims.is_empty() {
        return Err(DbError::InvalidData(
            "claiming round has no canonical Claims".to_string(),
        ));
    }
    let winner_index = match selector {
        WinnerSelector::UniformRandom => rand::random_range(0..claims.len()),
        WinnerSelector::FixedIndex(index) if index < claims.len() => index,
        WinnerSelector::FixedIndex(index) => {
            return Err(DbError::InvalidData(format!(
                "fixed winner index {index} is out of range for {} Claims",
                claims.len()
            )));
        }
    };
    let winner = &claims[winner_index];
    let lease_expires_at = now + chrono_duration(config.grant_lease)?;
    let next_revision = session.floor_revision + 1;
    let grant_event = build_round_state_event(
        relay_keys,
        session_id,
        round.round_number,
        next_revision,
        FloorPhase::Granted,
        Some(&winner.claimant_pubkey),
        serde_json::json!({
            "lease_expires_at_ms": lease_expires_at.timestamp_millis(),
            "claim_event_ids": claim_ids_hex(&claims),
            "claimants": claimant_hex(&claims),
        }),
        now,
    )?;
    persist_meeting_event_tx(tx, community_id, session_id, &grant_event, now).await?;
    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'granted', floor_revision = $4, state_event_id = $5, \
             holder_pubkey = $6, grant_event_id = $5, lease_expires_at = $7, \
             updated_at = $8 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND phase = 'claiming'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round.round_number)
    .bind(next_revision)
    .bind(grant_event.id.as_bytes().as_slice())
    .bind(&winner.claimant_pubkey)
    .bind(lease_expires_at)
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    set_session_floor(
        tx,
        community_id,
        session_id,
        round.round_number,
        next_revision,
    )
    .await?;
    session.floor_revision = next_revision;
    Ok(RoundRow {
        round_number: round.round_number,
        floor_revision: next_revision,
        phase: FloorPhase::Granted,
        state_event_id: grant_event.id.as_bytes().to_vec(),
        claim_deadline: round.claim_deadline,
        holder_pubkey: Some(winner.claimant_pubkey.clone()),
        grant_event_id: Some(grant_event.id.as_bytes().to_vec()),
        lease_expires_at: Some(lease_expires_at),
        outcome: None,
        speech_event_id: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn expire_round_locked(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    session: &mut SessionLock,
    round: &RoundRow,
    now: DateTime<Utc>,
    relay_keys: &Keys,
) -> Result<()> {
    let closed_revision = session.floor_revision + 1;
    let closed_event = build_round_state_event(
        relay_keys,
        session_id,
        round.round_number,
        closed_revision,
        FloorPhase::Closed,
        None,
        serde_json::json!({ "outcome": "expired" }),
        now,
    )?;
    persist_meeting_event_tx(tx, community_id, session_id, &closed_event, now).await?;
    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'closed', floor_revision = $4, state_event_id = $5, \
             outcome = 'expired', updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND phase = 'granted'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round.round_number)
    .bind(closed_revision)
    .bind(closed_event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    session.floor_revision = closed_revision;
    session.current_round = round.round_number + 1;
    create_open_round_locked(
        tx,
        community_id,
        session_id,
        session,
        now,
        relay_keys,
        Some(PreviousRound {
            round_number: round.round_number,
            outcome: "expired",
            speech_event_id: None,
        }),
    )
    .await?;
    Ok(())
}

async fn set_session_floor(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    floor_revision: i64,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE meeting_sessions \
         SET current_round = $3, floor_revision = $4 \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(floor_revision)
    .execute(tx.as_mut())
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::NotFound(format!("meeting {session_id}")));
    }
    Ok(())
}

async fn ensure_active_participant(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    pubkey: &[u8],
) -> Result<()> {
    let member: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
               AND removed_at IS NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(pubkey)
    .fetch_one(tx.as_mut())
    .await?;
    if !member {
        return Err(DbError::AccessDenied(
            "only a meeting participant may use the speech floor".to_string(),
        ));
    }
    Ok(())
}

async fn persist_meeting_event_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    event: &Event,
    received_at: DateTime<Utc>,
) -> Result<()> {
    let created_at_secs = event.created_at.as_secs() as i64;
    let created_at = DateTime::from_timestamp(created_at_secs, 0)
        .ok_or(DbError::InvalidTimestamp(created_at_secs))?;
    let tags = serde_json::to_value(&event.tags)?;
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
    .bind(tags)
    .bind(&event.content)
    .bind(event.sig.serialize().as_slice())
    .bind(received_at)
    .bind(session_id)
    .execute(tx.as_mut())
    .await?;
    if result.rows_affected() != 1 {
        return Err(DbError::InvalidData(format!(
            "meeting event {} already exists without its canonical projection",
            event.id
        )));
    }
    sqlx::query(
        "INSERT INTO meeting_event_outbox (community_id, session_id, event_id) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (community_id, event_id) DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(event.id.as_bytes().as_slice())
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_round_state_event(
    relay_keys: &Keys,
    session_id: Uuid,
    round_number: i64,
    floor_revision: i64,
    phase: FloorPhase,
    holder_pubkey: Option<&[u8]>,
    content: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<Event> {
    let session = session_id.to_string();
    let round = round_number.to_string();
    let revision = floor_revision.to_string();
    let mut tags = vec![
        parse_tag(["h", session.as_str()])?,
        parse_tag(["meeting-round", round.as_str()])?,
        parse_tag(["floor-revision", revision.as_str()])?,
        parse_tag(["phase", phase.as_str()])?,
        parse_tag(["policy", FLOOR_POLICY_VERSION])?,
    ];
    if let Some(holder_pubkey) = holder_pubkey {
        let holder = hex::encode(holder_pubkey);
        tags.push(parse_tag(["holder", holder.as_str()])?);
    }
    let timestamp =
        u64::try_from(now.timestamp()).map_err(|_| DbError::InvalidTimestamp(now.timestamp()))?;
    EventBuilder::new(
        Kind::Custom(buzz_core::kind::KIND_MEETING_ROUND_STATE as u16),
        serde_json::to_string(&content)?,
    )
    .tags(tags)
    .custom_created_at(Timestamp::from(timestamp))
    .sign_with_keys(relay_keys)
    .map_err(|error| DbError::InvalidData(format!("sign meeting round state: {error}")))
}

fn parse_tag<const N: usize>(parts: [&str; N]) -> Result<Tag> {
    Tag::parse(parts).map_err(|error| DbError::InvalidData(format!("build meeting tag: {error}")))
}

fn validate_event_identity(event: &Event, session_id: Uuid, round_number: i64) -> Result<()> {
    if round_number <= 0 {
        return Err(DbError::InvalidData(
            "meeting round must be positive".to_string(),
        ));
    }
    let h_values = event_tag_values(event, "h");
    if h_values.as_slice() != [session_id.to_string()] {
        return Err(DbError::InvalidData(
            "meeting event has the wrong session tag".to_string(),
        ));
    }
    let round_values = event_tag_values(event, "meeting-round");
    if round_values.as_slice() != [round_number.to_string()] {
        return Err(DbError::InvalidData(
            "meeting event has the wrong round tag".to_string(),
        ));
    }
    Ok(())
}

fn event_tag_values(event: &Event, name: &str) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|tag| {
            let parts = tag.as_slice();
            (parts.len() >= 2 && parts[0].as_str() == name).then(|| parts[1].to_string())
        })
        .collect()
}

fn validate_duration(duration: StdDuration, field: &str) -> Result<()> {
    if duration.is_zero() {
        return Err(DbError::InvalidData(format!("{field} must be positive")));
    }
    if duration > StdDuration::from_secs(300) {
        return Err(DbError::InvalidData(format!(
            "{field} must not exceed 300 seconds"
        )));
    }
    Ok(())
}

fn chrono_duration(duration: StdDuration) -> Result<Duration> {
    Duration::from_std(duration)
        .map_err(|_| DbError::InvalidData("meeting duration is too large".to_string()))
}

fn validate_32(value: &[u8], field: &str) -> Result<()> {
    if value.len() != 32 {
        return Err(DbError::InvalidData(format!(
            "{field} must be 32 bytes, got {}",
            value.len()
        )));
    }
    Ok(())
}

fn claim_ids_hex(claims: &[CanonicalClaim]) -> Vec<String> {
    claims
        .iter()
        .map(|claim| hex::encode(&claim.event_id))
        .collect()
}

fn claimant_hex(claims: &[CanonicalClaim]) -> Vec<String> {
    claims
        .iter()
        .map(|claim| hex::encode(&claim.claimant_pubkey))
        .collect()
}

async fn snapshot_from_round(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round: RoundRow,
) -> Result<FloorSnapshot> {
    let claims = load_claims_tx(tx, community_id, session_id, round.round_number).await?;
    Ok(snapshot(round, session_id, claims))
}

async fn snapshot_from_round_pool(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round: RoundRow,
) -> Result<FloorSnapshot> {
    let claims = load_claims_pool(db, community_id, session_id, round.round_number).await?;
    Ok(snapshot(round, session_id, claims))
}

fn snapshot(round: RoundRow, session_id: Uuid, claims: Vec<CanonicalClaim>) -> FloorSnapshot {
    FloorSnapshot {
        session_id,
        round_number: round.round_number,
        floor_revision: round.floor_revision,
        phase: round.phase,
        state_event_id: round.state_event_id,
        claim_deadline: round.claim_deadline,
        holder_pubkey: round.holder_pubkey,
        grant_event_id: round.grant_event_id,
        lease_expires_at: round.lease_expires_at,
        outcome: round.outcome,
        speech_event_id: round.speech_event_id,
        claim_event_ids: claims.iter().map(|claim| claim.event_id.clone()).collect(),
        claimant_pubkeys: claims
            .into_iter()
            .map(|claim| claim.claimant_pubkey)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Kind, Tag};
    use sqlx::PgPool;

    #[test]
    fn phase_parser_is_fail_closed() {
        assert_eq!(FloorPhase::parse("open").unwrap(), FloorPhase::Open);
        assert!(FloorPhase::parse("waiting").is_err());
    }

    #[test]
    fn duration_validation_rejects_zero_and_unbounded_values() {
        assert!(validate_duration(StdDuration::ZERO, "test").is_err());
        assert!(validate_duration(StdDuration::from_secs(301), "test").is_err());
        assert!(validate_duration(StdDuration::from_millis(1), "test").is_ok());
    }

    async fn setup_meeting() -> (Db, CommunityId, Uuid, Keys, Vec<Keys>) {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to Meeting V0 floor test database");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting V0 floor migration");

        let community_uuid = Uuid::new_v4();
        let community_id = CommunityId::from_uuid(community_uuid);
        let session_id = Uuid::new_v4();
        let relay_keys = Keys::generate();
        let participants = vec![Keys::generate(), Keys::generate(), Keys::generate()];
        let host = participants[0].public_key().to_bytes().to_vec();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_uuid)
            .bind(format!("meeting-floor-{}.example", community_uuid.simple()))
            .execute(&pool)
            .await
            .expect("insert floor test community");
        sqlx::query(
            "INSERT INTO channels \
                 (id, community_id, name, channel_type, visibility, created_by, room_kind) \
             VALUES ($1, $2, 'floor-test', 'stream', 'private', $3, 'meeting')",
        )
        .bind(session_id)
        .bind(community_uuid)
        .bind(&host)
        .execute(&pool)
        .await
        .expect("insert floor test channel");
        for (index, participant) in participants.iter().enumerate() {
            sqlx::query(
                "INSERT INTO channel_members \
                     (community_id, channel_id, pubkey, role, invited_by) \
                 VALUES ($1, $2, $3, $4::member_role, $5)",
            )
            .bind(community_uuid)
            .bind(session_id)
            .bind(participant.public_key().as_bytes())
            .bind(if index == 0 { "owner" } else { "member" })
            .bind(&host)
            .execute(&pool)
            .await
            .expect("insert floor test participant");
        }
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey) \
             VALUES ($1, $2, $3, $4)",
        )
        .bind(community_uuid)
        .bind(session_id)
        .bind([91_u8; 32].as_slice())
        .bind(&host)
        .execute(&pool)
        .await
        .expect("insert floor test session");

        let db = Db::from_pool(pool);
        let mut tx = db.begin_transaction().await.expect("begin floor init");
        initialize_floor_tx(&mut tx, community_id, session_id, &relay_keys)
            .await
            .expect("initialize Round 1");
        tx.commit().await.expect("commit floor init");
        (db, community_id, session_id, relay_keys, participants)
    }

    async fn reconnect_test_db() -> Db {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://buzz:buzz_dev@localhost:5432/buzz".to_string());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("reconnect to Meeting V0 floor test database");
        Db::from_pool(pool)
    }

    fn claim_event(keys: &Keys, session_id: Uuid, round_number: i64) -> Event {
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_MEETING_FLOOR_CLAIM as u16),
            "",
        )
        .tags([
            Tag::parse(["h", &session_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", &round_number.to_string()]).expect("round tag"),
        ])
        .sign_with_keys(keys)
        .expect("sign Claim")
    }

    fn speech_event(
        keys: &Keys,
        session_id: Uuid,
        round_number: i64,
        grant_id: &[u8],
        content: &str,
    ) -> Event {
        EventBuilder::new(Kind::Custom(9), content)
            .tags([
                Tag::parse(["h", &session_id.to_string()]).expect("h tag"),
                Tag::parse(["meeting-round", &round_number.to_string()]).expect("round tag"),
                Tag::parse(["meeting-grant", &hex::encode(grant_id)]).expect("grant tag"),
            ])
            .sign_with_keys(keys)
            .expect("sign speech")
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn claim_grant_say_expiry_restart_and_idempotency_are_atomic() {
        let (db, community_id, session_id, relay_keys, participants) = setup_meeting().await;
        let config = FloorConfig {
            claim_window: StdDuration::from_millis(80),
            grant_lease: StdDuration::from_secs(2),
        };
        let claim_a = claim_event(&participants[0], session_id, 1);
        let claim_b = claim_event(&participants[1], session_id, 1);
        let future_claim = claim_event(&participants[2], session_id, 2);
        assert!(claim_floor(
            &db,
            community_id,
            session_id,
            2,
            &future_claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect_err("future Claim must be rejected")
        .to_string()
        .contains("current round is 1"));

        let db_a = db.clone();
        let relay_a = relay_keys.clone();
        let task_a = tokio::spawn(async move {
            claim_floor(
                &db_a,
                community_id,
                session_id,
                1,
                &claim_a,
                &relay_a,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
        });
        let db_b = db.clone();
        let relay_b = relay_keys.clone();
        let task_b = tokio::spawn(async move {
            claim_floor(
                &db_b,
                community_id,
                session_id,
                1,
                &claim_b,
                &relay_b,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
        });
        assert!(matches!(
            task_a.await.expect("Claim task A").expect("Claim A"),
            ClaimFloorOutcome::Accepted { .. }
        ));
        assert!(matches!(
            task_b.await.expect("Claim task B").expect("Claim B"),
            ClaimFloorOutcome::Accepted { .. }
        ));

        let conflicting = EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_MEETING_FLOOR_CLAIM as u16),
            "",
        )
        .tags([
            Tag::parse(["h", &session_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", "1"]).expect("round tag"),
        ])
        .custom_created_at(Timestamp::from(
            u64::try_from(Utc::now().timestamp()).expect("positive test timestamp") + 1,
        ))
        .sign_with_keys(&participants[0])
        .expect("sign conflicting Claim");
        assert!(matches!(
            claim_floor(
                &db,
                community_id,
                session_id,
                1,
                &conflicting,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("conflicting Claim result"),
            ClaimFloorOutcome::Conflict { .. }
        ));

        let claiming_before_restart = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("claiming snapshot before restart");
        assert_eq!(claiming_before_restart.phase, FloorPhase::Claiming);
        drop(db);
        let db = reconnect_test_db().await;
        let claiming_after_restart = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("claiming snapshot after restart");
        assert_eq!(
            claiming_after_restart.claim_deadline,
            claiming_before_restart.claim_deadline
        );
        assert_eq!(
            claiming_after_restart.claim_event_ids,
            claiming_before_restart.claim_event_ids
        );

        tokio::time::sleep(StdDuration::from_millis(100)).await;
        assert!(
            recover_due_floors(&db, &relay_keys, config, WinnerSelector::FixedIndex(0), 10,)
                .await
                .expect("settle Round 1")
                >= 1
        );
        let granted = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("granted snapshot");
        assert_eq!(granted.phase, FloorPhase::Granted);
        assert_eq!(granted.claim_event_ids.len(), 2);
        let late_claim = claim_event(&participants[2], session_id, 1);
        assert!(claim_floor(
            &db,
            community_id,
            session_id,
            1,
            &late_claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect_err("late Claim must be rejected")
        .to_string()
        .contains("already settled"));
        assert!(matches!(
            claim_floor(
                &db,
                community_id,
                session_id,
                1,
                &conflicting,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("settled-round conflicting Claim result"),
            ClaimFloorOutcome::Conflict { .. }
        ));
        let holder = granted.holder_pubkey.clone().expect("winner");
        let holder_keys = participants
            .iter()
            .find(|keys| keys.public_key().as_bytes() == holder.as_slice())
            .expect("winner keys");
        let grant_id = granted.grant_event_id.clone().expect("Grant ID");

        drop(db);
        let db = reconnect_test_db().await;
        let granted_after_restart = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("granted snapshot after restart");
        assert_eq!(
            granted_after_restart.grant_event_id.as_deref(),
            Some(grant_id.as_slice())
        );
        assert_eq!(
            granted_after_restart.holder_pubkey.as_deref(),
            Some(holder.as_slice())
        );

        let speech_a = speech_event(holder_keys, session_id, 1, &grant_id, "candidate speech A");
        let speech_b = speech_event(holder_keys, session_id, 1, &grant_id, "candidate speech B");
        let say_a = {
            let db = db.clone();
            let relay_keys = relay_keys.clone();
            let grant_id = grant_id.clone();
            let speech = speech_a.clone();
            tokio::spawn(async move {
                say(
                    &db,
                    community_id,
                    session_id,
                    1,
                    &grant_id,
                    &speech,
                    &relay_keys,
                    config,
                    WinnerSelector::FixedIndex(0),
                )
                .await
            })
        };
        let say_b = {
            let db = db.clone();
            let relay_keys = relay_keys.clone();
            let grant_id = grant_id.clone();
            let speech = speech_b.clone();
            tokio::spawn(async move {
                say(
                    &db,
                    community_id,
                    session_id,
                    1,
                    &grant_id,
                    &speech,
                    &relay_keys,
                    config,
                    WinnerSelector::FixedIndex(0),
                )
                .await
            })
        };
        let results = [
            say_a.await.expect("Say task A").expect("Say A result"),
            say_b.await.expect("Say task B").expect("Say B result"),
        ];
        let accepted_speech_id = results
            .iter()
            .find_map(|outcome| match outcome {
                SayOutcome::Accepted {
                    speech_event_id,
                    next_round_number: 2,
                    ..
                } => Some(speech_event_id.clone()),
                _ => None,
            })
            .expect("one concurrent speech is accepted");
        assert_eq!(
            results
                .iter()
                .filter(|outcome| matches!(outcome, SayOutcome::Accepted { .. }))
                .count(),
            1
        );
        assert!(results.iter().any(|outcome| matches!(
            outcome,
            SayOutcome::GrantConsumed {
                accepted_speech_event_id
            } if accepted_speech_event_id == &accepted_speech_id
        )));
        let canonical_speech = if speech_a.id.as_bytes() == accepted_speech_id.as_slice() {
            &speech_a
        } else {
            &speech_b
        };
        assert!(matches!(
            say(
                &db,
                community_id,
                session_id,
                1,
                &grant_id,
                canonical_speech,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("retry accepted speech"),
            SayOutcome::Duplicate { .. }
        ));
        let third_speech = speech_event(
            holder_keys,
            session_id,
            1,
            &grant_id,
            "must not double spend",
        );
        assert!(matches!(
            say(
                &db,
                community_id,
                session_id,
                1,
                &grant_id,
                &third_speech,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("second Grant spend result"),
            SayOutcome::GrantConsumed { .. }
        ));

        assert!(claim_floor(
            &db,
            community_id,
            session_id,
            1,
            &late_claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect_err("old-round Claim must be rejected")
        .to_string()
        .contains("current round is 2"));
        let round_two_claim = claim_event(&participants[2], session_id, 2);
        claim_floor(
            &db,
            community_id,
            session_id,
            2,
            &round_two_claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect("Claim Round 2");
        tokio::time::sleep(StdDuration::from_millis(100)).await;
        recover_due_floors(&db, &relay_keys, config, WinnerSelector::FixedIndex(0), 10)
            .await
            .expect("grant Round 2");
        tokio::time::sleep(StdDuration::from_millis(2_050)).await;
        recover_due_floors(&db, &relay_keys, config, WinnerSelector::FixedIndex(0), 10)
            .await
            .expect("expire Round 2");
        let round_three = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("Round 3 snapshot");
        assert_eq!(round_three.round_number, 3);
        assert_eq!(round_three.phase, FloorPhase::Open);
        let round_two = load_round_pool(&db, community_id, session_id, 2)
            .await
            .expect("load Round 2")
            .expect("Round 2 exists");
        assert_eq!(round_two.outcome.as_deref(), Some("expired"));

        let outbox = claim_outbox_batch(&db, Uuid::new_v4(), StdDuration::from_secs(1), 100)
            .await
            .expect("claim durable meeting outbox");
        assert!(outbox.len() >= 10);
    }
}
