//! Durable Meeting termination after a real authorization revocation.
//!
//! Revocation producers atomically enqueue work with the authorization
//! mutation. This module lets a bounded Relay worker enumerate affected active
//! sessions and terminate each one in its own short, policy-aware transaction.

use buzz_core::{kind::KIND_MEETING_END, CommunityId};
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{Postgres, Row as _, Transaction};
use uuid::Uuid;

use crate::meeting_baton::{BATON_POLICY_VERSION, SCHEMA_VERSION};
use crate::meeting_floor::FLOOR_POLICY_VERSION;
use crate::meeting_v2::{BOARD_POLICY_VERSION, SCHEMA_VERSION as BOARD_SCHEMA_VERSION};
use crate::{Db, DbError, Result};

/// One active Meeting whose frozen roster contains a revoked identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokedMeetingSession {
    /// Meeting/channel identity.
    pub session_id: Uuid,
    /// Persisted protocol schema discriminator.
    pub schema_version: i32,
    /// Persisted speech-floor policy discriminator.
    pub floor_policy_version: String,
    /// Signed Meeting Create event referenced by the Relay-authored End.
    pub create_event_id: Vec<u8>,
}

/// Result of one idempotent security-revocation termination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevocationEndOutcome {
    /// The active Meeting was ended and its terminal events were queued.
    Ended {
        /// Relay-authored Meeting End event.
        end_event_id: Vec<u8>,
        /// Policy-specific terminal floor/baton State event.
        terminal_state_event_id: Vec<u8>,
    },
    /// A concurrent command or prior worker attempt already ended the Meeting.
    AlreadyEnded,
}

/// List active Meetings containing `revoked_pubkey`, ordered by Session UUID.
///
/// `revocation_security_order` fences the job to Meetings whose globally
/// monotonic security order predates the revocation, so reactivation followed
/// by a new Meeting is not terminated by an older job. `after_session_id` is
/// an exclusive cursor. The query is community-scoped and only inspects the
/// immutable Meeting roster, never ordinary Channel membership.
pub async fn list_revoked_participant_sessions(
    db: &Db,
    community_id: CommunityId,
    revoked_pubkey: &[u8],
    revocation_security_order: i64,
    after_session_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<RevokedMeetingSession>> {
    validate_32_bytes(revoked_pubkey, "revoked pubkey")?;
    if revocation_security_order <= 0 {
        return Err(DbError::InvalidData(
            "revocation security order must be positive".to_string(),
        ));
    }
    if limit <= 0 {
        return Ok(Vec::new());
    }
    let rows = sqlx::query(
        "SELECT ms.session_id, ms.schema_version, ms.floor_policy_version, \
                ms.create_event_id \
         FROM meeting_sessions ms \
         WHERE ms.community_id = $1 \
           AND ms.status = 'active' \
           AND ( \
             (( \
                (ms.schema_version = $6 AND ms.floor_policy_version = $7) \
                OR (ms.schema_version = $8 AND ms.floor_policy_version = $9) \
              ) AND EXISTS( \
               SELECT 1 FROM meeting_participants mp \
               WHERE mp.community_id = ms.community_id \
                 AND mp.session_id = ms.session_id AND mp.pubkey = $2 \
             )) \
             OR (ms.schema_version = $10 AND ms.floor_policy_version = $11 AND EXISTS( \
               SELECT 1 FROM channel_members cm \
               WHERE cm.community_id = ms.community_id \
                 AND cm.channel_id = ms.session_id AND cm.pubkey = $2 \
             )) \
           ) \
           AND ms.security_order < $3 \
           AND ($4::uuid IS NULL OR ms.session_id > $4) \
         ORDER BY ms.session_id \
         LIMIT $5",
    )
    .bind(community_id.as_uuid())
    .bind(revoked_pubkey)
    .bind(revocation_security_order)
    .bind(after_session_id)
    .bind(limit)
    .bind(SCHEMA_VERSION)
    .bind(BATON_POLICY_VERSION)
    .bind(BOARD_SCHEMA_VERSION)
    .bind(BOARD_POLICY_VERSION)
    .bind(1_i32)
    .bind(FLOOR_POLICY_VERSION)
    .fetch_all(&db.pool)
    .await?;

    rows.into_iter()
        .map(|row| {
            Ok(RevokedMeetingSession {
                session_id: row.try_get("session_id")?,
                schema_version: row.try_get("schema_version")?,
                floor_policy_version: row.try_get("floor_policy_version")?,
                create_event_id: row.try_get("create_event_id")?,
            })
        })
        .collect()
}

/// Idempotently end one Meeting because a frozen participant lost access.
///
/// The function serializes on `meeting_sessions`, verifies the revoked identity
/// belongs to the frozen roster, emits a Relay-signed
/// `reason=participant_revoked` End, closes the persisted V0 or V1 state
/// machine, archives the Meeting Channel, and queues both events atomically.
pub async fn end_meeting_for_revocation(
    db: &Db,
    community_id: CommunityId,
    session_id: Uuid,
    revoked_pubkey: &[u8],
    revocation_event_id: &[u8],
    relay_keys: &Keys,
) -> Result<RevocationEndOutcome> {
    if session_id.is_nil() {
        return Err(DbError::InvalidData(
            "meeting session id must not be nil".to_string(),
        ));
    }
    validate_32_bytes(revoked_pubkey, "revoked pubkey")?;
    validate_32_bytes(revocation_event_id, "revocation event id")?;

    let mut tx = db.begin_transaction().await?;
    let row = sqlx::query(
        "SELECT create_event_id, schema_version, floor_policy_version, status \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2 \
         FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;

    let session = LockedMeetingSession::from_row(row, session_id)?;
    if session.status == "ended" {
        tx.rollback().await?;
        return Ok(RevocationEndOutcome::AlreadyEnded);
    }
    if session.status != "active" {
        return Err(DbError::InvalidData(format!(
            "unknown meeting status: {}",
            session.status
        )));
    }
    let belongs_to_roster = revoked_identity_belongs_to_roster_tx(
        &mut tx,
        community_id,
        session_id,
        revoked_pubkey,
        session.protocol,
    )
    .await?;
    if !belongs_to_roster {
        return Err(DbError::AccessDenied(
            "revoked identity is not in the meeting roster".to_string(),
        ));
    }
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    let ended = end_meeting_for_revocation_locked_tx(
        &mut tx,
        community_id,
        session_id,
        revoked_pubkey,
        relay_keys,
        &session,
        now,
    )
    .await?;

    tx.commit().await?;
    Ok(RevocationEndOutcome::Ended {
        end_event_id: ended.end_event_id,
        terminal_state_event_id: ended.terminal_state_event_id,
    })
}

async fn revoked_identity_belongs_to_roster_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    revoked_pubkey: &[u8],
    protocol: RevocationProtocol,
) -> Result<bool> {
    let query = match protocol {
        RevocationProtocol::ModeratedBatonV1 | RevocationProtocol::ModeratedBoardV2 => {
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_participants \
                 WHERE community_id = $1 AND session_id = $2 AND pubkey = $3 \
             )"
        }
        // V0 projected its immutable meeting roster into channel_members.
        // A later ordinary Channel removal only soft-deletes the row, so
        // removed_at must deliberately not filter security-revocation lookup.
        RevocationProtocol::UniformV0 => {
            "SELECT EXISTS( \
                 SELECT 1 FROM channel_members \
                 WHERE community_id = $1 AND channel_id = $2 AND pubkey = $3 \
             )"
        }
    };
    sqlx::query_scalar(query)
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(revoked_pubkey)
        .fetch_one(tx.as_mut())
        .await
        .map_err(DbError::from)
}

async fn lock_roster_security_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    protocol: RevocationProtocol,
) -> Result<()> {
    let roster_query = match protocol {
        RevocationProtocol::ModeratedBatonV1 | RevocationProtocol::ModeratedBoardV2 => {
            "SELECT pubkey FROM meeting_participants \
             WHERE community_id = $1 AND session_id = $2 \
             ORDER BY pubkey"
        }
        RevocationProtocol::UniformV0 => {
            "SELECT pubkey FROM channel_members \
             WHERE community_id = $1 AND channel_id = $2 \
             ORDER BY pubkey"
        }
    };
    let roster: Vec<Vec<u8>> = sqlx::query_scalar(roster_query)
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_all(tx.as_mut())
        .await?;

    // `agent_owner_pubkey` is assigned when the Agent identity is created and
    // is immutable thereafter. Read that mapping before taking identity row
    // locks so roster identities and their owners can be merged into one
    // globally ordered lock set; locking them in separate batches permits an
    // X-owned-by-Y / Y-owned-by-X cross-Session deadlock.
    for pubkey in &roster {
        validate_32_bytes(pubkey, "frozen roster pubkey")?;
    }
    let mut owners: Vec<Vec<u8>> = sqlx::query_scalar(
        "SELECT agent_owner_pubkey FROM users \
         WHERE community_id = $1 AND pubkey = ANY($2::bytea[]) \
           AND agent_owner_pubkey IS NOT NULL \
         ORDER BY pubkey",
    )
    .bind(community_id.as_uuid())
    .bind(&roster)
    .fetch_all(tx.as_mut())
    .await?;
    for owner in &owners {
        validate_32_bytes(owner, "frozen Agent owner pubkey")?;
    }
    owners.sort();
    owners.dedup();
    let mut identity_principals = roster.clone();
    identity_principals.extend(owners);
    identity_principals.sort();
    identity_principals.dedup();

    // Match producer lock order. Lock every frozen participant membership
    // before any authoritative user row so a command and a concurrent
    // membership delete/ban have one deterministic linearization point.
    // PostgreSQL's LockRows node consumes the explicitly sorted rows, making
    // lock acquisition order deterministic within each resource class.
    let roster_hex: Vec<String> = roster.iter().map(hex::encode).collect();
    sqlx::query_scalar::<_, String>(
        "SELECT pubkey FROM relay_members \
         WHERE community_id = $1 AND pubkey = ANY($2::text[]) \
         ORDER BY pubkey \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(&roster_hex)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT pubkey FROM users \
         WHERE community_id = $1 AND pubkey = ANY($2::bytea[]) \
         ORDER BY pubkey \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(&identity_principals)
    .fetch_all(tx.as_mut())
    .await?;
    sqlx::query_scalar::<_, Vec<u8>>(
        "SELECT pubkey FROM community_bans \
         WHERE community_id = $1 AND pubkey = ANY($2::bytea[]) \
         ORDER BY pubkey \
         FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(&identity_principals)
    .fetch_all(tx.as_mut())
    .await?;
    Ok(())
}

/// Check a locked active moderated roster for a real loss of authorization and
/// end the Meeting in the caller's transaction when one is found.
///
/// The caller must already hold the `meeting_sessions` row lock and must invoke
/// this before persisting the incoming participant command. Identity archival
/// is intentionally absent: only relay membership removal, authoritative user
/// deactivation/deletion, and an active ban count as security revocation.
pub async fn recover_revoked_roster_v1_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
) -> Result<Option<crate::meeting_baton::BatonSnapshot>> {
    let row = sqlx::query(
        "SELECT create_event_id, schema_version, floor_policy_version, status \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let session = LockedMeetingSession::from_row(row, session_id)?;
    if !matches!(session.status.as_str(), "active" | "ended") {
        return Err(DbError::InvalidData(format!(
            "unknown meeting status: {}",
            session.status
        )));
    }
    if !matches!(
        session.protocol,
        RevocationProtocol::ModeratedBatonV1 | RevocationProtocol::ModeratedBoardV2
    ) {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a moderated Meeting session"
        )));
    }

    lock_roster_security_tx(tx, community_id, session_id, session.protocol).await?;
    if session.status == "ended" {
        return Ok(None);
    }
    let revoked_pubkey = find_revoked_v1_roster_pubkey_tx(tx, community_id, session_id).await?;
    let Some(revoked_pubkey) = revoked_pubkey else {
        return Ok(None);
    };
    validate_32_bytes(&revoked_pubkey, "revoked roster pubkey")?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    let ended = end_meeting_for_revocation_locked_tx(
        tx,
        community_id,
        session_id,
        &revoked_pubkey,
        relay_keys,
        &session,
        now,
    )
    .await?;
    ended.baton_snapshot.map(Some).ok_or_else(|| {
        DbError::InvalidData("moderated revocation did not produce a Baton snapshot".to_string())
    })
}

/// Check a locked active V0 roster for a real loss of authorization and end
/// the Meeting in the caller's transaction when one is found.
///
/// V0's immutable roster is the complete set of `channel_members` rows,
/// including rows soft-removed by ordinary Channel membership operations.
/// The caller must already hold the `meeting_sessions` row lock and must commit
/// the returned terminal transition before rejecting the late write.
pub async fn recover_revoked_roster_v0_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    relay_keys: &Keys,
) -> Result<bool> {
    let row = sqlx::query(
        "SELECT create_event_id, schema_version, floor_policy_version, status \
         FROM meeting_sessions \
         WHERE community_id = $1 AND session_id = $2",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("meeting {session_id}")))?;
    let session = LockedMeetingSession::from_row(row, session_id)?;
    if !matches!(session.status.as_str(), "active" | "ended") {
        return Err(DbError::InvalidData(format!(
            "unknown meeting status: {}",
            session.status
        )));
    }
    if !matches!(session.protocol, RevocationProtocol::UniformV0) {
        return Err(DbError::InvalidData(format!(
            "meeting {session_id} is not a {FLOOR_POLICY_VERSION} session"
        )));
    }

    lock_roster_security_tx(tx, community_id, session_id, RevocationProtocol::UniformV0).await?;
    if session.status == "ended" {
        return Ok(false);
    }
    let revoked_pubkey = find_revoked_v0_roster_pubkey_tx(tx, community_id, session_id).await?;
    let Some(revoked_pubkey) = revoked_pubkey else {
        return Ok(false);
    };
    validate_32_bytes(&revoked_pubkey, "revoked roster pubkey")?;
    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(tx.as_mut())
        .await?;
    end_meeting_for_revocation_locked_tx(
        tx,
        community_id,
        session_id,
        &revoked_pubkey,
        relay_keys,
        &session,
        now,
    )
    .await?;
    Ok(true)
}

pub(crate) async fn actor_durably_revoked_for_session_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    actor_pubkey: &[u8],
) -> Result<bool> {
    validate_32_bytes(actor_pubkey, "Meeting actor pubkey")?;
    sqlx::query_scalar(
        "SELECT EXISTS( \
             SELECT 1 \
             FROM meeting_revocation_jobs j \
             JOIN meeting_sessions ms \
               ON ms.community_id = j.community_id \
              AND ms.session_id = $2 \
             WHERE j.community_id = $1 \
               AND j.revoked_pubkey = $3 \
               AND j.security_order > ms.security_order \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(actor_pubkey)
    .fetch_one(tx.as_mut())
    .await
    .map_err(DbError::from)
}

async fn find_revoked_v1_roster_pubkey_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<Vec<u8>>> {
    sqlx::query_scalar(
        "WITH roster_security AS ( \
             SELECT mp.pubkey, \
                    EXISTS( \
                      SELECT 1 FROM meeting_revocation_jobs j \
                      WHERE j.community_id = mp.community_id \
                        AND j.revoked_pubkey = mp.pubkey \
                        AND j.security_order > ms.security_order \
                    ) AS durable_revocation, \
                    (rm.pubkey IS NULL \
                     OR u.pubkey IS NULL \
                     OR u.deactivated_at IS NOT NULL \
                     OR (b.banned AND ( \
                         b.ban_expires_at IS NULL \
                         OR b.ban_expires_at > clock_timestamp() \
                     )) \
                     OR (u.agent_owner_pubkey IS NOT NULL AND ( \
                         owner_u.pubkey IS NULL \
                         OR owner_u.deactivated_at IS NOT NULL \
                         OR (owner_ban.banned AND ( \
                             owner_ban.ban_expires_at IS NULL \
                             OR owner_ban.ban_expires_at > clock_timestamp() \
                         )) \
                     ))) AS current_revocation \
             FROM meeting_participants mp \
             JOIN meeting_sessions ms \
               ON ms.community_id = mp.community_id \
              AND ms.session_id = mp.session_id \
         LEFT JOIN relay_members rm \
           ON rm.community_id = mp.community_id \
          AND rm.pubkey = encode(mp.pubkey, 'hex') \
         LEFT JOIN users u \
           ON u.community_id = mp.community_id AND u.pubkey = mp.pubkey \
         LEFT JOIN users owner_u \
           ON owner_u.community_id = mp.community_id \
          AND owner_u.pubkey = u.agent_owner_pubkey \
         LEFT JOIN community_bans b \
           ON b.community_id = mp.community_id AND b.pubkey = mp.pubkey \
         LEFT JOIN community_bans owner_ban \
           ON owner_ban.community_id = mp.community_id \
          AND owner_ban.pubkey = u.agent_owner_pubkey \
             WHERE mp.community_id = $1 AND mp.session_id = $2 \
         ) \
         SELECT pubkey FROM roster_security \
         WHERE durable_revocation OR current_revocation \
         ORDER BY durable_revocation DESC, pubkey \
         LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::from)
}

async fn find_revoked_v0_roster_pubkey_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
) -> Result<Option<Vec<u8>>> {
    sqlx::query_scalar(
        "WITH roster_security AS ( \
             SELECT cm.pubkey, \
                    EXISTS( \
                      SELECT 1 FROM meeting_revocation_jobs j \
                      WHERE j.community_id = cm.community_id \
                        AND j.revoked_pubkey = cm.pubkey \
                        AND j.security_order > ms.security_order \
                    ) AS durable_revocation, \
                    (rm.pubkey IS NULL \
                     OR u.pubkey IS NULL \
                     OR u.deactivated_at IS NOT NULL \
                     OR (b.banned AND ( \
                         b.ban_expires_at IS NULL \
                         OR b.ban_expires_at > clock_timestamp() \
                     )) \
                     OR (cm.role = 'bot' \
                         AND u.agent_owner_pubkey IS NOT NULL AND ( \
                           owner_u.pubkey IS NULL \
                           OR owner_u.deactivated_at IS NOT NULL \
                           OR (owner_ban.banned AND ( \
                               owner_ban.ban_expires_at IS NULL \
                               OR owner_ban.ban_expires_at > clock_timestamp() \
                           )) \
                         ))) AS current_revocation \
             FROM channel_members cm \
             JOIN meeting_sessions ms \
               ON ms.community_id = cm.community_id \
              AND ms.session_id = cm.channel_id \
         LEFT JOIN relay_members rm \
           ON rm.community_id = cm.community_id \
          AND rm.pubkey = encode(cm.pubkey, 'hex') \
         LEFT JOIN users u \
           ON u.community_id = cm.community_id AND u.pubkey = cm.pubkey \
         LEFT JOIN users owner_u \
           ON owner_u.community_id = cm.community_id \
          AND owner_u.pubkey = u.agent_owner_pubkey \
         LEFT JOIN community_bans b \
           ON b.community_id = cm.community_id AND b.pubkey = cm.pubkey \
         LEFT JOIN community_bans owner_ban \
           ON owner_ban.community_id = cm.community_id \
          AND owner_ban.pubkey = u.agent_owner_pubkey \
             WHERE cm.community_id = $1 AND cm.channel_id = $2 \
         ) \
         SELECT pubkey FROM roster_security \
         WHERE durable_revocation OR current_revocation \
         ORDER BY durable_revocation DESC, pubkey \
         LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .fetch_optional(tx.as_mut())
    .await
    .map_err(DbError::from)
}

struct LockedMeetingSession {
    create_event_id: Vec<u8>,
    status: String,
    protocol: RevocationProtocol,
}

impl LockedMeetingSession {
    fn from_row(row: sqlx::postgres::PgRow, session_id: Uuid) -> Result<Self> {
        let create_event_id: Vec<u8> = row.try_get("create_event_id")?;
        validate_32_bytes(&create_event_id, "meeting create event id")?;
        let schema_version: i32 = row.try_get("schema_version")?;
        let floor_policy_version: String = row.try_get("floor_policy_version")?;
        Ok(Self {
            create_event_id,
            status: row.try_get("status")?,
            protocol: RevocationProtocol::parse(schema_version, &floor_policy_version, session_id)?,
        })
    }
}

struct LockedRevocationEnd {
    end_event_id: Vec<u8>,
    terminal_state_event_id: Vec<u8>,
    baton_snapshot: Option<crate::meeting_baton::BatonSnapshot>,
}

#[allow(clippy::too_many_arguments)]
async fn end_meeting_for_revocation_locked_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    session_id: Uuid,
    revoked_pubkey: &[u8],
    relay_keys: &Keys,
    session: &LockedMeetingSession,
    now: DateTime<Utc>,
) -> Result<LockedRevocationEnd> {
    if matches!(session.protocol, RevocationProtocol::ModeratedBoardV2) {
        crate::meeting_v2::ensure_runtime_initialized_tx(
            tx,
            community_id,
            session_id,
            relay_keys,
            now,
        )
        .await?;
    }
    let end_event = build_revocation_end_event(
        relay_keys,
        session.protocol,
        session_id,
        &session.create_event_id,
        revoked_pubkey,
        now,
    )?;
    persist_end_event_tx(tx, community_id, session_id, &end_event, now).await?;
    crate::meeting::enqueue_meeting_event_tx(tx, community_id, session_id, end_event.id.as_bytes())
        .await?;

    let ended_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE meeting_sessions \
         SET status = 'ended', ended_at = $3, ended_by = $4, end_event_id = $5, \
             terminal_outcome = CASE WHEN schema_version = $6 THEN 'aborted' ELSE NULL END, \
             terminal_reason_code = CASE WHEN schema_version = $6 \
                 THEN 'participant_revoked' ELSE NULL END \
         WHERE community_id = $1 AND session_id = $2 AND status = 'active' \
         RETURNING ended_at",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(now)
    .bind(relay_keys.public_key().as_bytes())
    .bind(end_event.id.as_bytes())
    .bind(BOARD_SCHEMA_VERSION)
    .fetch_one(tx.as_mut())
    .await?;
    let archived = sqlx::query(
        "UPDATE channels \
         SET archived_at = $3, updated_at = $3 \
         WHERE community_id = $1 AND id = $2 \
           AND room_kind = 'meeting' AND archived_at IS NULL AND deleted_at IS NULL",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(ended_at)
    .execute(tx.as_mut())
    .await?;
    if archived.rows_affected() != 1 {
        return Err(DbError::InvalidData(
            "meeting channel is missing or not active".to_string(),
        ));
    }

    let (terminal_state_event_id, baton_snapshot) = match session.protocol {
        RevocationProtocol::UniformV0 => (
            crate::meeting_floor::close_floor_for_end_tx(tx, community_id, session_id, relay_keys)
                .await?
                .state_event_id,
            None,
        ),
        RevocationProtocol::ModeratedBatonV1 | RevocationProtocol::ModeratedBoardV2 => {
            if matches!(session.protocol, RevocationProtocol::ModeratedBoardV2) {
                sqlx::query(
                    "UPDATE meeting_v2_bootstrap_state \
                     SET runtime_phase = 'ended', board_deadline_at = NULL, \
                         board_completed_at = COALESCE(board_completed_at, $3), \
                         board_outcome = CASE WHEN runtime_phase = 'board_pending' \
                             THEN 'preempted' ELSE board_outcome END, \
                         terminal_outcome = 'aborted', \
                         terminal_reason_code = 'participant_revoked', \
                         terminal_at = $3, updated_at = $3 \
                     WHERE community_id = $1 AND session_id = $2",
                )
                .bind(community_id.as_uuid())
                .bind(session_id)
                .bind(ended_at)
                .execute(tx.as_mut())
                .await?;
            }
            let snapshot = crate::meeting_baton::close_baton_for_security_revocation_tx(
                tx,
                community_id,
                session_id,
                end_event.id.as_bytes(),
                relay_keys,
                ended_at,
            )
            .await?;
            crate::meeting::enqueue_meeting_event_tx(
                tx,
                community_id,
                session_id,
                &snapshot.state_event_id,
            )
            .await?;
            (snapshot.state_event_id.clone(), Some(snapshot))
        }
    };
    Ok(LockedRevocationEnd {
        end_event_id: end_event.id.as_bytes().to_vec(),
        terminal_state_event_id,
        baton_snapshot,
    })
}

#[derive(Debug, Clone, Copy)]
enum RevocationProtocol {
    UniformV0,
    ModeratedBatonV1,
    ModeratedBoardV2,
}

impl RevocationProtocol {
    fn parse(schema_version: i32, policy: &str, session_id: Uuid) -> Result<Self> {
        match (schema_version, policy) {
            (1, FLOOR_POLICY_VERSION) => Ok(Self::UniformV0),
            (SCHEMA_VERSION, BATON_POLICY_VERSION) => Ok(Self::ModeratedBatonV1),
            (BOARD_SCHEMA_VERSION, BOARD_POLICY_VERSION) => Ok(Self::ModeratedBoardV2),
            _ => Err(DbError::InvalidData(format!(
                "meeting {session_id} has unsupported protocol {schema_version}/{policy}"
            ))),
        }
    }
}

fn build_revocation_end_event(
    relay_keys: &Keys,
    protocol: RevocationProtocol,
    session_id: Uuid,
    create_event_id: &[u8],
    revoked_pubkey: &[u8],
    now: DateTime<Utc>,
) -> Result<Event> {
    let session_id = session_id.to_string();
    let create_event_id = hex::encode(create_event_id);
    let revoked_pubkey = hex::encode(revoked_pubkey);
    let mut tags = vec![
        parse_tag(["h", session_id.as_str()])?,
        parse_tag(["e", create_event_id.as_str()])?,
        parse_tag(["reason", "participant_revoked"])?,
        parse_tag(["p", revoked_pubkey.as_str()])?,
    ];
    match protocol {
        RevocationProtocol::UniformV0 => {}
        RevocationProtocol::ModeratedBatonV1 => {
            tags.insert(1, parse_tag(["v", "2"])?);
            tags.insert(2, parse_tag(["policy", BATON_POLICY_VERSION])?);
        }
        RevocationProtocol::ModeratedBoardV2 => {
            tags = vec![
                parse_tag(["h", session_id.as_str()])?,
                parse_tag(["v", buzz_sdk::MEETING_V2_SCHEMA_VERSION])?,
                parse_tag(["policy", BOARD_POLICY_VERSION])?,
                parse_tag(["e", create_event_id.as_str()])?,
                parse_tag(["outcome", "aborted"])?,
                parse_tag(["reason-code", "participant_revoked"])?,
                parse_tag(["p", revoked_pubkey.as_str()])?,
            ];
        }
    }
    let timestamp =
        u64::try_from(now.timestamp()).map_err(|_| DbError::InvalidTimestamp(now.timestamp()))?;
    EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), "")
        .tags(tags)
        .custom_created_at(Timestamp::from(timestamp))
        .sign_with_keys(relay_keys)
        .map_err(|error| DbError::InvalidData(format!("sign revoked Meeting End: {error}")))
}

async fn persist_end_event_tx(
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
    .bind(event.id.as_bytes())
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
            "revoked Meeting End {} already exists without its terminal projection",
            event.id
        )));
    }
    Ok(())
}

fn parse_tag<const N: usize>(values: [&str; N]) -> Result<Tag> {
    Tag::parse(values).map_err(|error| DbError::InvalidData(format!("build Meeting tag: {error}")))
}

fn validate_32_bytes(value: &[u8], field: &str) -> Result<()> {
    if value.len() != 32 {
        return Err(DbError::InvalidData(format!(
            "{field} must be exactly 32 bytes"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    const TEST_DB_URL: &str = "postgres://buzz:buzz_dev@localhost:5432/buzz";

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| TEST_DB_URL.to_owned());
        let pool = PgPool::connect(&database_url)
            .await
            .expect("connect to test DB");
        crate::migration::run_migrations(&pool)
            .await
            .expect("apply Meeting revocation migrations");
        pool
    }

    #[test]
    fn protocol_discriminator_accepts_only_persisted_meeting_policies() {
        let session_id = Uuid::new_v4();
        assert!(matches!(
            RevocationProtocol::parse(1, FLOOR_POLICY_VERSION, session_id),
            Ok(RevocationProtocol::UniformV0)
        ));
        assert!(matches!(
            RevocationProtocol::parse(SCHEMA_VERSION, BATON_POLICY_VERSION, session_id),
            Ok(RevocationProtocol::ModeratedBatonV1)
        ));
        assert!(matches!(
            RevocationProtocol::parse(BOARD_SCHEMA_VERSION, BOARD_POLICY_VERSION, session_id),
            Ok(RevocationProtocol::ModeratedBoardV2)
        ));
        assert!(RevocationProtocol::parse(2, FLOOR_POLICY_VERSION, session_id).is_err());
    }

    #[test]
    fn revocation_end_identifies_reason_and_revoked_participant() {
        let keys = Keys::generate();
        let session_id = Uuid::new_v4();
        let create_event_id = [7_u8; 32];
        let revoked_pubkey = [9_u8; 32];
        let event = build_revocation_end_event(
            &keys,
            RevocationProtocol::ModeratedBatonV1,
            session_id,
            &create_event_id,
            &revoked_pubkey,
            Utc::now(),
        )
        .expect("build security-revocation End");
        let tags = event.tags.to_vec();
        assert!(tags
            .iter()
            .any(|tag| tag.as_slice() == ["reason", "participant_revoked"]));
        assert!(tags
            .iter()
            .any(|tag| tag.as_slice() == ["p", &hex::encode(revoked_pubkey)]));
        assert!(tags.iter().any(|tag| tag.as_slice() == ["v", "2"]));
        assert!(tags
            .iter()
            .any(|tag| tag.as_slice() == ["policy", BATON_POLICY_VERSION]));

        let v2_event = build_revocation_end_event(
            &keys,
            RevocationProtocol::ModeratedBoardV2,
            session_id,
            &create_event_id,
            &revoked_pubkey,
            Utc::now(),
        )
        .expect("build Meeting V2 security-revocation End");
        let v2_tags = v2_event.tags.to_vec();
        assert!(v2_tags.iter().any(|tag| tag.as_slice() == ["v", "3"]));
        assert!(v2_tags
            .iter()
            .any(|tag| tag.as_slice() == ["policy", BOARD_POLICY_VERSION]));
        assert!(v2_tags
            .iter()
            .any(|tag| tag.as_slice() == ["outcome", "aborted"]));
        assert!(v2_tags
            .iter()
            .any(|tag| tag.as_slice() == ["reason-code", "participant_revoked"]));
        assert!(v2_event.content.is_empty());
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn v0_revocation_ends_from_the_frozen_soft_removed_channel_roster() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let host = [0x61_u8; 32];
        let revoked_participant = [0x62_u8; 32];
        let create_event_id = [0x63_u8; 32];
        let revocation_event_id = [0x64_u8; 32];
        let session_id = Uuid::new_v4();
        let relay_keys = Keys::generate();

        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(format!(
                "meeting-v0-revocation-{}.example",
                Uuid::new_v4().simple()
            ))
            .execute(&pool)
            .await
            .expect("insert test community");
        sqlx::query(
            "INSERT INTO channels \
                 (community_id, id, name, visibility, created_by, room_kind) \
             VALUES ($1, $2, $3, 'private', $4, 'meeting')",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(format!("v0-revocation-{session_id}"))
        .bind(host.as_slice())
        .execute(&pool)
        .await
        .expect("insert Meeting channel");
        sqlx::query(
            "INSERT INTO channel_members \
                 (community_id, channel_id, pubkey, role, invited_by, removed_at) \
             VALUES ($1, $2, $3, 'member', $4, clock_timestamp())",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(revoked_participant.as_slice())
        .bind(host.as_slice())
        .execute(&pool)
        .await
        .expect("insert soft-removed frozen V0 participant");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version, status) \
             VALUES ($1, $2, $3, $4, 1, $5, 'active')",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(create_event_id.as_slice())
        .bind(host.as_slice())
        .bind(FLOOR_POLICY_VERSION)
        .execute(&pool)
        .await
        .expect("insert active V0 Meeting");

        let ended = end_meeting_for_revocation(
            &db,
            community_id,
            session_id,
            &revoked_participant,
            &revocation_event_id,
            &relay_keys,
        )
        .await
        .expect("end V0 Meeting from its frozen roster");
        let RevocationEndOutcome::Ended {
            end_event_id,
            terminal_state_event_id,
        } = ended
        else {
            panic!("active V0 Meeting must transition to ended");
        };

        let session = sqlx::query(
            "SELECT status, ended_by, end_event_id \
             FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("load ended Meeting");
        assert_eq!(
            session.try_get::<String, _>("status").expect("status"),
            "ended"
        );
        assert_eq!(
            session
                .try_get::<Vec<u8>, _>("ended_by")
                .expect("Relay end author"),
            relay_keys.public_key().as_bytes()
        );
        assert_eq!(
            session
                .try_get::<Vec<u8>, _>("end_event_id")
                .expect("End event id"),
            end_event_id
        );

        let archived: bool = sqlx::query_scalar(
            "SELECT archived_at IS NOT NULL FROM channels \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("load archived Meeting channel");
        assert!(archived);

        let terminal_outcome: String = sqlx::query_scalar(
            "SELECT outcome FROM meeting_rounds \
             WHERE community_id = $1 AND session_id = $2 AND round_number = 1",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("load terminal V0 floor");
        assert_eq!(terminal_outcome, "ended");

        let queued_event_ids = sqlx::query_scalar::<_, Vec<u8>>(
            "SELECT event_id FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2 \
             ORDER BY sequence",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_all(&pool)
        .await
        .expect("load terminal outbox events");
        assert_eq!(
            queued_event_ids,
            vec![end_event_id, terminal_state_event_id],
            "Relay End must be observed before terminal V0 State"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn roster_security_locks_linearize_a_non_actor_ban_with_meeting_writes() {
        let pool = setup_pool().await;
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let actor = [0xb1_u8; 32];
        let other_participant = [0xb2_u8; 32];
        let session_id = Uuid::new_v4();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(format!(
                "meeting-security-lock-{}.example",
                Uuid::new_v4().simple()
            ))
            .execute(&pool)
            .await
            .expect("insert security-lock community");
        for (pubkey, role) in [
            (actor.as_slice(), "owner"),
            (other_participant.as_slice(), "member"),
        ] {
            sqlx::query("INSERT INTO users (community_id, pubkey) VALUES ($1, $2)")
                .bind(community_id.as_uuid())
                .bind(pubkey)
                .execute(&pool)
                .await
                .expect("insert security-lock identity");
            sqlx::query(
                "INSERT INTO relay_members (community_id, pubkey, role) \
                 VALUES ($1, $2, $3)",
            )
            .bind(community_id.as_uuid())
            .bind(hex::encode(pubkey))
            .bind(role)
            .execute(&pool)
            .await
            .expect("insert security-lock membership");
        }
        sqlx::query(
            "INSERT INTO channels \
                 (community_id, id, name, visibility, created_by, room_kind) \
             VALUES ($1, $2, $3, 'private', $4, 'meeting')",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(format!("security-lock-{session_id}"))
        .bind(actor.as_slice())
        .execute(&pool)
        .await
        .expect("insert security-lock channel");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  schema_version, floor_policy_version, moderator_pubkey, status) \
             VALUES ($1, $2, $3, $4, 2, $5, $4, 'active')",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind([0xb3_u8; 32].as_slice())
        .bind(actor.as_slice())
        .bind(BATON_POLICY_VERSION)
        .execute(&pool)
        .await
        .expect("insert security-lock Meeting");
        for (pubkey, role) in [
            (actor.as_slice(), "owner"),
            (other_participant.as_slice(), "member"),
        ] {
            sqlx::query(
                "INSERT INTO meeting_participants \
                     (community_id, session_id, pubkey, participant_type, channel_role) \
                 VALUES ($1, $2, $3, 'human', $4)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(pubkey)
            .bind(role)
            .execute(&pool)
            .await
            .expect("insert frozen security-lock participant");
        }

        let mut command_tx = pool.begin().await.expect("begin simulated Meeting write");
        lock_roster_security_tx(
            &mut command_tx,
            community_id,
            session_id,
            RevocationProtocol::ModeratedBatonV1,
        )
        .await
        .expect("lock the complete frozen roster");
        let producer_pool = pool.clone();
        let mut ban_task = tokio::spawn(async move {
            crate::moderation::ban_member_with_revocation(
                &producer_pool,
                community_id,
                other_participant.as_slice(),
                actor.as_slice(),
                Some("concurrent non-actor revocation"),
                None,
                &[0xb4_u8; 32],
            )
            .await
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(150), &mut ban_task)
                .await
                .is_err(),
            "revocation must wait while the earlier Meeting write holds roster SHARE locks"
        );
        command_tx
            .commit()
            .await
            .expect("linearize simulated Meeting write first");
        tokio::time::timeout(std::time::Duration::from_secs(5), ban_task)
            .await
            .expect("revocation resumes after Meeting write")
            .expect("join revocation task")
            .expect("commit non-actor ban");

        let mut after_revoke = pool
            .begin()
            .await
            .expect("begin post-revocation Meeting write");
        lock_roster_security_tx(
            &mut after_revoke,
            community_id,
            session_id,
            RevocationProtocol::ModeratedBatonV1,
        )
        .await
        .expect("lock roster after revocation commit");
        assert_eq!(
            find_revoked_v1_roster_pubkey_tx(&mut after_revoke, community_id, session_id)
                .await
                .expect("scan roster after revocation"),
            Some(other_participant.to_vec()),
            "a revocation that linearizes first must be observed by the next Meeting write"
        );
        after_revoke
            .rollback()
            .await
            .expect("rollback post-revocation scan");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn meeting_created_after_restore_uses_post_validation_revocation_cutoff() {
        let pool = setup_pool().await;
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let host = [0xc1_u8; 32];
        let participant = [0xc2_u8; 32];
        let session_id = Uuid::new_v4();
        let create_event_id = [0xc3_u8; 32];
        let relay_keys = Keys::generate();
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(format!(
                "meeting-cutoff-{}.example",
                Uuid::new_v4().simple()
            ))
            .execute(&pool)
            .await
            .expect("insert cutoff community");
        for (pubkey, role) in [
            (host.as_slice(), "owner"),
            (participant.as_slice(), "member"),
        ] {
            sqlx::query(
                "INSERT INTO users (community_id, pubkey, channel_add_policy) \
                 VALUES ($1, $2, 'anyone')",
            )
            .bind(community_id.as_uuid())
            .bind(pubkey)
            .execute(&pool)
            .await
            .expect("insert cutoff identity");
            sqlx::query(
                "INSERT INTO relay_members (community_id, pubkey, role) \
                 VALUES ($1, $2, $3)",
            )
            .bind(community_id.as_uuid())
            .bind(hex::encode(pubkey))
            .bind(role)
            .execute(&pool)
            .await
            .expect("insert cutoff membership");
        }

        let mut create_tx = pool.begin().await.expect("begin pre-revocation Create");
        let transaction_started_at: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
            .fetch_one(create_tx.as_mut())
            .await
            .expect("freeze Create transaction timestamp");
        crate::moderation::ban_member_with_revocation(
            &pool,
            community_id,
            participant.as_slice(),
            host.as_slice(),
            Some("revoke before restored Create validation"),
            None,
            &[0xc4_u8; 32],
        )
        .await
        .expect("commit revocation during older Create transaction");
        assert!(crate::moderation::unban_member(
            &pool,
            community_id,
            participant.as_slice(),
            host.as_slice(),
        )
        .await
        .expect("restore participant before Create validation"));
        let event_time = Utc::now();
        sqlx::query(
            "INSERT INTO events \
                 (community_id, id, pubkey, created_at, kind, tags, content, sig, \
                  received_at, channel_id) \
             VALUES ($1, $2, $3, $4, $5, $6, '', $7, $4, $8)",
        )
        .bind(community_id.as_uuid())
        .bind(create_event_id.as_slice())
        .bind(host.as_slice())
        .bind(event_time)
        .bind(buzz_core::kind::KIND_MEETING_CREATE as i32)
        .bind(serde_json::json!([
            ["h", session_id.to_string()],
            ["v", "2"]
        ]))
        .bind(vec![0_u8; 64])
        .bind(session_id)
        .execute(create_tx.as_mut())
        .await
        .expect("insert restored Meeting Create event");
        crate::meeting_baton::create_meeting_v1_tx(
            &mut create_tx,
            crate::meeting_baton::CreateMeetingV1Params {
                community_id,
                session_id,
                title: "Post-restore Meeting",
                description: None,
                source_channel_id: None,
                host_pubkey: host.as_slice(),
                moderator_pubkey: host.as_slice(),
                create_event_id: create_event_id.as_slice(),
                participant_pubkeys: &[host.to_vec(), participant.to_vec()],
                relay_keys: &relay_keys,
                config: crate::meeting_baton::BatonConfig::default(),
            },
        )
        .await
        .expect("Create after participant restore");
        create_tx
            .commit()
            .await
            .expect("commit post-restore Meeting");

        let (job_created_at, job_security_order): (DateTime<Utc>, i64) = sqlx::query_as(
            "SELECT created_at, security_order FROM meeting_revocation_jobs \
             WHERE community_id = $1 AND revoked_pubkey = $2 \
             ORDER BY security_order DESC LIMIT 1",
        )
        .bind(community_id.as_uuid())
        .bind(participant.as_slice())
        .fetch_one(&pool)
        .await
        .expect("load revocation cutoff");
        let (session_created_at, session_security_order): (DateTime<Utc>, i64) = sqlx::query_as(
            "SELECT created_at, security_order FROM meeting_sessions \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("load Session cutoff");
        assert!(transaction_started_at < job_created_at);
        assert!(
            session_security_order > job_security_order,
            "Session order must be allocated after its post-validation insert"
        );
        assert!(session_created_at >= transaction_started_at);
        let mut check_tx = pool.begin().await.expect("begin durable cutoff check");
        assert!(
            !actor_durably_revoked_for_session_tx(
                &mut check_tx,
                community_id,
                session_id,
                participant.as_slice(),
            )
            .await
            .expect("evaluate restored Session durable fence"),
            "a Meeting created after restore must not inherit the old revocation"
        );
        check_tx
            .rollback()
            .await
            .expect("rollback durable cutoff check");
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn revocation_order_fence_ends_old_meetings_but_not_reactivated_new_meetings() {
        let pool = setup_pool().await;
        let db = Db::from_pool(pool.clone());
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let host = format!("meeting-revocation-{}.example", Uuid::new_v4().simple());
        let participant = [0x71_u8; 32];
        let participant_hex = hex::encode(participant);
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(host)
            .execute(&pool)
            .await
            .expect("insert test community");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) \
             VALUES ($1, $2, 'member')",
        )
        .bind(community_id.as_uuid())
        .bind(&participant_hex)
        .execute(&pool)
        .await
        .expect("simulate membership reactivation");
        sqlx::query("INSERT INTO users (community_id, pubkey) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(participant.as_slice())
            .execute(&pool)
            .await
            .expect("simulate active authoritative identity");

        let revocation_security_order = 20_i64;
        let old_session = Uuid::new_v4();
        let new_session = Uuid::new_v4();
        for (session_id, security_order) in [(old_session, 10_i64), (new_session, 30_i64)] {
            sqlx::query(
                "INSERT INTO channels \
                     (community_id, id, name, visibility, created_by, room_kind) \
                 VALUES ($1, $2, $3, 'private', $4, 'meeting')",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(format!("revocation-fence-{session_id}"))
            .bind(participant.as_slice())
            .execute(&pool)
            .await
            .expect("insert Meeting channel");
            sqlx::query(
                "INSERT INTO meeting_sessions \
                      (community_id, session_id, create_event_id, host_pubkey, \
                      schema_version, floor_policy_version, moderator_pubkey, \
                      status, security_order) \
                 VALUES ($1, $2, $3, $4, 2, $5, $4, 'active', $6)",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(rand::random::<[u8; 32]>().as_slice())
            .bind(participant.as_slice())
            .bind(BATON_POLICY_VERSION)
            .bind(security_order)
            .execute(&pool)
            .await
            .expect("insert Meeting Session");
            sqlx::query(
                "INSERT INTO meeting_participants \
                     (community_id, session_id, pubkey, participant_type, channel_role) \
                 VALUES ($1, $2, $3, 'human', 'owner')",
            )
            .bind(community_id.as_uuid())
            .bind(session_id)
            .bind(participant.as_slice())
            .execute(&pool)
            .await
            .expect("insert frozen participant");
        }
        sqlx::query(
            "INSERT INTO meeting_revocation_jobs \
                 (community_id, job_id, revoked_pubkey, revocation_event_id, \
                  state, security_order, completed_at) \
             VALUES ($1, $2, $3, $4, 'completed', $5, clock_timestamp())",
        )
        .bind(community_id.as_uuid())
        .bind(Uuid::new_v4())
        .bind(participant.as_slice())
        .bind([0x72_u8; 32].as_slice())
        .bind(revocation_security_order)
        .execute(&pool)
        .await
        .expect("insert completed historical revocation job");

        let worker_sessions = list_revoked_participant_sessions(
            &db,
            community_id,
            &participant,
            revocation_security_order,
            None,
            10,
        )
        .await
        .expect("list time-fenced worker sessions");
        assert_eq!(
            worker_sessions
                .iter()
                .map(|session| session.session_id)
                .collect::<Vec<_>>(),
            vec![old_session]
        );

        let mut tx = db.begin_transaction().await.expect("begin lazy check");
        assert_eq!(
            find_revoked_v1_roster_pubkey_tx(&mut tx, community_id, old_session)
                .await
                .expect("old Meeting lazy check"),
            Some(participant.to_vec()),
            "completed durable revocation survives rapid reactivation"
        );
        assert_eq!(
            find_revoked_v1_roster_pubkey_tx(&mut tx, community_id, new_session)
                .await
                .expect("new Meeting lazy check"),
            None,
            "a Meeting created after reactivation must survive the old job"
        );
        tx.rollback().await.expect("rollback lazy check");

        let owner = [0x73_u8; 32];
        sqlx::query("INSERT INTO users (community_id, pubkey) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(owner.as_slice())
            .execute(&pool)
            .await
            .expect("insert Agent owner identity");
        sqlx::query(
            "UPDATE users SET agent_owner_pubkey = $3 \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(participant.as_slice())
        .bind(owner.as_slice())
        .execute(&pool)
        .await
        .expect("mark participant as owned Agent");
        sqlx::query(
            "INSERT INTO community_bans \
                 (community_id, pubkey, banned, actor_pubkey) \
             VALUES ($1, $2, true, $2)",
        )
        .bind(community_id.as_uuid())
        .bind(owner.as_slice())
        .execute(&pool)
        .await
        .expect("ban NIP-OA owner");
        let mut tx = db
            .begin_transaction()
            .await
            .expect("begin owner-ban lazy check");
        assert_eq!(
            find_revoked_v1_roster_pubkey_tx(&mut tx, community_id, new_session)
                .await
                .expect("owner-ban cascade lazy check"),
            Some(participant.to_vec()),
            "an active owner ban must revoke its owned Agent participant"
        );
        tx.rollback().await.expect("rollback owner-ban lazy check");

        sqlx::query(
            "UPDATE community_bans SET banned = false \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(owner.as_slice())
        .execute(&pool)
        .await
        .expect("unban NIP-OA owner");
        sqlx::query(
            "UPDATE users SET deactivated_at = clock_timestamp() \
             WHERE community_id = $1 AND pubkey = $2",
        )
        .bind(community_id.as_uuid())
        .bind(owner.as_slice())
        .execute(&pool)
        .await
        .expect("deactivate authoritative owner");
        let mut tx = db
            .begin_transaction()
            .await
            .expect("begin owner-deactivation lazy check");
        assert_eq!(
            find_revoked_v1_roster_pubkey_tx(&mut tx, community_id, new_session)
                .await
                .expect("owner-deactivation cascade lazy check"),
            Some(participant.to_vec()),
            "an authoritative owner deactivation must revoke its owned Agent participant"
        );
        tx.rollback()
            .await
            .expect("rollback owner-deactivation lazy check");
    }
}
