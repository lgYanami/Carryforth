//! Durable operator approval for Community-wide Meeting reads.
//!
//! The environment-level Relay flag is only a deployment master switch. This
//! module owns the host-scoped contract publication state and the immutable
//! audit evidence that must exist before historical Meeting visibility widens.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Postgres, QueryBuilder, Row, Transaction};

use buzz_core::CommunityId;
use uuid::Uuid;

use crate::{Db, DbError, Result};

const AUDIT_DIGEST_DOMAIN: &[u8] = b"buzz/meeting-community-read/legacy-audit/v1\0";

/// Risk classification totals for legacy Meeting source Channels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct MeetingSourceRiskCounts {
    /// Source is a live open ordinary Channel in this Community.
    pub community_wide: u64,
    /// Source exists but has a narrower read boundary.
    pub private: u64,
    /// Meeting has no source, or its source is no longer live.
    pub missing: u64,
}

/// Stable full-corpus audit evidence produced while Meeting creation is paused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingVisibilityAudit {
    /// Community whose legacy Meeting corpus was audited.
    pub community_id: CommunityId,
    /// Highest Meeting security order included in the digest, or zero when empty.
    pub watermark: u64,
    /// SHA-256 over the complete terminal Meeting corpus through `watermark`.
    pub digest: [u8; 32],
    /// Number of Meetings included in the digest.
    pub meeting_count: u64,
    /// Source risk totals; their sum equals `meeting_count`.
    pub source_risks: MeetingSourceRiskCounts,
}

/// Durable Meeting Community-read control state for one host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingCommunityReadStatus {
    /// Community identity.
    pub community_id: CommunityId,
    /// Normalized host.
    pub host: String,
    /// Whether the Community is archived.
    pub archived: bool,
    /// Whether the Community-wide Meeting read contract has been published.
    pub enabled: bool,
    /// Whether new Meeting creation is paused for audit/cutover.
    pub create_paused: bool,
    /// Approved legacy corpus watermark.
    pub watermark: Option<u64>,
    /// Approved legacy corpus digest.
    pub audit_digest: Option<[u8; 32]>,
    /// Audited Meeting count.
    pub meeting_count: Option<u64>,
    /// Audited source risk totals.
    pub source_risks: Option<MeetingSourceRiskCounts>,
    /// Time the current audit was recorded.
    pub audited_at: Option<DateTime<Utc>>,
    /// Time an operator approved the current audit.
    pub approved_at: Option<DateTime<Utc>>,
    /// Durable operator identity or change-ticket reference.
    pub approved_by: Option<String>,
    /// Time the Community-wide read contract was published.
    pub enabled_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Serialize)]
struct LegacyMeetingDigestRecord {
    meeting_id: String,
    security_order: i64,
    create_event_id: String,
    host_pubkey: String,
    moderator_pubkey: Option<String>,
    schema_version: i32,
    policy_version: String,
    status: String,
    terminal_outcome: Option<String>,
    terminal_reason_code: Option<String>,
    end_event_id: Option<String>,
    final_state_revision: Option<i64>,
    final_state_event_id: Option<String>,
    final_board_event_id: Option<String>,
    final_board_sha256: Option<String>,
    speech_event_ids: Vec<String>,
    terminal_action_run_id: Option<String>,
    terminal_action_status: Option<String>,
    terminal_action_begin_event_id: Option<String>,
    terminal_action_completion_event_id: Option<String>,
    meeting_channel_deleted: bool,
    source_channel_id: Option<String>,
    source_visibility: Option<String>,
    source_channel_type: Option<String>,
    source_room_kind: Option<String>,
    source_deleted: bool,
    source_risk: &'static str,
}

impl Db {
    /// Probe the additive Meeting Community-read control schema.
    pub async fn meeting_community_read_schema_ready(&self) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT \
                EXISTS (SELECT 1 FROM pg_attribute \
                        WHERE attrelid = 'communities'::regclass \
                          AND attname = 'meeting_community_read_enabled' \
                          AND NOT attisdropped) \
                AND EXISTS (SELECT 1 FROM pg_attribute \
                        WHERE attrelid = 'communities'::regclass \
                          AND attname = 'meeting_community_read_create_paused' \
                          AND NOT attisdropped) \
                AND EXISTS (SELECT 1 FROM pg_attribute \
                        WHERE attrelid = 'communities'::regclass \
                          AND attname = 'legacy_meeting_visibility_audit_digest' \
                          AND NOT attisdropped) \
                AND to_regprocedure('meeting_community_read_contract_immutable()') \
                    IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// Return all host-scoped control states in stable Community order.
    pub async fn list_meeting_community_read_statuses(
        &self,
    ) -> Result<Vec<MeetingCommunityReadStatus>> {
        if !self.meeting_community_read_schema_ready().await? {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::<Postgres>::new(meeting_community_read_status_sql());
        query.push(" ORDER BY id");
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(status_from_row).collect()
    }

    /// Return one host-scoped control state.
    pub async fn meeting_community_read_status(
        &self,
        community_id: CommunityId,
    ) -> Result<Option<MeetingCommunityReadStatus>> {
        if !self.meeting_community_read_schema_ready().await? {
            return Ok(None);
        }
        let mut query = QueryBuilder::<Postgres>::new(meeting_community_read_status_sql());
        query.push(" WHERE id = ").push_bind(community_id.as_uuid());
        let row = query.build().fetch_optional(&self.pool).await?;
        row.map(status_from_row).transpose()
    }

    /// Return the durable Community-read publication bit.
    ///
    /// Relay callers must additionally require their deployment master switch.
    pub async fn meeting_community_read_enabled(&self, community_id: CommunityId) -> Result<bool> {
        if !self.meeting_community_read_schema_ready().await? {
            return Ok(false);
        }
        Ok(sqlx::query_scalar(
            "SELECT COALESCE((SELECT meeting_community_read_enabled \
                              FROM communities WHERE id = $1), FALSE)",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&self.pool)
        .await?)
    }

    /// Deployment readiness for the environment-level master switch.
    ///
    /// Pre-migration and all-unpublished deployments remain rolling-start
    /// compatible. Once any active Community publishes the contract, every
    /// serving Relay must carry migration 0052 and enable its master switch.
    pub async fn meeting_community_read_deployment_ready(
        &self,
        master_enabled: bool,
    ) -> Result<bool> {
        let column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
             WHERE attrelid = 'communities'::regclass \
               AND attname = 'meeting_community_read_enabled' AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !column_exists {
            return Ok(true);
        }
        let any_enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities \
             WHERE meeting_community_read_enabled AND archived_at IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !any_enabled {
            return Ok(true);
        }
        Ok(master_enabled && self.meeting_community_read_schema_ready().await?)
    }

    /// Pause or resume new Meeting creation without changing existing data.
    ///
    /// Resuming before publication clears any pre-publication audit and
    /// approval so a stale digest can never be reused after new writes.
    pub async fn set_meeting_community_read_create_paused(
        &self,
        community_id: CommunityId,
        paused: bool,
    ) -> Result<MeetingCommunityReadStatus> {
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT archived_at IS NOT NULL AS archived, \
                    meeting_community_read_enabled \
             FROM communities WHERE id = $1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")))?;
        if row.try_get::<bool, _>("archived")? {
            return Err(DbError::InvalidData(
                "archived Community cannot change Meeting read control".to_owned(),
            ));
        }
        let enabled: bool = row.try_get("meeting_community_read_enabled")?;
        sqlx::query(
            "UPDATE communities SET \
                 meeting_community_read_create_paused = $2, \
                 legacy_meeting_visibility_watermark = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_watermark ELSE NULL END, \
                 legacy_meeting_visibility_audit_digest = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_audit_digest ELSE NULL END, \
                 legacy_meeting_visibility_meeting_count = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_meeting_count ELSE NULL END, \
                 legacy_meeting_visibility_community_source_count = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_community_source_count ELSE NULL END, \
                 legacy_meeting_visibility_private_source_count = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_private_source_count ELSE NULL END, \
                 legacy_meeting_visibility_missing_source_count = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_missing_source_count ELSE NULL END, \
                 legacy_meeting_visibility_audited_at = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_audited_at ELSE NULL END, \
                 legacy_meeting_visibility_approved_at = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_approved_at ELSE NULL END, \
                 legacy_meeting_visibility_approved_by = \
                    CASE WHEN $2 OR meeting_community_read_enabled \
                         THEN legacy_meeting_visibility_approved_by ELSE NULL END \
             WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(paused)
        .execute(tx.as_mut())
        .await?;
        tx.commit().await?;
        let status = self
            .meeting_community_read_status(community_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")))?;
        debug_assert_eq!(status.enabled, enabled);
        Ok(status)
    }

    /// Compute a stable full-corpus visibility audit while creation is paused.
    pub async fn audit_legacy_meeting_visibility(
        &self,
        community_id: CommunityId,
    ) -> Result<MeetingVisibilityAudit> {
        let mut tx = self.pool.begin().await?;
        lock_cutover_state(&mut tx, community_id, false).await?;
        let audit = compute_legacy_audit_tx(&mut tx, community_id).await?;
        tx.commit().await?;
        Ok(audit)
    }

    /// Persist an operator approval bound to the exact current audit.
    pub async fn approve_legacy_meeting_visibility(
        &self,
        community_id: CommunityId,
        expected_watermark: u64,
        expected_digest: &[u8; 32],
        approved_by: &str,
    ) -> Result<MeetingVisibilityAudit> {
        let approved_by = approved_by.trim();
        if approved_by.is_empty() || approved_by.len() > 255 {
            return Err(DbError::InvalidData(
                "approved_by must contain 1-255 bytes".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        lock_cutover_state(&mut tx, community_id, true).await?;
        let audit = compute_legacy_audit_tx(&mut tx, community_id).await?;
        if audit.watermark != expected_watermark || &audit.digest != expected_digest {
            return Err(DbError::InvalidData(
                "legacy Meeting visibility audit changed; rerun audit before approval".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE communities SET \
                 legacy_meeting_visibility_watermark = $2, \
                 legacy_meeting_visibility_audit_digest = $3, \
                 legacy_meeting_visibility_meeting_count = $4, \
                 legacy_meeting_visibility_community_source_count = $5, \
                 legacy_meeting_visibility_private_source_count = $6, \
                 legacy_meeting_visibility_missing_source_count = $7, \
                 legacy_meeting_visibility_audited_at = clock_timestamp(), \
                 legacy_meeting_visibility_approved_at = clock_timestamp(), \
                 legacy_meeting_visibility_approved_by = $8 \
             WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .bind(u64_to_i64(audit.watermark, "Meeting visibility watermark")?)
        .bind(audit.digest.as_slice())
        .bind(u64_to_i64(audit.meeting_count, "Meeting count")?)
        .bind(u64_to_i64(
            audit.source_risks.community_wide,
            "Community source count",
        )?)
        .bind(u64_to_i64(
            audit.source_risks.private,
            "private source count",
        )?)
        .bind(u64_to_i64(
            audit.source_risks.missing,
            "missing source count",
        )?)
        .bind(approved_by)
        .execute(tx.as_mut())
        .await?;
        tx.commit().await?;
        Ok(audit)
    }

    /// Atomically re-verify approval, publish Community reads, and resume Create.
    pub async fn enable_meeting_community_read(
        &self,
        community_id: CommunityId,
    ) -> Result<MeetingCommunityReadStatus> {
        let mut tx = self.pool.begin().await?;
        let state = lock_cutover_state(&mut tx, community_id, true).await?;
        if state.enabled {
            tx.commit().await?;
            return self
                .meeting_community_read_status(community_id)
                .await?
                .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")));
        }
        let expected_watermark = state.watermark.ok_or_else(|| {
            DbError::InvalidData("legacy Meeting visibility audit is not approved".to_owned())
        })?;
        let expected_digest = state.audit_digest.ok_or_else(|| {
            DbError::InvalidData("legacy Meeting visibility audit is not approved".to_owned())
        })?;
        if state.approved_at.is_none() || state.approved_by.is_none() {
            return Err(DbError::InvalidData(
                "legacy Meeting visibility audit is not approved".to_owned(),
            ));
        }
        let audit = compute_legacy_audit_tx(&mut tx, community_id).await?;
        if audit.watermark != expected_watermark || audit.digest != expected_digest {
            return Err(DbError::InvalidData(
                "legacy Meeting visibility audit changed after approval".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE communities SET \
                 meeting_community_read_enabled = TRUE, \
                 meeting_community_read_create_paused = FALSE, \
                 meeting_community_read_enabled_at = clock_timestamp() \
             WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .execute(tx.as_mut())
        .await?;
        tx.commit().await?;
        self.meeting_community_read_status(community_id)
            .await?
            .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")))
    }
}

/// Block Meeting Create while an operator holds a legacy visibility window.
pub(crate) async fn ensure_meeting_create_allowed_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> Result<()> {
    let paused: bool = sqlx::query_scalar(
        "SELECT meeting_community_read_create_paused \
         FROM communities WHERE id = $1 AND archived_at IS NULL FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(tx.as_mut())
    .await?
    .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")))?;
    if paused {
        return Err(DbError::AccessDenied(
            "Meeting creation is paused for Community-read cutover".to_owned(),
        ));
    }
    Ok(())
}

#[derive(Debug)]
struct LockedCutoverState {
    enabled: bool,
    watermark: Option<u64>,
    audit_digest: Option<[u8; 32]>,
    approved_at: Option<DateTime<Utc>>,
    approved_by: Option<String>,
}

async fn lock_cutover_state(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    exclusive: bool,
) -> Result<LockedCutoverState> {
    let query = if exclusive {
        sqlx::query(
            "SELECT archived_at IS NOT NULL AS archived, \
                meeting_community_read_enabled, \
                meeting_community_read_create_paused, \
                legacy_meeting_visibility_watermark, \
                legacy_meeting_visibility_audit_digest, \
                legacy_meeting_visibility_approved_at, \
                legacy_meeting_visibility_approved_by \
             FROM communities WHERE id = $1 FOR UPDATE",
        )
    } else {
        sqlx::query(
            "SELECT archived_at IS NOT NULL AS archived, \
                meeting_community_read_enabled, \
                meeting_community_read_create_paused, \
                legacy_meeting_visibility_watermark, \
                legacy_meeting_visibility_audit_digest, \
                legacy_meeting_visibility_approved_at, \
                legacy_meeting_visibility_approved_by \
             FROM communities WHERE id = $1 FOR SHARE",
        )
    };
    let row = query
        .bind(community_id.as_uuid())
        .fetch_optional(tx.as_mut())
        .await?
        .ok_or_else(|| DbError::NotFound(format!("Community {community_id}")))?;
    if row.try_get::<bool, _>("archived")? {
        return Err(DbError::InvalidData(
            "archived Community cannot publish Meeting reads".to_owned(),
        ));
    }
    let enabled: bool = row.try_get("meeting_community_read_enabled")?;
    if !row.try_get::<bool, _>("meeting_community_read_create_paused")? && !enabled {
        return Err(DbError::InvalidData(
            "Meeting creation must remain paused during audit and approval".to_owned(),
        ));
    }
    if enabled && !exclusive {
        return Err(DbError::InvalidData(
            "Meeting Community-read contract is already published".to_owned(),
        ));
    }
    Ok(LockedCutoverState {
        enabled,
        watermark: optional_nonnegative_u64(
            row.try_get("legacy_meeting_visibility_watermark")?,
            "legacy Meeting visibility watermark",
        )?,
        audit_digest: optional_digest(row.try_get("legacy_meeting_visibility_audit_digest")?)?,
        approved_at: row.try_get("legacy_meeting_visibility_approved_at")?,
        approved_by: row.try_get("legacy_meeting_visibility_approved_by")?,
    })
}

async fn compute_legacy_audit_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> Result<MeetingVisibilityAudit> {
    let active_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM meeting_sessions \
         WHERE community_id = $1 AND status = 'active'",
    )
    .bind(community_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    if active_count != 0 {
        return Err(DbError::InvalidData(format!(
            "{active_count} active Meeting(s) must end before visibility audit"
        )));
    }
    let watermark: i64 = sqlx::query_scalar(
        "SELECT COALESCE(max(security_order), 0) FROM meeting_sessions \
         WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_one(tx.as_mut())
    .await?;
    let rows = sqlx::query(
        "SELECT session.session_id, session.security_order, \
                session.create_event_id, session.host_pubkey, \
                session.moderator_pubkey, session.schema_version, \
                session.floor_policy_version, session.status, \
                session.terminal_outcome, session.terminal_reason_code, \
                session.end_event_id, session.source_channel_id, \
                meeting_channel.deleted_at IS NOT NULL AS meeting_channel_deleted, \
                source.visibility::text AS source_visibility, \
                source.channel_type::text AS source_channel_type, \
                source.room_kind AS source_room_kind, \
                source.deleted_at IS NOT NULL AS source_deleted, \
                CASE WHEN session.schema_version = 1 \
                     THEN v0_state.floor_revision \
                     ELSE baton_state.state_revision END AS final_state_revision, \
                CASE WHEN session.schema_version = 1 \
                     THEN v0_state.state_event_id \
                     ELSE baton_state.state_event_id END AS final_state_event_id, \
                board.board_event_id AS final_board_event_id, \
                board.board_content AS final_board_content, \
                ARRAY( \
                    SELECT accepted.speech_event_id FROM ( \
                        SELECT round.speech_event_id \
                        FROM meeting_rounds round \
                        WHERE round.community_id = session.community_id \
                          AND round.session_id = session.session_id \
                          AND round.speech_event_id IS NOT NULL \
                        UNION \
                        SELECT baton_grant.speech_event_id \
                        FROM meeting_baton_grants baton_grant \
                        WHERE baton_grant.community_id = session.community_id \
                          AND baton_grant.session_id = session.session_id \
                          AND baton_grant.speech_event_id IS NOT NULL \
                    ) accepted ORDER BY accepted.speech_event_id \
                ) AS speech_event_ids, \
                action.action_run_id AS terminal_action_run_id, \
                action.terminal_status AS terminal_action_status, \
                action.begin_event_id AS terminal_action_begin_event_id, \
                action.completion_event_id AS terminal_action_completion_event_id \
         FROM meeting_sessions session \
         JOIN channels meeting_channel \
           ON meeting_channel.community_id = session.community_id \
          AND meeting_channel.id = session.session_id \
         LEFT JOIN channels source \
           ON source.community_id = session.community_id \
          AND source.id = session.source_channel_id \
         LEFT JOIN meeting_baton_state baton_state \
           ON baton_state.community_id = session.community_id \
          AND baton_state.session_id = session.session_id \
         LEFT JOIN LATERAL ( \
             SELECT round.floor_revision, round.state_event_id \
             FROM meeting_rounds round \
             WHERE round.community_id = session.community_id \
               AND round.session_id = session.session_id \
             ORDER BY round.round_number DESC LIMIT 1 \
         ) v0_state ON TRUE \
         LEFT JOIN meeting_current_boards board \
           ON board.community_id = session.community_id \
          AND board.session_id = session.session_id \
         LEFT JOIN LATERAL ( \
             SELECT run.action_run_id, run.terminal_status, \
                    run.begin_event_id, run.completion_event_id \
             FROM meeting_v2_action_runs run \
             WHERE run.community_id = session.community_id \
               AND run.session_id = session.session_id \
             ORDER BY run.created_at DESC, run.action_run_id DESC LIMIT 1 \
         ) action ON TRUE \
         WHERE session.community_id = $1 \
           AND session.security_order <= $2 \
         ORDER BY session.security_order, session.session_id",
    )
    .bind(community_id.as_uuid())
    .bind(watermark)
    .fetch_all(tx.as_mut())
    .await?;

    let mut source_risks = MeetingSourceRiskCounts::default();
    let mut hasher = Sha256::new();
    hasher.update(AUDIT_DIGEST_DOMAIN);
    hasher.update(watermark.to_be_bytes());
    hasher.update((rows.len() as u64).to_be_bytes());
    for row in rows {
        let source_channel_id: Option<Uuid> = row.try_get("source_channel_id")?;
        let source_visibility: Option<String> = row.try_get("source_visibility")?;
        let source_channel_type: Option<String> = row.try_get("source_channel_type")?;
        let source_room_kind: Option<String> = row.try_get("source_room_kind")?;
        let source_deleted: bool = row.try_get("source_deleted")?;
        let source_risk = if source_channel_id.is_none() || source_deleted {
            source_risks.missing += 1;
            "missing"
        } else if source_visibility.as_deref() == Some("open")
            && source_room_kind.as_deref() == Some("standard")
            && source_channel_type.as_deref() != Some("dm")
        {
            source_risks.community_wide += 1;
            "community_wide"
        } else {
            source_risks.private += 1;
            "private"
        };
        let final_board_content: Option<String> = row.try_get("final_board_content")?;
        let final_board_sha256 = final_board_content.map(|content| {
            let digest: [u8; 32] = Sha256::digest(content.as_bytes()).into();
            hex::encode(digest)
        });
        let speech_event_ids: Vec<Vec<u8>> = row.try_get("speech_event_ids")?;
        let record = LegacyMeetingDigestRecord {
            meeting_id: row.try_get::<Uuid, _>("session_id")?.to_string(),
            security_order: row.try_get("security_order")?,
            create_event_id: hex::encode(row.try_get::<Vec<u8>, _>("create_event_id")?),
            host_pubkey: hex::encode(row.try_get::<Vec<u8>, _>("host_pubkey")?),
            moderator_pubkey: optional_hex(row.try_get("moderator_pubkey")?),
            schema_version: row.try_get("schema_version")?,
            policy_version: row.try_get("floor_policy_version")?,
            status: row.try_get("status")?,
            terminal_outcome: row.try_get("terminal_outcome")?,
            terminal_reason_code: row.try_get("terminal_reason_code")?,
            end_event_id: optional_hex(row.try_get("end_event_id")?),
            final_state_revision: row.try_get("final_state_revision")?,
            final_state_event_id: optional_hex(row.try_get("final_state_event_id")?),
            final_board_event_id: optional_hex(row.try_get("final_board_event_id")?),
            final_board_sha256,
            speech_event_ids: speech_event_ids.into_iter().map(hex::encode).collect(),
            terminal_action_run_id: row
                .try_get::<Option<Uuid>, _>("terminal_action_run_id")?
                .map(|value| value.to_string()),
            terminal_action_status: row.try_get("terminal_action_status")?,
            terminal_action_begin_event_id: optional_hex(
                row.try_get("terminal_action_begin_event_id")?,
            ),
            terminal_action_completion_event_id: optional_hex(
                row.try_get("terminal_action_completion_event_id")?,
            ),
            meeting_channel_deleted: row.try_get("meeting_channel_deleted")?,
            source_channel_id: source_channel_id.map(|value| value.to_string()),
            source_visibility,
            source_channel_type,
            source_room_kind,
            source_deleted,
            source_risk,
        };
        let encoded = serde_json::to_vec(&record)?;
        hasher.update((encoded.len() as u64).to_be_bytes());
        hasher.update(encoded);
    }
    Ok(MeetingVisibilityAudit {
        community_id,
        watermark: nonnegative_u64(watermark, "Meeting visibility watermark")?,
        digest: hasher.finalize().into(),
        meeting_count: source_risks.community_wide + source_risks.private + source_risks.missing,
        source_risks,
    })
}

fn meeting_community_read_status_sql() -> &'static str {
    "SELECT id, host, archived_at IS NOT NULL AS archived, \
            meeting_community_read_enabled, \
            meeting_community_read_create_paused, \
            legacy_meeting_visibility_watermark, \
            legacy_meeting_visibility_audit_digest, \
            legacy_meeting_visibility_meeting_count, \
            legacy_meeting_visibility_community_source_count, \
            legacy_meeting_visibility_private_source_count, \
            legacy_meeting_visibility_missing_source_count, \
            legacy_meeting_visibility_audited_at, \
            legacy_meeting_visibility_approved_at, \
            legacy_meeting_visibility_approved_by, \
            meeting_community_read_enabled_at \
     FROM communities"
}

fn status_from_row(row: sqlx::postgres::PgRow) -> Result<MeetingCommunityReadStatus> {
    let community = row.try_get::<Uuid, _>("id")?;
    let community_source = optional_nonnegative_u64(
        row.try_get("legacy_meeting_visibility_community_source_count")?,
        "Community source count",
    )?;
    let private_source = optional_nonnegative_u64(
        row.try_get("legacy_meeting_visibility_private_source_count")?,
        "private source count",
    )?;
    let missing_source = optional_nonnegative_u64(
        row.try_get("legacy_meeting_visibility_missing_source_count")?,
        "missing source count",
    )?;
    let source_risks = match (community_source, private_source, missing_source) {
        (Some(community_wide), Some(private), Some(missing)) => Some(MeetingSourceRiskCounts {
            community_wide,
            private,
            missing,
        }),
        (None, None, None) => None,
        _ => {
            return Err(DbError::InvalidData(format!(
                "partial Meeting source audit counts for Community {community}"
            )))
        }
    };
    Ok(MeetingCommunityReadStatus {
        community_id: CommunityId::from_uuid(community),
        host: row.try_get("host")?,
        archived: row.try_get("archived")?,
        enabled: row.try_get("meeting_community_read_enabled")?,
        create_paused: row.try_get("meeting_community_read_create_paused")?,
        watermark: optional_nonnegative_u64(
            row.try_get("legacy_meeting_visibility_watermark")?,
            "legacy Meeting visibility watermark",
        )?,
        audit_digest: optional_digest(row.try_get("legacy_meeting_visibility_audit_digest")?)?,
        meeting_count: optional_nonnegative_u64(
            row.try_get("legacy_meeting_visibility_meeting_count")?,
            "Meeting count",
        )?,
        source_risks,
        audited_at: row.try_get("legacy_meeting_visibility_audited_at")?,
        approved_at: row.try_get("legacy_meeting_visibility_approved_at")?,
        approved_by: row.try_get("legacy_meeting_visibility_approved_by")?,
        enabled_at: row.try_get("meeting_community_read_enabled_at")?,
    })
}

fn optional_digest(value: Option<Vec<u8>>) -> Result<Option<[u8; 32]>> {
    value
        .map(|bytes| {
            bytes.try_into().map_err(|bytes: Vec<u8>| {
                DbError::InvalidData(format!(
                    "Meeting visibility digest must be 32 bytes, got {}",
                    bytes.len()
                ))
            })
        })
        .transpose()
}

fn optional_hex(value: Option<Vec<u8>>) -> Option<String> {
    value.map(hex::encode)
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value).map_err(|_| DbError::InvalidData(format!("{field} must be non-negative")))
}

fn optional_nonnegative_u64(value: Option<i64>, field: &str) -> Result<Option<u64>> {
    value.map(|value| nonnegative_u64(value, field)).transpose()
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value).map_err(|_| DbError::InvalidData(format!("{field} exceeds BIGINT")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::PgPool;

    #[test]
    fn source_risk_counts_default_to_zero() {
        assert_eq!(
            MeetingSourceRiskCounts::default(),
            MeetingSourceRiskCounts {
                community_wide: 0,
                private: 0,
                missing: 0,
            }
        );
    }

    #[test]
    fn optional_digest_rejects_wrong_length() {
        assert!(optional_digest(Some(vec![0; 31])).is_err());
        assert_eq!(optional_digest(Some(vec![7; 32])).unwrap(), Some([7; 32]));
    }

    struct ScratchDatabase {
        admin: PgPool,
        pool: PgPool,
        name: String,
    }

    impl ScratchDatabase {
        async fn create() -> Self {
            let admin_url = std::env::var("BUZZ_TEST_DATABASE_URL")
                .expect("Meeting Community-read tests require explicit BUZZ_TEST_DATABASE_URL");
            let admin_name = admin_url
                .rsplit('/')
                .next()
                .and_then(|tail| tail.split(['?', '#']).next())
                .filter(|name| !name.is_empty())
                .expect("BUZZ_TEST_DATABASE_URL must include a database name");
            assert!(
                admin_name.starts_with("buzz_"),
                "refused non-disposable administrative database {admin_name}"
            );
            let admin = PgPool::connect(&admin_url)
                .await
                .expect("connect disposable database server");
            let name = format!(
                "buzz_meeting_read_{}",
                &Uuid::new_v4().simple().to_string()[..20]
            );
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
                .execute(&admin)
                .await
                .expect("create Meeting Community-read scratch database");
            let slash = admin_url.rfind('/').expect("database URL has path");
            let database_url = format!("{}/{}", &admin_url[..slash], name);
            let pool = PgPool::connect(&database_url)
                .await
                .expect("connect Meeting Community-read scratch database");
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate Meeting Community-read scratch database");
            Self { admin, pool, name }
        }

        async fn cleanup(self) {
            let actual: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(&self.pool)
                .await
                .expect("read scratch database identity");
            assert_eq!(actual, self.name);
            assert!(actual.starts_with("buzz_meeting_read_"));
            self.pool.close().await;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.name
            )))
            .execute(&self.admin)
            .await
            .expect("drop Meeting Community-read scratch database");
            self.admin.close().await;
        }
    }

    async fn seed_legacy_meeting(
        pool: &PgPool,
        community_id: CommunityId,
        ordinal: u8,
        source_channel_id: Option<Uuid>,
    ) -> Uuid {
        let meeting_id = Uuid::new_v4();
        let host = vec![ordinal; 32];
        sqlx::query(
            "INSERT INTO channels \
                 (community_id, id, name, channel_type, visibility, created_by, room_kind) \
             VALUES ($1, $2, $3, 'stream', 'private', $4, 'meeting')",
        )
        .bind(community_id.as_uuid())
        .bind(meeting_id)
        .bind(format!("legacy meeting {ordinal}"))
        .bind(&host)
        .execute(pool)
        .await
        .expect("seed legacy Meeting channel");
        sqlx::query(
            "INSERT INTO meeting_sessions \
                 (community_id, session_id, create_event_id, host_pubkey, \
                  source_channel_id, status, ended_at, ended_by, end_event_id) \
             VALUES ($1, $2, $3, $4, $5, 'ended', clock_timestamp(), $4, $6)",
        )
        .bind(community_id.as_uuid())
        .bind(meeting_id)
        .bind(vec![ordinal.saturating_add(20); 32])
        .bind(&host)
        .bind(source_channel_id)
        .bind(vec![ordinal.saturating_add(40); 32])
        .execute(pool)
        .await
        .expect("seed legacy Meeting session");
        meeting_id
    }

    #[tokio::test]
    #[ignore = "requires explicit disposable Postgres with CREATE DATABASE"]
    async fn approval_rechecks_full_corpus_and_publishes_without_rewrites() {
        let scratch = ScratchDatabase::create().await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(format!("meeting-read-{}.test", community_id.as_uuid()))
            .execute(&scratch.pool)
            .await
            .expect("seed Community");
        let open_source = Uuid::new_v4();
        let private_source = Uuid::new_v4();
        for (id, visibility, name) in [
            (open_source, "open", "open source"),
            (private_source, "private", "private source"),
        ] {
            sqlx::query(
                "INSERT INTO channels \
                     (community_id, id, name, channel_type, visibility, created_by) \
                 VALUES ($1, $2, $3, 'stream', $4::channel_visibility, $5)",
            )
            .bind(community_id.as_uuid())
            .bind(id)
            .bind(name)
            .bind(visibility)
            .bind(vec![9_u8; 32])
            .execute(&scratch.pool)
            .await
            .expect("seed source Channel");
        }
        seed_legacy_meeting(&scratch.pool, community_id, 1, Some(open_source)).await;
        seed_legacy_meeting(&scratch.pool, community_id, 2, Some(private_source)).await;
        seed_legacy_meeting(&scratch.pool, community_id, 3, None).await;
        let counts_before: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM meeting_sessions), \
                    (SELECT count(*) FROM events)",
        )
        .fetch_one(&scratch.pool)
        .await
        .expect("snapshot legacy counts");
        assert!(db
            .meeting_community_read_deployment_ready(false)
            .await
            .expect("unpublished deployment readiness"));

        let paused = db
            .set_meeting_community_read_create_paused(community_id, true)
            .await
            .expect("pause Meeting creation");
        assert!(paused.create_paused);
        let mut create_tx = scratch.pool.begin().await.expect("begin Create gate probe");
        assert!(
            ensure_meeting_create_allowed_tx(&mut create_tx, community_id)
                .await
                .is_err()
        );
        create_tx
            .rollback()
            .await
            .expect("rollback Create gate probe");

        let audit = db
            .audit_legacy_meeting_visibility(community_id)
            .await
            .expect("audit legacy Meeting corpus");
        assert_eq!(audit.meeting_count, 3);
        assert_eq!(
            audit.source_risks,
            MeetingSourceRiskCounts {
                community_wide: 1,
                private: 1,
                missing: 1,
            }
        );
        assert_eq!(
            db.audit_legacy_meeting_visibility(community_id)
                .await
                .expect("repeat deterministic audit"),
            audit
        );
        let mut wrong = audit.digest;
        wrong[0] ^= 0xff;
        assert!(db
            .approve_legacy_meeting_visibility(
                community_id,
                audit.watermark,
                &wrong,
                "test-operator"
            )
            .await
            .is_err());
        db.approve_legacy_meeting_visibility(
            community_id,
            audit.watermark,
            &audit.digest,
            "test-operator",
        )
        .await
        .expect("approve exact legacy Meeting audit");
        let enabled = db
            .enable_meeting_community_read(community_id)
            .await
            .expect("publish Community Meeting reads");
        assert!(enabled.enabled);
        assert!(!enabled.create_paused);
        assert_eq!(enabled.audit_digest, Some(audit.digest));
        assert!(!db
            .meeting_community_read_deployment_ready(false)
            .await
            .expect("published deployment without master"));
        assert!(db
            .meeting_community_read_deployment_ready(true)
            .await
            .expect("published deployment with master"));
        let mut create_tx = scratch
            .pool
            .begin()
            .await
            .expect("begin enabled Create probe");
        ensure_meeting_create_allowed_tx(&mut create_tx, community_id)
            .await
            .expect("Create resumes after atomic publication");
        create_tx
            .rollback()
            .await
            .expect("rollback enabled Create probe");
        assert!(
            sqlx::query(
                "UPDATE communities SET meeting_community_read_enabled = FALSE WHERE id = $1"
            )
            .bind(community_id.as_uuid())
            .execute(&scratch.pool)
            .await
            .is_err(),
            "published read contract must be irreversible"
        );
        let counts_after: (i64, i64) = sqlx::query_as(
            "SELECT (SELECT count(*) FROM meeting_sessions), \
                    (SELECT count(*) FROM events)",
        )
        .fetch_one(&scratch.pool)
        .await
        .expect("snapshot post-publication counts");
        assert_eq!(counts_after, counts_before);
        scratch.cleanup().await;
    }
}
