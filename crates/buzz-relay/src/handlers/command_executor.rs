//! Command executor — transactional event processing for command kinds.
//!
//! Command kinds (41010–41012, 42100–42101, 30620, 46020, 46030–46031) are processed
//! transactionally: validate → begin tx → insert event → execute mutations → commit.
//!
//! SECURITY: This module is only reachable AFTER the ingest pipeline has verified:
//! 1. Event signature (verify_event)
//! 2. Timestamp freshness (±15 min)
//! 3. Pubkey/auth identity match
//! 4. Per-kind scope authorization

use std::sync::Arc;

use chrono::Utc;
use nostr::Event;
use sha2::{Digest, Sha256};
use tracing::warn;
use uuid::Uuid;

use buzz_core::kind::*;
use buzz_core::tenant::{CommunityId, TenantContext};
use buzz_db::meeting::{
    CreateMeetingParams, EndMeetingOutcome, EndMeetingParams, MAX_MEETING_PARTICIPANTS,
};
use buzz_db::meeting_floor::{
    ClaimFloorOutcome, FloorSignalAction, FloorSignalOutcome, WinnerSelector, YieldOutcome,
};
use buzz_db::workflow::{ApprovalStatus, RunStatus};
use buzz_db::DbError;
use buzz_workflow::executor::TriggerContext;

use crate::state::AppState;
use crate::webhook_secret;

use super::ingest::{extract_channel_id, IngestAuth, IngestError, IngestResult};
use super::side_effects::{
    emit_group_discovery_events, emit_membership_notification, emit_system_message,
    publish_dm_visibility_snapshot,
};

const MEETING_V1_POLICY: &str = buzz_sdk::MEETING_V1_POLICY;
const MEETING_V2_POLICY: &str = buzz_sdk::MEETING_V2_POLICY;
const MEETING_V2_ACTIONS_POLICY: &str = buzz_sdk::MEETING_V2_ACTIONS_POLICY;

/// Frozen Meeting floor-control protocol selected by the persisted Session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeetingProtocol {
    /// Meeting V0 uniform floor competition.
    UniformV0,
    /// Meeting V1 moderator-controlled baton.
    ModeratedBatonV1,
    /// Meeting V2 moderator-maintained current board.
    ModeratedBoardV2,
    /// Meeting V2 with optional action finalization before normal close.
    ModeratedBoardActionsV2,
}

impl MeetingProtocol {
    pub(crate) fn policy(self) -> &'static str {
        match self {
            Self::UniformV0 => buzz_db::meeting_floor::FLOOR_POLICY_VERSION,
            Self::ModeratedBatonV1 => MEETING_V1_POLICY,
            Self::ModeratedBoardV2 => MEETING_V2_POLICY,
            Self::ModeratedBoardActionsV2 => MEETING_V2_ACTIONS_POLICY,
        }
    }

    pub(crate) fn schema_version(self) -> i32 {
        match self {
            Self::UniformV0 => 1,
            Self::ModeratedBatonV1 => 2,
            Self::ModeratedBoardV2 | Self::ModeratedBoardActionsV2 => 3,
        }
    }

    /// Fail closed when a Session contains an unsupported schema/policy pair.
    pub(crate) fn from_persisted(
        schema_version: i32,
        floor_policy_version: &str,
    ) -> Result<Self, IngestError> {
        match (schema_version, floor_policy_version) {
            (1, buzz_db::meeting_floor::FLOOR_POLICY_VERSION) => Ok(Self::UniformV0),
            (2, MEETING_V1_POLICY) => Ok(Self::ModeratedBatonV1),
            (3, MEETING_V2_POLICY) => Ok(Self::ModeratedBoardV2),
            (3, MEETING_V2_ACTIONS_POLICY) => Ok(Self::ModeratedBoardActionsV2),
            _ => Err(IngestError::Internal(format!(
                "error: meeting has unsupported persisted protocol v={schema_version}, policy={floor_policy_version}"
            ))),
        }
    }

    pub(crate) const fn is_v2(self) -> bool {
        matches!(self, Self::ModeratedBoardV2 | Self::ModeratedBoardActionsV2)
    }

    pub(crate) const fn has_action_finalization(self) -> bool {
        matches!(self, Self::ModeratedBoardActionsV2)
    }
}

/// Route a command-kind event to the appropriate handler.
pub async fn handle_command(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: Event,
    auth: IngestAuth,
) -> Result<IngestResult, IngestError> {
    // Ensure the authenticated user exists in the users table (foreign key requirement).
    // The old REST handlers did this via extract_auth_context; command executor must do it explicitly.
    let pubkey_bytes = auth.pubkey().to_bytes().to_vec();
    match state
        .db
        .ensure_user(tenant.community(), &pubkey_bytes)
        .await
    {
        Ok(true) => {
            metrics::counter!(
                "buzz_users_created_total",
                "community" => tenant.host().to_owned()
            )
            .increment(1);
        }
        Ok(false) => {}
        Err(e) => {
            tracing::warn!("command_executor: ensure_user failed: {e}");
        }
    }

    let kind = event.kind.as_u16() as u32;
    match kind {
        KIND_DM_OPEN => handle_dm_open(tenant, state, &event, &auth).await,
        KIND_DM_ADD_MEMBER => handle_dm_add_member(tenant, state, &event, &auth).await,
        KIND_DM_HIDE => handle_dm_hide(tenant, state, &event, &auth).await,
        KIND_MEETING_CREATE => handle_meeting_create(tenant, state, &event, &auth).await,
        KIND_MEETING_END => handle_meeting_end(tenant, state, &event, &auth).await,
        KIND_MEETING_FLOOR_CLAIM => handle_meeting_floor_claim(tenant, state, &event, &auth).await,
        KIND_MEETING_FLOOR_SIGNAL => {
            handle_meeting_floor_signal(tenant, state, &event, &auth).await
        }
        KIND_MEETING_SPEECH_INTENT
        | KIND_MEETING_MODERATOR_COMMAND
        | KIND_MEETING_HUMAN_FLOOR_REQUEST
        | KIND_MEETING_OFFER_RESPONSE
        | KIND_MEETING_GRANT_SIGNAL => {
            super::meeting_baton::handle_command(tenant, state, &event, &auth).await
        }
        KIND_MEETING_BOARD_COMMAND => {
            super::meeting_baton::handle_board_action(tenant, state, &event, &auth).await
        }
        KIND_MEETING_ACTION_COMMAND => {
            super::meeting_baton::handle_action_command(tenant, state, &event, &auth).await
        }
        KIND_WORKFLOW_DEF => handle_workflow_def(tenant, state, &event, &auth).await,
        KIND_WORKFLOW_TRIGGER => handle_workflow_trigger(tenant, state, &event, &auth).await,
        KIND_APPROVAL_GRANT => handle_approval_grant(tenant, state, &event, &auth).await,
        KIND_APPROVAL_DENY => handle_approval_deny(tenant, state, &event, &auth).await,
        _ => Err(IngestError::Rejected(format!(
            "unknown command kind: {kind}"
        ))),
    }
}

/// Result of persisting a command event: either a duplicate (already processed)
/// or an open transaction that the handler must commit after executing mutations.
enum PersistResult {
    /// Event was already processed — return idempotent success.
    Duplicate,
    /// Event inserted — transaction is open, handler must commit after mutations.
    Inserted(sqlx::Transaction<'static, sqlx::Postgres>),
}

/// Persist a command event inside a transaction. Returns the OPEN transaction
/// as an idempotency guard — if the event was already stored, `Duplicate` is
/// returned and the handler skips execution.
///
/// If the event is a duplicate (ON CONFLICT DO NOTHING), the transaction is
/// rolled back and `PersistResult::Duplicate` is returned — no mutations needed.
///
/// Meeting create/end handlers execute their domain mutations through this
/// returned transaction, so their event and lifecycle projection are atomic.
/// Some older command handlers still execute idempotent mutations through the
/// connection pool before committing this event guard.
async fn persist_command_event(
    state: &Arc<AppState>,
    tenant: &TenantContext,
    event: &Event,
    channel_id_override: Option<Uuid>,
) -> Result<PersistResult, IngestError> {
    let channel_id = channel_id_override.or_else(|| extract_channel_id(event));

    let mut tx = state
        .db
        .begin_transaction()
        .await
        .map_err(|e| IngestError::Internal(format!("error: begin transaction: {e}")))?;

    // INSERT with ON CONFLICT DO NOTHING — idempotency guard.
    let id_bytes = event.id.as_bytes();
    let pubkey_bytes = event.pubkey.to_bytes();
    let sig_bytes = event.sig.serialize();
    let tags_json = serde_json::to_value(&event.tags)
        .map_err(|e| IngestError::Internal(format!("error: serialize tags: {e}")))?;
    let kind_i32 = event.kind.as_u16() as i32;
    let created_at_secs = event.created_at.as_secs() as i64;
    let created_at = chrono::DateTime::from_timestamp(created_at_secs, 0).ok_or_else(|| {
        IngestError::Rejected(format!("invalid: bad timestamp {created_at_secs}"))
    })?;
    let received_at = chrono::Utc::now();

    // Extract d_tag for parameterized replaceable kinds (NIP-33).
    let d_tag = buzz_db::event::extract_d_tag(event);
    if let Some(ref d_tag) = d_tag {
        if d_tag.len() > buzz_db::event::D_TAG_MAX_LEN {
            return Err(IngestError::Rejected(format!(
                "invalid: d tag too long ({} bytes, max {})",
                d_tag.len(),
                buzz_db::event::D_TAG_MAX_LEN,
            )));
        }

        // Command kinds normally use plain insert semantics, but workflow
        // definitions are NIP-33 events. Serialize writers for the same
        // coordinate and reject stale writes before executing the domain
        // mutation, otherwise old updates can overwrite newer workflow state.
        let lock_key = {
            let mut h: u64 = 0xcbf29ce484222325;
            for b in tenant.community().as_uuid().as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            for b in kind_i32.to_le_bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            for b in pubkey_bytes.as_slice() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            for b in d_tag.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            h as i64
        };

        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(lock_key)
            .execute(tx.as_mut())
            .await
            .map_err(|e| IngestError::Internal(format!("error: lock event coordinate: {e}")))?;

        let existing: Option<(chrono::DateTime<chrono::Utc>, Vec<u8>)> = sqlx::query_as(
            "SELECT created_at, id FROM events \
             WHERE community_id = $1 AND kind = $2 AND pubkey = $3 AND d_tag = $4 AND deleted_at IS NULL \
             ORDER BY created_at DESC, id ASC LIMIT 1",
        )
        .bind(tenant.community().as_uuid())
        .bind(kind_i32)
        .bind(pubkey_bytes.as_slice())
        .bind(d_tag)
        .fetch_optional(tx.as_mut())
        .await
        .map_err(|e| IngestError::Internal(format!("error: query event coordinate: {e}")))?;

        let incoming_id = event.id.as_bytes().as_slice();
        if let Some((existing_ts, existing_id)) = existing {
            let dominated = created_at < existing_ts
                || (created_at == existing_ts && incoming_id >= existing_id.as_slice());
            if dominated {
                return Ok(PersistResult::Duplicate);
            }

            sqlx::query(
                "UPDATE events SET deleted_at = NOW() \
                 WHERE community_id = $1 AND kind = $2 AND pubkey = $3 AND d_tag = $4 AND deleted_at IS NULL",
            )
            .bind(tenant.community().as_uuid())
            .bind(kind_i32)
            .bind(pubkey_bytes.as_slice())
            .bind(d_tag)
            .execute(tx.as_mut())
            .await
            .map_err(|e| IngestError::Internal(format!("error: replace old event: {e}")))?;
        }
    }

    let result = sqlx::query(
        r#"
        INSERT INTO events (community_id, id, pubkey, created_at, kind, tags, content, sig, received_at, channel_id, d_tag)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(tenant.community().as_uuid())
    .bind(id_bytes.as_slice())
    .bind(pubkey_bytes.as_slice())
    .bind(created_at)
    .bind(kind_i32)
    .bind(&tags_json)
    .bind(&event.content)
    .bind(sig_bytes.as_slice())
    .bind(received_at)
    .bind(channel_id)
    .bind(d_tag.as_deref())
    .execute(tx.as_mut())
    .await
    .map_err(|e| IngestError::Internal(format!("error: insert event: {e}")))?;

    if result.rows_affected() == 0 {
        // Duplicate — rollback (implicit on drop) and signal idempotent success.
        Ok(PersistResult::Duplicate)
    } else {
        Ok(PersistResult::Inserted(tx))
    }
}

/// Extract all `p` tag values (hex pubkeys) from an event.
fn extract_p_tags(event: &Event) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|t| {
            if t.kind().to_string() == "p" {
                t.content().map(|s| s.to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Extract the first `h` tag value (channel UUID) from an event.
fn extract_h_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        if t.kind().to_string() == "h" {
            t.content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Extract the first `d` tag value from an event.
fn extract_d_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        if t.kind().to_string() == "d" {
            t.content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Extract the first `e` tag value from an event.
fn extract_e_tag(event: &Event) -> Option<String> {
    event.tags.iter().find_map(|t| {
        if t.kind().to_string() == "e" {
            t.content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Extract a tag value by name.
fn extract_tag(event: &Event, tag_name: &str) -> Option<String> {
    event.tags.iter().find_map(|t| {
        if t.kind().to_string() == tag_name {
            t.content().map(|s| s.to_string())
        } else {
            None
        }
    })
}

/// Decode a hex pubkey string to 32 bytes.
fn decode_pubkey(hex_str: &str) -> Result<Vec<u8>, IngestError> {
    let bytes = hex::decode(hex_str)
        .map_err(|_| IngestError::Rejected(format!("invalid: bad pubkey hex: {hex_str}")))?;
    if bytes.len() != 32 {
        return Err(IngestError::Rejected(format!(
            "invalid: pubkey must be 32 bytes: {hex_str}"
        )));
    }
    Ok(bytes)
}

/// Compute SHA-256 hash of a string, returning raw bytes.
fn compute_definition_hash(json_str: &str) -> Vec<u8> {
    Sha256::digest(json_str.as_bytes()).to_vec()
}

async fn handle_meeting_create(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    if auth.channel_ids().is_some() {
        return Err(IngestError::AuthFailed(
            "restricted: meeting creation requires a global token".into(),
        ));
    }
    let protocol = meeting_create_protocol(event)?;
    let initial_board = match protocol {
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => Some(
            buzz_sdk::parse_meeting_v2_board_content(&event.content)
                .map_err(|error| IngestError::Rejected(error.to_string()))?,
        ),
        MeetingProtocol::UniformV0 | MeetingProtocol::ModeratedBatonV1 => {
            if !event.content.is_empty() {
                return Err(IngestError::Rejected(
                    "invalid: Meeting V0/V1 create content must be empty".into(),
                ));
            }
            None
        }
    };

    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if session_id.is_nil() {
        return Err(IngestError::Rejected(
            "invalid: meeting session id must not be nil".into(),
        ));
    }
    let title = require_single_tag(event, "name")?;
    if buzz_core::channel::canonical_channel_name(&title)
        .trim()
        .is_empty()
    {
        return Err(IngestError::Rejected(
            "invalid: meeting title is required".into(),
        ));
    }
    if buzz_core::channel::canonical_channel_name(&title)
        .chars()
        .count()
        > 255
    {
        return Err(IngestError::Rejected(
            "invalid: meeting title exceeds 255 characters".into(),
        ));
    }
    let description = optional_single_tag(event, "about")?;
    let source_channel_id = optional_single_tag(event, "source")?
        .map(|source| {
            Uuid::parse_str(&source)
                .map_err(|_| IngestError::Rejected("invalid: bad source channel id".into()))
        })
        .transpose()?;
    if source_channel_id == Some(session_id) {
        return Err(IngestError::Rejected(
            "invalid: meeting cannot use itself as its source channel".into(),
        ));
    }

    let host_pubkey = auth.pubkey().to_bytes().to_vec();
    let mut participant_pubkeys = Vec::with_capacity(MAX_MEETING_PARTICIPANTS);
    participant_pubkeys.push(host_pubkey.clone());
    for pubkey_hex in tag_values(event, "p") {
        let pubkey = decode_pubkey(&pubkey_hex)?;
        if pubkey == host_pubkey {
            return Err(IngestError::Rejected(
                "invalid: meeting create p tags must not repeat the event author".into(),
            ));
        }
        if participant_pubkeys
            .iter()
            .any(|existing| existing == &pubkey)
        {
            return Err(IngestError::Rejected(format!(
                "invalid: duplicate meeting participant {pubkey_hex}"
            )));
        }
        participant_pubkeys.push(pubkey);
    }
    if !(2..=MAX_MEETING_PARTICIPANTS).contains(&participant_pubkeys.len()) {
        return Err(IngestError::Rejected(format!(
            "invalid: meeting requires 2-{MAX_MEETING_PARTICIPANTS} participants"
        )));
    }
    let moderator_pubkey = match protocol {
        MeetingProtocol::ModeratedBatonV1 => {
            let moderator = decode_pubkey(&require_single_tag(event, "moderator")?)?;
            if !participant_pubkeys.contains(&moderator) {
                return Err(IngestError::Rejected(
                    "invalid: meeting moderator must be in the frozen participant roster".into(),
                ));
            }
            Some(moderator)
        }
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            Some(host_pubkey.clone())
        }
        MeetingProtocol::UniformV0 => None,
    };

    let mut tx = match persist_command_event(state, tenant, event, Some(session_id)).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };
    // Apply the rollout gate only after the idempotency guard: replaying the
    // exact Create of an existing gated session remains a successful duplicate
    // even when operators later disable creation of new sessions for that
    // protocol. For a new event, returning here drops and rolls back the
    // still-open transaction.
    ensure_meeting_create_enabled(
        protocol,
        state.config.meeting_v1_create_enabled,
        state.config.meeting_v2_create_enabled,
        state.config.meeting_v2_direct_actions_create_enabled,
    )?;
    if protocol == MeetingProtocol::ModeratedBoardActionsV2
        && !buzz_db::meeting_v2::action_roster_supports_capability_tx(
            &mut tx,
            tenant.community(),
            &participant_pubkeys,
            buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY,
        )
        .await
        .map_err(map_meeting_db_error)?
    {
        return Err(IngestError::Rejected(format!(
            "restricted: every Agent in the Meeting roster must advertise {}",
            buzz_sdk::MEETING_V2_ACTIONS_CAPABILITY
        )));
    }

    let (created_participant_pubkeys, board_event_id) = match protocol {
        MeetingProtocol::UniformV0 => {
            let (_, participants) = buzz_db::meeting::create_meeting_tx(
                &mut tx,
                CreateMeetingParams {
                    community_id: tenant.community(),
                    session_id,
                    title: &title,
                    description: description.as_deref(),
                    source_channel_id,
                    host_pubkey: &host_pubkey,
                    create_event_id: event.id.as_bytes(),
                    participant_pubkeys: &participant_pubkeys,
                },
            )
            .await
            .map_err(map_meeting_db_error)?;
            buzz_db::meeting::enqueue_meeting_event_tx(
                &mut tx,
                tenant.community(),
                session_id,
                event.id.as_bytes(),
            )
            .await
            .map_err(map_meeting_db_error)?;
            buzz_db::meeting_floor::initialize_floor_tx(
                &mut tx,
                tenant.community(),
                session_id,
                &state.relay_keypair,
            )
            .await
            .map_err(map_meeting_db_error)?;
            (
                participants
                    .into_iter()
                    .map(|participant| participant.pubkey)
                    .collect::<Vec<_>>(),
                None,
            )
        }
        MeetingProtocol::ModeratedBatonV1 => {
            let moderator_pubkey = moderator_pubkey.as_deref().ok_or_else(|| {
                IngestError::Internal("error: missing validated V1 moderator".into())
            })?;
            let snapshot = buzz_db::meeting_baton::create_meeting_v1_tx(
                &mut tx,
                buzz_db::meeting_baton::CreateMeetingV1Params {
                    community_id: tenant.community(),
                    session_id,
                    title: &title,
                    description: description.as_deref(),
                    source_channel_id,
                    host_pubkey: &host_pubkey,
                    moderator_pubkey,
                    create_event_id: event.id.as_bytes(),
                    participant_pubkeys: &participant_pubkeys,
                    relay_keys: &state.relay_keypair,
                    config: crate::meeting_runtime::baton_config_from_env(),
                },
            )
            .await
            .map_err(map_meeting_db_error)?;
            (
                snapshot
                    .participants
                    .into_iter()
                    .map(|participant| participant.pubkey)
                    .collect::<Vec<_>>(),
                None,
            )
        }
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            let initial_board = initial_board.as_ref().ok_or_else(|| {
                IngestError::Internal("error: missing validated V2 initial board".into())
            })?;
            let snapshot = buzz_db::meeting_v2::create_meeting_v2_tx(
                &mut tx,
                buzz_db::meeting_v2::CreateMeetingV2Params {
                    community_id: tenant.community(),
                    session_id,
                    policy: if protocol.has_action_finalization() {
                        buzz_db::meeting_v2::MeetingV2Policy::Actions
                    } else {
                        buzz_db::meeting_v2::MeetingV2Policy::Board
                    },
                    title: &title,
                    description: description.as_deref(),
                    source_channel_id,
                    host_pubkey: &host_pubkey,
                    create_event_id: event.id.as_bytes(),
                    participant_pubkeys: &participant_pubkeys,
                    initial_board,
                    relay_keys: &state.relay_keypair,
                    baton_config: crate::meeting_runtime::v2_baton_config_from_env(),
                    board_maintenance_ms: crate::meeting_runtime::v2_board_maintenance_ms_from_env(
                    ),
                },
            )
            .await
            .map_err(map_meeting_db_error)?;
            (
                snapshot
                    .participants
                    .into_iter()
                    .map(|participant| participant.pubkey)
                    .collect::<Vec<_>>(),
                Some(hex::encode(snapshot.board_event_id)),
            )
        }
    };

    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit meeting create: {e}")))?;

    metrics::counter!(
        "buzz_channels_created_total",
        "community" => tenant.host().to_owned(),
        "type" => "meeting"
    )
    .increment(1);

    for participant_pubkey in &created_participant_pubkeys {
        state.invalidate_membership(tenant, session_id, participant_pubkey);
    }

    // Discovery metadata and membership notifications are non-canonical,
    // best-effort side effects. Create and initial State ordering is owned by
    // the transactional meeting outbox above.
    if let Err(error) = emit_group_discovery_events(tenant, state, session_id).await {
        warn!(meeting = %session_id, "meeting create: discovery emission failed: {error}");
    }
    for participant_pubkey in &created_participant_pubkeys {
        if let Err(error) = emit_membership_notification(
            tenant,
            state,
            session_id,
            participant_pubkey,
            &host_pubkey,
            KIND_MEMBER_ADDED_NOTIFICATION,
        )
        .await
        {
            warn!(
                meeting = %session_id,
                participant = %hex::encode(participant_pubkey),
                "meeting create: membership notification failed: {error}"
            );
        }
    }

    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "meeting_id": session_id.to_string(),
                "room_kind": "meeting",
                "status": "active",
                "participant_count": created_participant_pubkeys.len(),
                "schema_version": protocol.schema_version(),
                "floor_policy_version": protocol.policy(),
                "moderator": moderator_pubkey.as_deref().map(hex::encode),
                "board_event_id": board_event_id,
            })
        ),
    })
}

async fn handle_meeting_end(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if auth
        .channel_ids()
        .is_some_and(|ids| !ids.contains(&session_id))
    {
        return Err(IngestError::AuthFailed(
            "restricted: token not authorized for this meeting".into(),
        ));
    }
    let actor_pubkey = auth.pubkey().to_bytes();
    let is_participant = state
        .is_member_cached(tenant.community(), session_id, &actor_pubkey)
        .await
        .map_err(|error| {
            IngestError::Internal(format!(
                "error: checking meeting participant access: {error}"
            ))
        })?;
    let community_role = if is_participant {
        None
    } else {
        state
            .db
            .get_relay_member(tenant.community(), &auth.pubkey().to_hex())
            .await
            .map_err(map_meeting_db_error)?
            .map(|member| member.role)
    };
    if !meeting_end_preflight_allowed(is_participant, community_role.as_deref()) {
        return Err(IngestError::AuthFailed(
            "restricted: not authorized for this meeting".into(),
        ));
    }
    let persisted_policy =
        buzz_db::meeting::get_meeting_policy(&state.db, tenant.community(), session_id)
            .await
            .map_err(map_meeting_db_error)?;
    let protocol = MeetingProtocol::from_persisted(
        persisted_policy.schema_version,
        &persisted_policy.floor_policy_version,
    )?;
    match (protocol, persisted_policy.moderator_pubkey.as_ref()) {
        (MeetingProtocol::UniformV0, None)
        | (MeetingProtocol::ModeratedBatonV1, Some(_))
        | (MeetingProtocol::ModeratedBoardV2, Some(_))
        | (MeetingProtocol::ModeratedBoardActionsV2, Some(_)) => {}
        _ => {
            return Err(IngestError::Internal(
                "error: meeting persisted protocol has an invalid moderator shape".into(),
            ));
        }
    }
    validate_meeting_end_protocol(event, protocol)?;

    let create_event_id_hex = require_single_tag(event, "e")?;
    let create_event_id = decode_event_id(&create_event_id_hex, "meeting create event id")?;
    let v2_terminal = if protocol.is_v2() {
        let outcome = match require_single_tag(event, "outcome")?.as_str() {
            "closed" => buzz_db::meeting_v2::TerminalOutcome::Closed,
            "aborted" => buzz_db::meeting_v2::TerminalOutcome::Aborted,
            _ => {
                return Err(IngestError::Rejected(
                    "invalid: Meeting V2 End outcome must be closed or aborted".into(),
                ));
            }
        };
        (outcome, optional_single_tag(event, "reason-code")?)
    } else {
        if !event.content.is_empty() {
            return Err(IngestError::Rejected(
                "invalid: Meeting V0/V1 End content must be empty".into(),
            ));
        }
        let reason = require_single_tag(event, "reason")?;
        if reason != "manual" {
            return Err(IngestError::Rejected(
                "invalid: client meeting end reason must be manual".into(),
            ));
        }
        (buzz_db::meeting_v2::TerminalOutcome::Closed, None)
    };
    struct ParsedActionEndFence {
        action_run_id: Uuid,
        action_window_epoch: i64,
        board_event_id: Vec<u8>,
    }
    let action_end_fence = if protocol.has_action_finalization()
        && v2_terminal.0 == buzz_db::meeting_v2::TerminalOutcome::Closed
    {
        optional_single_tag(event, "action-run")?
            .map(|run_id| {
                let action_run_id = Uuid::parse_str(&run_id).map_err(|_| {
                    IngestError::Rejected("invalid: bad Meeting action run id".into())
                })?;
                if action_run_id.is_nil() {
                    return Err(IngestError::Rejected(
                        "invalid: Meeting action run id must not be nil".into(),
                    ));
                }
                let action_window_epoch = require_single_tag(event, "action-window")?
                    .parse::<i64>()
                    .map_err(|_| {
                        IngestError::Rejected(
                            "invalid: Meeting action window must be a positive integer".into(),
                        )
                    })?;
                if action_window_epoch <= 0 {
                    return Err(IngestError::Rejected(
                        "invalid: Meeting action window must be positive".into(),
                    ));
                }
                let board_event_id = decode_event_id(
                    &require_single_tag(event, "board")?,
                    "Meeting final Board event id",
                )?;
                if require_single_tag(event, "attestation")? != "actions-recorded" {
                    return Err(IngestError::Rejected(
                        "invalid: Meeting action close must attest actions-recorded".into(),
                    ));
                }
                Ok(ParsedActionEndFence {
                    action_run_id,
                    action_window_epoch,
                    board_event_id,
                })
            })
            .transpose()?
    } else {
        None
    };

    let actor_pubkey = auth.pubkey().to_bytes().to_vec();
    // A normal close is a Floor Decision, so a due Board/Floor deadline must
    // linearize first. The DB End transaction fences the deadline again under
    // the Session lock in case it becomes due between these transactions.
    if protocol.is_v2()
        && v2_terminal.0 == buzz_db::meeting_v2::TerminalOutcome::Closed
        && persisted_policy.moderator_pubkey.as_deref() == Some(actor_pubkey.as_slice())
    {
        let recovery = buzz_db::meeting_baton::recover_meeting_v1(
            &state.db,
            tenant.community(),
            session_id,
            &state.relay_keypair,
        )
        .await
        .map_err(map_meeting_db_error)?;
        if recovery
            .iter()
            .any(|transition| transition.primary_type == "participant_revoked")
        {
            return Err(IngestError::AuthFailed(
                "restricted: meeting ended because a participant was revoked".into(),
            ));
        }
    }
    let mut tx = match persist_command_event(state, tenant, event, Some(session_id)).await? {
        PersistResult::Duplicate => {
            if !buzz_db::meeting::is_meeting_actor_session_security_active(
                &state.db,
                tenant.community(),
                session_id,
                &actor_pubkey,
            )
            .await
            .map_err(map_meeting_db_error)?
            {
                return Err(IngestError::AuthFailed(
                    "restricted: meeting End author is no longer active".into(),
                ));
            }
            if protocol.is_v2() {
                let terminal_outcome = buzz_db::meeting_v2::get_terminal_outcome(
                    &state.db,
                    tenant.community(),
                    session_id,
                )
                .await
                .map_err(map_meeting_db_error)?
                .ok_or_else(|| {
                    IngestError::Internal(
                        "error: duplicate Meeting V2 End exists for an active Session".into(),
                    )
                })?;
                let terminal_outcome = match terminal_outcome {
                    buzz_db::meeting_v2::TerminalOutcome::Closed => "closed",
                    buzz_db::meeting_v2::TerminalOutcome::Aborted => "aborted",
                };
                record_meeting_v2_end(v2_terminal.0, v2_terminal.1.as_deref(), true);
                return Ok(IngestResult {
                    event_id: event.id.to_hex(),
                    accepted: true,
                    message: format!(
                        "response:{}",
                        serde_json::json!({
                            "meeting_id": session_id.to_string(),
                            "status": "ended",
                            "already_ended": true,
                            "terminal_outcome": terminal_outcome,
                        })
                    ),
                });
            }
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    enum ManualEndResult {
        Ended,
        AlreadyEnded(Option<buzz_db::meeting_v2::TerminalOutcome>),
        ParticipantRevoked,
    }

    let end_result = match protocol {
        MeetingProtocol::UniformV0 => {
            let outcome = buzz_db::meeting::end_meeting_tx(
                &mut tx,
                EndMeetingParams {
                    community_id: tenant.community(),
                    session_id,
                    actor_pubkey: &actor_pubkey,
                    create_event_id: &create_event_id,
                    end_event_id: event.id.as_bytes(),
                    relay_keys: &state.relay_keypair,
                },
            )
            .await
            .map_err(map_meeting_db_error)?;
            if outcome == EndMeetingOutcome::Ended {
                buzz_db::meeting::enqueue_meeting_event_tx(
                    &mut tx,
                    tenant.community(),
                    session_id,
                    event.id.as_bytes(),
                )
                .await
                .map_err(map_meeting_db_error)?;
                buzz_db::meeting_floor::close_floor_for_end_tx(
                    &mut tx,
                    tenant.community(),
                    session_id,
                    &state.relay_keypair,
                )
                .await
                .map_err(map_meeting_db_error)?;
            }
            match outcome {
                EndMeetingOutcome::Ended => ManualEndResult::Ended,
                EndMeetingOutcome::AlreadyEnded => ManualEndResult::AlreadyEnded(None),
                EndMeetingOutcome::ParticipantRevoked => ManualEndResult::ParticipantRevoked,
            }
        }
        MeetingProtocol::ModeratedBatonV1 => {
            match buzz_db::meeting_baton::end_meeting_v1_tx(
                &mut tx,
                buzz_db::meeting_baton::EndMeetingV1Params {
                    community_id: tenant.community(),
                    session_id,
                    actor_pubkey: &actor_pubkey,
                    create_event_id: &create_event_id,
                    end_event_id: event.id.as_bytes(),
                    relay_keys: &state.relay_keypair,
                },
            )
            .await
            .map_err(map_meeting_db_error)?
            {
                buzz_db::meeting_baton::EndMeetingV1Outcome::Ended(_) => ManualEndResult::Ended,
                buzz_db::meeting_baton::EndMeetingV1Outcome::AlreadyEnded => {
                    ManualEndResult::AlreadyEnded(None)
                }
                buzz_db::meeting_baton::EndMeetingV1Outcome::ParticipantRevoked(_) => {
                    ManualEndResult::ParticipantRevoked
                }
            }
        }
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            let action_fence = action_end_fence.as_ref().map(|fence| {
                buzz_db::meeting_v2::EndMeetingV2ActionFence {
                    action_run_id: fence.action_run_id,
                    action_window_epoch: fence.action_window_epoch,
                    board_event_id: &fence.board_event_id,
                }
            });
            let end = buzz_db::meeting_v2::end_meeting_v2_tx(
                &mut tx,
                buzz_db::meeting_v2::EndMeetingV2Params {
                    community_id: tenant.community(),
                    session_id,
                    actor_pubkey: &actor_pubkey,
                    create_event_id: &create_event_id,
                    end_event_id: event.id.as_bytes(),
                    outcome: v2_terminal.0,
                    reason_code: v2_terminal.1.as_deref(),
                    action_fence,
                    relay_keys: &state.relay_keypair,
                },
            )
            .await;
            if protocol.has_action_finalization()
                && v2_terminal.0 == buzz_db::meeting_v2::TerminalOutcome::Closed
                && matches!(
                    &end,
                    Err(buzz_db::DbError::InvalidData(message))
                        if message.contains("required action completion gate")
                )
            {
                metrics::counter!(
                    "meeting_v2_action_close_gate_rejection_total",
                    "reason" => "not_ready"
                )
                .increment(1);
            }
            match end.map_err(map_meeting_db_error)? {
                buzz_db::meeting_v2::EndMeetingV2Outcome::Ended(_) => ManualEndResult::Ended,
                buzz_db::meeting_v2::EndMeetingV2Outcome::AlreadyEnded(outcome) => {
                    ManualEndResult::AlreadyEnded(Some(outcome))
                }
                buzz_db::meeting_v2::EndMeetingV2Outcome::ParticipantRevoked(_) => {
                    ManualEndResult::ParticipantRevoked
                }
            }
        }
    };

    match end_result {
        ManualEndResult::Ended => {}
        ManualEndResult::AlreadyEnded(terminal_outcome) => {
            tx.rollback().await.map_err(|e| {
                IngestError::Internal(format!("error: rollback duplicate end: {e}"))
            })?;
            if let Some(terminal_outcome) = terminal_outcome {
                record_meeting_v2_end(terminal_outcome, None, true);
            }
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: format!(
                    "response:{}",
                    serde_json::json!({
                        "meeting_id": session_id.to_string(),
                        "status": "ended",
                        "already_ended": true,
                        "terminal_outcome": terminal_outcome.map(|outcome| {
                            match outcome {
                                buzz_db::meeting_v2::TerminalOutcome::Closed => "closed",
                                buzz_db::meeting_v2::TerminalOutcome::Aborted => "aborted",
                            }
                        }),
                    })
                ),
            });
        }
        ManualEndResult::ParticipantRevoked => {
            tx.commit().await.map_err(|e| {
                IngestError::Internal(format!("error: commit meeting revocation recovery: {e}"))
            })?;
            if protocol.is_v2() {
                metrics::counter!(
                    "meeting_v2_end_total",
                    "outcome" => "aborted",
                    "reason_code" => "participant_revoked",
                    "duplicate" => "false"
                )
                .increment(1);
            }
            return Err(IngestError::AuthFailed(
                "restricted: meeting ended because a participant was revoked".into(),
            ));
        }
    }

    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit meeting end: {e}")))?;

    if protocol.is_v2() {
        record_meeting_v2_end(v2_terminal.0, v2_terminal.1.as_deref(), false);
    }

    // Discovery metadata remains a best-effort projection. The canonical End
    // and terminal State are delivered in causal order by the meeting outbox.
    if let Err(error) = emit_group_discovery_events(tenant, state, session_id).await {
        warn!(meeting = %session_id, "meeting end: discovery emission failed: {error}");
    }

    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "meeting_id": session_id.to_string(),
                "status": "ended",
                "already_ended": false,
                "schema_version": persisted_policy.schema_version,
                "floor_policy_version": persisted_policy.floor_policy_version,
                "terminal_outcome": if protocol.is_v2() {
                    Some(match v2_terminal.0 {
                        buzz_db::meeting_v2::TerminalOutcome::Closed => "closed",
                        buzz_db::meeting_v2::TerminalOutcome::Aborted => "aborted",
                    })
                } else {
                    None
                },
            })
        ),
    })
}

fn record_meeting_v2_end(
    outcome: buzz_db::meeting_v2::TerminalOutcome,
    reason_code: Option<&str>,
    duplicate: bool,
) {
    let (outcome, reason_code) = meeting_v2_end_metric_labels(outcome, reason_code);
    metrics::counter!(
        "meeting_v2_end_total",
        "outcome" => outcome,
        "reason_code" => reason_code,
        "duplicate" => if duplicate { "true" } else { "false" }
    )
    .increment(1);
}

fn meeting_v2_end_metric_labels(
    outcome: buzz_db::meeting_v2::TerminalOutcome,
    reason_code: Option<&str>,
) -> (&'static str, &'static str) {
    match outcome {
        buzz_db::meeting_v2::TerminalOutcome::Closed => ("closed", "none"),
        buzz_db::meeting_v2::TerminalOutcome::Aborted => {
            let reason_code = match reason_code {
                Some("goal_unreachable") => "goal_unreachable",
                Some("insufficient_information") => "insufficient_information",
                Some("discussion_blocked") => "discussion_blocked",
                Some("unable_to_form_conclusion") => "unable_to_form_conclusion",
                Some("moderator_unable_to_continue") => "moderator_unable_to_continue",
                Some("participant_revoked") => "participant_revoked",
                Some(_) | None => "other",
            };
            ("aborted", reason_code)
        }
    }
}

async fn handle_meeting_floor_claim(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    if !event.content.is_empty() {
        return Err(IngestError::Rejected(
            "invalid: meeting floor Claim content must be empty".into(),
        ));
    }
    validate_meeting_tag_vocabulary(event, &["h", "meeting-round"], &[])?;
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if auth
        .channel_ids()
        .is_some_and(|ids| !ids.contains(&session_id))
    {
        return Err(IngestError::AuthFailed(
            "restricted: token not authorized for this meeting".into(),
        ));
    }
    ensure_uniform_v0_protocol(tenant, state, session_id).await?;
    let round_number = parse_positive_round(event)?;
    let config = crate::meeting_runtime::floor_config_from_env();
    let outcome = buzz_db::meeting_floor::claim_floor(
        &state.db,
        tenant.community(),
        session_id,
        round_number,
        event,
        &state.relay_keypair,
        config,
        WinnerSelector::UniformRandom,
    )
    .await
    .map_err(map_meeting_db_error)?;

    match outcome {
        ClaimFloorOutcome::Accepted {
            round_number,
            floor_revision,
            claim_event_id,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "floor_revision": floor_revision,
                    "claim_event_id": hex::encode(claim_event_id),
                    "canonical": true,
                })
            ),
        }),
        ClaimFloorOutcome::Duplicate {
            round_number,
            floor_revision,
            claim_event_id,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "floor_revision": floor_revision,
                    "claim_event_id": hex::encode(claim_event_id),
                    "canonical": true,
                    "duplicate": true,
                })
            ),
        }),
        ClaimFloorOutcome::Conflict {
            canonical_claim_event_id,
        } => Err(IngestError::Rejected(format!(
            "conflict: meeting round already has a canonical Claim for this participant: {}",
            hex::encode(canonical_claim_event_id)
        ))),
    }
}

async fn handle_meeting_floor_signal(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    if !event.content.is_empty() {
        return Err(IngestError::Rejected(
            "invalid: meeting floor signal content must be empty".into(),
        ));
    }
    let action = require_single_tag(event, "action")?;
    match action.as_str() {
        "ready" | "pass" => validate_meeting_tag_vocabulary(
            event,
            &["h", "meeting-round", "action", "intent-basis"],
            &[],
        )?,
        "yield" => validate_meeting_tag_vocabulary(
            event,
            &["h", "meeting-round", "action", "meeting-grant"],
            &[],
        )?,
        _ => {
            return Err(IngestError::Rejected(
                "invalid: meeting floor action must be ready, pass, or yield".into(),
            ));
        }
    }
    let session_id = parse_single_uuid_tag(event, "h", "meeting session id")?;
    if auth
        .channel_ids()
        .is_some_and(|ids| !ids.contains(&session_id))
    {
        return Err(IngestError::AuthFailed(
            "restricted: token not authorized for this meeting".into(),
        ));
    }
    ensure_uniform_v0_protocol(tenant, state, session_id).await?;
    let round_number = parse_positive_round(event)?;
    let config = crate::meeting_runtime::floor_config_from_env();

    if action == "yield" {
        let grant_event_id = decode_event_id(
            &require_single_tag(event, "meeting-grant")?,
            "meeting Grant event id",
        )?;
        let outcome = buzz_db::meeting_floor::yield_floor(
            &state.db,
            tenant.community(),
            session_id,
            round_number,
            &grant_event_id,
            event,
            &state.relay_keypair,
            config,
            WinnerSelector::UniformRandom,
        )
        .await
        .map_err(map_meeting_db_error)?;
        return match outcome {
            YieldOutcome::Accepted {
                round_number,
                signal_event_id,
                next_round_number,
                floor_revision,
            } => Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: format!(
                    "response:{}",
                    serde_json::json!({
                        "meeting_id": session_id,
                        "round_number": round_number,
                        "action": "yield",
                        "signal_event_id": hex::encode(signal_event_id),
                        "next_round_number": next_round_number,
                        "floor_revision": floor_revision,
                    })
                ),
            }),
            YieldOutcome::Duplicate {
                round_number,
                signal_event_id,
            } => Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: format!(
                    "response:{}",
                    serde_json::json!({
                        "meeting_id": session_id,
                        "round_number": round_number,
                        "action": "yield",
                        "signal_event_id": hex::encode(signal_event_id),
                        "duplicate": true,
                    })
                ),
            }),
        };
    }

    let intent_basis = require_single_tag(event, "intent-basis")?;
    let signal_action = if action == "ready" {
        FloorSignalAction::Ready
    } else {
        FloorSignalAction::Pass
    };
    let outcome = buzz_db::meeting_floor::signal_intent(
        &state.db,
        tenant.community(),
        session_id,
        round_number,
        signal_action,
        &intent_basis,
        event,
        &state.relay_keypair,
        config,
        WinnerSelector::UniformRandom,
    )
    .await
    .map_err(map_meeting_db_error)?;
    match outcome {
        FloorSignalOutcome::Accepted {
            round_number,
            floor_revision,
            signal_event_id,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "action": action,
                    "intent_basis": intent_basis,
                    "floor_revision": floor_revision,
                    "signal_event_id": hex::encode(signal_event_id),
                    "canonical": true,
                })
            ),
        }),
        FloorSignalOutcome::Duplicate {
            round_number,
            floor_revision,
            signal_event_id,
        } => Ok(IngestResult {
            event_id: event.id.to_hex(),
            accepted: true,
            message: format!(
                "response:{}",
                serde_json::json!({
                    "meeting_id": session_id,
                    "round_number": round_number,
                    "action": action,
                    "intent_basis": intent_basis,
                    "floor_revision": floor_revision,
                    "signal_event_id": hex::encode(signal_event_id),
                    "canonical": true,
                    "duplicate": true,
                })
            ),
        }),
    }
}

pub(crate) fn map_meeting_db_error(error: DbError) -> IngestError {
    match error {
        DbError::AccessDenied(_)
        | DbError::InvalidData(_)
        | DbError::NotFound(_)
        | DbError::ChannelNotFound(_) => IngestError::Rejected(format!("invalid: {error}")),
        other => IngestError::Internal(format!("error: meeting persistence: {other}")),
    }
}

pub(crate) fn validate_meeting_tag_vocabulary(
    event: &Event,
    required: &[&str],
    repeatable: &[&str],
) -> Result<(), IngestError> {
    validate_meeting_tag_schema(event, required, &[], repeatable)
}

pub(crate) fn validate_meeting_tag_schema(
    event: &Event,
    required: &[&str],
    optional: &[&str],
    repeatable: &[&str],
) -> Result<(), IngestError> {
    let mut seen = std::collections::HashSet::with_capacity(event.tags.len());
    let mut repeatable_count = std::collections::HashMap::<&str, usize>::new();
    for tag in event.tags.iter() {
        let parts = tag.as_slice();
        let Some(name) = parts.first().map(String::as_str) else {
            return Err(IngestError::Rejected(
                "invalid: meeting events cannot contain empty tags".into(),
            ));
        };
        if name == "auth" {
            if !seen.insert(name) {
                return Err(IngestError::Rejected(
                    "invalid: duplicate meeting auth tag".into(),
                ));
            }
            continue;
        }
        if repeatable.contains(&name) {
            if parts.len() != 2 {
                return Err(IngestError::Rejected(format!(
                    "invalid: meeting {name} tag must contain exactly two values"
                )));
            }
            if name == "p"
                && (parts[1].len() != 64
                    || !parts[1]
                        .chars()
                        .all(|character| character.is_ascii_hexdigit()))
            {
                return Err(IngestError::Rejected(
                    "invalid: meeting p tag must contain a 64-character pubkey".into(),
                ));
            }
            let count = repeatable_count.entry(name).or_default();
            *count += 1;
            if name == "p" && *count > buzz_sdk::mentions::MENTION_CAP {
                return Err(IngestError::Rejected(format!(
                    "invalid: meeting speech exceeds {} p mentions",
                    buzz_sdk::mentions::MENTION_CAP
                )));
            }
            continue;
        }
        if !required.contains(&name) && !optional.contains(&name) {
            return Err(IngestError::Rejected(format!(
                "invalid: tag {name} is not allowed on this meeting event"
            )));
        }
        if parts.len() != 2 {
            return Err(IngestError::Rejected(format!(
                "invalid: meeting {name} tag must contain exactly two values"
            )));
        }
        if !seen.insert(name) {
            return Err(IngestError::Rejected(format!(
                "invalid: duplicate meeting {name} tag"
            )));
        }
    }
    for name in required {
        if !seen.contains(name) {
            return Err(IngestError::Rejected(format!(
                "invalid: missing {name} tag"
            )));
        }
    }
    Ok(())
}

fn meeting_create_protocol(event: &Event) -> Result<MeetingProtocol, IngestError> {
    match require_single_tag(event, "v")?.as_str() {
        "1" => {
            validate_meeting_tag_schema(event, &["h", "name", "v"], &["about", "source"], &["p"])?;
            Ok(MeetingProtocol::UniformV0)
        }
        "2" => {
            validate_meeting_tag_schema(
                event,
                &["h", "name", "v", "policy", "moderator"],
                &["about", "source"],
                &["p"],
            )?;
            let policy = require_single_tag(event, "policy")?;
            if policy != MEETING_V1_POLICY {
                return Err(IngestError::Rejected(format!(
                    "invalid: Meeting V1 policy must be {MEETING_V1_POLICY}"
                )));
            }
            Ok(MeetingProtocol::ModeratedBatonV1)
        }
        "3" => {
            validate_meeting_tag_schema(
                event,
                &["h", "name", "v", "policy"],
                &["about", "source"],
                &["p"],
            )?;
            let policy = require_single_tag(event, "policy")?;
            match policy.as_str() {
                MEETING_V2_POLICY => Ok(MeetingProtocol::ModeratedBoardV2),
                MEETING_V2_ACTIONS_POLICY => Ok(MeetingProtocol::ModeratedBoardActionsV2),
                _ => Err(IngestError::Rejected(format!(
                    "invalid: unsupported Meeting V2 policy {policy}"
                ))),
            }
        }
        version => Err(IngestError::Rejected(format!(
            "invalid: unsupported meeting schema version {version}"
        ))),
    }
}

fn ensure_meeting_create_enabled(
    protocol: MeetingProtocol,
    meeting_v1_create_enabled: bool,
    meeting_v2_create_enabled: bool,
    meeting_v2_direct_actions_create_enabled: bool,
) -> Result<(), IngestError> {
    if protocol == MeetingProtocol::ModeratedBatonV1 && !meeting_v1_create_enabled {
        return Err(IngestError::Rejected(
            "restricted: Meeting V1 creation is disabled".into(),
        ));
    }
    if matches!(protocol, MeetingProtocol::ModeratedBoardV2) && !meeting_v2_create_enabled {
        return Err(IngestError::Rejected(
            "restricted: Meeting V2 creation is disabled".into(),
        ));
    }
    if protocol == MeetingProtocol::ModeratedBoardActionsV2
        && (!meeting_v2_create_enabled || !meeting_v2_direct_actions_create_enabled)
    {
        return Err(IngestError::Rejected(
            "restricted: action-capable Meeting V2 creation is disabled".into(),
        ));
    }
    Ok(())
}

async fn ensure_uniform_v0_protocol(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    session_id: Uuid,
) -> Result<(), IngestError> {
    let persisted = buzz_db::meeting::get_meeting_policy(&state.db, tenant.community(), session_id)
        .await
        .map_err(map_meeting_db_error)?;
    let protocol =
        MeetingProtocol::from_persisted(persisted.schema_version, &persisted.floor_policy_version)?;
    if protocol != MeetingProtocol::UniformV0 {
        return Err(IngestError::Rejected(
            "restricted: this floor command is only available for Meeting V0".into(),
        ));
    }
    Ok(())
}

fn meeting_end_preflight_allowed(is_participant: bool, community_role: Option<&str>) -> bool {
    is_participant || matches!(community_role, Some("owner" | "admin"))
}

fn validate_meeting_end_protocol(
    event: &Event,
    protocol: MeetingProtocol,
) -> Result<(), IngestError> {
    match protocol {
        MeetingProtocol::UniformV0 => {
            validate_meeting_tag_schema(event, &["h", "e", "reason"], &[], &[])
        }
        MeetingProtocol::ModeratedBatonV1 => {
            validate_meeting_tag_schema(event, &["h", "v", "policy", "e", "reason"], &[], &[])?;
            if require_single_tag(event, "v")? != "2" {
                return Err(IngestError::Rejected(
                    "invalid: Meeting V1 End must use schema version 2".into(),
                ));
            }
            if require_single_tag(event, "policy")? != MEETING_V1_POLICY {
                return Err(IngestError::Rejected(format!(
                    "invalid: Meeting V1 End policy must be {MEETING_V1_POLICY}"
                )));
            }
            Ok(())
        }
        MeetingProtocol::ModeratedBoardV2 | MeetingProtocol::ModeratedBoardActionsV2 => {
            if require_single_tag(event, "v")? != buzz_sdk::MEETING_V2_SCHEMA_VERSION {
                return Err(IngestError::Rejected(
                    "invalid: Meeting V2 End must use schema version 3".into(),
                ));
            }
            if require_single_tag(event, "policy")? != protocol.policy() {
                return Err(IngestError::Rejected(format!(
                    "invalid: Meeting V2 End policy must be {}",
                    protocol.policy()
                )));
            }
            match require_single_tag(event, "outcome")?.as_str() {
                "closed" => {
                    let has_action_fence = optional_single_tag(event, "action-run")?.is_some();
                    if protocol.has_action_finalization() && has_action_fence {
                        validate_meeting_tag_schema(
                            event,
                            &[
                                "h",
                                "v",
                                "policy",
                                "e",
                                "outcome",
                                "action-run",
                                "action-window",
                                "board",
                                "attestation",
                            ],
                            &[],
                            &[],
                        )?;
                    } else {
                        validate_meeting_tag_schema(
                            event,
                            &["h", "v", "policy", "e", "outcome"],
                            &[],
                            &[],
                        )?;
                    }
                    if !event.content.is_empty() {
                        return Err(IngestError::Rejected(
                            "invalid: Meeting V2 close content must be empty".into(),
                        ));
                    }
                }
                "aborted" => {
                    validate_meeting_tag_schema(
                        event,
                        &["h", "v", "policy", "e", "outcome", "reason-code"],
                        &[],
                        &[],
                    )?;
                    validate_meeting_v2_end_text(
                        &require_single_tag(event, "reason-code")?,
                        128,
                        "abort reason code",
                        false,
                    )?;
                    validate_meeting_v2_end_text(&event.content, 1_024, "abort reason", true)?;
                }
                _ => {
                    return Err(IngestError::Rejected(
                        "invalid: Meeting V2 End outcome must be closed or aborted".into(),
                    ));
                }
            }
            Ok(())
        }
    }
}

fn validate_meeting_v2_end_text(
    value: &str,
    max_bytes: usize,
    field_name: &str,
    allow_empty: bool,
) -> Result<(), IngestError> {
    if value.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(IngestError::Rejected(format!(
                "invalid: Meeting V2 {field_name} is required"
            )))
        };
    }
    if value.trim() != value || value.len() > max_bytes || value.chars().any(char::is_control) {
        return Err(IngestError::Rejected(format!(
            "invalid: Meeting V2 {field_name} must be clean and at most {max_bytes} UTF-8 bytes"
        )));
    }
    Ok(())
}

pub(crate) fn parse_positive_round(event: &Event) -> Result<i64, IngestError> {
    let value = require_single_tag(event, "meeting-round")?;
    value
        .parse::<i64>()
        .ok()
        .filter(|round| *round > 0)
        .ok_or_else(|| {
            IngestError::Rejected("invalid: meeting-round must be a positive integer".into())
        })
}

fn tag_values(event: &Event, tag_name: &str) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|tag| {
            if tag.kind().to_string() == tag_name {
                tag.content().map(str::to_string)
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn require_single_tag(event: &Event, tag_name: &str) -> Result<String, IngestError> {
    let values = tag_values(event, tag_name);
    match values.as_slice() {
        [value] if !value.is_empty() => Ok(value.clone()),
        [] => Err(IngestError::Rejected(format!(
            "invalid: missing {tag_name} tag"
        ))),
        [_] => Err(IngestError::Rejected(format!(
            "invalid: {tag_name} tag must not be empty"
        ))),
        _ => Err(IngestError::Rejected(format!(
            "invalid: expected exactly one {tag_name} tag"
        ))),
    }
}

pub(crate) fn optional_single_tag(
    event: &Event,
    tag_name: &str,
) -> Result<Option<String>, IngestError> {
    let values = tag_values(event, tag_name);
    match values.as_slice() {
        [] => Ok(None),
        [value] => Ok(Some(value.clone())),
        _ => Err(IngestError::Rejected(format!(
            "invalid: expected at most one {tag_name} tag"
        ))),
    }
}

pub(crate) fn parse_single_uuid_tag(
    event: &Event,
    tag_name: &str,
    field_name: &str,
) -> Result<Uuid, IngestError> {
    let value = require_single_tag(event, tag_name)?;
    Uuid::parse_str(&value).map_err(|_| IngestError::Rejected(format!("invalid: bad {field_name}")))
}

pub(crate) fn decode_event_id(value: &str, field_name: &str) -> Result<Vec<u8>, IngestError> {
    if value.len() != 64 || !value.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(IngestError::Rejected(format!(
            "invalid: {field_name} must be 64 hex characters"
        )));
    }
    hex::decode(value).map_err(|_| IngestError::Rejected(format!("invalid: bad {field_name} hex")))
}

async fn handle_dm_open(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();
    let self_hex = hex::encode(&self_bytes);

    // 1. Extract participant pubkeys from `p` tags
    let p_tags = extract_p_tags(event);

    // 2. Validate: at least 1 other participant, max 8 others (9 total)
    if p_tags.is_empty() {
        return Err(IngestError::Rejected(
            "invalid: pubkeys must contain at least 1 other participant".into(),
        ));
    }
    if p_tags.len() > 8 {
        return Err(IngestError::Rejected(
            "invalid: pubkeys may contain at most 8 other participants (9 total)".into(),
        ));
    }

    // Decode all provided pubkeys
    let mut other_bytes: Vec<Vec<u8>> = Vec::with_capacity(p_tags.len());
    for hex_str in &p_tags {
        other_bytes.push(decode_pubkey(hex_str)?);
    }

    // 3. Build full participant set (self + others, deduplicated)
    let mut all_bytes: Vec<Vec<u8>> = vec![self_bytes.clone()];
    for ob in &other_bytes {
        if !all_bytes.iter().any(|b| b == ob) {
            all_bytes.push(ob.clone());
        }
    }

    // Persist the command event (idempotency) — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 4. Execute: open_dm
    let all_refs: Vec<&[u8]> = all_bytes.iter().map(|b| b.as_slice()).collect();
    let (channel, was_created) = state
        .db
        .open_dm(tenant.community(), &all_refs, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: db open_dm: {e}")))?;

    // Commit: event + mutation succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 5. Side effects if newly created (post-commit, best-effort)
    if was_created {
        metrics::counter!(
            "buzz_channels_created_total",
            "community" => tenant.host().to_owned(),
            "type" => "dm"
        )
        .increment(1);

        // Invalidate caches for all participants
        for pk in &all_bytes {
            state.invalidate_membership(tenant, channel.id, pk);
        }

        let participant_hexes: Vec<String> = all_bytes.iter().map(hex::encode).collect();
        if let Err(e) = emit_system_message(
            tenant,
            state,
            channel.id,
            serde_json::json!({
                "type": "dm_created",
                "actor": self_hex,
                "participants": participant_hexes,
            }),
        )
        .await
        {
            warn!("DM open: system message failed: {e}");
        }

        if let Err(e) = emit_group_discovery_events(tenant, state, channel.id).await {
            warn!(channel = %channel.id, "DM open: discovery emission failed: {e}");
        }

        for participant in &all_bytes {
            if let Err(e) = emit_membership_notification(
                tenant,
                state,
                channel.id,
                participant,
                &self_bytes,
                KIND_MEMBER_ADDED_NOTIFICATION,
            )
            .await
            {
                warn!("DM open: membership notification failed: {e}");
            }
        }
    } else {
        // Re-open of an existing DM cleared the caller's hidden_at; refresh
        // their NIP-DV snapshot so the DM reappears in the sidebar.
        if let Err(e) = publish_dm_visibility_snapshot(tenant, state, &self_bytes).await {
            warn!("DM re-open: visibility snapshot failed: {e}");
        }
    }

    // 6. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "channel_id": channel.id.to_string(),
                "created": was_created,
            })
        ),
    })
}

async fn handle_dm_add_member(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();

    // 1. Extract target channel from `h` tag, new member pubkeys from `p` tags
    let channel_id_str = extract_h_tag(event)
        .ok_or_else(|| IngestError::Rejected("invalid: missing h tag (channel_id)".into()))?;
    let channel_id = Uuid::parse_str(&channel_id_str)
        .map_err(|_| IngestError::Rejected("invalid: bad channel_id format".into()))?;

    let p_tags = extract_p_tags(event);
    if p_tags.is_empty() {
        return Err(IngestError::Rejected(
            "invalid: must specify at least 1 new participant in p tags".into(),
        ));
    }

    // 2. Validate caller is member of existing DM
    let is_member = state
        .is_member_cached(tenant.community(), channel_id, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: membership check: {e}")))?;
    if !is_member {
        return Err(IngestError::Rejected(
            "forbidden: not a member of this DM".into(),
        ));
    }

    // 3. Validate channel is type "dm"
    let existing_channel = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .map_err(|_| IngestError::Rejected("invalid: DM not found".into()))?;
    if existing_channel.channel_type != "dm" {
        return Err(IngestError::Rejected("invalid: channel is not a DM".into()));
    }

    // 4. Get existing members, merge with new
    let existing_members = state
        .db
        .get_members(tenant.community(), channel_id)
        .await
        .map_err(|e| IngestError::Internal(format!("error: get members: {e}")))?;

    let mut all_bytes: Vec<Vec<u8>> = existing_members.into_iter().map(|m| m.pubkey).collect();

    // Decode and merge new pubkeys
    for hex_str in &p_tags {
        let bytes = decode_pubkey(hex_str)?;
        if !all_bytes.iter().any(|b| b == &bytes) {
            all_bytes.push(bytes);
        }
    }

    // 5. Enforce max 9 participants
    if all_bytes.len() > 9 {
        return Err(IngestError::Rejected(
            "invalid: DM supports at most 9 participants".into(),
        ));
    }

    // Persist the command event — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 6. Execute: open_dm with expanded set (creates NEW DM — DM sets are immutable)
    let all_refs: Vec<&[u8]> = all_bytes.iter().map(|b| b.as_slice()).collect();
    let (new_channel, was_created) = state
        .db
        .open_dm(tenant.community(), &all_refs, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: db open_dm: {e}")))?;

    // Commit: event + mutation succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 7. Cache invalidation + notifications for new DM (post-commit, best-effort)
    if was_created {
        metrics::counter!(
            "buzz_channels_created_total",
            "community" => tenant.host().to_owned(),
            "type" => "dm"
        )
        .increment(1);

        for pk in &all_bytes {
            state.invalidate_membership(tenant, new_channel.id, pk);
        }

        if let Err(e) = emit_group_discovery_events(tenant, state, new_channel.id).await {
            warn!(channel = %new_channel.id, "DM add_member: discovery emission failed: {e}");
        }

        for participant_bytes in &all_bytes {
            if let Err(e) = emit_membership_notification(
                tenant,
                state,
                new_channel.id,
                participant_bytes,
                &self_bytes,
                KIND_MEMBER_ADDED_NOTIFICATION,
            )
            .await
            {
                warn!("DM add_member: membership notification failed: {e}");
            }
        }
    }

    // 8. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "channel_id": new_channel.id.to_string(),
            })
        ),
    })
}

async fn handle_dm_hide(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();

    // 1. Extract channel from `h` tag
    let channel_id_str = extract_h_tag(event)
        .ok_or_else(|| IngestError::Rejected("invalid: missing h tag (channel_id)".into()))?;
    let channel_id = Uuid::parse_str(&channel_id_str)
        .map_err(|_| IngestError::Rejected("invalid: bad channel_id format".into()))?;

    // 2. Validate caller is member of the DM
    let is_member = state
        .is_member_cached(tenant.community(), channel_id, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: membership check: {e}")))?;
    if !is_member {
        return Err(IngestError::Rejected(
            "forbidden: not a member of this DM".into(),
        ));
    }

    // 3. Validate channel is type "dm"
    let channel = state
        .db
        .get_channel(tenant.community(), channel_id)
        .await
        .map_err(|_| IngestError::Rejected("invalid: DM not found".into()))?;
    if channel.channel_type != "dm" {
        return Err(IngestError::Rejected("invalid: channel is not a DM".into()));
    }

    // Persist the command event — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 4. Execute: hide_dm
    state
        .db
        .hide_dm(tenant.community(), channel_id, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: db hide_dm: {e}")))?;

    // Commit: event + mutation succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 5. Side effect (post-commit, best-effort): refresh the caller's NIP-DV
    // visibility snapshot so clients can filter this DM out of the sidebar.
    if let Err(e) = publish_dm_visibility_snapshot(tenant, state, &self_bytes).await {
        warn!("DM hide: visibility snapshot failed: {e}");
    }

    // 6. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: "{}".into(),
    })
}

async fn handle_workflow_def(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();

    // 1. Extract channel and the canonical workflow UUID from the NIP-33 d-tag.
    let channel_id_str = extract_h_tag(event)
        .ok_or_else(|| IngestError::Rejected("invalid: missing h tag (channel_id)".into()))?;
    let channel_id = Uuid::parse_str(&channel_id_str)
        .map_err(|_| IngestError::Rejected("invalid: bad channel_id format".into()))?;

    let workflow_id_str = extract_d_tag(event)
        .ok_or_else(|| IngestError::Rejected("invalid: missing d tag (workflow_id)".into()))?;
    let workflow_id = Uuid::parse_str(&workflow_id_str)
        .map_err(|_| IngestError::Rejected("invalid: bad workflow_id format".into()))?;

    // 2. Validate caller has channel access (minimum: is a member)
    let is_member = state
        .is_member_cached(tenant.community(), channel_id, &self_bytes)
        .await
        .map_err(|e| IngestError::Internal(format!("error: membership check: {e}")))?;
    if !is_member {
        return Err(IngestError::Rejected(
            "forbidden: not a member of this channel".into(),
        ));
    }

    // 3. Parse YAML from event.content
    let (def, definition_json_str) = buzz_workflow::WorkflowEngine::parse_yaml(&event.content)
        .map_err(|e| IngestError::Rejected(format!("invalid: workflow YAML parse error: {e}")))?;
    let workflow_name = extract_tag(event, "name").unwrap_or_else(|| def.name.clone());

    let mut definition_json: serde_json::Value = serde_json::from_str(&definition_json_str)
        .map_err(|e| IngestError::Internal(format!("error: json parse of definition: {e}")))?;

    let existing_workflow = match state.db.get_workflow(tenant.community(), workflow_id).await {
        Ok(workflow) => {
            if workflow.owner_pubkey != self_bytes || workflow.channel_id != Some(channel_id) {
                return Err(IngestError::Rejected(
                    "forbidden: workflow belongs to a different owner or channel".into(),
                ));
            }
            Some(workflow)
        }
        Err(DbError::NotFound(_)) => None,
        Err(e) => {
            return Err(IngestError::Internal(format!(
                "error: db get_workflow: {e}"
            )));
        }
    };

    // Preserve the existing webhook secret across updates. A new secret is
    // returned only when the workflow first gains a webhook trigger.
    let webhook_secret = if matches!(def.trigger, buzz_workflow::TriggerDef::Webhook) {
        let existing_secret = existing_workflow
            .as_ref()
            .and_then(|workflow| webhook_secret::extract_secret(&workflow.definition));
        let secret = existing_secret.unwrap_or_else(webhook_secret::generate_webhook_secret);
        webhook_secret::inject_secret(&mut definition_json, &secret);
        if existing_workflow
            .as_ref()
            .and_then(|workflow| webhook_secret::extract_secret(&workflow.definition))
            .is_none()
        {
            Some(secret)
        } else {
            None
        }
    } else {
        None
    };

    // Compute hash AFTER secret injection
    let definition_json_final = serde_json::to_string(&definition_json)
        .map_err(|e| IngestError::Internal(format!("error: json serialize: {e}")))?;
    let hash = compute_definition_hash(&definition_json_final);

    // Persist the command event — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 4. Execute: upsert by the NIP-33 d-tag UUID. A retry updates the same
    // row instead of creating another enabled workflow that would fan out on
    // every matching event. The workflow's community is the request's
    // server-bound tenant — never re-derived from the (client-supplied) channel
    // id. `community_of_channel(channel_id)` is ambiguous when the same channel
    // UUID exists in two communities and could mint the workflow under the wrong
    // tenant; `tenant.community()` is the authoritative owner. We then verify the
    // channel actually exists *inside that community* (scoped `get_channel`),
    // which fails closed if the client named a channel that belongs to a
    // different community — the same guarantee the `(community_id, channel_id)`
    // composite FK enforces on insert, surfaced here as a clean rejection.
    let community_id = tenant.community();
    state
        .db
        .get_channel(community_id, channel_id)
        .await
        .map_err(|_| IngestError::Rejected("invalid: workflow channel not found".into()))?;

    state
        .db
        .upsert_workflow(
            community_id,
            workflow_id,
            Some(channel_id),
            &self_bytes,
            &workflow_name,
            &definition_json_final,
            &hash,
        )
        .await
        .map_err(|e| match e {
            DbError::AccessDenied(_) => IngestError::Rejected(
                "forbidden: workflow belongs to a different owner or channel".into(),
            ),
            other => IngestError::Internal(format!("error: db upsert_workflow: {other}")),
        })?;

    // Drop the trigger-path cache entry so the new/updated definition fires on
    // the next matching event instead of after the cache TTL.
    state
        .workflow_engine
        .invalidate_channel_workflows(community_id, channel_id);

    // Commit the event transaction after the idempotent workflow upsert succeeds.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 5. Return response
    let mut resp = serde_json::json!({
        "workflow_id": workflow_id.to_string(),
    });
    if let Some(secret) = webhook_secret {
        resp["webhook_secret"] = serde_json::Value::String(secret);
    }

    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!("response:{}", resp),
    })
}

async fn handle_workflow_trigger(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();

    // 1. Extract workflow reference from `d` tag or `e` tag
    let workflow_id_str = extract_d_tag(event)
        .or_else(|| extract_e_tag(event))
        .ok_or_else(|| {
            IngestError::Rejected("invalid: missing workflow reference (d or e tag)".into())
        })?;
    let workflow_id = Uuid::parse_str(&workflow_id_str)
        .map_err(|_| IngestError::Rejected("invalid: bad workflow_id format".into()))?;

    // 2. Validate workflow exists — scoped to the caller's community. The same
    // workflow UUID can exist in another community; a bare-id lookup could load
    // B's workflow and then satisfy the membership check below against B's
    // colliding channel, letting B trigger A's workflow.
    let community_id = tenant.community();
    let workflow = state
        .db
        .get_workflow(community_id, workflow_id)
        .await
        .map_err(|_| IngestError::Rejected("invalid: workflow not found".into()))?;

    // 3. Manual triggers execute with the workflow owner's authority, so only
    // the owner may start them. Channel membership alone is insufficient: a
    // member could otherwise invoke another user's webhook or message actions.
    if workflow.owner_pubkey != self_bytes {
        return Err(IngestError::Rejected(
            "forbidden: not authorized to trigger this workflow".into(),
        ));
    }

    // Persist the command event under the workflow channel even though the
    // trigger event itself only carries the workflow UUID. Storing channel
    // triggers as global events leaks workflow IDs to unrelated relay members.
    let tx = match persist_command_event(state, tenant, event, workflow.channel_id).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 4. Execute: create workflow run
    let mut trigger_ctx = TriggerContext {
        channel_id: workflow
            .channel_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        author: hex::encode(&self_bytes),
        ..Default::default()
    };
    if !event.content.is_empty() {
        if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(&event.content) {
            for (k, v) in map {
                let val_str = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                trigger_ctx.webhook_fields.insert(k, val_str);
            }
        }
    }
    let trigger_ctx_json = serde_json::to_value(&trigger_ctx).ok();

    let event_id_bytes = event.id.as_bytes().to_vec();
    let run_id = state
        .db
        .create_workflow_run(
            community_id,
            workflow_id,
            Some(&event_id_bytes),
            trigger_ctx_json.as_ref(),
        )
        .await
        .map_err(|e| IngestError::Internal(format!("error: db create_workflow_run: {e}")))?;

    // Commit: event + run creation succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 5. Spawn workflow execution
    let engine = Arc::clone(&state.workflow_engine);
    let db = state.db.clone();
    let def_value = workflow.definition.clone();
    let trigger_ctx_clone = trigger_ctx.clone();
    tokio::spawn(async move {
        let def: buzz_workflow::WorkflowDef = match serde_json::from_value(def_value) {
            Ok(d) => d,
            Err(e) => {
                tracing::error!("workflow_trigger: failed to parse definition: {e}");
                if let Err(db_err) = db
                    .update_workflow_run(
                        community_id,
                        run_id,
                        RunStatus::Failed,
                        0,
                        &serde_json::json!([]),
                        Some(&format!("definition parse error: {e}")),
                    )
                    .await
                {
                    tracing::error!("workflow_trigger: failed to mark run as failed: {db_err}");
                }
                return;
            }
        };

        let result = buzz_workflow::executor::execute_from_step(
            &engine,
            community_id,
            run_id,
            &def,
            &trigger_ctx_clone,
            0,
            None,
        )
        .await;
        engine
            .finalize_run(community_id, run_id, result, None)
            .await;
    });

    // 6. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "run_id": run_id.to_string(),
            })
        ),
    })
}

/// Enforce the approver_spec field against the requesting pubkey.
///
/// Accepted specs:
/// - `""` or `"any"` — any authenticated user may approve.
/// - 64-char lowercase hex string — only that exact pubkey may approve.
///
/// All other formats are rejected (fail-closed).
fn check_approver_spec(approver_spec: &str, requester_hex: &str) -> Result<(), IngestError> {
    let spec = approver_spec.trim();

    // Empty or "any" — anyone may approve
    if spec.is_empty() || spec == "any" {
        return Ok(());
    }

    // Exact pubkey match (64-char hex, case-insensitive)
    if spec.len() == 64 && spec.chars().all(|c| c.is_ascii_hexdigit()) {
        if requester_hex.to_lowercase() == spec.to_lowercase() {
            return Ok(());
        }
        return Err(IngestError::Rejected(
            "forbidden: not the designated approver for this request".into(),
        ));
    }

    // Role-based or unrecognised — fail closed
    Err(IngestError::Rejected(format!(
        "forbidden: approver spec '{}' is not yet supported",
        spec
    )))
}

async fn handle_approval_grant(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();
    let self_hex = hex::encode(&self_bytes);

    // 1. Extract approval reference from `e` tag (references the approval-requested event)
    //    or `d` tag (contains the token hash hex)
    let token_hash_hex = extract_d_tag(event)
        .or_else(|| extract_e_tag(event))
        .ok_or_else(|| {
            IngestError::Rejected("invalid: missing approval reference (d or e tag)".into())
        })?;

    let token_hash = hex::decode(&token_hash_hex)
        .map_err(|_| IngestError::Rejected("invalid: bad approval token hash hex".into()))?;

    // 2. Look up the approval record
    let approval = state
        .db
        .get_approval_by_stored_hash(tenant.community(), &token_hash)
        .await
        .map_err(|_| IngestError::Rejected("invalid: approval not found".into()))?;

    // 3. Validate approval is pending and not expired
    if approval.status != ApprovalStatus::Pending {
        return Err(IngestError::Rejected(format!(
            "invalid: approval already {}",
            approval.status
        )));
    }
    if Utc::now() > approval.expires_at {
        return Err(IngestError::Rejected(
            "invalid: approval token has expired".into(),
        ));
    }

    // 4. Validate caller is authorized approver
    check_approver_spec(&approval.approver_spec, &self_hex)?;

    // Persist the command event — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 5. Execute: update approval status to granted
    let note = if event.content.is_empty() {
        None
    } else {
        Some(event.content.as_str())
    };

    let updated = state
        .db
        .update_approval_by_stored_hash(
            tenant.community(),
            &token_hash,
            ApprovalStatus::Granted,
            Some(&self_bytes),
            note,
        )
        .await
        .map_err(|e| IngestError::Internal(format!("error: db update_approval: {e}")))?;

    if !updated {
        return Err(IngestError::Rejected(
            "invalid: approval already acted on (race)".into(),
        ));
    }

    // Commit: event + approval update succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 6. Resume workflow execution (post-commit, async)
    let community_id = tenant.community();
    let run_id = approval.run_id;
    let workflow_id = approval.workflow_id;
    let resume_index = approval.step_index as usize + 1;
    let engine = Arc::clone(&state.workflow_engine);
    let db = state.db.clone();

    tokio::spawn(async move {
        resume_workflow_after_approval(engine, db, community_id, run_id, workflow_id, resume_index)
            .await;
    });

    // 7. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "status": "granted",
                "run_id": run_id.to_string(),
            })
        ),
    })
}

async fn handle_approval_deny(
    tenant: &TenantContext,
    state: &Arc<AppState>,
    event: &Event,
    auth: &IngestAuth,
) -> Result<IngestResult, IngestError> {
    let self_bytes = auth.pubkey().to_bytes().to_vec();
    let self_hex = hex::encode(&self_bytes);

    // 1. Extract approval reference
    let token_hash_hex = extract_d_tag(event)
        .or_else(|| extract_e_tag(event))
        .ok_or_else(|| {
            IngestError::Rejected("invalid: missing approval reference (d or e tag)".into())
        })?;

    let token_hash = hex::decode(&token_hash_hex)
        .map_err(|_| IngestError::Rejected("invalid: bad approval token hash hex".into()))?;

    // 2. Look up the approval record
    let approval = state
        .db
        .get_approval_by_stored_hash(tenant.community(), &token_hash)
        .await
        .map_err(|_| IngestError::Rejected("invalid: approval not found".into()))?;

    // 3. Validate approval is pending and not expired
    if approval.status != ApprovalStatus::Pending {
        return Err(IngestError::Rejected(format!(
            "invalid: approval already {}",
            approval.status
        )));
    }
    if Utc::now() > approval.expires_at {
        return Err(IngestError::Rejected(
            "invalid: approval token has expired".into(),
        ));
    }

    // 4. Validate caller is authorized approver
    check_approver_spec(&approval.approver_spec, &self_hex)?;

    // Persist the command event — returns open transaction
    let tx = match persist_command_event(state, tenant, event, None).await? {
        PersistResult::Duplicate => {
            return Ok(IngestResult {
                event_id: event.id.to_hex(),
                accepted: true,
                message: "duplicate: already processed".into(),
            });
        }
        PersistResult::Inserted(tx) => tx,
    };

    // 5. Execute: update approval status to denied
    let note = if event.content.is_empty() {
        None
    } else {
        Some(event.content.as_str())
    };

    let updated = state
        .db
        .update_approval_by_stored_hash(
            tenant.community(),
            &token_hash,
            ApprovalStatus::Denied,
            Some(&self_bytes),
            note,
        )
        .await
        .map_err(|e| IngestError::Internal(format!("error: db update_approval: {e}")))?;

    if !updated {
        return Err(IngestError::Rejected(
            "invalid: approval already acted on (race)".into(),
        ));
    }

    // Commit: event + approval denial succeeded atomically.
    tx.commit()
        .await
        .map_err(|e| IngestError::Internal(format!("error: commit transaction: {e}")))?;

    // 6. Cancel the workflow run (post-commit, async)
    let community_id = tenant.community();
    let run_id = approval.run_id;
    let pubkey_hex = self_hex.clone();
    let db = state.db.clone();

    tokio::spawn(async move {
        let run = match db.get_workflow_run(community_id, run_id).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("approval_deny: failed to fetch run {run_id}: {e}");
                return;
            }
        };

        if run.status != RunStatus::WaitingApproval {
            tracing::warn!(
                "approval_deny: run {run_id} has status '{}', expected 'waiting_approval'",
                run.status
            );
            return;
        }

        let cancel_msg = format!("workflow cancelled: approval denied by {pubkey_hex}");
        if let Err(e) = db
            .update_workflow_run(
                community_id,
                run_id,
                RunStatus::Cancelled,
                run.current_step,
                &run.execution_trace,
                Some(&cancel_msg),
            )
            .await
        {
            tracing::error!("approval_deny: failed to cancel run {run_id}: {e}");
        }
    });

    // 7. Return response
    Ok(IngestResult {
        event_id: event.id.to_hex(),
        accepted: true,
        message: format!(
            "response:{}",
            serde_json::json!({
                "status": "denied",
                "run_id": run_id.to_string(),
            })
        ),
    })
}

/// Resume a suspended workflow run after an approval gate has been granted.
async fn resume_workflow_after_approval(
    engine: Arc<buzz_workflow::WorkflowEngine>,
    db: buzz_db::Db,
    community_id: CommunityId,
    run_id: Uuid,
    workflow_id: Uuid,
    resume_index: usize,
) {
    let run = match db.get_workflow_run(community_id, run_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("resume_workflow: failed to fetch run {run_id}: {e}");
            return;
        }
    };

    // Guard: only resume runs that are actually waiting for approval
    if run.status != RunStatus::WaitingApproval {
        tracing::warn!(
            "resume_workflow: run {run_id} has status '{}', expected 'waiting_approval'",
            run.status
        );
        return;
    }

    let workflow = match db.get_workflow(community_id, workflow_id).await {
        Ok(w) => w,
        Err(e) => {
            tracing::error!("resume_workflow: failed to fetch workflow {workflow_id}: {e}");
            return;
        }
    };

    let def: buzz_workflow::WorkflowDef = match serde_json::from_value(workflow.definition.clone())
    {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("resume_workflow: failed to parse workflow definition: {e}");
            if let Err(db_err) = db
                .update_workflow_run(
                    community_id,
                    run_id,
                    RunStatus::Failed,
                    run.current_step,
                    &run.execution_trace,
                    Some(&format!("definition parse error: {e}")),
                )
                .await
            {
                tracing::error!("resume_workflow: failed to mark run as failed: {db_err}");
            }
            return;
        }
    };

    // Reconstruct step_outputs from execution trace for template resolution
    let mut initial_outputs: std::collections::HashMap<String, serde_json::Value> =
        std::collections::HashMap::new();
    if let Some(trace_arr) = run.execution_trace.as_array() {
        for entry in trace_arr {
            if let (Some(step_id), Some(output)) = (
                entry.get("step_id").and_then(|v| v.as_str()),
                entry.get("output"),
            ) {
                initial_outputs.insert(step_id.to_string(), output.clone());
            }
        }
    }

    // Restore trigger context for {{trigger.*}} templates
    let trigger_ctx: TriggerContext = run
        .trigger_context
        .as_ref()
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    // Execute remaining steps
    let existing_trace = run.execution_trace.as_array().cloned();
    let result = buzz_workflow::executor::execute_from_step(
        &engine,
        community_id,
        run_id,
        &def,
        &trigger_ctx,
        resume_index,
        Some(initial_outputs),
    )
    .await;
    engine
        .finalize_run(community_id, run_id, result, existing_trace)
        .await;
}

#[cfg(test)]
mod meeting_protocol_tests {
    use nostr::{EventBuilder, Keys, Kind, Tag};

    use super::*;

    fn meeting_create(tags: Vec<Tag>) -> Event {
        meeting_create_with_content(tags, "")
    }

    fn meeting_create_with_content(tags: Vec<Tag>, content: &str) -> Event {
        EventBuilder::new(Kind::Custom(KIND_MEETING_CREATE as u16), content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("test meeting event")
    }

    fn meeting_end(tags: Vec<Tag>) -> Event {
        meeting_end_with_content(tags, "")
    }

    fn meeting_end_with_content(tags: Vec<Tag>, content: &str) -> Event {
        EventBuilder::new(Kind::Custom(KIND_MEETING_END as u16), content)
            .tags(tags)
            .sign_with_keys(&Keys::generate())
            .expect("test meeting event")
    }

    fn tag(values: &[&str]) -> Tag {
        Tag::parse(values.to_vec()).expect("test tag")
    }

    #[test]
    fn v1_create_requires_exact_version_policy_and_tag_vocabulary() {
        let session = Uuid::new_v4().to_string();
        let moderator = "11".repeat(32);
        let participant = "22".repeat(32);
        let canonical = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "2"]),
            tag(&["policy", MEETING_V1_POLICY]),
            tag(&["moderator", &moderator]),
            tag(&["p", &participant]),
        ]);
        assert_eq!(
            meeting_create_protocol(&canonical).expect("valid V1 create"),
            MeetingProtocol::ModeratedBatonV1
        );

        let wrong_policy = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "2"]),
            tag(&["policy", "uniform-v0"]),
            tag(&["moderator", &moderator]),
            tag(&["p", &participant]),
        ]);
        assert!(meeting_create_protocol(&wrong_policy).is_err());

        let unknown_tag = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "2"]),
            tag(&["policy", MEETING_V1_POLICY]),
            tag(&["moderator", &moderator]),
            tag(&["p", &participant]),
            tag(&["unexpected", "value"]),
        ]);
        assert!(meeting_create_protocol(&unknown_tag).is_err());
    }

    #[test]
    fn v0_create_cannot_smuggle_v1_policy_tags() {
        let session = Uuid::new_v4().to_string();
        let moderator = "11".repeat(32);
        let participant = "22".repeat(32);
        let event = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "1"]),
            tag(&["policy", MEETING_V1_POLICY]),
            tag(&["moderator", &moderator]),
            tag(&["p", &participant]),
        ]);

        assert!(meeting_create_protocol(&event).is_err());
    }

    #[test]
    fn v2_create_requires_host_moderation_and_exact_protocol_tags() {
        let session = Uuid::new_v4().to_string();
        let participant = "22".repeat(32);
        let canonical = meeting_create_with_content(
            vec![
                tag(&["h", &session]),
                tag(&["name", "Protocol review"]),
                tag(&["v", "3"]),
                tag(&["policy", MEETING_V2_POLICY]),
                tag(&["p", &participant]),
            ],
            r##"{"format":"markdown","body":"# Goal"}"##,
        );
        assert_eq!(
            meeting_create_protocol(&canonical).expect("valid V2 create"),
            MeetingProtocol::ModeratedBoardV2
        );
        assert!(buzz_sdk::parse_meeting_v2_board_content(&canonical.content).is_ok());

        let action_capable = meeting_create_with_content(
            vec![
                tag(&["h", &session]),
                tag(&["name", "Action review"]),
                tag(&["v", "3"]),
                tag(&["policy", MEETING_V2_ACTIONS_POLICY]),
                tag(&["p", &participant]),
            ],
            r##"{"format":"markdown","body":"# Goal"}"##,
        );
        assert_eq!(
            meeting_create_protocol(&action_capable).expect("valid action-capable V2 create"),
            MeetingProtocol::ModeratedBoardActionsV2
        );

        let smuggled_moderator = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "3"]),
            tag(&["policy", MEETING_V2_POLICY]),
            tag(&["moderator", &"11".repeat(32)]),
            tag(&["p", &participant]),
        ]);
        assert!(meeting_create_protocol(&smuggled_moderator).is_err());

        let wrong_policy = meeting_create(vec![
            tag(&["h", &session]),
            tag(&["name", "Protocol review"]),
            tag(&["v", "3"]),
            tag(&["policy", MEETING_V1_POLICY]),
            tag(&["p", &participant]),
        ]);
        assert!(meeting_create_protocol(&wrong_policy).is_err());
    }

    #[test]
    fn end_shape_is_selected_by_persisted_protocol() {
        let session = Uuid::new_v4().to_string();
        let create_event_id = "11".repeat(32);
        let v1 = meeting_end(vec![
            tag(&["h", &session]),
            tag(&["v", "2"]),
            tag(&["policy", MEETING_V1_POLICY]),
            tag(&["e", &create_event_id]),
            tag(&["reason", "manual"]),
        ]);

        assert!(validate_meeting_end_protocol(&v1, MeetingProtocol::ModeratedBatonV1).is_ok());
        assert!(
            validate_meeting_end_protocol(&v1, MeetingProtocol::UniformV0).is_err(),
            "V1 tags must not be accepted merely because the kind is shared"
        );

        let v0 = meeting_end(vec![
            tag(&["h", &session]),
            tag(&["e", &create_event_id]),
            tag(&["reason", "manual"]),
        ]);
        assert!(validate_meeting_end_protocol(&v0, MeetingProtocol::UniformV0).is_ok());
        assert!(validate_meeting_end_protocol(&v0, MeetingProtocol::ModeratedBatonV1).is_err());
        assert!(validate_meeting_end_protocol(&v0, MeetingProtocol::ModeratedBoardV2).is_err());

        let v2_close = meeting_end(vec![
            tag(&["h", &session]),
            tag(&["v", "3"]),
            tag(&["policy", MEETING_V2_POLICY]),
            tag(&["e", &create_event_id]),
            tag(&["outcome", "closed"]),
        ]);
        assert!(
            validate_meeting_end_protocol(&v2_close, MeetingProtocol::ModeratedBoardV2).is_ok()
        );
        assert!(
            validate_meeting_end_protocol(&v2_close, MeetingProtocol::ModeratedBatonV1).is_err()
        );

        let v2_abort = meeting_end_with_content(
            vec![
                tag(&["h", &session]),
                tag(&["v", "3"]),
                tag(&["policy", MEETING_V2_POLICY]),
                tag(&["e", &create_event_id]),
                tag(&["outcome", "aborted"]),
                tag(&["reason-code", "goal_unreachable"]),
            ],
            "Required evidence is unavailable.",
        );
        assert!(
            validate_meeting_end_protocol(&v2_abort, MeetingProtocol::ModeratedBoardV2).is_ok()
        );

        let v2_actions_direct_close = meeting_end(vec![
            tag(&["h", &session]),
            tag(&["v", "3"]),
            tag(&["policy", MEETING_V2_ACTIONS_POLICY]),
            tag(&["e", &create_event_id]),
            tag(&["outcome", "closed"]),
        ]);
        assert!(validate_meeting_end_protocol(
            &v2_actions_direct_close,
            MeetingProtocol::ModeratedBoardActionsV2,
        )
        .is_ok());

        let action_run_id = Uuid::new_v4().to_string();
        let board_event_id = "22".repeat(32);
        let v2_actions_gated_close = meeting_end(vec![
            tag(&["h", &session]),
            tag(&["v", "3"]),
            tag(&["policy", MEETING_V2_ACTIONS_POLICY]),
            tag(&["e", &create_event_id]),
            tag(&["outcome", "closed"]),
            tag(&["action-run", &action_run_id]),
            tag(&["action-window", "2"]),
            tag(&["board", &board_event_id]),
            tag(&["attestation", "actions-recorded"]),
        ]);
        assert!(validate_meeting_end_protocol(
            &v2_actions_gated_close,
            MeetingProtocol::ModeratedBoardActionsV2,
        )
        .is_ok());
        assert!(validate_meeting_end_protocol(
            &v2_actions_gated_close,
            MeetingProtocol::ModeratedBoardV2,
        )
        .is_err());
    }

    #[test]
    fn persisted_protocol_mapping_fails_closed() {
        assert_eq!(
            MeetingProtocol::from_persisted(1, buzz_db::meeting_floor::FLOOR_POLICY_VERSION)
                .expect("V0 mapping"),
            MeetingProtocol::UniformV0
        );
        assert_eq!(
            MeetingProtocol::from_persisted(2, MEETING_V1_POLICY).expect("V1 mapping"),
            MeetingProtocol::ModeratedBatonV1
        );
        assert_eq!(
            MeetingProtocol::from_persisted(3, MEETING_V2_POLICY).expect("V2 mapping"),
            MeetingProtocol::ModeratedBoardV2
        );
        assert_eq!(
            MeetingProtocol::from_persisted(3, MEETING_V2_ACTIONS_POLICY)
                .expect("action-capable V2 mapping"),
            MeetingProtocol::ModeratedBoardActionsV2
        );
        assert!(MeetingProtocol::from_persisted(1, MEETING_V1_POLICY).is_err());
        assert!(
            MeetingProtocol::from_persisted(2, buzz_db::meeting_floor::FLOOR_POLICY_VERSION)
                .is_err()
        );
    }

    #[test]
    fn create_gates_only_control_their_new_protocol_sessions() {
        assert!(
            ensure_meeting_create_enabled(MeetingProtocol::UniformV0, false, false, false).is_ok()
        );
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBatonV1,
            true,
            false,
            false,
        )
        .is_ok());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBatonV1,
            false,
            true,
            true,
        )
        .is_err());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBoardV2,
            false,
            true,
            false,
        )
        .is_ok());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBoardV2,
            true,
            false,
            true,
        )
        .is_err());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBoardActionsV2,
            true,
            false,
            true,
        )
        .is_err());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBoardActionsV2,
            false,
            true,
            false,
        )
        .is_err());
        assert!(ensure_meeting_create_enabled(
            MeetingProtocol::ModeratedBoardActionsV2,
            false,
            true,
            true,
        )
        .is_ok());
    }

    #[test]
    fn meeting_end_privacy_preflight_allows_participants_and_community_admins_only() {
        assert!(meeting_end_preflight_allowed(true, None));
        assert!(meeting_end_preflight_allowed(false, Some("owner")));
        assert!(meeting_end_preflight_allowed(false, Some("admin")));
        assert!(!meeting_end_preflight_allowed(false, Some("member")));
        assert!(!meeting_end_preflight_allowed(false, None));
    }

    #[test]
    fn v2_end_metrics_collapse_unbounded_abort_reasons() {
        assert_eq!(
            meeting_v2_end_metric_labels(
                buzz_db::meeting_v2::TerminalOutcome::Closed,
                Some("ignored")
            ),
            ("closed", "none")
        );
        assert_eq!(
            meeting_v2_end_metric_labels(
                buzz_db::meeting_v2::TerminalOutcome::Aborted,
                Some("goal_unreachable")
            ),
            ("aborted", "goal_unreachable")
        );
        assert_eq!(
            meeting_v2_end_metric_labels(
                buzz_db::meeting_v2::TerminalOutcome::Aborted,
                Some("attacker-controlled-high-cardinality-value")
            ),
            ("aborted", "other")
        );
    }
}
