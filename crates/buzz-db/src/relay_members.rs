//! Relay-level membership persistence (NIP-43).
//!
//! The `relay_members` table is community-scoped: its primary key is
//! `(community_id, pubkey)`. Every read, write, and list is bound to a single
//! `community_id` so that admitting a pubkey to community A never admits it to
//! community B (NIP-43 admission confinement). `pubkey` values are 64-char
//! lowercase hex strings.

use chrono::{DateTime, Utc};
use nostr::PublicKey;
use sqlx::{PgPool, Postgres, Row as _, Transaction};

use crate::error::{DbError, Result};
use crate::CommunityId;

/// Acquire the exclusive Community/Project lock used by every membership
/// writer.
pub(crate) async fn acquire_membership_write_lock(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
) -> Result<()> {
    crate::community_lock::acquire(tx, community, false).await?;
    Ok(())
}

async fn begin_membership_write(
    pool: &PgPool,
    community: CommunityId,
) -> Result<Transaction<'_, Postgres>> {
    let mut tx = pool.begin().await?;
    acquire_membership_write_lock(&mut tx, community).await?;
    Ok(tx)
}

pub(crate) async fn project_view_schema_version_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
) -> Result<i16> {
    let column_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM pg_attribute \
             WHERE attrelid = 'communities'::regclass \
               AND attname = 'project_view_schema_version' \
               AND NOT attisdropped \
         )",
    )
    .fetch_one(&mut **tx)
    .await?;
    if !column_exists {
        return Err(DbError::InvalidData(
            "Project View schema migration 0026 is required".to_owned(),
        ));
    }
    // The caller already holds the Community/Project advisory lock. Taking a
    // row-level UPDATE lock here would unnecessarily block unrelated event
    // inserts whose Community foreign key needs a KEY SHARE lock.
    sqlx::query_scalar("SELECT project_view_schema_version FROM communities WHERE id = $1")
        .bind(community.as_uuid())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("community {community}")))
}

/// Return one Community's Project View schema version.
///
/// The schema coordinate is mandatory. A database older than migration 0026
/// is not a supported ordinary runtime and fails closed instead of being
/// silently interpreted as Project View v1.
pub async fn project_view_schema_version(pool: &PgPool, community: CommunityId) -> Result<i16> {
    let column_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM pg_attribute \
             WHERE attrelid = 'communities'::regclass \
               AND attname = 'project_view_schema_version' \
               AND NOT attisdropped \
         )",
    )
    .fetch_one(pool)
    .await?;
    if !column_exists {
        return Err(DbError::InvalidData(
            "Project View schema migration 0026 is required".to_owned(),
        ));
    }
    sqlx::query_scalar("SELECT project_view_schema_version FROM communities WHERE id = $1")
        .bind(community.as_uuid())
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| DbError::NotFound(format!("community {community}")))
}

async fn uses_project_view_membership_governance_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
) -> Result<bool> {
    Ok(matches!(
        project_view_schema_version_in_tx(tx, community).await?,
        2 | 3
    ))
}

fn membership_coordinator_unavailable() -> DbError {
    DbError::AccessDenied("unavailable:project_view:membership_coordinator".to_owned())
}

fn greenfield_v3_owner_bootstrap_allowed(
    schema_version: i16,
    project_view_enabled: bool,
    has_preparation: bool,
    has_project_view_state: bool,
    owner_count: usize,
    relay_member_count: usize,
) -> bool {
    schema_version == 3
        && !project_view_enabled
        && !has_preparation
        && !has_project_view_state
        && owner_count == 0
        && relay_member_count == 0
}

async fn greenfield_v3_owner_bootstrap_allowed_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    owner_count: usize,
) -> Result<bool> {
    // Every committed canonical Project View object, change, and continuity
    // row is directly or transitively constrained to project_view_state for
    // the same Community. Therefore absence of that root proves absence of
    // committed canonical children. The shared Community advisory lock also
    // prevents a concurrent Project View transaction from creating the root
    // after this check and before the owner insert.
    let (
        schema_version,
        project_view_enabled,
        has_preparation,
        has_project_view_state,
        relay_member_count,
        bootstrap_lifecycle_valid,
    ): (i16, bool, bool, bool, i64, bool) = sqlx::query_as(
        "SELECT community.project_view_schema_version, community.project_view_enabled, \
                community.project_view_preparation_operation_id IS NOT NULL, \
                EXISTS (SELECT 1 FROM project_view_state state \
                        WHERE state.community_id = community.id), \
                (SELECT count(*) FROM relay_members member \
                 WHERE member.community_id = community.id), \
                project_view_v3_bootstrap_lifecycle_valid(community.id) \
         FROM communities community WHERE community.id = $1",
    )
    .bind(community.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| DbError::NotFound(format!("community {community}")))?;

    let relay_member_count = usize::try_from(relay_member_count).map_err(|_| {
        DbError::InvalidData(format!(
            "negative relay member count for community {community}"
        ))
    })?;

    Ok(bootstrap_lifecycle_valid
        && greenfield_v3_owner_bootstrap_allowed(
            schema_version,
            project_view_enabled,
            has_preparation,
            has_project_view_state,
            owner_count,
            relay_member_count,
        ))
}

pub(crate) async fn insert_relay_member_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    pubkey: &str,
    role: &str,
    added_by: Option<&str>,
) -> Result<bool> {
    let result = sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role, added_by) \
         VALUES ($1, $2, $3, $4) ON CONFLICT (community_id, pubkey) DO NOTHING",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .bind(role)
    .bind(added_by)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub(crate) async fn has_active_assignment_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    pubkey: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_role_assignments \
             WHERE community_id = $1 AND member_pubkey = $2 AND ended_at IS NULL \
         )",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await?)
}

pub(crate) async fn has_owned_managed_agent_assignment_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    owner_pubkey: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 \
             FROM users agent \
             JOIN project_role_assignments assignment \
               ON assignment.community_id = agent.community_id \
              AND assignment.member_pubkey = encode(agent.pubkey, 'hex') \
              AND assignment.ended_at IS NULL \
             WHERE agent.community_id = $1 \
               AND encode(agent.agent_owner_pubkey, 'hex') = $2 \
         )",
    )
    .bind(community.as_uuid())
    .bind(owner_pubkey)
    .fetch_one(&mut **tx)
    .await?)
}

/// Derive the non-owner Community level implied by a Member's active Role
/// Assignment inside an already locked membership transaction.
///
/// Project View membership governance uses this when an owner is transferred or an Assignment
/// ends; Community `owner` itself remains an out-of-band governance root.
pub async fn assignment_derived_member_role_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    pubkey: &str,
) -> Result<String> {
    sqlx::query_scalar(
        "SELECT CASE WHEN EXISTS ( \
             SELECT 1 \
             FROM project_role_assignments assignment \
             JOIN project_view_objects role_object \
               ON role_object.community_id = assignment.community_id \
              AND role_object.object_id = assignment.role_id \
             WHERE assignment.community_id = $1 \
               AND assignment.member_pubkey = $2 \
               AND assignment.ended_at IS NULL \
               AND role_object.role_level = 'admin' \
         ) THEN 'admin' ELSE 'member' END",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await
    .map_err(DbError::from)
}

async fn known_managed_agent_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    pubkey: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM users \
             WHERE community_id = $1 \
               AND encode(pubkey, 'hex') = $2 \
               AND agent_owner_pubkey IS NOT NULL \
         )",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await?)
}

/// One atomic Relay-membership view of a principal.
///
/// For a known managed Agent, `managed_owner_eligible` includes both
/// principals' persistent-ban state, the owner's current Community
/// membership, and the requirement that the owner is not itself a managed
/// Agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayMembershipIdentity {
    /// Whether the principal has a direct `relay_members` row.
    pub direct_member: bool,
    /// Persisted owner of a known managed Agent.
    pub managed_owner_pubkey: Option<Vec<u8>>,
    /// Whether the managed Agent and owner currently satisfy the complete
    /// owner-backed membership rule.
    pub managed_owner_eligible: bool,
}

const ELIGIBLE_DIRECT_HUMAN_ROLE_SQL: &str = "SELECT member.role \
     FROM relay_members member \
     LEFT JOIN users actor \
       ON actor.community_id = member.community_id \
      AND actor.pubkey = decode(member.pubkey, 'hex') \
     WHERE member.community_id = $1 AND member.pubkey = $2 \
       AND ($3::boolean = FALSE OR member.role = 'owner') \
       AND actor.agent_owner_pubkey IS NULL \
       AND NOT EXISTS ( \
           SELECT 1 FROM community_bans restriction \
           WHERE restriction.community_id = member.community_id \
             AND restriction.pubkey = $4 \
             AND ( \
                 (restriction.banned AND (restriction.ban_expires_at IS NULL \
                     OR restriction.ban_expires_at > clock_timestamp())) \
                 OR restriction.muted_until > clock_timestamp() \
             ) \
       )";

/// Resolve the current eligible direct-Human role inside an existing
/// Community transaction.
///
/// This is the canonical identity predicate for reviewed Project View
/// migration input. Managed Agents, owner-delegated identities, active bans,
/// and active timeouts all fail closed.
pub(crate) async fn eligible_direct_human_role_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community: CommunityId,
    pubkey: &PublicKey,
    owner_only: bool,
) -> Result<Option<String>> {
    let bytes = pubkey.to_bytes();
    Ok(sqlx::query_scalar(ELIGIBLE_DIRECT_HUMAN_ROLE_SQL)
        .bind(community.as_uuid())
        .bind(pubkey.to_hex())
        .bind(owner_only)
        .bind(bytes.as_slice())
        .fetch_optional(&mut **tx)
        .await?)
}

/// Resolve the current eligible direct-Human role in one database snapshot.
pub async fn eligible_direct_human_role(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &PublicKey,
    owner_only: bool,
) -> Result<Option<String>> {
    let bytes = pubkey.to_bytes();
    Ok(sqlx::query_scalar(ELIGIBLE_DIRECT_HUMAN_ROLE_SQL)
        .bind(community.as_uuid())
        .bind(pubkey.to_hex())
        .bind(owner_only)
        .bind(bytes.as_slice())
        .fetch_optional(pool)
        .await?)
}

/// Resolve one principal's direct and managed-Agent membership state in one
/// database snapshot.
pub async fn relay_membership_identity(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &[u8],
) -> Result<RelayMembershipIdentity> {
    let (direct_member, managed_owner_pubkey, managed_owner_eligible): (
        bool,
        Option<Vec<u8>>,
        bool,
    ) = sqlx::query_as(
        "SELECT \
             EXISTS ( \
                 SELECT 1 FROM relay_members direct_member \
                 WHERE direct_member.community_id = $1 \
                   AND direct_member.pubkey = encode($2::bytea, 'hex') \
             ), \
             agent.agent_owner_pubkey, \
             COALESCE( \
                 agent.agent_owner_pubkey IS NOT NULL \
                 AND EXISTS ( \
                     SELECT 1 FROM relay_members owner_member \
                     WHERE owner_member.community_id = $1 \
                       AND owner_member.pubkey = encode(agent.agent_owner_pubkey, 'hex') \
                 ) \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM users owner_actor \
                     WHERE owner_actor.community_id = $1 \
                       AND owner_actor.pubkey = agent.agent_owner_pubkey \
                       AND owner_actor.agent_owner_pubkey IS NOT NULL \
                 ) \
                 AND NOT EXISTS ( \
                     SELECT 1 FROM community_bans restriction \
                     WHERE restriction.community_id = $1 \
                       AND restriction.pubkey IN (agent.pubkey, agent.agent_owner_pubkey) \
                       AND restriction.banned \
                       AND ( \
                           restriction.ban_expires_at IS NULL \
                           OR restriction.ban_expires_at > clock_timestamp() \
                       ) \
                 ), \
                 FALSE \
             ) \
         FROM (VALUES (1)) AS singleton(dummy) \
         LEFT JOIN users agent \
           ON agent.community_id = $1 AND agent.pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .fetch_one(pool)
    .await?;
    Ok(RelayMembershipIdentity {
        direct_member,
        managed_owner_pubkey,
        managed_owner_eligible,
    })
}

/// Check a self-proving NIP-OA Agent/owner pair against the same eligibility
/// rule used for a persisted managed Agent.
pub async fn delegated_agent_owner_is_eligible(
    pool: &PgPool,
    community: CommunityId,
    agent_pubkey: &[u8],
    owner_pubkey: &[u8],
) -> Result<bool> {
    sqlx::query_scalar(
        "SELECT \
             EXISTS ( \
                 SELECT 1 FROM relay_members owner_member \
                 WHERE owner_member.community_id = $1 \
                   AND owner_member.pubkey = encode($3::bytea, 'hex') \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM users owner_actor \
                 WHERE owner_actor.community_id = $1 \
                   AND owner_actor.pubkey = $3 \
                   AND owner_actor.agent_owner_pubkey IS NOT NULL \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM community_bans restriction \
                 WHERE restriction.community_id = $1 \
                   AND restriction.pubkey IN ($2, $3) \
                   AND restriction.banned \
                   AND ( \
                       restriction.ban_expires_at IS NULL \
                       OR restriction.ban_expires_at > clock_timestamp() \
                   ) \
             )",
    )
    .bind(community.as_uuid())
    .bind(agent_pubkey)
    .bind(owner_pubkey)
    .fetch_one(pool)
    .await
    .map_err(DbError::from)
}

/// A single relay member record.
#[derive(Debug, Clone)]
pub struct RelayMember {
    /// 64-char lowercase hex pubkey.
    pub pubkey: String,
    /// Role: `"owner"`, `"admin"`, or `"member"`.
    pub role: String,
    /// Hex pubkey of who added this member, or `None` for bootstrap entries.
    pub added_by: Option<String>,
    /// When the member was added.
    pub created_at: DateTime<Utc>,
    /// When the record was last updated.
    pub updated_at: DateTime<Utc>,
}

/// Returns `true` if `pubkey` (64-char hex) is a member of `community`.
pub async fn is_relay_member(pool: &PgPool, community: CommunityId, pubkey: &str) -> Result<bool> {
    let row = sqlx::query("SELECT 1 FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(community.as_uuid())
        .bind(pubkey)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

/// Returns the relay member record for `pubkey` in `community`, or `None`.
pub async fn get_relay_member(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
) -> Result<Option<RelayMember>> {
    let row = sqlx::query(
        "SELECT pubkey, role, added_by, created_at, updated_at \
         FROM relay_members WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .fetch_optional(pool)
    .await?;

    row.map(|r| -> std::result::Result<RelayMember, sqlx::Error> {
        Ok(RelayMember {
            pubkey: r.try_get("pubkey")?,
            role: r.try_get("role")?,
            added_by: r.try_get("added_by")?,
            created_at: r.try_get("created_at")?,
            updated_at: r.try_get("updated_at")?,
        })
    })
    .transpose()
    .map_err(crate::error::DbError::from)
}

/// Returns all relay members of `community` ordered by `created_at` ascending.
pub async fn list_relay_members(pool: &PgPool, community: CommunityId) -> Result<Vec<RelayMember>> {
    let rows = sqlx::query(
        "SELECT pubkey, role, added_by, created_at, updated_at \
         FROM relay_members WHERE community_id = $1 ORDER BY created_at ASC",
    )
    .bind(community.as_uuid())
    .fetch_all(pool)
    .await?;

    rows.into_iter()
        .map(|r| -> std::result::Result<RelayMember, sqlx::Error> {
            Ok(RelayMember {
                pubkey: r.try_get("pubkey")?,
                role: r.try_get("role")?,
                added_by: r.try_get("added_by")?,
                created_at: r.try_get("created_at")?,
                updated_at: r.try_get("updated_at")?,
            })
        })
        .collect::<std::result::Result<Vec<_>, sqlx::Error>>()
        .map_err(crate::error::DbError::from)
}

/// Adds a new relay member to `community`.
///
/// Returns `true` if the row was actually inserted, `false` if the pubkey
/// already existed in this community (idempotent — `ON CONFLICT DO NOTHING` on
/// the `(community_id, pubkey)` primary key).
pub async fn add_relay_member(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    role: &str,
    added_by: Option<&str>,
) -> Result<bool> {
    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        if matches!(role, "admin" | "owner") {
            return Err(DbError::AccessDenied(
                "forbidden:membership:role_requires_governance".to_owned(),
            ));
        }
        return Err(membership_coordinator_unavailable());
    }
    let inserted = insert_relay_member_in_tx(&mut tx, community, pubkey, role, added_by).await?;
    tx.commit().await?;
    Ok(inserted)
}

/// Claims relay membership via an invite and atomically persists policy evidence.
///
/// Returns `true` when membership was inserted, or `false` when the pubkey was
/// already a member. A configured `policy_version` is recorded in the same
/// transaction, so membership cannot be granted without its acceptance record.
pub async fn claim_relay_membership(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    role: &str,
    policy_version: Option<&str>,
) -> Result<bool> {
    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        if role != "member" {
            return Err(DbError::AccessDenied(
                "forbidden:membership:invite_cannot_grant_role".to_owned(),
            ));
        }
        return Err(membership_coordinator_unavailable());
    }
    let inserted =
        insert_relay_member_in_tx(&mut tx, community, pubkey, role, Some("invite")).await?;

    if let Some(version) = policy_version {
        sqlx::query(
            "INSERT INTO join_policy_acceptances (community_id, pubkey, policy_version) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(community.as_uuid())
        .bind(pubkey)
        .bind(version)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(inserted)
}

/// Returns whether a member has persisted acceptance evidence for a policy version.
pub async fn has_join_policy_acceptance(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    policy_version: &str,
) -> Result<bool> {
    let row = sqlx::query(
        "SELECT 1 FROM join_policy_acceptances \
         WHERE community_id = $1 AND pubkey = $2 AND policy_version = $3",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .bind(policy_version)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// The result of a relay member removal attempt.
#[derive(Debug, PartialEq)]
pub enum RemoveResult {
    /// Member was successfully removed.
    Removed,
    /// The pubkey belongs to the relay owner — removal is forbidden.
    IsOwner,
    /// No member with the given pubkey exists.
    NotFound,
    /// The member exists but their role doesn't match the expected role.
    RoleMismatch,
    /// Project View still has an active Assignment for this Member.
    AssignmentActive,
    /// The Human owns a managed Agent with an active Assignment.
    ManagedAgentAssignmentActive,
}

/// Removes a relay member atomically, refusing to delete the owner.
///
/// Uses a single conditional `DELETE … WHERE role <> 'owner'` so the
/// owner-protection check and the deletion are one atomic operation —
/// no TOCTOU race between a separate read and delete.
pub async fn remove_relay_member(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
) -> Result<RemoveResult> {
    let revocation_event_id: [u8; 32] = rand::random();
    remove_relay_member_with_revocation(pool, community, pubkey, &revocation_event_id).await
}

/// Removes a relay member and atomically enqueues Meeting cleanup using the
/// signed event or audit identifier that authorized the revocation.
pub async fn remove_relay_member_with_revocation(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    revocation_event_id: &[u8],
) -> Result<RemoveResult> {
    let revoked_pubkey = decode_pubkey(pubkey)?;
    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        if has_active_assignment_in_tx(&mut tx, community, pubkey).await? {
            return Ok(RemoveResult::AssignmentActive);
        }
        if has_owned_managed_agent_assignment_in_tx(&mut tx, community, pubkey).await? {
            return Ok(RemoveResult::ManagedAgentAssignmentActive);
        }
        return Err(membership_coordinator_unavailable());
    }
    let result = sqlx::query(
        "DELETE FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2 AND role <> 'owner'",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() > 0 {
        crate::meeting_baton::enqueue_revocation_job_tx(
            &mut tx,
            community,
            uuid::Uuid::new_v4(),
            &revoked_pubkey,
            revocation_event_id,
        )
        .await?;
        tx.commit().await?;
        return Ok(RemoveResult::Removed);
    }

    // rows_affected == 0: either not found or is owner.  One cheap read to
    // distinguish the two cases so callers can return the right error message.
    let exists = sqlx::query("SELECT 1 FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(community.as_uuid())
        .bind(pubkey)
        .fetch_optional(&mut *tx)
        .await?;

    let result = if exists.is_some() {
        Ok(RemoveResult::IsOwner)
    } else {
        Ok(RemoveResult::NotFound)
    };
    tx.rollback().await?;
    result
}

/// Removes a relay member only if their current role matches `expected_role`.
///
/// The delete and the role check are collapsed into a single
/// `DELETE … WHERE pubkey = $1 AND role = $2`, making the operation atomic —
/// no TOCTOU race between a prior read and this delete.
///
/// Returns:
/// - `Removed` — row was deleted.
/// - `NotFound` — no member with that pubkey exists.
/// - `IsOwner` — member exists with role `"owner"` (cannot be removed).
/// - `RoleMismatch` — member exists but their role no longer matches
///   `expected_role` (e.g., they were promoted between the caller's read and
///   this delete).
pub async fn remove_relay_member_if_role(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    expected_role: &str,
) -> Result<RemoveResult> {
    let revocation_event_id: [u8; 32] = rand::random();
    remove_relay_member_if_role_with_revocation(
        pool,
        community,
        pubkey,
        expected_role,
        &revocation_event_id,
    )
    .await
}

/// Removes a relay member with a matching role and atomically enqueues Meeting
/// cleanup using the signed event or audit identifier that caused revocation.
pub async fn remove_relay_member_if_role_with_revocation(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    expected_role: &str,
    revocation_event_id: &[u8],
) -> Result<RemoveResult> {
    let revoked_pubkey = decode_pubkey(pubkey)?;
    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        if has_active_assignment_in_tx(&mut tx, community, pubkey).await? {
            return Ok(RemoveResult::AssignmentActive);
        }
        if has_owned_managed_agent_assignment_in_tx(&mut tx, community, pubkey).await? {
            return Ok(RemoveResult::ManagedAgentAssignmentActive);
        }
        return Err(membership_coordinator_unavailable());
    }
    let result = sqlx::query(
        "DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2 AND role = $3",
    )
    .bind(community.as_uuid())
    .bind(pubkey)
    .bind(expected_role)
    .execute(&mut *tx)
    .await?;

    if result.rows_affected() > 0 {
        crate::meeting_baton::enqueue_revocation_job_tx(
            &mut tx,
            community,
            uuid::Uuid::new_v4(),
            &revoked_pubkey,
            revocation_event_id,
        )
        .await?;
        tx.commit().await?;
        return Ok(RemoveResult::Removed);
    }

    // rows_affected == 0: either not found or role changed. One cheap read to
    // distinguish the cases so callers can return the right error message.
    let row = sqlx::query("SELECT role FROM relay_members WHERE community_id = $1 AND pubkey = $2")
        .bind(community.as_uuid())
        .bind(pubkey)
        .fetch_optional(&mut *tx)
        .await?;

    let result = match row {
        None => Ok(RemoveResult::NotFound),
        Some(r) => {
            let role: String = r.try_get("role")?;
            if role == "owner" {
                Ok(RemoveResult::IsOwner)
            } else {
                // Role changed between the caller's check and this delete
                // (e.g., target was promoted to admin). Signal that the
                // caller no longer has authority to remove this target.
                Ok(RemoveResult::RoleMismatch)
            }
        }
    };
    tx.rollback().await?;
    result
}

fn decode_pubkey(pubkey: &str) -> Result<Vec<u8>> {
    let decoded = hex::decode(pubkey).map_err(|error| {
        crate::error::DbError::InvalidData(format!("invalid relay member pubkey: {error}"))
    })?;
    if decoded.len() != 32 {
        return Err(crate::error::DbError::InvalidData(
            "relay member pubkey must decode to exactly 32 bytes".to_string(),
        ));
    }
    Ok(decoded)
}

/// Updates the role of an existing relay member in `community`. Returns `true`
/// if updated.
pub async fn update_relay_member_role(
    pool: &PgPool,
    community: CommunityId,
    pubkey: &str,
    new_role: &str,
) -> Result<bool> {
    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        let current_role: Option<String> = sqlx::query_scalar(
            "SELECT role FROM relay_members WHERE community_id = $1 AND pubkey = $2 FOR UPDATE",
        )
        .bind(community.as_uuid())
        .bind(pubkey)
        .fetch_optional(&mut *tx)
        .await?;
        if new_role == "admin" {
            return Err(DbError::AccessDenied(
                "forbidden:membership:leader_assignment_required".to_owned(),
            ));
        }
        if current_role.as_deref() == Some("admin")
            && has_active_assignment_in_tx(&mut tx, community, pubkey).await?
        {
            return Err(DbError::AccessDenied(
                "forbidden:membership:assignment_active".to_owned(),
            ));
        }
        return Err(membership_coordinator_unavailable());
    }

    let result = sqlx::query(
        "UPDATE relay_members SET role = $1, updated_at = now() \
         WHERE community_id = $2 AND pubkey = $3 AND role <> 'owner'",
    )
    .bind(new_role)
    .bind(community.as_uuid())
    .bind(pubkey)
    .execute(&mut *tx)
    .await?;
    let updated = result.rows_affected() > 0;
    tx.commit().await?;
    Ok(updated)
}

/// Ensures the configured owner pubkey holds the `"owner"` role in
/// `community`.
///
/// A fresh schema-v3 Community has no canonical Project View state yet, so its
/// first Human owner must exist before that owner can authorize `prepare-v3`
/// and `initialize-v3`. This deployment-root path permits exactly that empty,
/// disabled, unprepared state with no Relay Members. Once any membership
/// exists, preparation starts, or canonical state exists, Project View
/// membership governance fails closed; an owner rotation must use its
/// coordinated governance path. Repeating the bootstrap with the already-sole
/// owner remains an idempotent no-op.
///
/// Legacy schema v1 retains its historical bootstrap-and-demote behavior for
/// explicit migration/recovery use. The operation remains scoped to one
/// Community.
///
/// Runs in a single transaction. Safe to call at every startup — idempotent.
///
/// **Bootstrap authority exception:** This function is called by startup
/// initialization, operator provisioning (`community_provisioning.rs`), and
/// the loopback-only Carryforth Desktop bootstrap endpoint. The Desktop path
/// is restricted to the exact local Community and uses the same greenfield
/// transaction fence: it cannot rotate or replace an existing owner. This
/// function does not enforce the per-owner community limit
/// (`MAX_COMMUNITIES_PER_OWNER`) or acquire the per-recipient advisory lock.
/// Those remain end-user invariants of `create_community_with_owner` and
/// `transfer_ownership`.
pub async fn bootstrap_owner(
    pool: &PgPool,
    community: CommunityId,
    owner_pubkey: &str,
) -> Result<()> {
    let pubkey = owner_pubkey.to_ascii_lowercase();
    let mut tx = begin_membership_write(pool, community).await?;
    let governed = uses_project_view_membership_governance_in_tx(&mut tx, community).await?;
    if governed && known_managed_agent_in_tx(&mut tx, community, &pubkey).await? {
        return Err(DbError::AccessDenied(
            "forbidden:managed_agent:owner_ineligible".to_owned(),
        ));
    }
    if governed {
        let current_owners: Vec<String> = sqlx::query_scalar(
            "SELECT pubkey FROM relay_members \
             WHERE community_id = $1 AND role = 'owner' FOR UPDATE",
        )
        .bind(community.as_uuid())
        .fetch_all(&mut *tx)
        .await?;
        if current_owners.as_slice() == [pubkey.as_str()] {
            tx.rollback().await?;
            return Ok(());
        }
        if !greenfield_v3_owner_bootstrap_allowed_in_tx(&mut tx, community, current_owners.len())
            .await?
        {
            return Err(membership_coordinator_unavailable());
        }
    }

    // 1. Upsert the configured owner for this community.
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role, added_by) \
         VALUES ($1, $2, 'owner', NULL) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = 'owner', updated_at = now()",
    )
    .bind(community.as_uuid())
    .bind(&pubkey)
    .execute(&mut *tx)
    .await?;

    // 2. Schema v1 preserves the legacy demotion behaviour. The schema-v3
    // greenfield exception can reach this point only with zero current owners,
    // so this update is a no-op there. Every governed owner rotation returned
    // above because it requires the source/audit/projection coordinator.
    sqlx::query(
        "UPDATE relay_members SET role = 'admin', updated_at = now() \
         WHERE community_id = $1 AND role = 'owner' AND pubkey <> $2",
    )
    .bind(community.as_uuid())
    .bind(&pubkey)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// The result of a transfer-ownership attempt.
#[derive(Debug, PartialEq)]
pub enum TransferResult {
    /// Transfer completed: the new owner was upserted and the previous
    /// owner(s) were demoted to `member`.
    Transferred {
        /// Pubkey of the previous sole owner, if exactly one existed.
        previous_owner: Option<String>,
    },
    /// The new owner pubkey is already the sole owner — nothing to do.
    AlreadyOwner,
    /// No owner row exists for this community (community may not exist).
    NoOwner,
    /// The `expected_owner_pubkey` did not match the current owner. A
    /// concurrent transfer or owner rotation has already changed ownership.
    /// The caller must NOT retry blindly — re-read ownership and re-evaluate.
    OwnerConflict,
    /// The transferee already owns the maximum number of communities.
    /// Enforced atomically inside the transfer transaction so concurrent
    /// transfers to the same recipient cannot both pass the limit.
    LimitReached,
    /// The requested owner is a known managed Agent identity.
    ManagedAgentIneligible,
}

/// Maximum number of communities a single pubkey can own. Enforced at the
/// relay layer — the authoritative layer — so that concurrent transfers or
/// transfer-vs-create races cannot both pass a preflight count.
pub const MAX_COMMUNITIES_PER_OWNER: i64 = 3;

/// Stable advisory-lock key for serializing ownership-granting operations
/// (transfer + create) per recipient pubkey. Uses FNV-1a over the hex pubkey
/// so the same recipient always maps to the same lock across processes.
pub fn owner_count_advisory_lock_key(pubkey_hex: &str) -> i64 {
    let mut h: u64 = 0xcbf29ce484222325; // FNV offset basis
    for b in pubkey_hex.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3); // FNV prime
    }
    h as i64
}

/// Atomically transfers ownership of `community` to `new_owner_pubkey`.
///
/// Runs in a single transaction:
/// 1. Acquires a transaction-scoped advisory lock on the *transferee* pubkey
///    so that concurrent transfers to the same recipient serialize. The same
///    lock key is also used by `Db::create_community_with_owner` to prevent
///    transfer-vs-create races.
/// 2. Locks the current owner row `FOR UPDATE` and verifies
///    `expected_owner_pubkey` matches. This prevents a stale-owner race where
///    a delayed/retried request overwrites a completed transfer.
/// 3. Enforces the [`MAX_COMMUNITIES_PER_OWNER`] limit on the transferee by
///    counting owned communities inside the same transaction.
/// 4. Upserts `new_owner_pubkey` as `owner` (insert or promote).
/// 5. Demotes every other owner in this Community to `member`.
///
/// Scoped to one community — an ownership transfer in A never touches B.
/// A Project View-governed Community fails closed until the source/audit/projection coordinator
/// can commit the ownership change and its derived old-owner level together.
pub async fn transfer_ownership(
    pool: &PgPool,
    community: CommunityId,
    new_owner_pubkey: &str,
    expected_owner_pubkey: &str,
) -> Result<TransferResult> {
    let pubkey = new_owner_pubkey.to_ascii_lowercase();
    let expected_owner = expected_owner_pubkey.to_ascii_lowercase();
    let mut tx = begin_membership_write(pool, community).await?;
    let governed = uses_project_view_membership_governance_in_tx(&mut tx, community).await?;
    if governed && known_managed_agent_in_tx(&mut tx, community, &pubkey).await? {
        return Ok(TransferResult::ManagedAgentIneligible);
    }
    if governed {
        return Err(membership_coordinator_unavailable());
    }

    // 1. Serialize on the transferee so concurrent transfers to the same
    //    recipient cannot both pass the ownership count check.
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(owner_count_advisory_lock_key(&pubkey))
        .execute(&mut *tx)
        .await?;

    // 2. Lock the current owner row FOR UPDATE and verify the expected owner.
    //    FOR UPDATE prevents the stale-owner race: a concurrent transfer that
    //    already changed the owner will block on this lock until our txn
    //    completes (or vice versa), and the expected_owner check will fail.
    let existing_owners: Vec<String> = sqlx::query_scalar(
        "SELECT pubkey FROM relay_members \
         WHERE community_id = $1 AND role = 'owner' \
         FOR UPDATE",
    )
    .bind(community.as_uuid())
    .fetch_all(&mut *tx)
    .await?;

    if existing_owners.is_empty() {
        tx.rollback().await?;
        return Ok(TransferResult::NoOwner);
    }

    // Stale-owner guard: if the current owner doesn't match the expected
    // owner, a concurrent transfer or rotation has already changed hands.
    if !existing_owners.iter().any(|p| p == &expected_owner) {
        tx.rollback().await?;
        return Ok(TransferResult::OwnerConflict);
    }

    // Already the sole owner — no transfer needed.
    if existing_owners.len() == 1 && existing_owners[0] == pubkey {
        tx.rollback().await?;
        return Ok(TransferResult::AlreadyOwner);
    }

    let previous_owner = if existing_owners.len() == 1 {
        Some(existing_owners[0].clone())
    } else {
        existing_owners.iter().find(|p| **p != pubkey).cloned()
    };

    // 3. Enforce the transferee's community ownership limit inside the same
    //    transaction that holds the advisory lock. This is the authoritative
    //    check — kgoose's preflight count is advisory only.
    let owned_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM relay_members WHERE pubkey = $1 AND role = 'owner'",
    )
    .bind(&pubkey)
    .fetch_one(&mut *tx)
    .await?;

    if owned_count >= MAX_COMMUNITIES_PER_OWNER {
        tx.rollback().await?;
        return Ok(TransferResult::LimitReached);
    }

    // 4. Upsert the new owner.
    sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role, added_by) \
         VALUES ($1, $2, 'owner', NULL) \
         ON CONFLICT (community_id, pubkey) DO UPDATE SET role = 'owner', updated_at = now()",
    )
    .bind(community.as_uuid())
    .bind(&pubkey)
    .execute(&mut *tx)
    .await?;

    // 5. This path is v1-only; v2 returned above.
    sqlx::query(
        "UPDATE relay_members SET role = 'member', updated_at = now() \
         WHERE community_id = $1 AND role = 'owner' AND pubkey <> $2",
    )
    .bind(community.as_uuid())
    .bind(&pubkey)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(TransferResult::Transferred { previous_owner })
}

/// Migrates existing `pubkey_allowlist` entries into `relay_members` for
/// `community` (the deployment's default community).
///
/// Converts BYTEA pubkeys to lowercase hex text and inserts them as members of
/// `community`. Returns the number of rows inserted, or 0 if:
/// - the `pubkey_allowlist` table doesn't exist, or
/// - the Community already uses a Role-governed Project View schema, where
///   legacy allowlist backfill must not run, or
/// - `relay_members` already has rows for this community (migration ran in a
///   prior v1 startup).
///
/// The empty-table guard prevents re-adding members that were intentionally
/// removed by an admin after the initial backfill.
pub async fn backfill_from_allowlist(pool: &PgPool, community: CommunityId) -> Result<u64> {
    // Check if pubkey_allowlist table exists.
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_name = 'pubkey_allowlist')",
    )
    .fetch_one(pool)
    .await?;

    if !exists {
        return Ok(0);
    }

    let mut tx = begin_membership_write(pool, community).await?;
    if uses_project_view_membership_governance_in_tx(&mut tx, community).await? {
        tx.rollback().await?;
        return Ok(0);
    }

    // Only backfill if this community's relay_members is empty — once it has
    // rows (from a previous backfill or manual admin commands), we must not
    // re-add members that were intentionally removed.
    let has_members: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM relay_members WHERE community_id = $1)")
            .bind(community.as_uuid())
            .fetch_one(&mut *tx)
            .await?;

    if has_members {
        tx.rollback().await?;
        return Ok(0);
    }

    let result = sqlx::query(
        "INSERT INTO relay_members (community_id, pubkey, role, added_by, created_at) \
         SELECT $1, encode(pubkey, 'hex'), 'member', NULL, added_at \
         FROM pubkey_allowlist \
         WHERE community_id = $1 \
         ON CONFLICT (community_id, pubkey) DO NOTHING",
    )
    .bind(community.as_uuid())
    .execute(&mut *tx)
    .await?;

    let inserted = result.rows_affected();
    tx.commit().await?;
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    const TEST_DB_URL: &str = "postgres://buzz:buzz_dev@localhost:5432/buzz";

    async fn setup_pool() -> PgPool {
        let database_url = std::env::var("BUZZ_TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .unwrap_or_else(|_| TEST_DB_URL.to_owned());
        PgPool::connect(&database_url)
            .await
            .expect("connect to test DB")
    }

    async fn make_test_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        let host = format!("relay-members-test-{}.example", id.simple());
        // These tests exercise the explicit legacy membership paths. Pin the
        // fixture instead of inheriting the schema-v3 greenfield default.
        sqlx::query(
            "INSERT INTO communities (id, host, project_view_schema_version) \
             VALUES ($1, $2, 1)",
        )
        .bind(id)
        .bind(host)
        .execute(pool)
        .await
        .expect("insert test community");
        CommunityId::from_uuid(id)
    }

    async fn make_test_v3_community(pool: &PgPool) -> CommunityId {
        let id = Uuid::new_v4();
        let host = format!("relay-members-v3-test-{}.example", id.simple());
        sqlx::query(
            "INSERT INTO communities \
                (id, host, project_view_schema_version, project_view_enabled) \
             VALUES ($1, $2, 3, FALSE)",
        )
        .bind(id)
        .bind(host)
        .execute(pool)
        .await
        .expect("insert schema-v3 test community");
        CommunityId::from_uuid(id)
    }

    fn test_pubkey() -> String {
        format!("{:064x}", Uuid::new_v4().as_u128())
    }

    async fn assert_role(pool: &PgPool, community: CommunityId, pubkey: &str, role: &str) {
        assert_eq!(
            get_relay_member(pool, community, pubkey)
                .await
                .expect("get relay member")
                .map(|member| member.role)
                .as_deref(),
            Some(role)
        );
    }

    async fn owned_community(pool: &PgPool) -> (CommunityId, String) {
        let community = make_test_community(pool).await;
        let owner = test_pubkey();
        bootstrap_owner(pool, community, &owner)
            .await
            .expect("bootstrap owner");
        (community, owner)
    }

    #[test]
    fn greenfield_v3_owner_bootstrap_boundary_is_exact() {
        assert!(greenfield_v3_owner_bootstrap_allowed(
            3, false, false, false, 0, 0
        ));

        for denied in [
            (1, false, false, false, 0, 0),
            (2, false, false, false, 0, 0),
            (3, true, false, false, 0, 0),
            (3, false, true, false, 0, 0),
            (3, false, false, true, 0, 0),
            (3, false, false, false, 1, 1),
            (3, false, false, false, 0, 1),
        ] {
            assert!(
                !greenfield_v3_owner_bootstrap_allowed(
                    denied.0, denied.1, denied.2, denied.3, denied.4, denied.5,
                ),
                "unexpectedly allowed bootstrap state {denied:?}"
            );
        }
    }

    #[test]
    fn governed_membership_error_is_not_bound_to_a_legacy_schema_name() {
        assert_eq!(
            membership_coordinator_unavailable().to_string(),
            "access denied: unavailable:project_view:membership_coordinator"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn bootstrap_owner_allows_only_the_first_greenfield_v3_human_owner() {
        let pool = setup_pool().await;
        let community = make_test_v3_community(&pool).await;
        let owner = test_pubkey();
        let replacement = test_pubkey();

        bootstrap_owner(&pool, community, &owner)
            .await
            .expect("bootstrap first schema-v3 owner");
        assert_role(&pool, community, &owner, "owner").await;

        // Startup convergence with the same configured owner is idempotent.
        bootstrap_owner(&pool, community, &owner)
            .await
            .expect("repeat schema-v3 owner bootstrap");

        let error = bootstrap_owner(&pool, community, &replacement)
            .await
            .expect_err("schema-v3 owner rotation must fail closed");
        assert_eq!(
            error.to_string(),
            "access denied: unavailable:project_view:membership_coordinator"
        );
        assert_role(&pool, community, &owner, "owner").await;
        assert!(get_relay_member(&pool, community, &replacement)
            .await
            .expect("read rejected replacement")
            .is_none());

        let state: (i16, bool, Option<Uuid>, bool) = sqlx::query_as(
            "SELECT community.project_view_schema_version, \
                    community.project_view_enabled, \
                    community.project_view_preparation_operation_id, \
                    EXISTS (SELECT 1 FROM project_view_state state \
                            WHERE state.community_id = community.id) \
             FROM communities community WHERE community.id = $1",
        )
        .bind(community.as_uuid())
        .fetch_one(&pool)
        .await
        .expect("read schema-v3 bootstrap boundary");
        assert_eq!(state, (3, false, None, false));

        let anomalous = make_test_v3_community(&pool).await;
        let preexisting_member = test_pubkey();
        let invalid_membership = sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role, added_by) \
             VALUES ($1, $2, 'member', NULL)",
        )
        .bind(anomalous.as_uuid())
        .bind(&preexisting_member)
        .execute(&pool)
        .await;
        assert!(
            invalid_membership.is_err(),
            "schema-v3 bootstrap lifecycle must reject ownerless membership"
        );
        let anomalous_owner = test_pubkey();
        bootstrap_owner(&pool, anomalous, &anomalous_owner)
            .await
            .expect("failed ownerless mutation must not poison owner bootstrap");
        assert_role(&pool, anomalous, &anomalous_owner, "owner").await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn invite_claim_persists_policy_version_and_legacy_claim_does_not() {
        let pool = setup_pool().await;
        let community = make_test_community(&pool).await;
        let policy_member = test_pubkey();
        let legacy_member = test_pubkey();
        let version = "a".repeat(64);

        assert!(
            claim_relay_membership(&pool, community, &policy_member, "member", Some(&version),)
                .await
                .expect("claim membership with policy")
        );
        assert!(
            has_join_policy_acceptance(&pool, community, &policy_member, &version)
                .await
                .expect("policy acceptance lookup")
        );

        assert!(
            claim_relay_membership(&pool, community, &legacy_member, "member", None)
                .await
                .expect("legacy claim membership")
        );
        assert!(
            !has_join_policy_acceptance(&pool, community, &legacy_member, &version)
                .await
                .expect("legacy acceptance lookup")
        );
    }

    /// NIP-43 admission confinement: a pubkey admitted to community A is *not*
    /// admitted to community B. This is the exact mutation #1285 targets — a
    /// `WHERE pubkey = $1` membership check (no community predicate) would let an
    /// A-member authenticate against B. We add the pubkey only to A and assert
    /// every read path (`is_relay_member`, `get_relay_member`, `list_relay_members`)
    /// confines it to A.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn membership_is_confined_to_its_community() {
        let pool = setup_pool().await;
        let community_a = make_test_community(&pool).await;
        let community_b = make_test_community(&pool).await;
        // 64-char lowercase hex, unique per run so reruns don't collide.
        let pubkey = test_pubkey();

        let inserted = add_relay_member(&pool, community_a, &pubkey, "member", None)
            .await
            .expect("add member to community A");
        assert!(inserted, "first insert into A should report inserted");

        // is_relay_member: member of A, NOT of B.
        assert!(
            is_relay_member(&pool, community_a, &pubkey)
                .await
                .expect("is_relay_member A"),
            "pubkey must be a member of community A"
        );
        assert!(
            !is_relay_member(&pool, community_b, &pubkey)
                .await
                .expect("is_relay_member B"),
            "pubkey admitted to A must NOT be a member of B (admission confinement)"
        );

        // get_relay_member (used by the NIP-OA owner check + admin role lookups):
        // resolves in A, absent in B.
        assert!(
            get_relay_member(&pool, community_a, &pubkey)
                .await
                .expect("get_relay_member A")
                .is_some(),
            "get_relay_member must resolve in community A"
        );
        assert!(
            get_relay_member(&pool, community_b, &pubkey)
                .await
                .expect("get_relay_member B")
                .is_none(),
            "get_relay_member must not resolve the A pubkey in community B"
        );

        // list_relay_members: B's list never contains A's member.
        let list_a = list_relay_members(&pool, community_a)
            .await
            .expect("list A");
        assert!(
            list_a.iter().any(|m| m.pubkey == pubkey),
            "community A list must contain the admitted pubkey"
        );
        let list_b = list_relay_members(&pool, community_b)
            .await
            .expect("list B");
        assert!(
            list_b.iter().all(|m| m.pubkey != pubkey),
            "community B list must not contain A's member"
        );
    }

    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn removal_and_meeting_revocation_job_commit_atomically() {
        let pool = setup_pool().await;
        let community = make_test_community(&pool).await;
        let member = test_pubkey();
        add_relay_member(&pool, community, &member, "member", None)
            .await
            .expect("add removable member");
        let revocation_event_id = [0x51_u8; 32];

        assert_eq!(
            remove_relay_member_with_revocation(&pool, community, &member, &revocation_event_id,)
                .await
                .expect("remove with durable Meeting cleanup"),
            RemoveResult::Removed
        );
        let queued: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_revocation_jobs \
                 WHERE community_id = $1 AND revocation_event_id = $2 \
             )",
        )
        .bind(community.as_uuid())
        .bind(revocation_event_id.as_slice())
        .fetch_one(&pool)
        .await
        .expect("read Meeting revocation job");
        assert!(queued);
        assert!(add_relay_member(&pool, community, &member, "member", None)
            .await
            .expect("rapidly re-add removed member"));
        let durable_after_readd: bool = sqlx::query_scalar(
            "SELECT EXISTS( \
                 SELECT 1 FROM meeting_revocation_jobs \
                 WHERE community_id = $1 AND revocation_event_id = $2 \
             )",
        )
        .bind(community.as_uuid())
        .bind(revocation_event_id.as_slice())
        .fetch_one(&pool)
        .await
        .expect("read durable job after re-add");
        assert!(durable_after_readd);

        let rollback_member = test_pubkey();
        add_relay_member(&pool, community, &rollback_member, "member", None)
            .await
            .expect("add rollback member");
        assert!(
            remove_relay_member_with_revocation(&pool, community, &rollback_member, &[1_u8; 31])
                .await
                .is_err(),
            "invalid job evidence must abort the removal transaction"
        );
        assert!(is_relay_member(&pool, community, &rollback_member)
            .await
            .expect("membership survived rollback"));
    }

    /// Owner bootstrap is community-scoped: bootstrapping the owner in A does not
    /// make that pubkey an owner (or member) of B. Guards against a global
    /// `INSERT ... (pubkey, role)` bootstrap leaking the owner across tenants.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn owner_bootstrap_is_confined_to_its_community() {
        let pool = setup_pool().await;
        let community_a = make_test_community(&pool).await;
        let community_b = make_test_community(&pool).await;
        let owner = test_pubkey();

        bootstrap_owner(&pool, community_a, &owner)
            .await
            .expect("bootstrap owner in A");

        let in_a = get_relay_member(&pool, community_a, &owner)
            .await
            .expect("get owner A")
            .expect("owner exists in A");
        assert_eq!(in_a.role, "owner", "bootstrapped pubkey must be owner in A");

        assert!(
            !is_relay_member(&pool, community_b, &owner)
                .await
                .expect("is_relay_member B"),
            "owner bootstrapped in A must NOT be a member of B"
        );
    }

    /// Transfer ownership: upserts new owner, demotes previous owner to
    /// `member` (not `admin`), and returns the previous owner's pubkey.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_demotes_old_owner_to_member() {
        let pool = setup_pool().await;
        let (community, old_owner) = owned_community(&pool).await;
        let new_owner = test_pubkey();

        let result = transfer_ownership(&pool, community, &new_owner, &old_owner)
            .await
            .expect("transfer ownership");

        assert_eq!(
            result,
            TransferResult::Transferred {
                previous_owner: Some(old_owner.clone()),
            }
        );

        assert_role(&pool, community, &new_owner, "owner").await;
        assert_role(&pool, community, &old_owner, "member").await;
    }

    /// Transferring to the current sole owner is a no-op (`AlreadyOwner`).
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_already_owner_is_noop() {
        let pool = setup_pool().await;
        let (community, owner) = owned_community(&pool).await;

        let result = transfer_ownership(&pool, community, &owner, &owner)
            .await
            .expect("transfer ownership to self");

        assert_eq!(result, TransferResult::AlreadyOwner);

        assert_role(&pool, community, &owner, "owner").await;
    }

    /// Transferring a community with no owner row returns `NoOwner`.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_no_owner_returns_no_owner() {
        let pool = setup_pool().await;
        let community = make_test_community(&pool).await;
        let new_owner = test_pubkey();
        let expected = test_pubkey();

        // No bootstrap — community exists but has no owner row.

        let result = transfer_ownership(&pool, community, &new_owner, &expected)
            .await
            .expect("transfer ownership on empty community");

        assert_eq!(result, TransferResult::NoOwner);
    }

    /// Transfer ownership is community-scoped: transferring in A does not
    /// affect ownership in B.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_is_community_scoped() {
        let pool = setup_pool().await;
        let community_a = make_test_community(&pool).await;
        let community_b = make_test_community(&pool).await;
        let owner_a = test_pubkey();
        let owner_b = test_pubkey();
        let new_owner = test_pubkey();

        bootstrap_owner(&pool, community_a, &owner_a)
            .await
            .expect("bootstrap owner A");
        bootstrap_owner(&pool, community_b, &owner_b)
            .await
            .expect("bootstrap owner B");

        transfer_ownership(&pool, community_a, &new_owner, &owner_a)
            .await
            .expect("transfer A");

        assert_role(&pool, community_a, &new_owner, "owner").await;
        assert_role(&pool, community_a, &owner_a, "member").await;
        assert_role(&pool, community_b, &owner_b, "owner").await;
        assert!(
            !is_relay_member(&pool, community_b, &new_owner)
                .await
                .expect("is_relay_member B"),
            "new owner of A must NOT be a member of B"
        );
    }

    /// Transfer ownership to someone who is already a member promotes them to
    /// owner and demotes the old owner to member.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_promotes_existing_member() {
        let pool = setup_pool().await;
        let community = make_test_community(&pool).await;
        let old_owner = test_pubkey();
        let existing_member = test_pubkey();

        bootstrap_owner(&pool, community, &old_owner)
            .await
            .expect("bootstrap owner");
        add_relay_member(&pool, community, &existing_member, "member", None)
            .await
            .expect("add member");

        let result = transfer_ownership(&pool, community, &existing_member, &old_owner)
            .await
            .expect("transfer to existing member");

        assert!(matches!(result, TransferResult::Transferred { .. }));

        assert_eq!(
            get_relay_member(&pool, community, &existing_member)
                .await
                .expect("get new owner")
                .expect("exists")
                .role,
            "owner"
        );
        assert_eq!(
            get_relay_member(&pool, community, &old_owner)
                .await
                .expect("get old owner")
                .expect("exists")
                .role,
            "member"
        );
    }

    /// Transfer returns `OwnerConflict` when `expected_owner_pubkey` doesn't
    /// match the current owner — simulates a stale/delayed request after a
    /// concurrent transfer has already changed ownership.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_returns_owner_conflict_when_expected_mismatches() {
        let pool = setup_pool().await;
        let community = make_test_community(&pool).await;
        let old_owner = test_pubkey();
        let new_owner = test_pubkey();
        let wrong_expected = test_pubkey();

        bootstrap_owner(&pool, community, &old_owner)
            .await
            .expect("bootstrap initial owner");

        // expected_owner_pubkey doesn't match the actual owner — should conflict.
        let result = transfer_ownership(&pool, community, &new_owner, &wrong_expected)
            .await
            .expect("transfer ownership with wrong expected");

        assert_eq!(result, TransferResult::OwnerConflict);

        // Old owner is still owner — nothing changed.
        assert_eq!(
            get_relay_member(&pool, community, &old_owner)
                .await
                .expect("get old owner")
                .expect("exists")
                .role,
            "owner"
        );
        // New owner was not added.
        assert!(
            get_relay_member(&pool, community, &new_owner)
                .await
                .expect("get new owner")
                .is_none(),
            "new owner must not be added on conflict"
        );
    }

    /// Transfer returns `LimitReached` when the transferee already owns the
    /// maximum number of communities. The limit is enforced inside the
    /// transfer transaction at the relay layer.
    #[tokio::test]
    #[ignore = "requires Postgres"]
    async fn transfer_ownership_returns_limit_reached_for_maxed_transferee() {
        let pool = setup_pool().await;
        let owner = test_pubkey();
        let transferee = test_pubkey();

        // Give the transferee 3 communities (the max).
        for _ in 0..3 {
            let c = make_test_community(&pool).await;
            bootstrap_owner(&pool, c, &transferee)
                .await
                .expect("bootstrap transferee community");
        }

        // Create a community owned by `owner` and try to transfer to `transferee`.
        let community = make_test_community(&pool).await;
        bootstrap_owner(&pool, community, &owner)
            .await
            .expect("bootstrap owner");

        let result = transfer_ownership(&pool, community, &transferee, &owner)
            .await
            .expect("transfer to maxed transferee");

        assert_eq!(result, TransferResult::LimitReached);

        // Owner is still owner — transfer did not happen.
        assert_eq!(
            get_relay_member(&pool, community, &owner)
                .await
                .expect("get owner")
                .expect("exists")
                .role,
            "owner"
        );
    }
}
