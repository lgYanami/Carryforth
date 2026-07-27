//! Meeting V0 lifecycle persistence.
//!
//! A meeting reuses a private stream channel, but its identity, frozen roster,
//! and terminal lifecycle are committed as one transaction with the signed
//! command event.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{DbError, Result};
use buzz_core::CommunityId;

/// Maximum number of participants in a Meeting V0 session.
pub const MAX_MEETING_PARTICIPANTS: usize = 12;
/// Maximum number of managed agents in a Meeting V0 session.
pub const MAX_MEETING_AGENTS: usize = 4;

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

/// Outcome of an idempotent Meeting End mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndMeetingOutcome {
    /// This command transitioned the meeting from active to ended.
    Ended,
    /// The meeting was already terminal; no state was changed.
    AlreadyEnded,
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

    if let Some(source_id) = params.source_channel_id {
        let source_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility::text FROM channels \
             WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL \
             FOR SHARE",
        )
        .bind(params.community_id.as_uuid())
        .bind(source_id)
        .fetch_optional(&mut **tx)
        .await?;
        let source_visibility = source_visibility.ok_or_else(|| {
            DbError::InvalidData(format!("source channel not found: {source_id}"))
        })?;

        if source_visibility == "private" {
            for pubkey in params.participant_pubkeys {
                let source_membership: Option<i32> = sqlx::query_scalar(
                    "SELECT 1 FROM channel_members \
                     WHERE community_id = $1 AND channel_id = $2 \
                       AND pubkey = $3 AND removed_at IS NULL \
                     FOR SHARE",
                )
                .bind(params.community_id.as_uuid())
                .bind(source_id)
                .bind(pubkey)
                .fetch_optional(&mut **tx)
                .await?;
                if source_membership.is_none() {
                    return Err(DbError::AccessDenied(format!(
                        "participant {} cannot read source channel {source_id}",
                        hex::encode(pubkey)
                    )));
                }
            }
        }
    }

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
            "SELECT agent_owner_pubkey, channel_add_policy::text AS channel_add_policy \
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
                let policy: String = row.try_get("channel_add_policy")?;
                (owner.is_some(), owner, policy)
            }
            None => (false, None, "anyone".to_string()),
        };

        if is_agent {
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
              source_channel_id, schema_version, status) \
         VALUES ($1, $2, $3, $4, $5, 1, 'active') \
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
        "SELECT host_pubkey, create_event_id, status \
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

    if stored_create_event_id != params.create_event_id {
        return Err(DbError::InvalidData(
            "meeting end references the wrong create event".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        CommunityId::from_uuid(id)
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
}
