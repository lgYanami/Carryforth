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

/// Default minimum delay before an otherwise-complete Claim cohort settles.
pub const DEFAULT_CLAIM_SETTLE_DELAY: StdDuration = StdDuration::from_secs(3);
/// Default maximum duration of the Claim competition window.
pub const DEFAULT_CLAIM_WINDOW: StdDuration = StdDuration::from_secs(300);
/// Default duration of a granted speech lease.
pub const DEFAULT_GRANT_LEASE: StdDuration = StdDuration::from_secs(300);

/// Runtime timing configuration for the Meeting V0 floor.
#[derive(Debug, Clone, Copy)]
pub struct FloorConfig {
    /// Minimum time from the first Claim until winner selection.
    pub claim_settle_delay: StdDuration,
    /// Maximum time from the first Claim until winner selection.
    pub claim_window: StdDuration,
    /// Time a winner has to publish one valid speech.
    pub grant_lease: StdDuration,
}

impl Default for FloorConfig {
    fn default() -> Self {
        Self {
            claim_settle_delay: DEFAULT_CLAIM_SETTLE_DELAY,
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
    /// Earliest cohort-complete settlement time, when in `claiming` or later.
    pub settle_not_before: Option<DateTime<Utc>>,
    /// Maximum Claim deadline, when in `claiming` or later.
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
    /// Agent pubkeys frozen into the first-Claim decision cohort.
    pub decision_cohort_pubkeys: Vec<Vec<u8>>,
    /// Agent pubkeys that have declared Ready in the current round.
    pub ready_pubkeys: Vec<Vec<u8>>,
    /// Agent pubkeys that have declared Pass in the current round.
    pub passer_pubkeys: Vec<Vec<u8>>,
}

/// Participant action carried by a Meeting V0 floor signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorSignalAction {
    /// An Agent has synchronized the round and will decide Claim or Pass.
    Ready,
    /// An Agent has completed its decision without claiming.
    Pass,
}

impl FloorSignalAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Pass => "pass",
        }
    }
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

/// Result of submitting an Agent Ready or Pass signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloorSignalOutcome {
    /// A new canonical signal was committed.
    Accepted {
        /// Round signalled.
        round_number: i64,
        /// Revision assigned to the signal's Round State.
        floor_revision: i64,
        /// Canonical signal event ID.
        signal_event_id: Vec<u8>,
    },
    /// The same logical action was already committed.
    Duplicate {
        /// Round originally signalled.
        round_number: i64,
        /// Revision assigned to the canonical signal.
        floor_revision: i64,
        /// Canonical signal event ID.
        signal_event_id: Vec<u8>,
    },
}

/// Result of submitting a holder-authored Yield signal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YieldOutcome {
    /// The Grant was yielded and the next round was opened.
    Accepted {
        /// Yielded round.
        round_number: i64,
        /// Canonical Yield event ID.
        signal_event_id: Vec<u8>,
        /// Newly opened round.
        next_round_number: i64,
        /// Revision of the newly opened round.
        floor_revision: i64,
    },
    /// This Grant was already yielded by the same canonical signal.
    Duplicate {
        /// Yielded round.
        round_number: i64,
        /// Canonical Yield event ID.
        signal_event_id: Vec<u8>,
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
    settle_not_before: Option<DateTime<Utc>>,
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

#[derive(Debug, Default)]
struct FloorControlSets {
    decision_cohort_pubkeys: Vec<Vec<u8>>,
    ready_pubkeys: Vec<Vec<u8>>,
    passer_pubkeys: Vec<Vec<u8>>,
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
    validate_floor_config(config)?;
    validate_event_identity(event, session_id, round_number)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;
    if crate::meeting_revocation::recover_revoked_roster_v0_tx(
        &mut tx,
        community_id,
        session_id,
        relay_keys,
    )
    .await?
    {
        tx.commit().await?;
        return Err(participant_revoked_error());
    }
    ensure_actor_security_active_tx(&mut tx, community_id, session_id, event.pubkey.as_bytes())
        .await?;

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

    let first_claim = round.phase == FloorPhase::Open;
    let settle_not_before = match round.phase {
        FloorPhase::Open => Some(now + chrono_duration(config.claim_settle_delay)?),
        FloorPhase::Claiming => round.settle_not_before,
        _ => None,
    }
    .ok_or_else(|| {
        DbError::InvalidData("claiming round is missing its settle boundary".to_string())
    })?;
    let claim_deadline = match round.phase {
        FloorPhase::Open => Some(now + chrono_duration(config.claim_window)?),
        FloorPhase::Claiming => round.claim_deadline,
        _ => None,
    }
    .ok_or_else(|| DbError::InvalidData("claiming round is missing its deadline".to_string()))?;

    if first_claim {
        freeze_decision_cohort_tx(&mut tx, community_id, session_id, round_number, now).await?;
    }

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
    let controls = load_control_sets_tx(&mut tx, community_id, session_id, round_number).await?;
    let state_event = build_round_state_event(
        relay_keys,
        session_id,
        round_number,
        next_revision,
        FloorPhase::Claiming,
        None,
        serde_json::json!({
            "settle_not_before_ms": settle_not_before.timestamp_millis(),
            "claim_deadline_ms": claim_deadline.timestamp_millis(),
            "claim_event_ids": claim_ids_hex(&claims),
            "claimants": claimant_hex(&claims),
            "decision_cohort": pubkeys_hex(&controls.decision_cohort_pubkeys),
            "ready": pubkeys_hex(&controls.ready_pubkeys),
            "passed": pubkeys_hex(&controls.passer_pubkeys),
        }),
        now,
    )?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, &state_event, now).await?;

    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'claiming', floor_revision = $4, state_event_id = $5, \
             settle_not_before = $6, claim_deadline = $7, updated_at = $8 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(next_revision)
    .bind(state_event.id.as_bytes().as_slice())
    .bind(settle_not_before)
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
    session.floor_revision = next_revision;

    let claiming_round = RoundRow {
        round_number,
        floor_revision: next_revision,
        phase: FloorPhase::Claiming,
        state_event_id: state_event.id.as_bytes().to_vec(),
        settle_not_before: Some(settle_not_before),
        claim_deadline: Some(claim_deadline),
        holder_pubkey: None,
        grant_event_id: None,
        lease_expires_at: None,
        outcome: None,
        speech_event_id: None,
    };
    advance_due_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        &claiming_round,
        now,
        relay_keys,
        config,
        selector,
    )
    .await?;

    tx.commit().await?;
    Ok(ClaimFloorOutcome::Accepted {
        round_number,
        floor_revision: next_revision,
        claim_event_id: event.id.as_bytes().to_vec(),
    })
}

/// Submit one Agent-authored Ready or Pass signal.
///
/// Signals are logically idempotent by meeting, round, Agent, action, and
/// intent basis. A Pass is accepted only after the same Agent's Ready and
/// cannot withdraw an already-canonical Claim.
#[allow(clippy::too_many_arguments)]
pub async fn signal_intent(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    action: FloorSignalAction,
    intent_basis: &str,
    event: &Event,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<FloorSignalOutcome> {
    validate_floor_config(config)?;
    validate_event_identity(event, session_id, round_number)?;
    validate_intent_signal_event(event, action, intent_basis)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;
    if crate::meeting_revocation::recover_revoked_roster_v0_tx(
        &mut tx,
        community_id,
        session_id,
        relay_keys,
    )
    .await?
    {
        tx.commit().await?;
        return Err(participant_revoked_error());
    }
    ensure_actor_security_active_tx(&mut tx, community_id, session_id, event.pubkey.as_bytes())
        .await?;

    if let Some(row) = sqlx::query(
        "SELECT round_number, floor_revision, signal_event_id \
         FROM meeting_floor_signals \
         WHERE community_id = $1 \
           AND (signal_event_id = $2 OR ( \
             session_id = $3 AND round_number = $4 \
             AND participant_pubkey = $5 AND action = $6 \
             AND intent_basis = $7 \
           )) \
         ORDER BY (signal_event_id = $2) DESC \
         LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .bind(session_id)
    .bind(round_number)
    .bind(event.pubkey.as_bytes())
    .bind(action.as_str())
    .bind(intent_basis)
    .fetch_optional(tx.as_mut())
    .await?
    {
        let outcome = FloorSignalOutcome::Duplicate {
            round_number: row.try_get("round_number")?,
            floor_revision: row.try_get("floor_revision")?,
            signal_event_id: row.try_get("signal_event_id")?,
        };
        tx.rollback().await?;
        return Ok(outcome);
    }

    if session.status != "active" {
        return Err(DbError::InvalidData("meeting has ended".to_string()));
    }
    ensure_active_agent_participant(&mut tx, community_id, session_id, event.pubkey.as_bytes())
        .await?;

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
            "floor signal targets round {round_number}, current round is {}",
            session.current_round
        )));
    }
    if !matches!(round.phase, FloorPhase::Open | FloorPhase::Claiming) {
        return Err(DbError::InvalidData(
            "floor signal arrived after the round settled".to_string(),
        ));
    }
    if round.claim_deadline.is_some_and(|deadline| now >= deadline) {
        return Err(DbError::InvalidData(
            "floor signal arrived at or after the competition deadline".to_string(),
        ));
    }

    if action == FloorSignalAction::Pass {
        let was_ready: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_floor_signals \
                 WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
                   AND participant_pubkey = $4 AND action = 'ready' \
                   AND intent_basis = $5 \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(round_number)
        .bind(event.pubkey.as_bytes())
        .bind(intent_basis)
        .fetch_one(tx.as_mut())
        .await?;
        if !was_ready {
            return Err(DbError::InvalidData(
                "meeting Pass requires a matching Ready signal".to_string(),
            ));
        }
        let already_claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_floor_claims \
                 WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
                   AND claimant_pubkey = $4 \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(round_number)
        .bind(event.pubkey.as_bytes())
        .fetch_one(tx.as_mut())
        .await?;
        if already_claimed {
            return Err(DbError::InvalidData(
                "a canonical Claim cannot be withdrawn with Pass".to_string(),
            ));
        }
    }

    persist_meeting_event_tx(&mut tx, community_id, session_id, event, now).await?;
    let next_revision = session.floor_revision + 1;
    sqlx::query(
        "INSERT INTO meeting_floor_signals \
             (community_id, session_id, round_number, participant_pubkey, action, \
              intent_basis, signal_event_id, floor_revision, received_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(event.pubkey.as_bytes())
    .bind(action.as_str())
    .bind(intent_basis)
    .bind(event.id.as_bytes().as_slice())
    .bind(next_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    let claims = load_claims_tx(&mut tx, community_id, session_id, round_number).await?;
    let controls = load_control_sets_tx(&mut tx, community_id, session_id, round_number).await?;
    let state_event = build_round_state_event(
        relay_keys,
        session_id,
        round_number,
        next_revision,
        round.phase,
        None,
        round_state_content(&round, &claims, &controls),
        now,
    )?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, &state_event, now).await?;
    sqlx::query(
        "UPDATE meeting_rounds \
         SET floor_revision = $4, state_event_id = $5, updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND phase IN ('open', 'claiming')",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(next_revision)
    .bind(state_event.id.as_bytes().as_slice())
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
    session.floor_revision = next_revision;

    let signalled_round = RoundRow {
        floor_revision: next_revision,
        state_event_id: state_event.id.as_bytes().to_vec(),
        ..round
    };
    advance_due_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        &signalled_round,
        now,
        relay_keys,
        config,
        selector,
    )
    .await?;

    tx.commit().await?;
    Ok(FloorSignalOutcome::Accepted {
        round_number,
        floor_revision: next_revision,
        signal_event_id: event.id.as_bytes().to_vec(),
    })
}

/// Yield one active Grant and atomically open the next round.
#[allow(clippy::too_many_arguments)]
pub async fn yield_floor(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    grant_event_id: &[u8],
    event: &Event,
    relay_keys: &Keys,
    config: FloorConfig,
    selector: WinnerSelector,
) -> Result<YieldOutcome> {
    validate_floor_config(config)?;
    validate_32(grant_event_id, "meeting Grant event id")?;
    validate_event_identity(event, session_id, round_number)?;
    validate_yield_event(event, grant_event_id)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;
    if crate::meeting_revocation::recover_revoked_roster_v0_tx(
        &mut tx,
        community_id,
        session_id,
        relay_keys,
    )
    .await?
    {
        tx.commit().await?;
        return Err(participant_revoked_error());
    }
    ensure_actor_security_active_tx(&mut tx, community_id, session_id, event.pubkey.as_bytes())
        .await?;
    if let Some(row) = sqlx::query(
        "SELECT round_number, signal_event_id \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND action = 'yield' \
           AND (signal_event_id = $2 OR grant_event_id = $3) \
         ORDER BY (signal_event_id = $2) DESC \
         LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(event.id.as_bytes().as_slice())
    .bind(grant_event_id)
    .fetch_optional(tx.as_mut())
    .await?
    {
        let outcome = YieldOutcome::Duplicate {
            round_number: row.try_get("round_number")?,
            signal_event_id: row.try_get("signal_event_id")?,
        };
        tx.rollback().await?;
        return Ok(outcome);
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
            "Yield targets round {round_number}, current round is {}",
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
            "Yield references the wrong meeting Grant".to_string(),
        ));
    }
    if round.holder_pubkey.as_deref() != Some(event.pubkey.as_bytes()) {
        return Err(DbError::AccessDenied(
            "only the current floor holder may Yield this Grant".to_string(),
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
    sqlx::query(
        "INSERT INTO meeting_floor_signals \
             (community_id, session_id, round_number, participant_pubkey, action, \
              grant_event_id, signal_event_id, floor_revision, received_at) \
         VALUES ($1, $2, $3, $4, 'yield', $5, $6, $7, $8)",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(event.pubkey.as_bytes())
    .bind(grant_event_id)
    .bind(event.id.as_bytes().as_slice())
    .bind(closed_revision)
    .bind(now)
    .execute(tx.as_mut())
    .await?;

    let closed_event = build_round_state_event(
        relay_keys,
        session_id,
        round_number,
        closed_revision,
        FloorPhase::Closed,
        None,
        serde_json::json!({
            "outcome": "yielded",
            "yield_event_id": event.id.to_hex(),
        }),
        now,
    )?;
    persist_meeting_event_tx(&mut tx, community_id, session_id, &closed_event, now).await?;
    sqlx::query(
        "UPDATE meeting_rounds \
         SET phase = 'closed', floor_revision = $4, state_event_id = $5, \
             outcome = 'yielded', updated_at = $6 \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND phase = 'granted'",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(closed_revision)
    .bind(closed_event.id.as_bytes().as_slice())
    .bind(now)
    .execute(tx.as_mut())
    .await?;
    session.floor_revision = closed_revision;
    session.current_round = round_number + 1;
    let opened = create_open_round_locked(
        &mut tx,
        community_id,
        session_id,
        &mut session,
        now,
        relay_keys,
        Some(PreviousRound {
            round_number,
            outcome: "yielded",
            speech_event_id: None,
        }),
    )
    .await?;

    tx.commit().await?;
    Ok(YieldOutcome::Accepted {
        round_number,
        signal_event_id: event.id.as_bytes().to_vec(),
        next_round_number: opened.round_number,
        floor_revision: opened.floor_revision,
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
    validate_floor_config(config)?;
    validate_32(grant_event_id, "meeting grant event id")?;
    validate_event_identity(event, session_id, round_number)?;

    let mut tx = db.begin_transaction().await?;
    let mut session = lock_session(&mut tx, community_id, session_id).await?;
    if crate::meeting_revocation::recover_revoked_roster_v0_tx(
        &mut tx,
        community_id,
        session_id,
        relay_keys,
    )
    .await?
    {
        tx.commit().await?;
        return Err(participant_revoked_error());
    }
    ensure_actor_security_active_tx(&mut tx, community_id, session_id, event.pubkey.as_bytes())
        .await?;

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
    validate_floor_config(config)?;
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
           AND ms.schema_version = 1 \
           AND ms.floor_policy_version = 'uniform-v0' \
           AND ( \
             mr.session_id IS NULL \
             OR (mr.phase = 'claiming' AND ( \
               mr.claim_deadline <= clock_timestamp() \
               OR mr.settle_not_before <= clock_timestamp() \
             )) \
             OR (mr.phase = 'granted' AND mr.lease_expires_at <= clock_timestamp()) \
           ) \
         ORDER BY COALESCE( \
                    mr.settle_not_before, mr.claim_deadline, \
                    mr.lease_expires_at, ms.created_at \
                  ), \
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
        if crate::meeting_revocation::recover_revoked_roster_v0_tx(
            &mut tx,
            community_id,
            session_id,
            relay_keys,
        )
        .await?
        {
            tx.commit().await?;
            advanced += 1;
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
        "SELECT current_round, schema_version, floor_policy_version \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(&db.pool)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let schema_version: i32 = row.try_get("schema_version")?;
    let policy: String = row.try_get("floor_policy_version")?;
    if schema_version != 1 || policy != FLOOR_POLICY_VERSION {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a {FLOOR_POLICY_VERSION} session"
        )));
    }
    let current_round: i64 = row.try_get("current_round")?;
    let round = load_round_pool(db, community_id, session_id, current_round)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("meeting floor {session_id}/{current_round}")))?;
    snapshot_from_round_pool(db, community_id, session_id, round).await
}

/// Claim a batch of transactional meeting events for post-commit delivery.
///
/// At most the earliest undelivered event for each Meeting Session is eligible.
/// A claimed, backed-off, or otherwise unfinished predecessor therefore blocks
/// every later event in that Session, preserving the canonical control-log
/// order across delivery failures and concurrent workers.
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
             SELECT pending.community_id, pending.sequence \
             FROM meeting_event_outbox pending \
             WHERE pending.delivered_at IS NULL \
               AND pending.available_at <= clock_timestamp() \
               AND (pending.claimed_until IS NULL \
                    OR pending.claimed_until <= clock_timestamp()) \
               AND NOT EXISTS ( \
                   SELECT 1 \
                   FROM meeting_event_outbox predecessor \
                   WHERE predecessor.community_id = pending.community_id \
                     AND predecessor.session_id = pending.session_id \
                     AND predecessor.sequence < pending.sequence \
                     AND predecessor.delivered_at IS NULL \
               ) \
             ORDER BY pending.available_at, pending.sequence \
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
        "SELECT status, current_round, floor_revision, schema_version, \
                floor_policy_version \
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
    if schema_version != 1 || policy != FLOOR_POLICY_VERSION {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a {FLOOR_POLICY_VERSION} session"
        )));
    }
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
                settle_not_before, \
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
                settle_not_before, \
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
        settle_not_before: row.try_get("settle_not_before")?,
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

async fn load_control_sets_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<FloorControlSets> {
    let cohort = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT participant_pubkey \
         FROM meeting_round_decision_cohort \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(tx.as_mut())
    .await?;
    let ready = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT DISTINCT participant_pubkey \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND action = 'ready' \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(tx.as_mut())
    .await?;
    let passed = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT DISTINCT participant_pubkey \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND action = 'pass' \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(tx.as_mut())
    .await?;
    Ok(FloorControlSets {
        decision_cohort_pubkeys: cohort,
        ready_pubkeys: ready,
        passer_pubkeys: passed,
    })
}

async fn load_control_sets_pool(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<FloorControlSets> {
    let cohort = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT participant_pubkey \
         FROM meeting_round_decision_cohort \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(&db.pool)
    .await?;
    let ready = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT DISTINCT participant_pubkey \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND action = 'ready' \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(&db.pool)
    .await?;
    let passed = sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT DISTINCT participant_pubkey \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND action = 'pass' \
         ORDER BY participant_pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_all(&db.pool)
    .await?;
    Ok(FloorControlSets {
        decision_cohort_pubkeys: cohort,
        ready_pubkeys: ready,
        passer_pubkeys: passed,
    })
}

async fn freeze_decision_cohort_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
    frozen_at: DateTime<Utc>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO meeting_round_decision_cohort \
             (community_id, session_id, round_number, participant_pubkey, \
              ready_event_id, frozen_at) \
         SELECT DISTINCT ON (participant_pubkey) \
                community_id, session_id, round_number, participant_pubkey, \
                signal_event_id, $4 \
         FROM meeting_floor_signals \
         WHERE community_id = $1 AND session_id = $2 AND round_number = $3 \
           AND action = 'ready' \
         ORDER BY participant_pubkey, received_at, signal_event_id \
         ON CONFLICT DO NOTHING",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(frozen_at)
    .execute(tx.as_mut())
    .await?;
    Ok(())
}

async fn decision_cohort_complete_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round_number: i64,
) -> Result<bool> {
    sqlx::query_scalar(
        "SELECT NOT EXISTS( \
             SELECT 1 \
             FROM meeting_round_decision_cohort cohort \
             WHERE cohort.community_id = $1 \
               AND cohort.session_id = $2 \
               AND cohort.round_number = $3 \
               AND NOT EXISTS( \
                 SELECT 1 FROM meeting_floor_claims claim \
                 WHERE claim.community_id = cohort.community_id \
                   AND claim.session_id = cohort.session_id \
                   AND claim.round_number = cohort.round_number \
                   AND claim.claimant_pubkey = cohort.participant_pubkey \
               ) \
               AND NOT EXISTS( \
                 SELECT 1 FROM meeting_floor_signals signal \
                 WHERE signal.community_id = cohort.community_id \
                   AND signal.session_id = cohort.session_id \
                   AND signal.round_number = cohort.round_number \
                   AND signal.participant_pubkey = cohort.participant_pubkey \
                   AND signal.action = 'pass' \
               ) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .fetch_one(tx.as_mut())
    .await
    .map_err(Into::into)
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
        settle_not_before: None,
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
        FloorPhase::Claiming => {
            let deadline_due = round.claim_deadline.is_some_and(|deadline| now >= deadline);
            let cohort_due = round
                .settle_not_before
                .is_some_and(|boundary| now >= boundary)
                && decision_cohort_complete_tx(tx, community_id, session_id, round.round_number)
                    .await?;
            if !deadline_due && !cohort_due {
                return Ok(false);
            }
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
    let controls = load_control_sets_tx(tx, community_id, session_id, round.round_number).await?;
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
            "settle_not_before_ms": round
                .settle_not_before
                .map(|value| value.timestamp_millis()),
            "claim_deadline_ms": round
                .claim_deadline
                .map(|value| value.timestamp_millis()),
            "lease_expires_at_ms": lease_expires_at.timestamp_millis(),
            "claim_event_ids": claim_ids_hex(&claims),
            "claimants": claimant_hex(&claims),
            "decision_cohort": pubkeys_hex(&controls.decision_cohort_pubkeys),
            "ready": pubkeys_hex(&controls.ready_pubkeys),
            "passed": pubkeys_hex(&controls.passer_pubkeys),
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
        settle_not_before: round.settle_not_before,
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
         WHERE community_id = $1 AND session_id = $2 \
           AND schema_version = 1 AND floor_policy_version = $5",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(round_number)
    .bind(floor_revision)
    .bind(FLOOR_POLICY_VERSION)
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

async fn ensure_active_agent_participant(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    pubkey: &[u8],
) -> Result<()> {
    let agent: bool = sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
               AND role = 'bot' AND removed_at IS NULL \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(pubkey)
    .fetch_one(tx.as_mut())
    .await?;
    if !agent {
        return Err(DbError::AccessDenied(
            "only an Agent participant may submit Ready or Pass".to_string(),
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

async fn ensure_actor_security_active_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    actor_pubkey: &[u8],
) -> Result<()> {
    let durably_revoked = crate::meeting_revocation::actor_durably_revoked_for_session_tx(
        tx,
        community_id,
        session_id,
        actor_pubkey,
    )
    .await?;
    if !durably_revoked
        && crate::meeting::actor_security_active_tx(tx, community_id, actor_pubkey).await?
    {
        Ok(())
    } else {
        Err(DbError::AccessDenied(
            "meeting actor is no longer an active writable community principal".to_string(),
        ))
    }
}

fn participant_revoked_error() -> DbError {
    DbError::AccessDenied(
        "meeting ended because a participant authorization was revoked".to_string(),
    )
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

fn validate_intent_signal_event(
    event: &Event,
    action: FloorSignalAction,
    intent_basis: &str,
) -> Result<()> {
    if event.kind.as_u16() as u32 != buzz_core::kind::KIND_MEETING_FLOOR_SIGNAL {
        return Err(DbError::InvalidData(
            "meeting floor signal has the wrong event kind".to_string(),
        ));
    }
    if !event.content.is_empty() {
        return Err(DbError::InvalidData(
            "meeting floor signal content must be empty".to_string(),
        ));
    }
    if intent_basis.is_empty()
        || intent_basis.len() > 512
        || intent_basis.trim() != intent_basis
        || intent_basis.chars().any(char::is_control)
    {
        return Err(DbError::InvalidData(
            "meeting intent basis must be 1-512 bytes without surrounding whitespace or control characters"
                .to_string(),
        ));
    }
    if event_tag_values(event, "action").as_slice() != [action.as_str()] {
        return Err(DbError::InvalidData(
            "meeting floor signal has the wrong action tag".to_string(),
        ));
    }
    if event_tag_values(event, "intent-basis").as_slice() != [intent_basis] {
        return Err(DbError::InvalidData(
            "meeting floor signal has the wrong intent basis".to_string(),
        ));
    }
    if !event_tag_values(event, "meeting-grant").is_empty() {
        return Err(DbError::InvalidData(
            "Ready and Pass must not reference a meeting Grant".to_string(),
        ));
    }
    Ok(())
}

fn validate_yield_event(event: &Event, grant_event_id: &[u8]) -> Result<()> {
    if event.kind.as_u16() as u32 != buzz_core::kind::KIND_MEETING_FLOOR_SIGNAL {
        return Err(DbError::InvalidData(
            "meeting Yield has the wrong event kind".to_string(),
        ));
    }
    if !event.content.is_empty() {
        return Err(DbError::InvalidData(
            "meeting Yield content must be empty".to_string(),
        ));
    }
    if event_tag_values(event, "action").as_slice() != ["yield"] {
        return Err(DbError::InvalidData(
            "meeting Yield has the wrong action tag".to_string(),
        ));
    }
    if !event_tag_values(event, "intent-basis").is_empty() {
        return Err(DbError::InvalidData(
            "meeting Yield must not contain an intent basis".to_string(),
        ));
    }
    if event_tag_values(event, "meeting-grant").as_slice() != [hex::encode(grant_event_id)] {
        return Err(DbError::InvalidData(
            "meeting Yield has the wrong Grant tag".to_string(),
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

fn validate_floor_config(config: FloorConfig) -> Result<()> {
    validate_duration(config.claim_settle_delay, "meeting Claim settle delay")?;
    validate_duration(config.claim_window, "meeting Claim window")?;
    validate_duration(config.grant_lease, "meeting Grant lease")?;
    if config.claim_settle_delay > config.claim_window {
        return Err(DbError::InvalidData(
            "meeting Claim settle delay must not exceed the Claim window".to_string(),
        ));
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

fn pubkeys_hex(pubkeys: &[Vec<u8>]) -> Vec<String> {
    pubkeys.iter().map(hex::encode).collect()
}

fn round_state_content(
    round: &RoundRow,
    claims: &[CanonicalClaim],
    controls: &FloorControlSets,
) -> serde_json::Value {
    let mut content = serde_json::json!({
        "claim_event_ids": claim_ids_hex(claims),
        "claimants": claimant_hex(claims),
        "decision_cohort": pubkeys_hex(&controls.decision_cohort_pubkeys),
        "ready": pubkeys_hex(&controls.ready_pubkeys),
        "passed": pubkeys_hex(&controls.passer_pubkeys),
    });
    if let Some(settle_not_before) = round.settle_not_before {
        content["settle_not_before_ms"] = serde_json::json!(settle_not_before.timestamp_millis());
    }
    if let Some(claim_deadline) = round.claim_deadline {
        content["claim_deadline_ms"] = serde_json::json!(claim_deadline.timestamp_millis());
    }
    content
}

async fn snapshot_from_round(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    round: RoundRow,
) -> Result<FloorSnapshot> {
    let claims = load_claims_tx(tx, community_id, session_id, round.round_number).await?;
    let controls = load_control_sets_tx(tx, community_id, session_id, round.round_number).await?;
    Ok(snapshot(round, session_id, claims, controls))
}

async fn snapshot_from_round_pool(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    round: RoundRow,
) -> Result<FloorSnapshot> {
    let claims = load_claims_pool(db, community_id, session_id, round.round_number).await?;
    let controls = load_control_sets_pool(db, community_id, session_id, round.round_number).await?;
    Ok(snapshot(round, session_id, claims, controls))
}

fn snapshot(
    round: RoundRow,
    session_id: Uuid,
    claims: Vec<CanonicalClaim>,
    controls: FloorControlSets,
) -> FloorSnapshot {
    FloorSnapshot {
        session_id,
        round_number: round.round_number,
        floor_revision: round.floor_revision,
        phase: round.phase,
        state_event_id: round.state_event_id,
        settle_not_before: round.settle_not_before,
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
        decision_cohort_pubkeys: controls.decision_cohort_pubkeys,
        ready_pubkeys: controls.ready_pubkeys,
        passer_pubkeys: controls.passer_pubkeys,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Kind, Tag};
    use sqlx::PgPool;

    // Postgres-backed Meeting tests intentionally share one isolated database.
    // Sweep every Session created by that test process so an earlier test's
    // due floor cannot consume a small batch and starve the Session under test.
    const TEST_SWEEP_LIMIT: i64 = 10_000;

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
                "INSERT INTO relay_members (community_id, pubkey, role) \
                 VALUES ($1, $2, 'member')",
            )
            .bind(community_uuid)
            .bind(participant.public_key().to_hex())
            .execute(&pool)
            .await
            .expect("insert floor test relay member");
            sqlx::query(
                "INSERT INTO users (community_id, pubkey, agent_owner_pubkey) \
                 VALUES ($1, $2, $3)",
            )
            .bind(community_uuid)
            .bind(participant.public_key().as_bytes())
            .bind((index != 0).then_some(host.as_slice()))
            .execute(&pool)
            .await
            .expect("insert floor test authoritative identity");
            sqlx::query(
                "INSERT INTO channel_members \
                     (community_id, channel_id, pubkey, role, invited_by) \
                 VALUES ($1, $2, $3, $4::member_role, $5)",
            )
            .bind(community_uuid)
            .bind(session_id)
            .bind(participant.public_key().as_bytes())
            .bind(if index == 0 { "owner" } else { "bot" })
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

    fn intent_signal_event(
        keys: &Keys,
        session_id: Uuid,
        round_number: i64,
        action: &str,
        intent_basis: &str,
    ) -> Event {
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_MEETING_FLOOR_SIGNAL as u16),
            "",
        )
        .tags([
            Tag::parse(["h", &session_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", &round_number.to_string()]).expect("round tag"),
            Tag::parse(["action", action]).expect("action tag"),
            Tag::parse(["intent-basis", intent_basis]).expect("basis tag"),
        ])
        .sign_with_keys(keys)
        .expect("sign intent signal")
    }

    fn yield_event(keys: &Keys, session_id: Uuid, round_number: i64, grant_id: &[u8]) -> Event {
        EventBuilder::new(
            Kind::Custom(buzz_core::kind::KIND_MEETING_FLOOR_SIGNAL as u16),
            "",
        )
        .tags([
            Tag::parse(["h", &session_id.to_string()]).expect("h tag"),
            Tag::parse(["meeting-round", &round_number.to_string()]).expect("round tag"),
            Tag::parse(["action", "yield"]).expect("action tag"),
            Tag::parse(["meeting-grant", &hex::encode(grant_id)]).expect("grant tag"),
        ])
        .sign_with_keys(keys)
        .expect("sign Yield")
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn late_v0_write_commits_lazy_owner_deactivation_end_without_the_command() {
        let (db, community_id, session_id, relay_keys, participants) = setup_meeting().await;
        let owner_pubkey = participants[0].public_key();
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(owner_pubkey.as_bytes())
        .execute(&db.pool)
        .await
        .expect("deactivate authoritative Agent owner");

        let late_claim = claim_event(&participants[1], session_id, 1);
        let error = claim_floor(
            &db,
            community_id,
            session_id,
            1,
            &late_claim,
            &relay_keys,
            FloorConfig::default(),
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect_err("late V0 command must be rejected after lazy termination");
        assert!(matches!(
            error,
            DbError::AccessDenied(message)
                if message == "meeting ended because a participant authorization was revoked"
        ));

        let persisted_late_command: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM events WHERE community_id = $1 AND id = $2 \
             )",
        )
        .bind(community_id.as_uuid())
        .bind(late_claim.id.as_bytes())
        .fetch_one(&db.pool)
        .await
        .expect("check rejected command persistence");
        assert!(
            !persisted_late_command,
            "the command that discovered revocation must not enter the event log"
        );

        let status: String = sqlx::query_scalar(
            "SELECT status FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&db.pool)
        .await
        .expect("load lazily-ended V0 Meeting");
        assert_eq!(status, "ended");
        let terminal_outcome: String = sqlx::query_scalar(
            "SELECT outcome FROM meeting_rounds \
             WHERE community_id = $1 AND session_id = $2 AND round_number = 1",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&db.pool)
        .await
        .expect("load terminal V0 floor");
        assert_eq!(terminal_outcome, "ended");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn v0_sweeper_ends_revoked_roster_before_a_due_grant_transition() {
        let (db, community_id, session_id, relay_keys, participants) = setup_meeting().await;
        let config = FloorConfig {
            claim_settle_delay: StdDuration::from_millis(1),
            claim_window: StdDuration::from_secs(2),
            grant_lease: StdDuration::from_secs(2),
        };
        let claim = claim_event(&participants[1], session_id, 1);
        claim_floor(
            &db,
            community_id,
            session_id,
            1,
            &claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect("open a due V0 Claim round");
        tokio::time::sleep(StdDuration::from_millis(5)).await;
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(participants[0].public_key().as_bytes())
        .execute(&db.pool)
        .await
        .expect("deactivate authoritative Agent owner");

        assert!(
            recover_due_floors(
                &db,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
                TEST_SWEEP_LIMIT,
            )
            .await
            .expect("recover due V0 floor")
                >= 1
        );
        let round = sqlx::query(
            "SELECT phase, outcome, grant_event_id \
             FROM meeting_rounds \
             WHERE community_id = $1 AND session_id = $2 AND round_number = 1",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&db.pool)
        .await
        .expect("load security-terminal round");
        assert_eq!(
            round.try_get::<String, _>("phase").expect("phase"),
            "closed"
        );
        assert_eq!(
            round
                .try_get::<Option<String>, _>("outcome")
                .expect("outcome")
                .as_deref(),
            Some("ended")
        );
        assert!(
            round
                .try_get::<Option<Vec<u8>>, _>("grant_event_id")
                .expect("Grant event id")
                .is_none(),
            "deadline recovery must not publish a Grant before security End"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn agent_cohort_settles_early_and_yield_opens_the_next_round() {
        let (db, community_id, session_id, relay_keys, participants) = setup_meeting().await;
        let config = FloorConfig {
            claim_settle_delay: StdDuration::from_millis(30),
            claim_window: StdDuration::from_secs(2),
            grant_lease: StdDuration::from_secs(2),
        };
        let basis_a = "speech:first";
        let basis_b = "speech:second";
        let ready_a = intent_signal_event(&participants[1], session_id, 1, "ready", basis_a);
        let ready_b = intent_signal_event(&participants[2], session_id, 1, "ready", basis_b);
        assert!(matches!(
            signal_intent(
                &db,
                community_id,
                session_id,
                1,
                FloorSignalAction::Ready,
                basis_a,
                &ready_a,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("Ready A"),
            FloorSignalOutcome::Accepted { .. }
        ));
        signal_intent(
            &db,
            community_id,
            session_id,
            1,
            FloorSignalAction::Ready,
            basis_b,
            &ready_b,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect("Ready B");

        let human_ready =
            intent_signal_event(&participants[0], session_id, 1, "ready", "activation:human");
        assert!(matches!(
            signal_intent(
                &db,
                community_id,
                session_id,
                1,
                FloorSignalAction::Ready,
                "activation:human",
                &human_ready,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await,
            Err(DbError::AccessDenied(_))
        ));

        let claim = claim_event(&participants[1], session_id, 1);
        claim_floor(
            &db,
            community_id,
            session_id,
            1,
            &claim,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect("Agent A Claim");
        let claiming = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("claiming floor");
        assert_eq!(claiming.phase, FloorPhase::Claiming);
        assert_eq!(claiming.decision_cohort_pubkeys.len(), 2);
        assert!(claiming
            .claim_deadline
            .zip(claiming.settle_not_before)
            .is_some_and(|(deadline, settle)| deadline > settle));

        let pass = intent_signal_event(&participants[2], session_id, 1, "pass", basis_b);
        signal_intent(
            &db,
            community_id,
            session_id,
            1,
            FloorSignalAction::Pass,
            basis_b,
            &pass,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
        )
        .await
        .expect("Agent B Pass");
        tokio::time::sleep(StdDuration::from_millis(40)).await;
        recover_due_floors(
            &db,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
            TEST_SWEEP_LIMIT,
        )
        .await
        .expect("settle complete cohort");
        let granted = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("granted floor");
        assert_eq!(granted.phase, FloorPhase::Granted);
        assert_eq!(
            granted.holder_pubkey.as_deref(),
            Some(participants[1].public_key().as_bytes().as_slice())
        );
        assert_eq!(granted.passer_pubkeys.len(), 1);

        let grant_id = granted.grant_event_id.expect("Grant ID");
        let yielded = yield_event(&participants[1], session_id, 1, &grant_id);
        assert!(matches!(
            yield_floor(
                &db,
                community_id,
                session_id,
                1,
                &grant_id,
                &yielded,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("Yield"),
            YieldOutcome::Accepted {
                next_round_number: 2,
                ..
            }
        ));
        assert!(matches!(
            yield_floor(
                &db,
                community_id,
                session_id,
                1,
                &grant_id,
                &yielded,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
            )
            .await
            .expect("duplicate Yield"),
            YieldOutcome::Duplicate { .. }
        ));
        let next = get_floor_snapshot(&db, community_id, session_id)
            .await
            .expect("next floor");
        assert_eq!(next.round_number, 2);
        assert_eq!(next.phase, FloorPhase::Open);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn outbox_blocks_later_session_events_behind_an_unfinished_predecessor() {
        let (db, community_id, session_id, relay_keys, _) = setup_meeting().await;
        let now = Utc::now();
        let mut tx = db.begin_transaction().await.expect("begin outbox setup");
        for content in ["second", "third"] {
            let event = EventBuilder::new(
                Kind::Custom(buzz_core::kind::KIND_MEETING_STATE as u16),
                content,
            )
            .tags([Tag::parse(["h", &session_id.to_string()]).expect("h tag")])
            .sign_with_keys(&relay_keys)
            .expect("sign outbox event");
            persist_meeting_event_tx(&mut tx, community_id, session_id, &event, now)
                .await
                .expect("persist outbox event");
        }
        tx.commit().await.expect("commit outbox setup");

        let expected_sequences: Vec<i64> = sqlx::query_scalar(
            "SELECT sequence FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2 \
             ORDER BY sequence",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_all(&db.pool)
        .await
        .expect("load target outbox sequence");
        assert_eq!(expected_sequences.len(), 3);

        let first_worker = Uuid::new_v4();
        let first_batch = claim_outbox_batch(&db, first_worker, StdDuration::from_secs(30), 10_000)
            .await
            .expect("claim first outbox event");
        let first_for_session: Vec<_> = first_batch
            .iter()
            .filter(|item| item.session_id == session_id)
            .collect();
        assert_eq!(first_for_session.len(), 1);
        assert_eq!(first_for_session[0].sequence, expected_sequences[0]);

        let concurrent_batch =
            claim_outbox_batch(&db, Uuid::new_v4(), StdDuration::from_secs(30), 10_000)
                .await
                .expect("claim while predecessor is held");
        assert!(
            concurrent_batch
                .iter()
                .all(|item| item.session_id != session_id),
            "a concurrent worker must not claim a later event for the same session"
        );

        assert!(release_outbox(
            &db,
            community_id,
            expected_sequences[0],
            first_worker,
            "simulated delivery failure",
        )
        .await
        .expect("release failed predecessor"));
        sqlx::query(
            "UPDATE meeting_event_outbox \
             SET available_at = clock_timestamp() + interval '1 hour' \
             WHERE community_id = $1 AND sequence = $2",
        )
        .bind(community_id.as_uuid())
        .bind(expected_sequences[0])
        .execute(&db.pool)
        .await
        .expect("hold predecessor in backoff");

        let backed_off_batch =
            claim_outbox_batch(&db, Uuid::new_v4(), StdDuration::from_secs(30), 10_000)
                .await
                .expect("claim while predecessor is backed off");
        assert!(
            backed_off_batch
                .iter()
                .all(|item| item.session_id != session_id),
            "a backed-off predecessor must continue to block later session events"
        );

        sqlx::query(
            "UPDATE meeting_event_outbox \
             SET available_at = clock_timestamp() \
             WHERE community_id = $1 AND sequence = $2",
        )
        .bind(community_id.as_uuid())
        .bind(expected_sequences[0])
        .execute(&db.pool)
        .await
        .expect("make predecessor retryable");
        let retry_worker = Uuid::new_v4();
        let retry_batch = claim_outbox_batch(&db, retry_worker, StdDuration::from_secs(30), 10_000)
            .await
            .expect("reclaim predecessor");
        let retry_for_session: Vec<_> = retry_batch
            .iter()
            .filter(|item| item.session_id == session_id)
            .collect();
        assert_eq!(retry_for_session.len(), 1);
        assert_eq!(retry_for_session[0].sequence, expected_sequences[0]);
        assert!(
            mark_outbox_delivered(&db, community_id, expected_sequences[0], retry_worker,)
                .await
                .expect("mark predecessor delivered")
        );

        let next_batch =
            claim_outbox_batch(&db, Uuid::new_v4(), StdDuration::from_secs(30), 10_000)
                .await
                .expect("claim successor");
        let next_for_session: Vec<_> = next_batch
            .iter()
            .filter(|item| item.session_id == session_id)
            .collect();
        assert_eq!(next_for_session.len(), 1);
        assert_eq!(next_for_session[0].sequence, expected_sequences[1]);
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn claim_grant_say_expiry_restart_and_idempotency_are_atomic() {
        let (db, community_id, session_id, relay_keys, participants) = setup_meeting().await;
        let config = FloorConfig {
            claim_settle_delay: StdDuration::from_millis(200),
            claim_window: StdDuration::from_millis(500),
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

        tokio::time::sleep(StdDuration::from_millis(550)).await;
        assert!(
            recover_due_floors(
                &db,
                &relay_keys,
                config,
                WinnerSelector::FixedIndex(0),
                TEST_SWEEP_LIMIT,
            )
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
        tokio::time::sleep(StdDuration::from_millis(550)).await;
        recover_due_floors(
            &db,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
            TEST_SWEEP_LIMIT,
        )
        .await
        .expect("grant Round 2");
        tokio::time::sleep(StdDuration::from_millis(2_050)).await;
        recover_due_floors(
            &db,
            &relay_keys,
            config,
            WinnerSelector::FixedIndex(0),
            TEST_SWEEP_LIMIT,
        )
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
        assert_eq!(
            outbox
                .iter()
                .filter(|item| item.session_id == session_id)
                .count(),
            1
        );
    }
}
