//! Meeting V2 stage-one persistence.
//!
//! Creation atomically freezes the private roster, persists exactly one
//! current Markdown board, and records a fail-closed bootstrap runtime. Board
//! mutation and the V2 control cycle intentionally do not exist in stage one.

use std::collections::HashSet;

use buzz_core::CommunityId;
use chrono::{DateTime, Utc};
use nostr::{Event, EventBuilder, Keys, Kind, Tag, Timestamp};
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::meeting::{is_meeting_reader_authorized_for_channel, MAX_MEETING_PARTICIPANTS};
use crate::meeting_baton::{
    create_moderated_meeting_base_tx, BatonParticipant, CreateModeratedMeetingBaseParams,
};
use crate::{Db, DbError, Result};

/// Persisted Meeting V2 wire schema version.
pub const SCHEMA_VERSION: i32 = 3;
/// Persisted Meeting V2 floor policy.
pub const BOARD_POLICY_VERSION: &str = buzz_sdk::MEETING_V2_POLICY;
/// Fail-closed runtime marker used until the stage-two control cycle lands.
pub const BOOTSTRAP_RUNTIME_PHASE: &str = "bootstrap_locked";

/// Parameters for atomically creating a stage-one Meeting V2 session.
pub struct CreateMeetingV2Params<'a> {
    /// Community that owns the Meeting.
    pub community_id: CommunityId,
    /// Stable Meeting identity; also the backing Channel UUID.
    pub session_id: Uuid,
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
}

/// Result of an atomic stage-one Meeting V2 creation.
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
    /// Board format; stage one accepts only `markdown`.
    pub format: String,
    /// Complete current board document.
    pub body: String,
    /// Initial projection creation time.
    pub created_at: DateTime<Utc>,
    /// Current projection update time.
    pub updated_at: DateTime<Utc>,
}

/// Atomically create a private Meeting V2 room and its initial current board.
///
/// The signed Create event must already exist in `events` inside `tx`. The
/// Create event enters the existing Meeting outbox; the board projection never
/// does, so board content is pull-only.
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
            policy_version: BOARD_POLICY_VERSION,
        },
    )
    .await?;

    let board_event = build_board_event(
        params.relay_keys,
        params.session_id,
        params.host_pubkey,
        params.initial_board,
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

    Ok(CreateMeetingV2Snapshot {
        session_id: params.session_id,
        moderator_pubkey: params.host_pubkey.to_vec(),
        participants: base.participants,
        board_event_id: board_event.id.as_bytes().to_vec(),
        created_at: base.created_at,
    })
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
           AND s.schema_version = $3 AND s.floor_policy_version = $4",
    )
    .bind(community_id.as_uuid())
    .bind(session_id)
    .bind(SCHEMA_VERSION)
    .bind(BOARD_POLICY_VERSION)
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
    now: DateTime<Utc>,
) -> Result<Event> {
    let session = session_id.to_string();
    let moderator = hex::encode(moderator_pubkey);
    let tags = vec![
        parse_tag(["h", session.as_str()])?,
        parse_tag(["v", buzz_sdk::MEETING_V2_SCHEMA_VERSION])?,
        parse_tag(["policy", BOARD_POLICY_VERSION])?,
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
            title: "V2",
            description: None,
            source_channel_id: None,
            host_pubkey: &host,
            create_event_id: &event_id,
            participant_pubkeys: &roster,
            initial_board: &board,
            relay_keys: &relay_keys,
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
        let event = build_board_event(&keys, session_id, &moderator, &board, Utc::now())
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
    async fn create_is_atomic_pull_only_and_readable_by_the_frozen_roster() {
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
                title: "Stage one acceptance",
                description: Some("pull-only current board"),
                source_channel_id: None,
                host_pubkey: &host,
                create_event_id: create.id.as_bytes(),
                participant_pubkeys: &roster,
                initial_board: &board,
                relay_keys: &relay_keys,
            },
        )
        .await
        .expect("atomically create Meeting V2");
        tx.commit().await.expect("commit Meeting V2 Create");

        assert_eq!(snapshot.session_id, session_id);
        assert_eq!(snapshot.moderator_pubkey, host);
        assert_eq!(snapshot.participants.len(), 2);
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
        let bootstrap: (String, i64) = sqlx::query_as(
            "SELECT runtime_phase, control_epoch FROM meeting_v2_bootstrap_state \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .fetch_one(&pool)
        .await
        .expect("read locked Meeting V2 bootstrap");
        assert_eq!(bootstrap, (BOOTSTRAP_RUNTIME_PHASE.to_string(), 1));
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
        let outbox_counts: (i64, i64) = sqlx::query_as(
            "SELECT \
                 count(*) FILTER (WHERE event_id = $3), \
                 count(*) FILTER (WHERE event_id = $4) \
             FROM meeting_event_outbox \
             WHERE community_id = $1 AND session_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(session_id)
        .bind(create.id.as_bytes().as_slice())
        .bind(&snapshot.board_event_id)
        .fetch_one(&pool)
        .await
        .expect("count Meeting V2 outbox rows");
        assert_eq!(outbox_counts, (1, 0));
    }
}
