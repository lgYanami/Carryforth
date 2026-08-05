//! Project Document canonical state, immutable history, and restricted writes.
//!
//! Relay and operator adapters enter through restricted coordinators that hold
//! the shared Community advisory lock, re-derive pure transitions, verify full
//! signed projection bundles, and commit command/event/history/pointers
//! atomically.

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{CommunityId, EventId, PublicKey, StoredEvent};
use buzz_project_document::{
    reduce_document, CurrentDocument, DocumentAttribution, DocumentCatalog, DocumentChangeContext,
    DocumentCommandRequest, DocumentError, DocumentHeadProjection, DocumentRevision,
    DocumentRevisionProjection, DocumentSnapshot, DocumentState, DocumentTransition,
    ProjectDocument, ProjectDocumentCommand, ProjectDocumentReceipt, MAX_SAFE_REVISION,
    PROJECT_DOCUMENT_SCHEMA_VERSION,
};
use buzz_sdk::project_document::{
    parse_document_command, parse_document_head, parse_document_meta, parse_document_revision,
    verify_document_projection_bundle, VerifiedCurrentDocument,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

/// Errors from the restricted Project Document storage coordinator.
#[derive(Debug, thiserror::Error)]
pub enum ProjectDocumentWriteError {
    /// Database abstraction failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// SQL execution failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Tamper-evident control audit append failed.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// The pure Project Document kernel rejected the transition.
    #[error(transparent)]
    Domain(#[from] DocumentError),
    /// The Community is archived, disabled, unbootstrapped, or on the wrong
    /// Project View schema.
    #[error("Project Document is unavailable for community {community_id}")]
    Unavailable {
        /// Host-derived Community identity.
        community_id: CommunityId,
    },
    /// The actor or managed-Agent owner no longer passes the Community writer
    /// gate.
    #[error("Project Document actor is no longer authorized by the Community")]
    NotAuthorized,
    /// An explicitly claimed Assignment is not active for the signing actor.
    #[error("Project Document acting Assignment is no longer valid")]
    ActingAssignmentInvalid,
    /// An explicitly claimed managed Runtime fence is missing or stale.
    #[error("Project Document Runtime fence is missing or stale")]
    RuntimeFence,
    /// A signed commit bundle does not exactly represent the locked basis.
    #[error("invalid prepared Project Document commit: {0}")]
    InvalidCommit(String),
}

/// Convenient Project Document write result.
pub type ProjectDocumentWriteResult<T> = Result<T, ProjectDocumentWriteError>;

/// Durable catalog metadata stored beside the canonical current rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentStateMetadata {
    /// Current global catalog observation revision.
    pub catalog_revision: u64,
    /// Number of active current Documents.
    pub active_document_count: u64,
    /// Latest accepted command, absent at empty bootstrap.
    pub last_change_id: Option<EventId>,
    /// Latest actor, absent at empty bootstrap.
    pub last_actor_pubkey: Option<PublicKey>,
    /// Stable signer for the active projection generation.
    pub projection_pubkey: PublicKey,
    /// Positive projection generation.
    pub projection_generation: u64,
    /// Current Relay-signed metadata event.
    pub meta_projection_event_id: EventId,
    /// Canonical bootstrap time.
    pub initialized_at: DateTime<Utc>,
    /// Canonical latest observation time.
    pub updated_at: DateTime<Utc>,
}

/// Locked canonical basis and database-derived monotonic time for one command.
#[derive(Debug, Clone)]
pub struct ProjectDocumentWriteContext {
    /// Current catalog metadata.
    pub catalog: DocumentCatalog,
    /// Current target revision, including a tombstone, or `None` for an unused ID.
    pub current: Option<CurrentDocument>,
    /// Monotonic canonical time to pass to the pure reducer.
    pub canonical_time: DateTime<Utc>,
    /// Whether any locked active cross-domain reference blocks deletion.
    pub deletion_blocked: bool,
}

/// Complete signed inputs to one atomic Document business commit.
#[derive(Debug, Clone)]
pub struct PreparedProjectDocumentCommit {
    /// Accepted member command event.
    pub command_event: Event,
    /// Strict typed command parsed before transaction entry.
    pub command: ProjectDocumentCommand,
    /// Deterministic pure transition derived from the locked context.
    pub transition: DocumentTransition,
    /// Relay-signed immutable revision projection.
    pub revision_projection: Event,
    /// Relay-signed current head projection.
    pub head_projection: Event,
    /// Relay-signed incremental catalog metadata projection.
    pub meta_projection: Event,
}

/// Result of a new commit or an exact accepted-event replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentCommitOutcome {
    /// Stable receipt with no projection event identifiers.
    pub receipt: ProjectDocumentReceipt,
    /// `true` when the command event was already accepted and nothing changed.
    pub replayed: bool,
}

/// Result of the security-first receipt lookup for one signed command.
///
/// A replay is returned before the caller loads the current Markdown body or
/// attempts to reduce the command against a later canonical revision. This is
/// particularly important for an accepted Create: reducing that old command
/// against current state would incorrectly report `id_exists` instead of the
/// durable receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectDocumentPrepareOutcome {
    /// The exact signed command was already accepted.
    Replayed(ProjectDocumentReceipt),
    /// No receipt exists; the caller may load the target and prepare a commit.
    New,
}

/// One bounded page of trusted Relay-signed Document projections.
#[derive(Debug, Clone)]
pub struct ProjectDocumentProjectionPage {
    /// Events in canonical keyset order.
    pub events: Vec<StoredEvent>,
}

/// Closed snapshot request for one page of active Document heads.
#[derive(Debug, Clone, Copy)]
pub struct ProjectDocumentActiveHeadsPageRequest<'a> {
    /// Host-derived Community identity.
    pub community_id: CommunityId,
    /// Stable Relay signer advertised for this Community.
    pub expected_pubkey: &'a PublicKey,
    /// Authenticated principal re-authorized under the shared lock.
    pub reader_pubkey: &'a [u8],
    /// Fixed positive projection generation.
    pub projection_generation: u64,
    /// Fixed catalog observation revision.
    pub catalog_revision: u64,
    /// Exclusive Document UUID cursor.
    pub after_document_id: Option<Uuid>,
    /// Page size in the closed `1..=500` range.
    pub limit: u16,
}

/// Closed snapshot request for one page of immutable Document history.
#[derive(Debug, Clone, Copy)]
pub struct ProjectDocumentHistoryPageRequest<'a> {
    /// Host-derived Community identity.
    pub community_id: CommunityId,
    /// Stable Relay signer advertised for this Community.
    pub expected_pubkey: &'a PublicKey,
    /// Authenticated principal re-authorized under the shared lock.
    pub reader_pubkey: &'a [u8],
    /// Fixed positive projection generation.
    pub projection_generation: u64,
    /// Document whose immutable revisions are requested.
    pub document_id: Uuid,
    /// Inclusive revision ceiling pinned by the caller's trusted head.
    pub max_document_revision: u64,
    /// Exclusive descending revision cursor.
    pub before_revision: Option<u64>,
    /// Page size in the closed `1..=50` range.
    pub limit: u16,
}

/// Failures from generation- and catalog-bound Document reads.
#[derive(Debug, thiserror::Error)]
pub enum ProjectDocumentReadError {
    /// The supplied snapshot observation is no longer current.
    #[error("Project Document snapshot changed")]
    Conflict,
    /// The capability, signer, schema, or projection state is unavailable.
    #[error("Project Document is unavailable")]
    Unavailable,
    /// The principal is no longer a current eligible Community reader.
    #[error("Project Document reader is no longer authorized")]
    Restricted,
    /// A cursor or page limit is outside the closed v1 contract.
    #[error("invalid Project Document page request: {0}")]
    InvalidRequest(String),
    /// Database abstraction failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// SQL execution failed.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Canonical pointers did not reconstruct the expected event page.
    #[error("inconsistent Project Document projection page: {0}")]
    Inconsistent(String),
}

/// Signed, revision-zero empty catalog prepared for disabled-only bootstrap.
#[derive(Debug, Clone)]
pub struct PreparedProjectDocumentBootstrap {
    /// Pure empty catalog.
    pub catalog: DocumentCatalog,
    /// Relay-signed reset metadata event.
    pub meta_projection: Event,
}

/// Operator-facing status for one Community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentFeatureStatus {
    /// Community identity.
    pub community_id: CommunityId,
    /// Normalized Community host.
    pub host: String,
    /// Whether the Community is archived.
    pub archived: bool,
    /// Per-Community capability flag.
    pub enabled: bool,
    /// Current Project View schema version.
    pub project_view_schema_version: i16,
    /// Catalog revision when bootstrapped.
    pub catalog_revision: Option<u64>,
    /// Active current Document count when bootstrapped.
    pub active_document_count: Option<u64>,
    /// Total immutable revision count.
    pub revision_count: u64,
    /// Projection generation when bootstrapped.
    pub projection_generation: Option<u64>,
    /// Stored projection signer when bootstrapped.
    pub projection_pubkey: Option<PublicKey>,
}

/// Durable state of one inactive-generation full-history reprojection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentReprojectStatus {
    /// Stable operation identity used for safe resume.
    pub operation_id: Uuid,
    /// Community whose projections are being replaced.
    pub community_id: CommunityId,
    /// `staging`, `ready`, `activated`, or `aborted`.
    pub state: String,
    /// Generation visible when staging started.
    pub source_generation: u64,
    /// Inactive generation being built.
    pub target_generation: u64,
    /// Signer of the inactive generation.
    pub target_pubkey: PublicKey,
    /// Immutable revisions fixed in the staging snapshot.
    pub revision_count: u64,
    /// Current heads fixed in the staging snapshot.
    pub document_count: u64,
    /// Signed revision events currently staged.
    pub staged_revision_count: u64,
    /// Signed current-head events currently staged.
    pub staged_head_count: u64,
    /// Whether the reset metadata event is staged.
    pub meta_staged: bool,
}

/// Indexed pointer diagnostics for operator status output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectDocumentIntegrityStatus {
    /// Live projection events that are not named by the active generation.
    pub orphan_projection_count: u64,
    /// Canonical meta/head/revision pointers whose event envelope is absent or
    /// belongs to another signer/generation.
    pub pointer_mismatch_count: u64,
}

/// Fixed canonical basis for an inactive-generation reprojection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentReprojectContext {
    /// Durable operation identity.
    pub operation_id: Uuid,
    /// Community/Project identity.
    pub community_id: CommunityId,
    /// Source generation that remains visible until activation.
    pub source_generation: u64,
    /// Inactive target generation.
    pub target_generation: u64,
    /// Target stable signer.
    pub target_pubkey: PublicKey,
    /// Fixed current catalog revision.
    pub catalog_revision: u64,
    /// Fixed active Document count.
    pub active_document_count: u64,
    /// Fixed current-row count, including tombstones.
    pub document_count: u64,
    /// Fixed immutable revision count.
    pub revision_count: u64,
    /// Canonical catalog initialization time.
    pub initialized_at: DateTime<Utc>,
    /// Canonical current catalog observation time.
    pub updated_at: DateTime<Utc>,
}

/// One immutable canonical revision plus the creation provenance needed to
/// reconstruct its Relay-signed projection without loading the whole history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentReprojectRevision {
    /// Stable Document identity.
    pub document_id: Uuid,
    /// Positive Document-local revision.
    pub document_revision: u64,
    /// Catalog revision originally committed with this revision.
    pub catalog_revision: u64,
    /// Active snapshot or tombstone.
    pub revision: DocumentRevision,
    /// Immutable Document creation attribution.
    pub created: DocumentAttribution,
    /// Original Human command event used as projection source.
    pub source_event_id: EventId,
    /// Whether this revision is the current head target.
    pub is_current: bool,
}

/// Staging subtype for one signed inactive-generation event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectDocumentReprojectEventType {
    /// Immutable revision projection.
    Revision,
    /// Current head projection.
    Head,
    /// Reset catalog metadata projection.
    Meta,
}

/// One signed event prepared for inactive-generation staging.
#[derive(Debug, Clone)]
pub struct PreparedProjectDocumentReprojectEvent {
    /// Projection subtype.
    pub projection_type: ProjectDocumentReprojectEventType,
    /// Document identity for revision/head events.
    pub document_id: Option<Uuid>,
    /// Document revision for revision/head events.
    pub document_revision: Option<u64>,
    /// Strict signed Nostr event.
    pub event: Event,
}

/// Read-only preflight explaining whether an already-bootstrapped catalog is
/// safe for a future explicit enable operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocumentPreflight {
    /// Community identity.
    pub community_id: CommunityId,
    /// Whether migration 0032 objects exist.
    pub schema_ready: bool,
    /// Whether the Project View schema is supported by the Document contract.
    pub project_view_schema_ready: bool,
    /// Whether revision-zero or later canonical state exists.
    pub bootstrapped: bool,
    /// Whether the configured signer equals the stored generation signer.
    pub signer_matches: bool,
    /// Whether metadata/current/revision pointers pass database parity checks.
    pub projection_parity: bool,
    /// Whether all enable prerequisites are satisfied.
    pub ready: bool,
}

/// Caller-owned transaction holding the Community exclusive advisory lock.
pub struct ProjectDocumentWriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    expected_projection_pubkey: PublicKey,
    loaded: Option<LoadedBasis>,
}

#[derive(Debug, Clone)]
struct LoadedBasis {
    target_id: Uuid,
    catalog: DocumentCatalog,
    current: Option<CurrentDocument>,
    projection_pubkey: PublicKey,
    canonical_time: DateTime<Utc>,
    deletion_blocked: bool,
}

impl std::fmt::Debug for ProjectDocumentWriteTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectDocumentWriteTx")
            .field("community_id", &self.community_id)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Probe the live catalog instead of trusting only the migration ledger.
    pub async fn project_document_schema_ready(&self) -> crate::Result<bool> {
        let ready: bool = sqlx::query_scalar(
            "SELECT \
                EXISTS (SELECT 1 FROM pg_attribute \
                        WHERE attrelid = 'communities'::regclass \
                          AND attname = 'project_document_enabled' AND NOT attisdropped) \
                AND to_regclass('project_document_state') IS NOT NULL \
                AND to_regclass('project_documents') IS NOT NULL \
                AND to_regclass('project_document_revisions') IS NOT NULL \
                AND to_regclass('project_document_changes') IS NOT NULL \
                AND to_regclass('project_document_reprojects') IS NOT NULL \
                AND to_regclass('project_document_reproject_events') IS NOT NULL \
                AND to_regclass('idx_project_documents_active') IS NOT NULL \
                AND to_regclass('idx_project_document_revisions_history') IS NOT NULL \
                AND to_regprocedure('project_document_validate_history_projection(uuid)') IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(ready)
    }

    /// Deployment-global rolling-start readiness.
    ///
    /// Pre-migration and all-disabled deployments remain ready. Once any active
    /// Community is enabled, the full schema and a configured stable signer are
    /// mandatory.
    pub async fn project_document_deployment_ready(
        &self,
        stable_signer_configured: bool,
    ) -> crate::Result<bool> {
        let column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
             WHERE attrelid = 'communities'::regclass \
               AND attname = 'project_document_enabled' AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !column_exists {
            return Ok(true);
        }
        let any_enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities \
             WHERE project_document_enabled AND archived_at IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !any_enabled {
            return Ok(true);
        }
        Ok(stable_signer_configured && self.project_document_schema_ready().await?)
    }

    /// Return a PostgreSQL-derived timestamp for a signed empty-catalog
    /// bootstrap. The caller may sign outside the transaction, but it never
    /// substitutes a client clock for canonical catalog time.
    pub async fn project_document_canonical_now(&self) -> crate::Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Return whether the capability is enabled and every signer/projection
    /// readiness condition currently passes.
    pub async fn project_document_capability_ready(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        if !self.project_document_schema_ready().await? {
            return Ok(false);
        }
        Ok(sqlx::query_scalar(
            "SELECT EXISTS ( \
                 SELECT 1 FROM communities c \
                 JOIN project_document_state s ON s.community_id = c.id \
                 WHERE c.id = $1 AND c.archived_at IS NULL \
                   AND c.project_document_enabled \
                   AND c.project_view_schema_version IN (2, 3) \
                   AND s.projection_pubkey = $2 \
                   AND s.projection_generation BETWEEN 1 AND 9007199254740991)",
        )
        .bind(community_id.as_uuid())
        .bind(expected_pubkey.as_bytes())
        .fetch_one(&self.pool)
        .await?)
    }

    /// Document readers use the same current Human / managed-owner and active
    /// ban policy as Project View. Timeouts intentionally remain write-only
    /// restrictions.
    pub async fn project_document_authorized_pubkey(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> crate::Result<bool> {
        self.project_view_authorized_pubkey(community_id, pubkey)
            .await
    }

    /// Set-based form used by local and Redis fan-out recipient filtering.
    pub async fn project_document_authorized_pubkeys(
        &self,
        community_id: CommunityId,
        pubkeys: &[Vec<u8>],
    ) -> crate::Result<std::collections::HashSet<Vec<u8>>> {
        self.project_view_authorized_pubkeys(community_id, pubkeys)
            .await
    }

    /// List status for every Community in stable UUID order.
    pub async fn list_project_document_statuses(
        &self,
    ) -> crate::Result<Vec<ProjectDocumentFeatureStatus>> {
        if !self.project_document_schema_ready().await? {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT c.id, c.host, c.archived_at IS NOT NULL AS archived, \
                    c.project_document_enabled, c.project_view_schema_version, \
                    s.catalog_revision, s.active_document_count, \
                    s.projection_generation, s.projection_pubkey, \
                    COALESCE(s.catalog_revision, 0)::bigint AS revision_count \
             FROM communities c \
             LEFT JOIN project_document_state s ON s.community_id = c.id \
             ORDER BY c.id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(status_from_row).collect()
    }

    /// Read status for one exact Community.
    pub async fn project_document_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<Option<ProjectDocumentFeatureStatus>> {
        if !self.project_document_schema_ready().await? {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.id, c.host, c.archived_at IS NOT NULL AS archived, \
                    c.project_document_enabled, c.project_view_schema_version, \
                    s.catalog_revision, s.active_document_count, \
                    s.projection_generation, s.projection_pubkey, \
                    COALESCE(s.catalog_revision, 0)::bigint AS revision_count \
             FROM communities c \
             LEFT JOIN project_document_state s ON s.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        row.map(status_from_row).transpose()
    }

    /// Verify signer, bootstrap, and pointer prerequisites without changing state.
    pub async fn project_document_preflight(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<ProjectDocumentPreflight> {
        let schema_ready = self.project_document_schema_ready().await?;
        if !schema_ready {
            return Ok(ProjectDocumentPreflight {
                community_id,
                schema_ready: false,
                project_view_schema_ready: false,
                bootstrapped: false,
                signer_matches: false,
                projection_parity: false,
                ready: false,
            });
        }
        let row = sqlx::query(
            "SELECT c.archived_at IS NULL AS active, c.project_view_schema_version, \
                    s.projection_pubkey, s.meta_projection_event_id, \
                    s.active_document_count \
             FROM communities c \
             LEFT JOIN project_document_state s ON s.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        let Some(row) = row else {
            return Ok(ProjectDocumentPreflight {
                community_id,
                schema_ready: true,
                project_view_schema_ready: false,
                bootstrapped: false,
                signer_matches: false,
                projection_parity: false,
                ready: false,
            });
        };
        let active: bool = row.try_get("active")?;
        let project_view_schema_version: i16 = row.try_get("project_view_schema_version")?;
        let project_view_schema_ready = matches!(project_view_schema_version, 2 | 3);
        let stored_pubkey: Option<Vec<u8>> = row.try_get("projection_pubkey")?;
        let meta_event_id: Option<Vec<u8>> = row.try_get("meta_projection_event_id")?;
        let active_count: Option<i64> = row.try_get("active_document_count")?;
        let bootstrapped = stored_pubkey.is_some();
        let signer_matches = stored_pubkey
            .as_deref()
            .is_some_and(|bytes| bytes == expected_pubkey.as_bytes());
        let projection_parity = if signer_matches {
            let mut connection = self.pool.acquire().await?;
            document_projection_parity(
                &mut connection,
                community_id,
                expected_pubkey,
                meta_event_id.as_deref(),
                active_count,
            )
            .await?
        } else {
            false
        };
        Ok(ProjectDocumentPreflight {
            community_id,
            schema_ready,
            project_view_schema_ready,
            bootstrapped,
            signer_matches,
            projection_parity,
            ready: active
                && schema_ready
                && project_view_schema_ready
                && bootstrapped
                && signer_matches
                && projection_parity,
        })
    }

    /// Enable or disable one Community under the shared Community/Project
    /// writer lock. Disable is always fail-closed and preserves every canonical
    /// row and event. Enable requires schema 2/3, a stable signer, bootstrap,
    /// and full pointer/projection parity.
    pub async fn set_project_document_enabled_checked(
        &self,
        community_id: CommunityId,
        enabled: bool,
        expected_pubkey: Option<&PublicKey>,
    ) -> ProjectDocumentWriteResult<bool> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let row = sqlx::query(
            "SELECT archived_at IS NULL AS active, project_view_schema_version \
             FROM communities WHERE id = $1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(false);
        };
        let active: bool = row.try_get("active")?;
        let schema_version: i16 = row.try_get("project_view_schema_version")?;
        if !active {
            return Ok(false);
        }
        if enabled {
            let expected_pubkey = expected_pubkey.ok_or_else(|| {
                DbError::InvalidData(
                    "a stable Relay signer is required to enable Project Document".to_owned(),
                )
            })?;
            if !matches!(schema_version, 2 | 3) {
                return Err(DbError::InvalidData(
                    "Project Document requires Project View schema 2 or 3".to_owned(),
                )
                .into());
            }
            let state = sqlx::query(
                "SELECT projection_pubkey, meta_projection_event_id, active_document_count \
                 FROM project_document_state WHERE community_id = $1 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
            let Some(state) = state else {
                return Err(DbError::InvalidData(
                    "Project Document catalog must be bootstrapped before enable".to_owned(),
                )
                .into());
            };
            let projection_pubkey: Vec<u8> = state.try_get("projection_pubkey")?;
            if projection_pubkey.as_slice() != expected_pubkey.as_bytes() {
                return Err(DbError::InvalidData(
                    "Project Document stable signer does not match bootstrap state".to_owned(),
                )
                .into());
            }
            sqlx::query("SELECT project_document_validate_community($1)")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
            let meta_event_id: Vec<u8> = state.try_get("meta_projection_event_id")?;
            let active_count: i64 = state.try_get("active_document_count")?;
            // The exclusive advisory lock prevents canonical writers while the
            // full cryptographic parser walks committed event rows.
            if !document_projection_parity(
                &mut tx,
                community_id,
                expected_pubkey,
                Some(&meta_event_id),
                Some(active_count),
            )
            .await?
            {
                return Err(DbError::InvalidData(
                    "Project Document canonical/projection parity is not ready".to_owned(),
                )
                .into());
            }
        }
        let result =
            sqlx::query("UPDATE communities SET project_document_enabled = $2 WHERE id = $1")
                .bind(community_id.as_uuid())
                .bind(enabled)
                .execute(&mut *tx)
                .await?;
        if result.rows_affected() == 1 {
            append_document_control_audit(
                &mut tx,
                community_id,
                if enabled { "enable" } else { "disable" },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    /// Read one stable active-head page under the Community shared lock.
    pub async fn project_document_active_heads_page(
        &self,
        request: ProjectDocumentActiveHeadsPageRequest<'_>,
    ) -> Result<ProjectDocumentProjectionPage, ProjectDocumentReadError> {
        let ProjectDocumentActiveHeadsPageRequest {
            community_id,
            expected_pubkey,
            reader_pubkey,
            projection_generation,
            catalog_revision,
            after_document_id,
            limit,
        } = request;
        if !(1..=500).contains(&limit) {
            return Err(ProjectDocumentReadError::InvalidRequest(
                "active-head limit must be in 1..=500".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        require_document_reader_in_tx(&mut tx, community_id, reader_pubkey).await?;
        require_document_read_state_in_tx(
            &mut tx,
            community_id,
            expected_pubkey,
            projection_generation,
            Some(catalog_revision),
        )
        .await?;
        let rows = sqlx::query(
            "SELECT e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content, e.sig, \
                    e.received_at, e.channel_id \
             FROM project_documents d \
             JOIN events e ON e.community_id = d.community_id \
                          AND e.id = d.current_head_event_id \
             WHERE d.community_id = $1 AND d.state = 'active' \
               AND ($2::uuid IS NULL OR d.document_id > $2) \
               AND e.deleted_at IS NULL AND e.kind = $3 AND e.pubkey = $4 \
             ORDER BY d.document_id ASC LIMIT $5",
        )
        .bind(community_id.as_uuid())
        .bind(after_document_id)
        .bind(KIND_PROJECT_DOCUMENT_HEAD as i32)
        .bind(expected_pubkey.as_bytes())
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        let events = rows
            .into_iter()
            .map(crate::event::row_to_stored_event)
            .collect::<crate::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ProjectDocumentReadError::Inconsistent(
                    "an active head event could not be reconstructed".to_owned(),
                )
            })?;
        tx.commit().await?;
        Ok(ProjectDocumentProjectionPage { events })
    }

    /// Read one immutable revision-history page under the Community shared
    /// lock. Newer concurrent revisions are excluded by the caller-fixed max.
    pub async fn project_document_history_page(
        &self,
        request: ProjectDocumentHistoryPageRequest<'_>,
    ) -> Result<ProjectDocumentProjectionPage, ProjectDocumentReadError> {
        let ProjectDocumentHistoryPageRequest {
            community_id,
            expected_pubkey,
            reader_pubkey,
            projection_generation,
            document_id,
            max_document_revision,
            before_revision,
            limit,
        } = request;
        if !(1..=50).contains(&limit)
            || max_document_revision == 0
            || max_document_revision > MAX_SAFE_REVISION
            || before_revision.is_some_and(|value| value == 0 || value > MAX_SAFE_REVISION)
        {
            return Err(ProjectDocumentReadError::InvalidRequest(
                "history revision or limit is outside the v1 range".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        require_document_reader_in_tx(&mut tx, community_id, reader_pubkey).await?;
        require_document_read_state_in_tx(
            &mut tx,
            community_id,
            expected_pubkey,
            projection_generation,
            None,
        )
        .await?;
        let rows = sqlx::query(
            "SELECT e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content, e.sig, \
                    e.received_at, e.channel_id \
             FROM project_document_revisions r \
             JOIN events e ON e.community_id = r.community_id \
                          AND e.id = r.projection_event_id \
             WHERE r.community_id = $1 AND r.document_id = $2 \
               AND r.document_revision <= $3 \
               AND ($4::bigint IS NULL OR r.document_revision < $4) \
               AND r.projection_generation = $5 \
               AND e.deleted_at IS NULL AND e.kind = $6 AND e.pubkey = $7 \
             ORDER BY r.document_revision DESC LIMIT $8",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .bind(
            revision_to_i64(max_document_revision, "max_document_revision")
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(
            before_revision
                .map(|value| revision_to_i64(value, "before_revision"))
                .transpose()
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(
            revision_to_i64(projection_generation, "projection_generation")
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .bind(expected_pubkey.as_bytes())
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        let events = rows
            .into_iter()
            .map(crate::event::row_to_stored_event)
            .collect::<crate::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                ProjectDocumentReadError::Inconsistent(
                    "a revision event could not be reconstructed".to_owned(),
                )
            })?;
        tx.commit().await?;
        Ok(ProjectDocumentProjectionPage { events })
    }

    /// Explain the exact closed history-page query under the same reader and
    /// generation gates used by live requests. Intended for local capacity
    /// acceptance; no business content is included in the plan.
    pub async fn project_document_history_query_plan(
        &self,
        request: ProjectDocumentHistoryPageRequest<'_>,
    ) -> Result<Value, ProjectDocumentReadError> {
        let ProjectDocumentHistoryPageRequest {
            community_id,
            expected_pubkey,
            reader_pubkey,
            projection_generation,
            document_id,
            max_document_revision,
            before_revision,
            limit,
        } = request;
        if !(1..=50).contains(&limit)
            || max_document_revision == 0
            || max_document_revision > MAX_SAFE_REVISION
            || before_revision.is_some_and(|value| value == 0 || value > MAX_SAFE_REVISION)
        {
            return Err(ProjectDocumentReadError::InvalidRequest(
                "history revision or limit is outside the v1 range".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        require_document_reader_in_tx(&mut tx, community_id, reader_pubkey).await?;
        require_document_read_state_in_tx(
            &mut tx,
            community_id,
            expected_pubkey,
            projection_generation,
            None,
        )
        .await?;
        let plan: Value = sqlx::query_scalar(
            "EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) \
             SELECT e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content, e.sig, \
                    e.received_at, e.channel_id \
             FROM project_document_revisions r \
             JOIN events e ON e.community_id = r.community_id \
                          AND e.id = r.projection_event_id \
             WHERE r.community_id = $1 AND r.document_id = $2 \
               AND r.document_revision <= $3 \
               AND ($4::bigint IS NULL OR r.document_revision < $4) \
               AND r.projection_generation = $5 \
               AND e.deleted_at IS NULL AND e.kind = $6 AND e.pubkey = $7 \
             ORDER BY r.document_revision DESC LIMIT $8",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .bind(
            revision_to_i64(max_document_revision, "max_document_revision")
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(
            before_revision
                .map(|value| revision_to_i64(value, "before_revision"))
                .transpose()
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(
            revision_to_i64(projection_generation, "projection_generation")
                .map_err(|error| ProjectDocumentReadError::InvalidRequest(error.to_string()))?,
        )
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .bind(expected_pubkey.as_bytes())
        .bind(i64::from(limit))
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(plan)
    }

    /// Commit a signed revision-zero reset catalog while the capability is off.
    ///
    /// The controlled admin workflow invokes this only while the capability is
    /// disabled; isolated DB tests also use it directly.
    pub async fn bootstrap_empty_project_document_catalog(
        &self,
        prepared: PreparedProjectDocumentBootstrap,
    ) -> ProjectDocumentWriteResult<()> {
        validate_bootstrap(&prepared)?;
        let community_id = prepared.catalog.project_id();
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let community = sqlx::query(
            "SELECT project_document_enabled, project_view_schema_version \
             FROM communities WHERE id = $1 AND archived_at IS NULL FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(community) = community else {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        };
        let enabled: bool = community.try_get("project_document_enabled")?;
        let schema_version: i16 = community.try_get("project_view_schema_version")?;
        if enabled || !matches!(schema_version, 1..=3) {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        }
        let occupied: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_document_state WHERE community_id = $1) \
             OR EXISTS (SELECT 1 FROM project_documents WHERE community_id = $1) \
             OR EXISTS (SELECT 1 FROM project_document_revisions WHERE community_id = $1) \
             OR EXISTS (SELECT 1 FROM project_document_changes WHERE community_id = $1)",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut *tx)
        .await?;
        if occupied {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "empty bootstrap requires completely uninitialized Document state".to_owned(),
            ));
        }
        let (_, inserted) = crate::event::insert_event_in_tx(
            &mut tx,
            community_id,
            &prepared.meta_projection,
            None,
        )
        .await?;
        if !inserted {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "bootstrap metadata event already exists".to_owned(),
            ));
        }
        let signer = prepared.meta_projection.pubkey.to_bytes();
        sqlx::query(
            "INSERT INTO project_document_state \
                (community_id, schema_version, catalog_revision, active_document_count, \
                 projection_pubkey, projection_generation, meta_projection_event_id, \
                 initialized_at, updated_at) \
             VALUES ($1, 1, 0, 0, $2, $3, $4, $5, $5)",
        )
        .bind(community_id.as_uuid())
        .bind(signer.as_slice())
        .bind(revision_to_i64(
            prepared.catalog.projection_generation(),
            "projection_generation",
        )?)
        .bind(prepared.meta_projection.id.as_bytes().as_slice())
        .bind(prepared.catalog.initialized_at())
        .execute(&mut *tx)
        .await?;
        append_document_control_audit(&mut tx, community_id, "bootstrap").await?;
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Return the newest durable reprojection operation for one Community.
    pub async fn project_document_reproject_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<Option<ProjectDocumentReprojectStatus>> {
        if !self.project_document_schema_ready().await? {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT operation_id, community_id, state, source_projection_generation, \
                    target_projection_generation, target_projection_pubkey, revision_count, \
                    document_count, \
                    (SELECT count(*)::bigint FROM project_document_reproject_events e \
                     WHERE e.community_id = r.community_id AND e.operation_id = r.operation_id \
                       AND e.projection_type = 'revision') AS staged_revision_count, \
                    (SELECT count(*)::bigint FROM project_document_reproject_events e \
                     WHERE e.community_id = r.community_id AND e.operation_id = r.operation_id \
                       AND e.projection_type = 'head') AS staged_head_count, \
                    EXISTS (SELECT 1 FROM project_document_reproject_events e \
                     WHERE e.community_id = r.community_id AND e.operation_id = r.operation_id \
                       AND e.projection_type = 'meta') AS meta_staged \
             FROM project_document_reprojects r WHERE community_id = $1 \
             ORDER BY started_at DESC, operation_id DESC LIMIT 1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?;
        row.map(reproject_status_from_row).transpose()
    }

    /// Count active-generation pointer mismatches and unreferenced live
    /// projection events under the Community shared lock.
    pub async fn project_document_integrity_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<ProjectDocumentIntegrityStatus> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let row = sqlx::query(
            "WITH state AS ( \
                 SELECT projection_pubkey, projection_generation, meta_projection_event_id \
                 FROM project_document_state WHERE community_id = $1), \
             mismatches AS ( \
                 SELECT 1 FROM state s WHERE NOT EXISTS ( \
                     SELECT 1 FROM events e WHERE e.community_id = $1 \
                       AND e.id = s.meta_projection_event_id AND e.kind = $2 \
                       AND e.pubkey = s.projection_pubkey AND e.deleted_at IS NULL) \
                 UNION ALL \
                 SELECT 1 FROM project_documents d CROSS JOIN state s \
                 WHERE d.community_id = $1 AND ( \
                     NOT EXISTS (SELECT 1 FROM events e WHERE e.community_id = d.community_id \
                       AND e.id = d.current_head_event_id AND e.kind = $3 \
                       AND e.pubkey = s.projection_pubkey AND e.deleted_at IS NULL) \
                     OR NOT EXISTS (SELECT 1 FROM events e WHERE e.community_id = d.community_id \
                       AND e.id = d.current_revision_event_id AND e.kind = $4 \
                       AND e.pubkey = s.projection_pubkey AND e.deleted_at IS NULL)) \
                 UNION ALL \
                 SELECT 1 FROM project_document_revisions r CROSS JOIN state s \
                 WHERE r.community_id = $1 AND ( \
                     r.projection_generation <> s.projection_generation \
                     OR NOT EXISTS (SELECT 1 FROM events e \
                        WHERE e.community_id = r.community_id \
                          AND e.id = r.projection_event_id AND e.kind = $4 \
                          AND e.pubkey = s.projection_pubkey AND e.deleted_at IS NULL))), \
             active_pointers AS ( \
                 SELECT meta_projection_event_id AS event_id FROM state \
                 UNION SELECT current_head_event_id FROM project_documents WHERE community_id = $1 \
                 UNION SELECT projection_event_id FROM project_document_revisions WHERE community_id = $1) \
             SELECT (SELECT count(*)::bigint FROM mismatches) AS pointer_mismatch_count, \
                    (SELECT count(*)::bigint FROM events e \
                     WHERE e.community_id = $1 AND e.kind IN ($2, $3, $4) \
                       AND e.deleted_at IS NULL \
                       AND NOT EXISTS (SELECT 1 FROM active_pointers p WHERE p.event_id = e.id)) \
                       AS orphan_projection_count",
        )
        .bind(community_id.as_uuid())
        .bind(KIND_PROJECT_DOCUMENT_META as i32)
        .bind(KIND_PROJECT_DOCUMENT_HEAD as i32)
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .fetch_one(&mut *tx)
        .await?;
        let status = ProjectDocumentIntegrityStatus {
            orphan_projection_count: db_nonnegative_revision_db(
                row.try_get("orphan_projection_count")?,
                "orphan_projection_count",
            )?,
            pointer_mismatch_count: db_nonnegative_revision_db(
                row.try_get("pointer_mismatch_count")?,
                "pointer_mismatch_count",
            )?,
        };
        tx.commit().await?;
        Ok(status)
    }

    /// Start or safely resume one inactive target generation while member
    /// access remains disabled.
    pub async fn begin_project_document_reproject(
        &self,
        community_id: CommunityId,
        target_pubkey: PublicKey,
    ) -> ProjectDocumentWriteResult<ProjectDocumentReprojectContext> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let row = sqlx::query(
            "SELECT c.project_document_enabled, c.archived_at IS NULL AS active, \
                    c.project_view_schema_version, s.catalog_revision, \
                    s.active_document_count, s.projection_pubkey, s.projection_generation, \
                    s.initialized_at, s.updated_at, \
                    (SELECT count(*)::bigint FROM project_documents d \
                     WHERE d.community_id = c.id) AS document_count \
             FROM communities c JOIN project_document_state s ON s.community_id = c.id \
             WHERE c.id = $1 FOR UPDATE OF c, s",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        };
        let enabled: bool = row.try_get("project_document_enabled")?;
        let active: bool = row.try_get("active")?;
        let schema: i16 = row.try_get("project_view_schema_version")?;
        if enabled || !active || !matches!(schema, 2 | 3) {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        }
        let source_generation = db_positive_revision(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?;
        let target_generation = source_generation.checked_add(1).ok_or_else(|| {
            ProjectDocumentWriteError::InvalidCommit(
                "projection generation cannot advance beyond the safe range".to_owned(),
            )
        })?;
        if target_generation > MAX_SAFE_REVISION {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "projection generation cannot advance beyond the safe range".to_owned(),
            ));
        }
        let source_pubkey: Vec<u8> = row.try_get("projection_pubkey")?;
        let catalog_revision =
            db_nonnegative_revision(row.try_get("catalog_revision")?, "catalog_revision")?;
        let active_document_count = db_nonnegative_revision(
            row.try_get("active_document_count")?,
            "active_document_count",
        )?;
        let document_count =
            db_nonnegative_revision(row.try_get("document_count")?, "document_count")?;
        let initialized_at = row.try_get("initialized_at")?;
        let updated_at = row.try_get("updated_at")?;
        let open = sqlx::query(
            "SELECT operation_id, source_projection_pubkey, source_projection_generation, \
                    target_projection_pubkey, target_projection_generation, catalog_revision, \
                    active_document_count, document_count, revision_count \
             FROM project_document_reprojects \
             WHERE community_id = $1 AND state IN ('staging', 'ready') FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let operation_id = if let Some(open) = open {
            let staged_target: Vec<u8> = open.try_get("target_projection_pubkey")?;
            let staged_source: Vec<u8> = open.try_get("source_projection_pubkey")?;
            let exact = staged_target.as_slice() == target_pubkey.as_bytes()
                && staged_source == source_pubkey
                && open.try_get::<i64, _>("source_projection_generation")?
                    == i64::try_from(source_generation).unwrap_or(i64::MAX)
                && open.try_get::<i64, _>("target_projection_generation")?
                    == i64::try_from(target_generation).unwrap_or(i64::MAX)
                && open.try_get::<i64, _>("catalog_revision")?
                    == i64::try_from(catalog_revision).unwrap_or(i64::MAX)
                && open.try_get::<i64, _>("active_document_count")?
                    == i64::try_from(active_document_count).unwrap_or(i64::MAX)
                && open.try_get::<i64, _>("document_count")?
                    == i64::try_from(document_count).unwrap_or(i64::MAX)
                && open.try_get::<i64, _>("revision_count")?
                    == i64::try_from(catalog_revision).unwrap_or(i64::MAX);
            if !exact {
                return Err(ProjectDocumentWriteError::InvalidCommit(
                    "a different or stale Project Document reproject is already open".to_owned(),
                ));
            }
            open.try_get("operation_id")?
        } else {
            let operation_id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO project_document_reprojects \
                    (operation_id, community_id, state, source_projection_pubkey, \
                     source_projection_generation, target_projection_pubkey, \
                     target_projection_generation, catalog_revision, active_document_count, \
                     document_count, revision_count) \
                 VALUES ($1, $2, 'staging', $3, $4, $5, $6, $7, $8, $9, $7)",
            )
            .bind(operation_id)
            .bind(community_id.as_uuid())
            .bind(source_pubkey.as_slice())
            .bind(revision_to_i64(source_generation, "source_generation")?)
            .bind(target_pubkey.as_bytes())
            .bind(revision_to_i64(target_generation, "target_generation")?)
            .bind(revision_to_i64(catalog_revision, "catalog_revision")?)
            .bind(revision_to_i64(
                active_document_count,
                "active_document_count",
            )?)
            .bind(revision_to_i64(document_count, "document_count")?)
            .execute(&mut *tx)
            .await?;
            append_document_control_audit(&mut tx, community_id, "reproject_stage_begin").await?;
            operation_id
        };
        tx.commit().await?;
        Ok(ProjectDocumentReprojectContext {
            operation_id,
            community_id,
            source_generation,
            target_generation,
            target_pubkey,
            catalog_revision,
            active_document_count,
            document_count,
            revision_count: catalog_revision,
            initialized_at,
            updated_at,
        })
    }

    /// Load a bounded keyset page from the immutable canonical history fixed by
    /// an open reprojection operation.
    pub async fn project_document_reproject_revision_page(
        &self,
        context: &ProjectDocumentReprojectContext,
        after_catalog_revision: u64,
        limit: u16,
    ) -> ProjectDocumentWriteResult<Vec<ProjectDocumentReprojectRevision>> {
        if !(1..=1000).contains(&limit) {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "reproject page limit must be in 1..=1000".to_owned(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, context.community_id, true).await?;
        require_reproject_basis(&mut tx, context, "staging").await?;
        let rows = sqlx::query(
            "SELECT r.document_id, r.document_revision, r.catalog_revision, r.state, \
                    r.title, r.summary, r.content_markdown, r.actor_pubkey, r.canonical_at, \
                    r.source_event_id, d.created_at AS document_created_at, d.created_by, \
                    r.document_revision = d.current_revision AS is_current \
             FROM project_document_revisions r \
             JOIN project_documents d ON d.community_id = r.community_id \
                                     AND d.document_id = r.document_id \
             WHERE r.community_id = $1 AND r.catalog_revision > $2 \
               AND r.catalog_revision <= $3 \
             ORDER BY r.catalog_revision ASC LIMIT $4",
        )
        .bind(context.community_id.as_uuid())
        .bind(revision_to_i64(
            after_catalog_revision,
            "after_catalog_revision",
        )?)
        .bind(revision_to_i64(
            context.catalog_revision,
            "catalog_revision",
        )?)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        let revisions = rows
            .iter()
            .map(reproject_revision_from_row)
            .collect::<ProjectDocumentWriteResult<Vec<_>>>()?;
        tx.commit().await?;
        Ok(revisions)
    }

    /// Persist one bounded batch of already-signed events in the inactive
    /// staging table. Staged rows are not protocol-visible.
    pub async fn stage_project_document_reproject_events(
        &self,
        context: &ProjectDocumentReprojectContext,
        events: &[PreparedProjectDocumentReprojectEvent],
    ) -> ProjectDocumentWriteResult<()> {
        if events.is_empty() || events.len() > 1000 {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "reproject event batch must contain 1..=1000 events".to_owned(),
            ));
        }
        let staged = events
            .iter()
            .map(|prepared| validate_staged_reproject_event(context, prepared))
            .collect::<ProjectDocumentWriteResult<Vec<_>>>()?;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, context.community_id, true).await?;
        require_reproject_basis(&mut tx, context, "staging").await?;
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO project_document_reproject_events \
             (community_id, operation_id, event_key, projection_type, document_id, \
              document_revision, event_id, pubkey, created_at, kind, tags, content, sig, d_tag) ",
        );
        query.push_values(staged, |mut row, event| {
            row.push_bind(context.community_id.as_uuid())
                .push_bind(context.operation_id)
                .push_bind(event.event_key)
                .push_bind(event.projection_type)
                .push_bind(event.document_id)
                .push_bind(event.document_revision)
                .push_bind(event.event_id)
                .push_bind(event.pubkey)
                .push_bind(event.created_at)
                .push_bind(event.kind)
                .push_bind(event.tags)
                .push_bind(event.content)
                .push_bind(event.sig)
                .push_bind(event.d_tag);
        });
        query.push(" ON CONFLICT DO NOTHING");
        query.build().execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Close staging only after exact canonical identity coverage is present.
    pub async fn ready_project_document_reproject(
        &self,
        context: &ProjectDocumentReprojectContext,
    ) -> ProjectDocumentWriteResult<()> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, context.community_id, false).await?;
        require_reproject_basis(&mut tx, context, "staging").await?;
        let complete: bool = sqlx::query_scalar(
            "SELECT \
                (SELECT count(*) FROM project_document_reproject_events e \
                 WHERE e.community_id = $1 AND e.operation_id = $2 \
                   AND e.projection_type = 'revision') = $3 \
                AND (SELECT count(*) FROM project_document_reproject_events e \
                 WHERE e.community_id = $1 AND e.operation_id = $2 \
                   AND e.projection_type = 'head') = $4 \
                AND (SELECT count(*) FROM project_document_reproject_events e \
                 WHERE e.community_id = $1 AND e.operation_id = $2 \
                   AND e.projection_type = 'meta') = 1 \
                AND NOT EXISTS ( \
                    SELECT 1 FROM project_document_revisions r \
                    LEFT JOIN project_document_reproject_events e \
                      ON e.community_id = r.community_id AND e.operation_id = $2 \
                     AND e.projection_type = 'revision' AND e.document_id = r.document_id \
                     AND e.document_revision = r.document_revision \
                    WHERE r.community_id = $1 AND e.event_id IS NULL) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM project_documents d \
                    LEFT JOIN project_document_reproject_events e \
                      ON e.community_id = d.community_id AND e.operation_id = $2 \
                     AND e.projection_type = 'head' AND e.document_id = d.document_id \
                     AND e.document_revision = d.current_revision \
                    WHERE d.community_id = $1 AND e.event_id IS NULL)",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .bind(revision_to_i64(context.revision_count, "revision_count")?)
        .bind(revision_to_i64(context.document_count, "document_count")?)
        .fetch_one(&mut *tx)
        .await?;
        if !complete {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "inactive Project Document generation is incomplete".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE project_document_reprojects SET state = 'ready', ready_at = clock_timestamp() \
             WHERE community_id = $1 AND operation_id = $2 AND state = 'staging'",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .execute(&mut *tx)
        .await?;
        append_document_control_audit(&mut tx, context.community_id, "reproject_stage_ready")
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Atomically expose the staged generation, rebind all canonical pointers,
    /// retire the old generation, and run full-history parity validation.
    pub async fn activate_project_document_reproject(
        &self,
        context: &ProjectDocumentReprojectContext,
    ) -> ProjectDocumentWriteResult<()> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, context.community_id, false).await?;
        require_reproject_basis(&mut tx, context, "ready").await?;
        sqlx::query("SELECT set_config('buzz.project_document_reproject', 'on', true)")
            .execute(&mut *tx)
            .await?;
        let inserted = sqlx::query(
            "INSERT INTO events \
                (community_id, id, pubkey, created_at, kind, tags, content, sig, \
                 received_at, channel_id, d_tag, not_before) \
             SELECT community_id, event_id, pubkey, created_at, kind, tags, content, sig, \
                    clock_timestamp(), NULL, d_tag, NULL \
             FROM project_document_reproject_events \
             WHERE community_id = $1 AND operation_id = $2",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .execute(&mut *tx)
        .await?;
        let expected_events = context
            .revision_count
            .checked_add(context.document_count)
            .and_then(|value| value.checked_add(1))
            .ok_or_else(|| {
                ProjectDocumentWriteError::InvalidCommit(
                    "reproject event count overflow".to_owned(),
                )
            })?;
        if inserted.rows_affected() != expected_events {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "inactive generation inserted an unexpected event count".to_owned(),
            ));
        }
        let old_generation_tag = serde_json::json!([[
            "projection_generation",
            context.source_generation.to_string()
        ]]);
        sqlx::query(
            "UPDATE events SET deleted_at = clock_timestamp() \
             WHERE community_id = $1 AND kind IN ($2, $3, $4) \
               AND tags @> $5 AND deleted_at IS NULL",
        )
        .bind(context.community_id.as_uuid())
        .bind(KIND_PROJECT_DOCUMENT_HEAD as i32)
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .bind(KIND_PROJECT_DOCUMENT_META as i32)
        .bind(old_generation_tag)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE project_document_revisions r \
             SET projection_generation = $3, projection_event_id = e.event_id \
             FROM project_document_reproject_events e \
             WHERE r.community_id = $1 AND e.operation_id = $2 \
               AND e.community_id = r.community_id AND e.projection_type = 'revision' \
               AND e.document_id = r.document_id \
               AND e.document_revision = r.document_revision",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .bind(revision_to_i64(
            context.target_generation,
            "target_generation",
        )?)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE project_documents d \
             SET current_head_event_id = h.event_id, current_revision_event_id = r.event_id \
             FROM project_document_reproject_events h, project_document_reproject_events r \
             WHERE d.community_id = $1 AND h.operation_id = $2 AND r.operation_id = $2 \
               AND h.community_id = d.community_id AND r.community_id = d.community_id \
               AND h.projection_type = 'head' AND r.projection_type = 'revision' \
               AND h.document_id = d.document_id AND r.document_id = d.document_id \
               AND h.document_revision = d.current_revision \
               AND r.document_revision = d.current_revision",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .execute(&mut *tx)
        .await?;
        let state_updated = sqlx::query(
            "UPDATE project_document_state s \
             SET projection_pubkey = $3, projection_generation = $4, \
                 meta_projection_event_id = e.event_id \
             FROM project_document_reproject_events e \
             WHERE s.community_id = $1 AND e.operation_id = $2 \
               AND e.community_id = s.community_id AND e.projection_type = 'meta'",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .bind(context.target_pubkey.as_bytes())
        .bind(revision_to_i64(
            context.target_generation,
            "target_generation",
        )?)
        .execute(&mut *tx)
        .await?;
        if state_updated.rows_affected() != 1 {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "inactive generation has no unique metadata event".to_owned(),
            ));
        }
        sqlx::query("SELECT project_document_validate_community($1)")
            .bind(context.community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT project_document_validate_history_projection($1)")
            .bind(context.community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        if !document_projection_parity(
            &mut tx,
            context.community_id,
            &context.target_pubkey,
            None,
            Some(i64::try_from(context.active_document_count).unwrap_or(i64::MAX)),
        )
        .await?
        {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "activated generation failed cryptographic parity".to_owned(),
            ));
        }
        sqlx::query(
            "UPDATE project_document_reprojects \
             SET state = 'activated', activated_at = clock_timestamp() \
             WHERE community_id = $1 AND operation_id = $2 AND state = 'ready'",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM project_document_reproject_events \
             WHERE community_id = $1 AND operation_id = $2",
        )
        .bind(context.community_id.as_uuid())
        .bind(context.operation_id)
        .execute(&mut *tx)
        .await?;
        append_document_control_audit(&mut tx, context.community_id, "reproject_activate").await?;
        tx.commit().await?;
        Ok(())
    }

    /// Begin a flag-on business write and acquire the Community exclusive lock.
    ///
    /// `expected_projection_pubkey` is the deployment's currently configured
    /// stable Relay signer. `load_current` rejects a rotated or mismatched
    /// catalog before returning canonical content or allowing receipt replay.
    pub async fn begin_project_document_write(
        &self,
        community_id: CommunityId,
        expected_projection_pubkey: PublicKey,
    ) -> ProjectDocumentWriteResult<ProjectDocumentWriteTx> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let available: Option<bool> = sqlx::query_scalar(
            "SELECT project_document_enabled \
             FROM communities \
             WHERE id = $1 AND archived_at IS NULL \
               AND project_view_schema_version IN (2, 3) FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if available != Some(true) {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        }
        Ok(ProjectDocumentWriteTx {
            tx,
            community_id,
            expected_projection_pubkey,
            loaded: None,
        })
    }
}

#[derive(Debug)]
struct StagedReprojectEvent {
    event_key: String,
    projection_type: &'static str,
    document_id: Option<Uuid>,
    document_revision: Option<i64>,
    event_id: Vec<u8>,
    pubkey: Vec<u8>,
    created_at: DateTime<Utc>,
    kind: i32,
    tags: Value,
    content: String,
    sig: Vec<u8>,
    d_tag: String,
}

async fn require_reproject_basis(
    tx: &mut Transaction<'_, Postgres>,
    context: &ProjectDocumentReprojectContext,
    required_state: &str,
) -> ProjectDocumentWriteResult<()> {
    let exact: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 FROM project_document_reprojects r \
             JOIN project_document_state s ON s.community_id = r.community_id \
             JOIN communities c ON c.id = r.community_id \
             WHERE r.community_id = $1 AND r.operation_id = $2 AND r.state = $3 \
               AND NOT c.project_document_enabled AND c.archived_at IS NULL \
               AND c.project_view_schema_version IN (2, 3) \
               AND s.projection_pubkey = r.source_projection_pubkey \
               AND s.projection_generation = r.source_projection_generation \
               AND s.catalog_revision = r.catalog_revision \
               AND s.active_document_count = r.active_document_count \
               AND r.source_projection_generation = $4 \
               AND r.target_projection_generation = $5 \
               AND r.target_projection_pubkey = $6 \
               AND r.catalog_revision = $7 \
               AND r.active_document_count = $8 \
               AND r.document_count = $9 AND r.revision_count = $10)",
    )
    .bind(context.community_id.as_uuid())
    .bind(context.operation_id)
    .bind(required_state)
    .bind(revision_to_i64(
        context.source_generation,
        "source_generation",
    )?)
    .bind(revision_to_i64(
        context.target_generation,
        "target_generation",
    )?)
    .bind(context.target_pubkey.as_bytes())
    .bind(revision_to_i64(
        context.catalog_revision,
        "catalog_revision",
    )?)
    .bind(revision_to_i64(
        context.active_document_count,
        "active_document_count",
    )?)
    .bind(revision_to_i64(context.document_count, "document_count")?)
    .bind(revision_to_i64(context.revision_count, "revision_count")?)
    .fetch_one(&mut **tx)
    .await?;
    if !exact {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "Project Document reproject basis changed or is in the wrong state".to_owned(),
        ));
    }
    Ok(())
}

fn reproject_revision_from_row(
    row: &sqlx::postgres::PgRow,
) -> ProjectDocumentWriteResult<ProjectDocumentReprojectRevision> {
    let document_id: Uuid = row.try_get("document_id")?;
    let document_revision =
        db_positive_revision(row.try_get("document_revision")?, "document_revision")?;
    let catalog_revision =
        db_positive_revision(row.try_get("catalog_revision")?, "catalog_revision")?;
    let actor = public_key_from_bytes(&row.try_get::<Vec<u8>, _>("actor_pubkey")?, "actor")?;
    let canonical_at = row.try_get("canonical_at")?;
    let state = parse_state(&row.try_get::<String, _>("state")?)?;
    let revision = match state {
        DocumentState::Active => DocumentRevision::Active {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            document_id,
            document_revision,
            snapshot: DocumentSnapshot {
                title: row.try_get::<Option<String>, _>("title")?.ok_or_else(|| {
                    ProjectDocumentWriteError::InvalidCommit(
                        "active reproject revision has no title".to_owned(),
                    )
                })?,
                summary: row.try_get("summary")?,
                content_markdown: row
                    .try_get::<Option<String>, _>("content_markdown")?
                    .ok_or_else(|| {
                        ProjectDocumentWriteError::InvalidCommit(
                            "active reproject revision has no Markdown".to_owned(),
                        )
                    })?,
            },
            actor,
            canonical_at,
        },
        DocumentState::Deleted => DocumentRevision::Deleted {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            document_id,
            document_revision,
            actor,
            canonical_at,
        },
    };
    revision.validate()?;
    let source = row
        .try_get::<Option<Vec<u8>>, _>("source_event_id")?
        .ok_or_else(|| {
            ProjectDocumentWriteError::InvalidCommit(
                "all v1 revisions must retain their Human source event for reprojection".to_owned(),
            )
        })?;
    Ok(ProjectDocumentReprojectRevision {
        document_id,
        document_revision,
        catalog_revision,
        revision,
        created: DocumentAttribution {
            at: row.try_get("document_created_at")?,
            by: public_key_from_bytes(&row.try_get::<Vec<u8>, _>("created_by")?, "created_by")?,
        },
        source_event_id: event_id_from_bytes(&source, "source_event_id")?,
        is_current: row.try_get("is_current")?,
    })
}

fn validate_staged_reproject_event(
    context: &ProjectDocumentReprojectContext,
    prepared: &PreparedProjectDocumentReprojectEvent,
) -> ProjectDocumentWriteResult<StagedReprojectEvent> {
    prepared.event.verify().map_err(|error| {
        ProjectDocumentWriteError::InvalidCommit(format!(
            "invalid staged projection signature: {error}"
        ))
    })?;
    if prepared.event.pubkey != context.target_pubkey {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "staged projection signer does not match target generation".to_owned(),
        ));
    }
    let (projection_type, document_id, document_revision, generation) = match prepared
        .projection_type
    {
        ProjectDocumentReprojectEventType::Revision => {
            let verified = parse_document_revision(
                &prepared.event,
                &context.target_pubkey,
                context.community_id,
            )
            .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
            match verified.projection {
                DocumentRevisionProjection::Active {
                    projection_generation,
                    document_id,
                    document_revision,
                    ..
                }
                | DocumentRevisionProjection::Deleted {
                    projection_generation,
                    document_id,
                    document_revision,
                    ..
                } => (
                    "revision",
                    Some(document_id),
                    Some(document_revision),
                    projection_generation,
                ),
            }
        }
        ProjectDocumentReprojectEventType::Head => {
            let verified = parse_document_head(
                &prepared.event,
                &context.target_pubkey,
                context.community_id,
            )
            .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
            match verified.projection {
                DocumentHeadProjection::Active {
                    projection_generation,
                    document_id,
                    document_revision,
                    ..
                }
                | DocumentHeadProjection::Deleted {
                    projection_generation,
                    document_id,
                    document_revision,
                    ..
                } => (
                    "head",
                    Some(document_id),
                    Some(document_revision),
                    projection_generation,
                ),
            }
        }
        ProjectDocumentReprojectEventType::Meta => {
            let verified = parse_document_meta(&prepared.event, &context.target_pubkey)
                .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
            if verified.projection.project_id != *context.community_id.as_uuid()
                || !verified.projection.reset
                || verified.projection.catalog_revision != context.catalog_revision
                || verified.projection.active_document_count != context.active_document_count
                || verified.projection.updated_at != context.updated_at
            {
                return Err(ProjectDocumentWriteError::InvalidCommit(
                    "staged reset metadata does not match the fixed canonical catalog".to_owned(),
                ));
            }
            (
                "meta",
                None,
                None,
                verified.projection.projection_generation,
            )
        }
    };
    if generation != context.target_generation
        || prepared.document_id != document_id
        || prepared.document_revision != document_revision
    {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "staged projection identity or generation does not match its envelope".to_owned(),
        ));
    }
    let created_at_seconds = i64::try_from(prepared.event.created_at.as_secs()).map_err(|_| {
        ProjectDocumentWriteError::InvalidCommit(
            "staged projection timestamp does not fit PostgreSQL".to_owned(),
        )
    })?;
    let created_at = DateTime::from_timestamp(created_at_seconds, 0).ok_or_else(|| {
        ProjectDocumentWriteError::InvalidCommit(
            "staged projection timestamp is invalid".to_owned(),
        )
    })?;
    let event_key = match (projection_type, document_id, document_revision) {
        ("meta", None, None) => "meta".to_owned(),
        (kind, Some(document_id), Some(revision)) => {
            format!("{kind}:{document_id}:{revision}")
        }
        _ => {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "invalid staged projection key".to_owned(),
            ));
        }
    };
    let kind = i32::from(prepared.event.kind.as_u16());
    let d_tag = crate::event::extract_d_tag(&prepared.event).ok_or_else(|| {
        ProjectDocumentWriteError::InvalidCommit(
            "staged projection has no canonical d tag".to_owned(),
        )
    })?;
    Ok(StagedReprojectEvent {
        event_key,
        projection_type,
        document_id,
        document_revision: document_revision
            .map(|value| revision_to_i64(value, "document_revision"))
            .transpose()?,
        event_id: prepared.event.id.as_bytes().to_vec(),
        pubkey: prepared.event.pubkey.to_bytes().to_vec(),
        created_at,
        kind,
        tags: serde_json::to_value(&prepared.event.tags).map_err(DbError::from)?,
        content: prepared.event.content.clone(),
        sig: prepared.event.sig.serialize().to_vec(),
        d_tag,
    })
}

fn expected_reproject_revision_projection(
    community_id: CommunityId,
    generation: u64,
    source: &ProjectDocumentReprojectRevision,
) -> DocumentRevisionProjection {
    match &source.revision {
        DocumentRevision::Active {
            snapshot,
            actor,
            canonical_at,
            ..
        } => DocumentRevisionProjection::Active {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: buzz_project_document::DocumentProjectionType::DocumentRevision,
            project_id: *community_id.as_uuid(),
            projection_generation: generation,
            catalog_revision: source.catalog_revision,
            document_id: source.document_id,
            document_revision: source.document_revision,
            title: snapshot.title.clone(),
            summary: snapshot.summary.clone(),
            content_markdown: snapshot.content_markdown.clone(),
            created_at: source.created.at,
            created_by: source.created.by,
            revision_at: *canonical_at,
            revision_by: *actor,
            source_event_id: source.source_event_id,
        },
        DocumentRevision::Deleted {
            actor,
            canonical_at,
            ..
        } => DocumentRevisionProjection::Deleted {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: buzz_project_document::DocumentProjectionType::DocumentRevision,
            project_id: *community_id.as_uuid(),
            projection_generation: generation,
            catalog_revision: source.catalog_revision,
            document_id: source.document_id,
            document_revision: source.document_revision,
            created_at: source.created.at,
            created_by: source.created.by,
            revision_at: *canonical_at,
            revision_by: *actor,
            source_event_id: source.source_event_id,
        },
    }
}

fn reproject_status_from_row(
    row: sqlx::postgres::PgRow,
) -> crate::Result<ProjectDocumentReprojectStatus> {
    Ok(ProjectDocumentReprojectStatus {
        operation_id: row.try_get("operation_id")?,
        community_id: CommunityId::from_uuid(row.try_get("community_id")?),
        state: row.try_get("state")?,
        source_generation: db_positive_revision_db(
            row.try_get("source_projection_generation")?,
            "source_projection_generation",
        )?,
        target_generation: db_positive_revision_db(
            row.try_get("target_projection_generation")?,
            "target_projection_generation",
        )?,
        target_pubkey: PublicKey::from_slice(
            &row.try_get::<Vec<u8>, _>("target_projection_pubkey")?,
        )
        .map_err(|error| DbError::InvalidData(format!("invalid target signer: {error}")))?,
        revision_count: db_nonnegative_revision_db(
            row.try_get("revision_count")?,
            "revision_count",
        )?,
        document_count: db_nonnegative_revision_db(
            row.try_get("document_count")?,
            "document_count",
        )?,
        staged_revision_count: db_nonnegative_revision_db(
            row.try_get("staged_revision_count")?,
            "staged_revision_count",
        )?,
        staged_head_count: db_nonnegative_revision_db(
            row.try_get("staged_head_count")?,
            "staged_head_count",
        )?,
        meta_staged: row.try_get("meta_staged")?,
    })
}

async fn append_document_control_audit(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    operation: &'static str,
) -> ProjectDocumentWriteResult<()> {
    buzz_audit::append_in_transaction(
        tx,
        NewAuditEntry {
            community_id,
            action: AuditAction::ProjectDocumentControl,
            actor_pubkey: None,
            object_id: Some(community_id.to_string()),
            detail: serde_json::json!({ "operation": operation }),
        },
    )
    .await?;
    Ok(())
}

impl ProjectDocumentWriteTx {
    /// Explicitly roll back and release the Community lock.
    pub async fn rollback(self) -> ProjectDocumentWriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Revalidate signer and actor authority, then look up a durable receipt
    /// before any current Markdown body is loaded.
    pub async fn prepare_command(
        &mut self,
        command_event: &Event,
        command: &ProjectDocumentCommand,
    ) -> ProjectDocumentWriteResult<ProjectDocumentPrepareOutcome> {
        let parsed = parse_document_command(command_event)
            .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
        if &parsed != command {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "command event does not carry the supplied strict command".to_owned(),
            ));
        }
        let signer: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT projection_pubkey FROM project_document_state \
             WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?;
        if signer.as_deref() != Some(self.expected_projection_pubkey.as_bytes()) {
            return Err(ProjectDocumentWriteError::Unavailable {
                community_id: self.community_id,
            });
        }
        sqlx::query("SELECT project_document_validate_community($1)")
            .bind(self.community_id.as_uuid())
            .execute(&mut *self.tx)
            .await?;
        validate_actor_in_tx(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command.acting_assignment_id,
            command.runtime_fence,
        )
        .await?;
        if let Some(receipt) =
            find_receipt(&mut self.tx, self.community_id, command_event.id.as_bytes()).await?
        {
            if receipt.actor != command_event.pubkey
                || receipt.operation != command.operation()
                || receipt.document_id != command.document_id()
                || receipt.expected_document_revision != command.expected_document_revision
                || receipt.acting_assignment_id != command.acting_assignment_id
            {
                return Err(ProjectDocumentWriteError::InvalidCommit(
                    "stored receipt does not match the replayed signed command".to_owned(),
                ));
            }
            return Ok(ProjectDocumentPrepareOutcome::Replayed(receipt));
        }
        Ok(ProjectDocumentPrepareOutcome::New)
    }

    /// Lock and reconstruct the catalog plus one exact current Document target.
    pub async fn load_current(
        &mut self,
        document_id: Uuid,
    ) -> ProjectDocumentWriteResult<ProjectDocumentWriteContext> {
        if self.loaded.is_some() {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "this transaction already loaded a Document target".to_owned(),
            ));
        }
        let state_row = sqlx::query(
            "SELECT catalog_revision, active_document_count, last_change_id, \
                    last_actor_pubkey, projection_pubkey, projection_generation, \
                    meta_projection_event_id, initialized_at, updated_at \
             FROM project_document_state WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some(state_row) = state_row else {
            return Err(ProjectDocumentWriteError::Unavailable {
                community_id: self.community_id,
            });
        };
        let metadata = state_metadata_from_row(&state_row)?;
        if metadata.projection_pubkey != self.expected_projection_pubkey {
            return Err(ProjectDocumentWriteError::Unavailable {
                community_id: self.community_id,
            });
        }
        // Validate current event pointers and aggregate parity before exposing
        // the canonical snapshot or allowing a stored receipt to replay. The
        // stable signer comparison above additionally fences signer rotation.
        sqlx::query("SELECT project_document_validate_community($1)")
            .bind(self.community_id.as_uuid())
            .execute(&mut *self.tx)
            .await?;
        let catalog = DocumentCatalog::from_snapshot(
            self.community_id,
            metadata.catalog_revision,
            metadata.active_document_count,
            metadata.projection_generation,
            metadata.initialized_at,
            metadata.updated_at,
        )?;

        let row = sqlx::query(
            "SELECT d.current_revision, d.state, d.created_at, d.created_by, \
                    d.updated_at, d.updated_by, r.title, r.summary, r.content_markdown, \
                    r.actor_pubkey, r.canonical_at \
             FROM project_documents d \
             JOIN project_document_revisions r \
               ON r.community_id = d.community_id \
              AND r.document_id = d.document_id \
              AND r.document_revision = d.current_revision \
             WHERE d.community_id = $1 AND d.document_id = $2 FOR UPDATE OF d, r",
        )
        .bind(self.community_id.as_uuid())
        .bind(document_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        let current = row
            .map(|row| current_document_from_row(document_id, &row))
            .transpose()?;
        let canonical_time: DateTime<Utc> = sqlx::query_scalar(
            "SELECT GREATEST(clock_timestamp(), $1::timestamptz + interval '1 microsecond')",
        )
        .bind(metadata.updated_at)
        .fetch_one(&mut *self.tx)
        .await?;

        // The shared Community lock serializes Document deletion with Project
        // View Resource/Context mutation and Project Context attach/detach.
        // Guides, Live references, and active Context Edge bindings protect
        // the current Document; Pinned references deliberately preserve only
        // an immutable historical revision and do not block ordinary delete.
        let deletion_blocked: bool = sqlx::query_scalar(
            "SELECT \
                EXISTS ( \
                    SELECT 1 FROM project_view_objects resource \
                    WHERE resource.community_id = $1 \
                      AND resource.schema_version = 3 \
                      AND resource.deleted_at IS NULL \
                      AND resource.guide_document_id = $2 \
                ) OR EXISTS ( \
                    SELECT 1 FROM project_view_document_context_references reference \
                    WHERE reference.community_id = $1 \
                      AND reference.target_document_id = $2 \
                      AND reference.reference_mode = 'live' \
                ) OR EXISTS ( \
                    SELECT 1 FROM project_context_document_bindings binding \
                    WHERE binding.community_id = $1 \
                      AND binding.context_document_id = $2 \
                      AND binding.state = 'active' \
                )",
        )
        .bind(self.community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&mut *self.tx)
        .await?;
        self.loaded = Some(LoadedBasis {
            target_id: document_id,
            catalog: catalog.clone(),
            current: current.clone(),
            projection_pubkey: metadata.projection_pubkey,
            canonical_time,
            deletion_blocked,
        });
        Ok(ProjectDocumentWriteContext {
            catalog,
            current,
            canonical_time,
            deletion_blocked,
        })
    }

    /// Commit command, receipt, full revision, current pointers, and all three
    /// projection events as one transaction.
    pub async fn commit(
        mut self,
        prepared: PreparedProjectDocumentCommit,
    ) -> ProjectDocumentWriteResult<ProjectDocumentCommitOutcome> {
        let parsed_command = parse_document_command(&prepared.command_event)
            .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
        if parsed_command != prepared.command
            || u32::from(prepared.command_event.kind.as_u16()) != KIND_PROJECT_DOCUMENT_COMMAND
        {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "command event does not carry the supplied strict command".to_owned(),
            ));
        }
        let loaded = self.loaded.as_ref().ok_or_else(|| {
            ProjectDocumentWriteError::InvalidCommit(
                "commit must be derived from load_current on the same transaction".to_owned(),
            )
        })?;
        if loaded.target_id != prepared.command.document_id() {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "loaded target differs from the command Document ID".to_owned(),
            ));
        }

        // Authority is rechecked under the Community lock and intentionally
        // precedes receipt lookup: revocation must make old retries fail.
        validate_actor_in_tx(
            &mut self.tx,
            self.community_id,
            prepared.command_event.pubkey,
            prepared.command.acting_assignment_id,
            prepared.command.runtime_fence,
        )
        .await?;
        if let Some(receipt) = find_receipt(
            &mut self.tx,
            self.community_id,
            prepared.command_event.id.as_bytes(),
        )
        .await?
        {
            self.tx.commit().await?;
            return Ok(ProjectDocumentCommitOutcome {
                receipt,
                replayed: true,
            });
        }

        let derived = reduce_document(
            &loaded.catalog,
            loaded.current.as_ref(),
            &prepared.command,
            DocumentChangeContext::new(
                prepared.command_event.pubkey,
                prepared.command_event.id,
                loaded.canonical_time,
            )
            .with_deletion_blocked(loaded.deletion_blocked),
        )?;
        if derived != prepared.transition {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "prepared transition was not derived from the locked canonical basis".to_owned(),
            ));
        }
        verify_document_projection_bundle(
            prepared.transition.projection_plan(),
            &prepared.revision_projection,
            &prepared.head_projection,
            &prepared.meta_projection,
            &loaded.projection_pubkey,
        )
        .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
        validate_projection_kinds(&prepared)?;

        let document_id = prepared.command.document_id();
        let existing = sqlx::query(
            "SELECT current_revision, current_head_event_id \
             FROM project_documents \
             WHERE community_id = $1 AND document_id = $2 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(document_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        match (&prepared.command.request, existing.as_ref()) {
            (DocumentCommandRequest::Create { .. }, None) => {}
            (DocumentCommandRequest::Create { .. }, Some(_)) => {
                return Err(DocumentError::DocumentIdAlreadyExists { document_id }.into());
            }
            (_, None) => return Err(DocumentError::DocumentNotFound { document_id }.into()),
            (_, Some(row)) => {
                let current =
                    db_nonnegative_revision(row.try_get("current_revision")?, "current_revision")?;
                if current != prepared.command.expected_document_revision {
                    return Err(DocumentError::RevisionConflict {
                        expected: prepared.command.expected_document_revision,
                        actual: Some(current),
                    }
                    .into());
                }
            }
        }

        let old_meta_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT meta_projection_event_id FROM project_document_state \
             WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_one(&mut *self.tx)
        .await?;
        if let Some(row) = existing.as_ref() {
            let old_head_event_id: Vec<u8> = row.try_get("current_head_event_id")?;
            if !crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                &old_head_event_id,
                KIND_PROJECT_DOCUMENT_HEAD,
            )
            .await?
            {
                return Err(ProjectDocumentWriteError::InvalidCommit(
                    "stored Document head pointer is not live".to_owned(),
                ));
            }
        }
        if !crate::event::retire_projection_head_in_tx(
            &mut self.tx,
            self.community_id,
            &old_meta_event_id,
            KIND_PROJECT_DOCUMENT_META,
        )
        .await?
        {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "stored Document metadata pointer is not live".to_owned(),
            ));
        }

        for (label, event) in [
            ("command", &prepared.command_event),
            ("revision", &prepared.revision_projection),
            ("head", &prepared.head_projection),
            ("metadata", &prepared.meta_projection),
        ] {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, event, None)
                    .await?;
            if !inserted {
                return Err(ProjectDocumentWriteError::InvalidCommit(format!(
                    "{label} event already exists without a canonical receipt"
                )));
            }
        }

        let receipt = prepared.transition.receipt().clone();
        let receipt_result = serde_json::to_value(&receipt).map_err(|error| {
            ProjectDocumentWriteError::InvalidCommit(format!("serialize receipt: {error}"))
        })?;
        insert_change(
            &mut self.tx,
            self.community_id,
            &prepared.command_event,
            &prepared.command,
            &receipt,
            &receipt_result,
        )
        .await?;
        insert_revision(
            &mut self.tx,
            self.community_id,
            prepared.transition.current(),
            receipt.catalog_revision,
            prepared.command_event.id,
            prepared.transition.catalog().projection_generation(),
            prepared.revision_projection.id,
        )
        .await?;
        write_current_document(
            &mut self.tx,
            self.community_id,
            prepared.transition.current(),
            prepared.command_event.id,
            prepared.head_projection.id,
            prepared.revision_projection.id,
            existing.is_some(),
        )
        .await?;

        let catalog = prepared.transition.catalog();
        let update = sqlx::query(
            "UPDATE project_document_state \
             SET catalog_revision = $2, active_document_count = $3, \
                 last_change_id = $4, last_actor_pubkey = $5, updated_at = $6, \
                 meta_projection_event_id = $7 \
             WHERE community_id = $1 AND catalog_revision = $8 \
               AND projection_generation = $9 AND projection_pubkey = $10",
        )
        .bind(self.community_id.as_uuid())
        .bind(revision_to_i64(
            catalog.catalog_revision(),
            "catalog_revision",
        )?)
        .bind(revision_to_i64(
            catalog.active_document_count(),
            "active_document_count",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(prepared.command_event.pubkey.as_bytes())
        .bind(catalog.updated_at())
        .bind(prepared.meta_projection.id.as_bytes().as_slice())
        .bind(revision_to_i64(
            loaded.catalog.catalog_revision(),
            "expected catalog_revision",
        )?)
        .bind(revision_to_i64(
            catalog.projection_generation(),
            "projection_generation",
        )?)
        .bind(loaded.projection_pubkey.as_bytes())
        .execute(&mut *self.tx)
        .await?;
        if update.rows_affected() != 1 {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "Document catalog changed while committing".to_owned(),
            ));
        }

        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *self.tx)
            .await?;
        self.tx.commit().await?;
        Ok(ProjectDocumentCommitOutcome {
            receipt,
            replayed: false,
        })
    }
}

async fn require_document_reader_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    reader_pubkey: &[u8],
) -> Result<(), ProjectDocumentReadError> {
    let authorized: bool = sqlx::query_scalar(
        "SELECT EXISTS ( \
             SELECT 1 \
             FROM (SELECT $2::bytea AS pubkey) requested \
             LEFT JOIN users actor \
               ON actor.community_id = $1 AND actor.pubkey = requested.pubkey \
             WHERE ( \
                 ( \
                     actor.agent_owner_pubkey IS NULL \
                     AND EXISTS ( \
                         SELECT 1 FROM relay_members direct_member \
                         WHERE direct_member.community_id = $1 \
                           AND direct_member.pubkey = encode(requested.pubkey, 'hex') \
                     ) \
                 ) \
                 OR ( \
                     actor.agent_owner_pubkey IS NOT NULL \
                     AND EXISTS ( \
                         SELECT 1 FROM relay_members owner_member \
                         WHERE owner_member.community_id = $1 \
                           AND owner_member.pubkey = encode(actor.agent_owner_pubkey, 'hex') \
                     ) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM community_bans owner_ban \
                         WHERE owner_ban.community_id = $1 \
                           AND owner_ban.pubkey = actor.agent_owner_pubkey \
                           AND owner_ban.banned \
                           AND (owner_ban.ban_expires_at IS NULL \
                                OR owner_ban.ban_expires_at > clock_timestamp()) \
                     ) \
                     AND NOT EXISTS ( \
                         SELECT 1 FROM users owner_actor \
                         WHERE owner_actor.community_id = $1 \
                           AND owner_actor.pubkey = actor.agent_owner_pubkey \
                           AND owner_actor.agent_owner_pubkey IS NOT NULL \
                     ) \
                 ) \
             ) \
             AND NOT EXISTS ( \
                 SELECT 1 FROM community_bans actor_ban \
                 WHERE actor_ban.community_id = $1 \
                   AND actor_ban.pubkey = requested.pubkey \
                   AND actor_ban.banned \
                   AND (actor_ban.ban_expires_at IS NULL \
                        OR actor_ban.ban_expires_at > clock_timestamp()) \
             ) \
         )",
    )
    .bind(community_id.as_uuid())
    .bind(reader_pubkey)
    .fetch_one(&mut **tx)
    .await?;
    if !authorized {
        return Err(ProjectDocumentReadError::Restricted);
    }
    Ok(())
}

async fn require_document_read_state_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    expected_pubkey: &PublicKey,
    projection_generation: u64,
    catalog_revision: Option<u64>,
) -> Result<(), ProjectDocumentReadError> {
    let row = sqlx::query(
        "SELECT c.project_document_enabled, c.project_view_schema_version, \
                s.projection_pubkey, s.projection_generation, s.catalog_revision \
         FROM communities c \
         LEFT JOIN project_document_state s ON s.community_id = c.id \
         WHERE c.id = $1 AND c.archived_at IS NULL FOR SHARE OF c",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Err(ProjectDocumentReadError::Unavailable);
    };
    let enabled: bool = row.try_get("project_document_enabled")?;
    let schema: i16 = row.try_get("project_view_schema_version")?;
    let signer: Option<Vec<u8>> = row.try_get("projection_pubkey")?;
    let generation: Option<i64> = row.try_get("projection_generation")?;
    let revision: Option<i64> = row.try_get("catalog_revision")?;
    if !enabled
        || !matches!(schema, 2 | 3)
        || signer.as_deref() != Some(expected_pubkey.as_bytes())
        || generation.and_then(|value| u64::try_from(value).ok()) != Some(projection_generation)
    {
        return Err(ProjectDocumentReadError::Unavailable);
    }
    if catalog_revision.is_some()
        && revision.and_then(|value| u64::try_from(value).ok()) != catalog_revision
    {
        return Err(ProjectDocumentReadError::Conflict);
    }
    // Canonical writers and controlled enable/reproject paths enforce pointer
    // parity. Re-running a whole-Community validator for every history page
    // makes keyset pagination O(total history × pages), defeating its bounded
    // query contract. The fixed generation/signer basis above plus the exact
    // indexed pointer join below are the read-time fence.
    Ok(())
}

pub(crate) async fn validate_actor_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    acting_assignment_id: Option<Uuid>,
    runtime_fence: Option<buzz_core::RuntimeFence>,
) -> ProjectDocumentWriteResult<()> {
    let actor_bytes = actor.to_bytes();
    let owner: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT agent_owner_pubkey FROM users \
         WHERE community_id = $1 AND pubkey = $2",
    )
    .bind(community_id.as_uuid())
    .bind(actor_bytes.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .flatten();
    let managed = owner.is_some();
    let direct_member: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM relay_members \
         WHERE community_id = $1 AND pubkey = $2)",
    )
    .bind(community_id.as_uuid())
    .bind(actor.to_hex())
    .fetch_one(&mut **tx)
    .await?;
    if active_write_restriction_in_tx(tx, community_id, actor_bytes.as_slice()).await? {
        return Err(ProjectDocumentWriteError::NotAuthorized);
    }
    if acting_assignment_id.is_none() && runtime_fence.is_some() {
        return Err(ProjectDocumentWriteError::RuntimeFence);
    }

    if let Some(owner) = owner {
        let owner_pubkey = public_key_from_bytes(&owner, "managed Agent owner")?;
        let owner_is_member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM relay_members \
             WHERE community_id = $1 AND pubkey = $2)",
        )
        .bind(community_id.as_uuid())
        .bind(owner_pubkey.to_hex())
        .fetch_one(&mut **tx)
        .await?;
        if !owner_is_member || active_write_restriction_in_tx(tx, community_id, &owner).await? {
            return Err(ProjectDocumentWriteError::NotAuthorized);
        }
    } else {
        if !direct_member {
            return Err(ProjectDocumentWriteError::NotAuthorized);
        }
        // Human Document commands never borrow Role/Runtime authority. This
        // keeps attribution unambiguous and prevents a stale optional v2 fence
        // from changing the meaning of a direct-member write.
        if acting_assignment_id.is_some() || runtime_fence.is_some() {
            return Err(ProjectDocumentWriteError::ActingAssignmentInvalid);
        }
    }

    if let Some(assignment_id) = acting_assignment_id {
        let assignment_valid: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM project_role_assignments \
             WHERE community_id = $1 AND assignment_id = $2 \
               AND member_pubkey = $3 AND ended_at IS NULL)",
        )
        .bind(community_id.as_uuid())
        .bind(assignment_id)
        .bind(actor.to_hex())
        .fetch_one(&mut **tx)
        .await?;
        if !assignment_valid {
            return Err(ProjectDocumentWriteError::ActingAssignmentInvalid);
        }
    }
    // Ordinary managed-Agent Document writes use Community authority and carry
    // no Role/Runtime attribution. When a managed caller explicitly claims an
    // Assignment, preserve the existing strict supervised-Runtime guarantee;
    // an invalid claim must never be silently downgraded to a Community write.
    if managed && acting_assignment_id.is_some() {
        crate::project_runtime::validate_runtime_command_fence_in_tx(
            tx,
            community_id,
            acting_assignment_id,
            runtime_fence,
            crate::project_runtime::RuntimeCommandFencePolicy::RequireSupervisedRuntime,
        )
        .await
        .map_err(|_| ProjectDocumentWriteError::RuntimeFence)?;
    }
    Ok(())
}

async fn active_write_restriction_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    pubkey: &[u8],
) -> ProjectDocumentWriteResult<bool> {
    let restricted: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM community_bans \
         WHERE community_id = $1 AND pubkey = $2 \
           AND ( \
             (banned AND (ban_expires_at IS NULL OR ban_expires_at > clock_timestamp())) \
             OR muted_until > clock_timestamp() \
           ))",
    )
    .bind(community_id.as_uuid())
    .bind(pubkey)
    .fetch_one(&mut **tx)
    .await?;
    Ok(restricted)
}

async fn find_receipt(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
) -> ProjectDocumentWriteResult<Option<ProjectDocumentReceipt>> {
    let row = sqlx::query(
        "SELECT source_type, source_event_id, actor_pubkey, operation, document_id, \
                expected_document_revision, document_revision, catalog_revision, \
                result, accepted_at \
         FROM project_document_changes \
         WHERE community_id = $1 AND change_id = $2 FOR SHARE",
    )
    .bind(community_id.as_uuid())
    .bind(change_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let source_type: String = row.try_get("source_type")?;
    let source_event_id: Option<Vec<u8>> = row.try_get("source_event_id")?;
    if source_type != "nostr_event" || source_event_id.as_deref() != Some(change_id) {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "stored member receipt has an invalid source shape".to_owned(),
        ));
    }
    let result: Value = row.try_get("result")?;
    let receipt: ProjectDocumentReceipt = serde_json::from_value(result).map_err(|error| {
        ProjectDocumentWriteError::InvalidCommit(format!("invalid stored receipt: {error}"))
    })?;
    receipt.validate()?;
    let actor: Vec<u8> = row.try_get("actor_pubkey")?;
    let operation: String = row.try_get("operation")?;
    let document_id: Uuid = row.try_get("document_id")?;
    let expected_revision = db_nonnegative_revision(
        row.try_get("expected_document_revision")?,
        "expected_document_revision",
    )?;
    let document_revision =
        db_positive_revision(row.try_get("document_revision")?, "document_revision")?;
    let catalog_revision =
        db_positive_revision(row.try_get("catalog_revision")?, "catalog_revision")?;
    let accepted_at: DateTime<Utc> = row.try_get("accepted_at")?;
    if receipt.change_id.as_bytes() != change_id
        || receipt.actor.as_bytes() != actor.as_slice()
        || receipt.operation.as_str() != operation
        || receipt.document_id != document_id
        || receipt.expected_document_revision != expected_revision
        || receipt.document_revision != document_revision
        || receipt.catalog_revision != catalog_revision
        || receipt.accepted_at != accepted_at
    {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "stored receipt columns and closed JSON result disagree".to_owned(),
        ));
    }
    Ok(Some(receipt))
}

async fn insert_change(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    command_event: &Event,
    command: &ProjectDocumentCommand,
    receipt: &ProjectDocumentReceipt,
    receipt_result: &Value,
) -> ProjectDocumentWriteResult<()> {
    let actor = command_event.pubkey.to_bytes();
    sqlx::query(
        "INSERT INTO project_document_changes \
            (community_id, change_id, source_type, source_event_id, actor_pubkey, \
             acting_assignment_id, operation, document_id, expected_document_revision, \
             document_revision, catalog_revision, result, accepted_at) \
         VALUES ($1, $2, 'nostr_event', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
    )
    .bind(community_id.as_uuid())
    .bind(command_event.id.as_bytes().as_slice())
    .bind(actor.as_slice())
    .bind(command.acting_assignment_id)
    .bind(command.operation().as_str())
    .bind(command.document_id())
    .bind(revision_to_i64(
        command.expected_document_revision,
        "expected_document_revision",
    )?)
    .bind(revision_to_i64(
        receipt.document_revision,
        "document_revision",
    )?)
    .bind(revision_to_i64(
        receipt.catalog_revision,
        "catalog_revision",
    )?)
    .bind(receipt_result)
    .bind(receipt.accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn insert_revision(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    current: &CurrentDocument,
    catalog_revision: u64,
    source_event_id: EventId,
    projection_generation: u64,
    projection_event_id: EventId,
) -> ProjectDocumentWriteResult<()> {
    let document = current.document();
    let revision = current.revision();
    let (title, summary, content_markdown) = match revision {
        DocumentRevision::Active { snapshot, .. } => (
            Some(snapshot.title.as_str()),
            snapshot.summary.as_deref(),
            Some(snapshot.content_markdown.as_str()),
        ),
        DocumentRevision::Deleted { .. } => (None, None, None),
    };
    let actor = revision.actor().to_bytes();
    sqlx::query(
        "INSERT INTO project_document_revisions \
            (community_id, document_id, document_revision, catalog_revision, state, \
             title, summary, content_markdown, actor_pubkey, canonical_at, \
             source_change_id, source_event_id, projection_generation, projection_event_id) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $11, $12, $13)",
    )
    .bind(community_id.as_uuid())
    .bind(document.document_id())
    .bind(revision_to_i64(
        document.current_revision(),
        "document_revision",
    )?)
    .bind(revision_to_i64(catalog_revision, "catalog_revision")?)
    .bind(document.state().as_str())
    .bind(title)
    .bind(summary)
    .bind(content_markdown)
    .bind(actor.as_slice())
    .bind(revision.canonical_at())
    .bind(source_event_id.as_bytes().as_slice())
    .bind(revision_to_i64(
        projection_generation,
        "projection_generation",
    )?)
    .bind(projection_event_id.as_bytes().as_slice())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn write_current_document(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    current: &CurrentDocument,
    source_change_id: EventId,
    head_event_id: EventId,
    revision_event_id: EventId,
    exists: bool,
) -> ProjectDocumentWriteResult<()> {
    let document = current.document();
    let created = document.created();
    let updated = document.updated();
    let created_by = created.by.to_bytes();
    let updated_by = updated.by.to_bytes();
    let deleted_at = (document.state() == DocumentState::Deleted).then_some(updated.at);
    if exists {
        let result = sqlx::query(
            "UPDATE project_documents \
             SET current_revision = $3, state = $4, updated_at = $5, updated_by = $6, \
                 deleted_at = $7, current_source_change_id = $8, \
                 current_head_event_id = $9, current_revision_event_id = $10 \
             WHERE community_id = $1 AND document_id = $2 AND current_revision = $11",
        )
        .bind(community_id.as_uuid())
        .bind(document.document_id())
        .bind(revision_to_i64(
            document.current_revision(),
            "current_revision",
        )?)
        .bind(document.state().as_str())
        .bind(updated.at)
        .bind(updated_by.as_slice())
        .bind(deleted_at)
        .bind(source_change_id.as_bytes().as_slice())
        .bind(head_event_id.as_bytes().as_slice())
        .bind(revision_event_id.as_bytes().as_slice())
        .bind(revision_to_i64(
            document.current_revision() - 1,
            "expected current_revision",
        )?)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectDocumentWriteError::InvalidCommit(
                "current Document revision changed while committing".to_owned(),
            ));
        }
    } else {
        sqlx::query(
            "INSERT INTO project_documents \
                (community_id, document_id, current_revision, state, created_at, created_by, \
                 updated_at, updated_by, deleted_at, current_source_change_id, \
                 current_head_event_id, current_revision_event_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
        )
        .bind(community_id.as_uuid())
        .bind(document.document_id())
        .bind(revision_to_i64(
            document.current_revision(),
            "current_revision",
        )?)
        .bind(document.state().as_str())
        .bind(created.at)
        .bind(created_by.as_slice())
        .bind(updated.at)
        .bind(updated_by.as_slice())
        .bind(deleted_at)
        .bind(source_change_id.as_bytes().as_slice())
        .bind(head_event_id.as_bytes().as_slice())
        .bind(revision_event_id.as_bytes().as_slice())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn validate_projection_kinds(
    prepared: &PreparedProjectDocumentCommit,
) -> ProjectDocumentWriteResult<()> {
    for (label, event, expected_kind) in [
        (
            "revision",
            &prepared.revision_projection,
            KIND_PROJECT_DOCUMENT_REVISION,
        ),
        (
            "head",
            &prepared.head_projection,
            KIND_PROJECT_DOCUMENT_HEAD,
        ),
        (
            "metadata",
            &prepared.meta_projection,
            KIND_PROJECT_DOCUMENT_META,
        ),
    ] {
        if u32::from(event.kind.as_u16()) != expected_kind {
            return Err(ProjectDocumentWriteError::InvalidCommit(format!(
                "{label} projection kind must be {expected_kind}"
            )));
        }
    }
    Ok(())
}

fn validate_bootstrap(
    prepared: &PreparedProjectDocumentBootstrap,
) -> ProjectDocumentWriteResult<()> {
    prepared.catalog.validate()?;
    if prepared.catalog.catalog_revision() != 0
        || prepared.catalog.active_document_count() != 0
        || prepared.catalog.initialized_at() != prepared.catalog.updated_at()
    {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "bootstrap catalog must be empty revision zero".to_owned(),
        ));
    }
    let verified = parse_document_meta(&prepared.meta_projection, &prepared.meta_projection.pubkey)
        .map_err(|error| ProjectDocumentWriteError::InvalidCommit(error.to_string()))?;
    let projection = verified.projection;
    if projection.project_id != *prepared.catalog.project_id().as_uuid()
        || !projection.initialized
        || !projection.reset
        || projection.catalog_revision != 0
        || projection.active_document_count != 0
        || projection.projection_generation != prepared.catalog.projection_generation()
        || projection.updated_at != prepared.catalog.updated_at()
        || projection.source_event_id.is_some()
        || !projection.changed_heads.is_empty()
    {
        return Err(ProjectDocumentWriteError::InvalidCommit(
            "bootstrap metadata does not exactly represent the empty catalog".to_owned(),
        ));
    }
    Ok(())
}

fn state_metadata_from_row(
    row: &sqlx::postgres::PgRow,
) -> ProjectDocumentWriteResult<ProjectDocumentStateMetadata> {
    let last_change: Option<Vec<u8>> = row.try_get("last_change_id")?;
    let last_actor: Option<Vec<u8>> = row.try_get("last_actor_pubkey")?;
    let projection_pubkey: Vec<u8> = row.try_get("projection_pubkey")?;
    let meta_event_id: Vec<u8> = row.try_get("meta_projection_event_id")?;
    Ok(ProjectDocumentStateMetadata {
        catalog_revision: db_nonnegative_revision(
            row.try_get("catalog_revision")?,
            "catalog_revision",
        )?,
        active_document_count: db_nonnegative_revision(
            row.try_get("active_document_count")?,
            "active_document_count",
        )?,
        last_change_id: last_change
            .map(|bytes| event_id_from_bytes(&bytes, "last_change_id"))
            .transpose()?,
        last_actor_pubkey: last_actor
            .map(|bytes| public_key_from_bytes(&bytes, "last_actor_pubkey"))
            .transpose()?,
        projection_pubkey: public_key_from_bytes(&projection_pubkey, "projection_pubkey")?,
        projection_generation: db_positive_revision(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?,
        meta_projection_event_id: event_id_from_bytes(&meta_event_id, "meta_projection_event_id")?,
        initialized_at: row.try_get("initialized_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn current_document_from_row(
    document_id: Uuid,
    row: &sqlx::postgres::PgRow,
) -> ProjectDocumentWriteResult<CurrentDocument> {
    let current_revision =
        db_positive_revision(row.try_get("current_revision")?, "current_revision")?;
    let state_text: String = row.try_get("state")?;
    let state = parse_state(&state_text)?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let created_by: Vec<u8> = row.try_get("created_by")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let updated_by: Vec<u8> = row.try_get("updated_by")?;
    let actor: Vec<u8> = row.try_get("actor_pubkey")?;
    let canonical_at: DateTime<Utc> = row.try_get("canonical_at")?;
    let created_by = public_key_from_bytes(&created_by, "created_by")?;
    let updated_by = public_key_from_bytes(&updated_by, "updated_by")?;
    let actor = public_key_from_bytes(&actor, "revision actor_pubkey")?;
    let document = ProjectDocument::from_snapshot(
        document_id,
        current_revision,
        state,
        DocumentAttribution {
            at: created_at,
            by: created_by,
        },
        DocumentAttribution {
            at: updated_at,
            by: updated_by,
        },
    )?;
    let revision = match state {
        DocumentState::Active => {
            let title: Option<String> = row.try_get("title")?;
            let content_markdown: Option<String> = row.try_get("content_markdown")?;
            DocumentRevision::Active {
                schema_version: 1,
                document_id,
                document_revision: current_revision,
                snapshot: buzz_project_document::DocumentSnapshot {
                    title: title.ok_or_else(|| {
                        ProjectDocumentWriteError::InvalidCommit(
                            "active revision has no title".to_owned(),
                        )
                    })?,
                    summary: row.try_get("summary")?,
                    content_markdown: content_markdown.ok_or_else(|| {
                        ProjectDocumentWriteError::InvalidCommit(
                            "active revision has no Markdown body".to_owned(),
                        )
                    })?,
                },
                actor,
                canonical_at,
            }
        }
        DocumentState::Deleted => DocumentRevision::Deleted {
            schema_version: 1,
            document_id,
            document_revision: current_revision,
            actor,
            canonical_at,
        },
    };
    Ok(CurrentDocument::new(document, revision)?)
}

fn status_from_row(row: sqlx::postgres::PgRow) -> crate::Result<ProjectDocumentFeatureStatus> {
    let community_id = CommunityId::from_uuid(row.try_get("id")?);
    let catalog_revision = row
        .try_get::<Option<i64>, _>("catalog_revision")?
        .map(|value| db_nonnegative_revision_db(value, "catalog_revision"))
        .transpose()?;
    let active_document_count = row
        .try_get::<Option<i64>, _>("active_document_count")?
        .map(|value| db_nonnegative_revision_db(value, "active_document_count"))
        .transpose()?;
    let projection_generation = row
        .try_get::<Option<i64>, _>("projection_generation")?
        .map(|value| db_positive_revision_db(value, "projection_generation"))
        .transpose()?;
    let projection_pubkey = row
        .try_get::<Option<Vec<u8>>, _>("projection_pubkey")?
        .map(|bytes| {
            PublicKey::from_slice(&bytes).map_err(|error| {
                DbError::InvalidData(format!("invalid projection_pubkey: {error}"))
            })
        })
        .transpose()?;
    let revision_count: i64 = row.try_get("revision_count")?;
    Ok(ProjectDocumentFeatureStatus {
        community_id,
        host: row.try_get("host")?,
        archived: row.try_get("archived")?,
        enabled: row.try_get("project_document_enabled")?,
        project_view_schema_version: row.try_get("project_view_schema_version")?,
        catalog_revision,
        active_document_count,
        revision_count: db_nonnegative_revision_db(revision_count, "revision_count")?,
        projection_generation,
        projection_pubkey,
    })
}

fn parse_state(value: &str) -> ProjectDocumentWriteResult<DocumentState> {
    match value {
        "active" => Ok(DocumentState::Active),
        "deleted" => Ok(DocumentState::Deleted),
        _ => Err(ProjectDocumentWriteError::InvalidCommit(format!(
            "unknown Document state {value}"
        ))),
    }
}

fn public_key_from_bytes(bytes: &[u8], field: &str) -> ProjectDocumentWriteResult<PublicKey> {
    PublicKey::from_slice(bytes).map_err(|error| {
        ProjectDocumentWriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn event_id_from_bytes(bytes: &[u8], field: &str) -> ProjectDocumentWriteResult<EventId> {
    EventId::from_slice(bytes).map_err(|error| {
        ProjectDocumentWriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn revision_to_i64(value: u64, field: &str) -> ProjectDocumentWriteResult<i64> {
    if value > MAX_SAFE_REVISION {
        return Err(ProjectDocumentWriteError::InvalidCommit(format!(
            "{field} exceeds the JSON-safe revision limit"
        )));
    }
    i64::try_from(value).map_err(|_| {
        ProjectDocumentWriteError::InvalidCommit(format!("{field} does not fit PostgreSQL BIGINT"))
    })
}

fn db_nonnegative_revision(value: i64, field: &str) -> ProjectDocumentWriteResult<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| {
            ProjectDocumentWriteError::InvalidCommit(format!(
                "{field} is outside the JSON-safe revision range"
            ))
        })
}

fn db_positive_revision(value: i64, field: &str) -> ProjectDocumentWriteResult<u64> {
    db_nonnegative_revision(value, field).and_then(|value| {
        if value == 0 {
            Err(ProjectDocumentWriteError::InvalidCommit(format!(
                "{field} must be positive"
            )))
        } else {
            Ok(value)
        }
    })
}

fn db_nonnegative_revision_db(value: i64, field: &str) -> crate::Result<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| DbError::InvalidData(format!("{field} is outside the safe range")))
}

fn db_positive_revision_db(value: i64, field: &str) -> crate::Result<u64> {
    let value = db_nonnegative_revision_db(value, field)?;
    if value == 0 {
        Err(DbError::InvalidData(format!("{field} must be positive")))
    } else {
        Ok(value)
    }
}

pub(crate) async fn document_projection_parity(
    connection: &mut sqlx::PgConnection,
    community_id: CommunityId,
    expected_pubkey: &PublicKey,
    meta_event_id: Option<&[u8]>,
    active_count: Option<i64>,
) -> crate::Result<bool> {
    let state = sqlx::query(
        "SELECT catalog_revision, projection_generation, initialized_at, updated_at, \
                meta_projection_event_id, active_document_count \
         FROM project_document_state WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut *connection)
    .await?;
    let Some(state) = state else {
        return Ok(false);
    };
    let catalog_revision: i64 = state.try_get("catalog_revision")?;
    let generation: i64 = state.try_get("projection_generation")?;
    let updated_at: DateTime<Utc> = state.try_get("updated_at")?;
    let stored_meta_event_id: Vec<u8> = state.try_get("meta_projection_event_id")?;
    let stored_active_count: i64 = state.try_get("active_document_count")?;
    let meta_event_id = meta_event_id.unwrap_or(&stored_meta_event_id);
    let active_count = active_count.unwrap_or(stored_active_count);
    if meta_event_id.len() != 32 || active_count < 0 {
        return Ok(false);
    }
    let meta = project_document_event_by_id(connection, community_id, meta_event_id).await?;
    let Some(meta) = meta else {
        return Ok(false);
    };
    let Ok(meta) = parse_document_meta(&meta.event, expected_pubkey) else {
        return Ok(false);
    };
    if meta.projection.project_id != *community_id.as_uuid()
        || i64::try_from(meta.projection.catalog_revision).ok() != Some(catalog_revision)
        || i64::try_from(meta.projection.projection_generation).ok() != Some(generation)
        || i64::try_from(meta.projection.active_document_count).ok() != Some(active_count)
        || meta.projection.updated_at != updated_at
    {
        return Ok(false);
    }

    let actual_active: i64 = sqlx::query_scalar(
        "SELECT count(*)::bigint FROM project_documents \
         WHERE community_id = $1 AND state = 'active'",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut *connection)
    .await?;
    if actual_active != active_count {
        return Ok(false);
    }
    let rows = sqlx::query(
        "SELECT document_id, current_revision, state, current_source_change_id, \
                current_head_event_id, current_revision_event_id \
         FROM project_documents WHERE community_id = $1 ORDER BY document_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut *connection)
    .await?;
    for row in rows {
        let document_id: Uuid = row.try_get("document_id")?;
        let document_revision: i64 = row.try_get("current_revision")?;
        let state: String = row.try_get("state")?;
        let source: Vec<u8> = row.try_get("current_source_change_id")?;
        let head_id: Vec<u8> = row.try_get("current_head_event_id")?;
        let revision_id: Vec<u8> = row.try_get("current_revision_event_id")?;
        let head = project_document_event_by_id(connection, community_id, &head_id).await?;
        let revision = project_document_event_by_id(connection, community_id, &revision_id).await?;
        let (Some(head), Some(revision)) = (head, revision) else {
            return Ok(false);
        };
        let Ok(head) = parse_document_head(&head.event, expected_pubkey, community_id) else {
            return Ok(false);
        };
        let Ok(revision) = parse_document_revision(&revision.event, expected_pubkey, community_id)
        else {
            return Ok(false);
        };
        let Ok(current) = VerifiedCurrentDocument::new(head, revision) else {
            return Ok(false);
        };
        let (projected_id, projected_revision, projected_state, projected_source) =
            verified_current_identity(&current);
        if projected_id != document_id
            || i64::try_from(projected_revision).ok() != Some(document_revision)
            || projected_state.as_str() != state
            || projected_source.as_bytes() != source.as_slice()
        {
            return Ok(false);
        }
    }
    let generation = db_positive_revision_db(generation, "projection_generation")?;
    let mut after_catalog_revision = 0_i64;
    let mut verified_revision_count = 0_u64;
    loop {
        let rows = sqlx::query(
            "SELECT e.id, e.pubkey, e.created_at, e.kind, e.tags, e.content, e.sig, \
                    e.received_at, e.channel_id, r.document_id, r.document_revision, \
                    r.catalog_revision, r.state, r.title, r.summary, r.content_markdown, \
                    r.actor_pubkey, r.canonical_at, r.source_event_id, \
                    d.created_at AS document_created_at, \
                    d.created_by, r.document_revision = d.current_revision AS is_current \
             FROM project_document_revisions r \
             JOIN project_documents d ON d.community_id = r.community_id \
                                     AND d.document_id = r.document_id \
             JOIN events e ON e.community_id = r.community_id \
                          AND e.id = r.projection_event_id \
                          AND e.kind = $2 AND e.pubkey = $3 AND e.deleted_at IS NULL \
             WHERE r.community_id = $1 AND r.catalog_revision > $4 \
               AND r.projection_generation = $5 \
             ORDER BY r.catalog_revision ASC LIMIT 500",
        )
        .bind(community_id.as_uuid())
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .bind(expected_pubkey.as_bytes())
        .bind(after_catalog_revision)
        .bind(i64::try_from(generation).unwrap_or(i64::MAX))
        .fetch_all(&mut *connection)
        .await?;
        if rows.is_empty() {
            break;
        }
        for row in rows {
            let canonical = reproject_revision_from_row(&row).map_err(|error| {
                DbError::InvalidData(format!(
                    "invalid canonical revision during full parity: {error}"
                ))
            })?;
            let stored = crate::event::row_to_stored_event(row)?.ok_or_else(|| {
                DbError::InvalidData(
                    "historical revision event could not be reconstructed".to_owned(),
                )
            })?;
            let parsed = match parse_document_revision(&stored.event, expected_pubkey, community_id)
            {
                Ok(parsed) => parsed,
                Err(_) => return Ok(false),
            };
            if parsed.projection
                != expected_reproject_revision_projection(community_id, generation, &canonical)
            {
                return Ok(false);
            }
            after_catalog_revision = i64::try_from(canonical.catalog_revision).unwrap_or(i64::MAX);
            verified_revision_count = verified_revision_count.saturating_add(1);
        }
    }
    if i64::try_from(verified_revision_count).ok() != Some(catalog_revision) {
        return Ok(false);
    }
    Ok(true)
}

async fn project_document_event_by_id(
    connection: &mut sqlx::PgConnection,
    community_id: CommunityId,
    event_id: &[u8],
) -> crate::Result<Option<StoredEvent>> {
    let row = sqlx::query(
        "SELECT id, pubkey, created_at, kind, tags, content, sig, received_at, channel_id \
         FROM events WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(community_id.as_uuid())
    .bind(event_id)
    .fetch_optional(&mut *connection)
    .await?;
    row.map(crate::event::row_to_stored_event)
        .transpose()
        .map(Option::flatten)
}

#[cfg(test)]
pub(crate) async fn begin_project_document_storage_test_write(
    db: &Db,
    community_id: CommunityId,
    expected_projection_pubkey: PublicKey,
) -> ProjectDocumentWriteResult<ProjectDocumentWriteTx> {
    let mut tx = db.pool.begin().await?;
    crate::community_lock::acquire(&mut tx, community_id, false).await?;
    let enabled: Option<bool> = sqlx::query_scalar(
        "SELECT project_document_enabled FROM communities \
         WHERE id = $1 AND archived_at IS NULL FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut *tx)
    .await?;
    if enabled != Some(true) {
        return Err(ProjectDocumentWriteError::Unavailable { community_id });
    }
    Ok(ProjectDocumentWriteTx {
        tx,
        community_id,
        expected_projection_pubkey,
        loaded: None,
    })
}

fn verified_current_identity(
    current: &VerifiedCurrentDocument,
) -> (Uuid, u64, DocumentState, EventId) {
    match &current.head.projection {
        buzz_project_document::DocumentHeadProjection::Active {
            document_id,
            document_revision,
            source_event_id,
            ..
        } => (
            *document_id,
            *document_revision,
            DocumentState::Active,
            *source_event_id,
        ),
        buzz_project_document::DocumentHeadProjection::Deleted {
            document_id,
            document_revision,
            source_event_id,
            ..
        } => (
            *document_id,
            *document_revision,
            DocumentState::Deleted,
            *source_event_id,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use buzz_project_document::{DocumentProjectionPlan, DocumentSnapshot};
    use buzz_sdk::project_document::{
        build_document_command, build_document_head_projection, build_document_head_reprojection,
        build_document_meta_projection, build_document_revision_projection,
        build_document_revision_reprojection, changed_head_for, document_revision_coordinate,
    };
    use nostr::Keys;
    use sqlx::PgPool;

    const TEST_DB_URL: &str = "postgres://buzz:buzz_dev@localhost:5432/buzz";

    struct ScratchDatabase {
        admin: PgPool,
        pool: PgPool,
        name: String,
    }

    impl ScratchDatabase {
        async fn create(prefix: &str) -> Self {
            let admin_url = std::env::var("TEST_DATABASE_URL")
                .or_else(|_| std::env::var("BUZZ_TEST_DATABASE_URL"))
                .unwrap_or_else(|_| TEST_DB_URL.to_owned());
            let admin = PgPool::connect(&admin_url)
                .await
                .expect("connect test database server");
            let name = format!("{prefix}_{}", Uuid::new_v4().simple());
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
                .execute(&admin)
                .await
                .expect("create Project Document scratch database");
            let slash = admin_url.rfind('/').expect("database URL has path");
            let database_url = format!("{}/{}", &admin_url[..slash], name);
            let pool = PgPool::connect(&database_url)
                .await
                .expect("connect Project Document scratch database");
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate Project Document scratch database");
            Self { admin, pool, name }
        }

        async fn cleanup(self) {
            self.pool.close().await;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.name
            )))
            .execute(&self.admin)
            .await
            .expect("drop Project Document scratch database");
            self.admin.close().await;
        }
    }

    async fn seed_community(pool: &PgPool, actor: &Keys) -> CommunityId {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query(
            "INSERT INTO communities (id, host, project_document_enabled) \
             VALUES ($1, $2, FALSE)",
        )
        .bind(community_id.as_uuid())
        .bind(format!("project-document-{}.test", community_id.as_uuid()))
        .execute(pool)
        .await
        .expect("seed Project Document Community");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) VALUES ($1, $2, 'member')",
        )
        .bind(community_id.as_uuid())
        .bind(actor.public_key().to_hex())
        .execute(pool)
        .await
        .expect("seed Project Document actor");
        community_id
    }

    async fn seed_managed_agent(
        pool: &PgPool,
        community_id: CommunityId,
        owner: &Keys,
        agent: &Keys,
    ) {
        sqlx::query(
            "INSERT INTO users (community_id, pubkey) VALUES ($1, $2) ON CONFLICT DO NOTHING",
        )
        .bind(community_id.as_uuid())
        .bind(owner.public_key().as_bytes())
        .execute(pool)
        .await
        .expect("seed managed Agent owner");
        sqlx::query(
            "INSERT INTO users (community_id, pubkey, agent_owner_pubkey) VALUES ($1, $2, $3)",
        )
        .bind(community_id.as_uuid())
        .bind(agent.public_key().as_bytes())
        .bind(owner.public_key().as_bytes())
        .execute(pool)
        .await
        .expect("seed managed Agent");
    }

    fn whole_second_now() -> DateTime<Utc> {
        DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("current timestamp")
    }

    async fn bootstrap(db: &Db, community_id: CommunityId, relay: &Keys) {
        let catalog = DocumentCatalog::empty(community_id, 1, whole_second_now())
            .expect("empty Document catalog");
        let plan = DocumentProjectionPlan::for_bootstrap(&catalog).expect("bootstrap plan");
        let meta_projection = build_document_meta_projection(&plan, &[])
            .expect("build bootstrap metadata")
            .sign_with_keys(relay)
            .expect("sign bootstrap metadata");
        db.bootstrap_empty_project_document_catalog(PreparedProjectDocumentBootstrap {
            catalog,
            meta_projection,
        })
        .await
        .expect("bootstrap empty Document catalog");
    }

    async fn enable_for_storage_test(pool: &PgPool, community_id: CommunityId) {
        sqlx::query("UPDATE communities SET project_document_enabled = TRUE WHERE id = $1")
            .bind(community_id.as_uuid())
            .execute(pool)
            .await
            .expect("enable only inside isolated storage test");
    }

    fn command_event(command: &ProjectDocumentCommand, actor: &Keys) -> Event {
        build_document_command(command.clone())
            .expect("build Document command")
            .sign_with_keys(actor)
            .expect("sign Document command")
    }

    fn prepare(
        context: &ProjectDocumentWriteContext,
        command: ProjectDocumentCommand,
        command_event: Event,
        relay: &Keys,
    ) -> ProjectDocumentWriteResult<PreparedProjectDocumentCommit> {
        let transition = reduce_document(
            &context.catalog,
            context.current.as_ref(),
            &command,
            DocumentChangeContext::new(
                command_event.pubkey,
                command_event.id,
                context.canonical_time,
            )
            .with_deletion_blocked(context.deletion_blocked),
        )?;
        let revision_projection = build_document_revision_projection(transition.projection_plan())
            .expect("build revision projection")
            .sign_with_keys(relay)
            .expect("sign revision projection");
        let head_projection =
            build_document_head_projection(transition.projection_plan(), &revision_projection)
                .expect("build head projection")
                .sign_with_keys(relay)
                .expect("sign head projection");
        let changed = changed_head_for(
            transition.projection_plan(),
            &head_projection,
            &revision_projection,
        )
        .expect("bind changed head");
        let meta_projection =
            build_document_meta_projection(transition.projection_plan(), &[changed])
                .expect("build metadata projection")
                .sign_with_keys(relay)
                .expect("sign metadata projection");
        Ok(PreparedProjectDocumentCommit {
            command_event,
            command,
            transition,
            revision_projection,
            head_projection,
            meta_projection,
        })
    }

    async fn prepare_for_commit(
        db: &Db,
        community_id: CommunityId,
        command: ProjectDocumentCommand,
        actor: &Keys,
        relay: &Keys,
    ) -> ProjectDocumentWriteResult<(ProjectDocumentWriteTx, PreparedProjectDocumentCommit)> {
        let event = command_event(&command, actor);
        let document_id = command.document_id();
        let mut tx = begin_storage_test_write(db, community_id, relay.public_key()).await?;
        let context = tx.load_current(document_id).await?;
        let prepared = prepare(&context, command, event, relay)?;
        Ok((tx, prepared))
    }

    async fn begin_storage_test_write(
        db: &Db,
        community_id: CommunityId,
        expected_projection_pubkey: PublicKey,
    ) -> ProjectDocumentWriteResult<ProjectDocumentWriteTx> {
        let mut tx = db.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let enabled: Option<bool> = sqlx::query_scalar(
            "SELECT project_document_enabled FROM communities \
             WHERE id = $1 AND archived_at IS NULL FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if enabled != Some(true) {
            return Err(ProjectDocumentWriteError::Unavailable { community_id });
        }
        Ok(ProjectDocumentWriteTx {
            tx,
            community_id,
            expected_projection_pubkey,
            loaded: None,
        })
    }

    fn create_command(document_id: Uuid) -> ProjectDocumentCommand {
        ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id,
                title: "Runbook".to_owned(),
                summary: Some("First full snapshot".to_owned()),
                content_markdown: "# Runbook\n\nInitial".to_owned(),
            },
        )
    }

    fn update_command(document_id: Uuid, expected: u64, suffix: &str) -> ProjectDocumentCommand {
        ProjectDocumentCommand::new(
            expected,
            DocumentCommandRequest::Update {
                document_id,
                title: format!("Runbook {suffix}"),
                summary: Some(format!("Snapshot {suffix}")),
                content_markdown: format!("# Runbook\n\n{suffix}"),
            },
        )
    }

    fn test_reproject_head(
        context: &ProjectDocumentReprojectContext,
        source: &ProjectDocumentReprojectRevision,
        revision_event_id: EventId,
    ) -> DocumentHeadProjection {
        let revision_coordinate = document_revision_coordinate(
            context.community_id,
            source.document_id,
            source.document_revision,
        );
        match &source.revision {
            DocumentRevision::Active {
                snapshot,
                actor,
                canonical_at,
                ..
            } => DocumentHeadProjection::Active {
                schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
                projection_type: buzz_project_document::DocumentProjectionType::DocumentHead,
                project_id: *context.community_id.as_uuid(),
                projection_generation: context.target_generation,
                catalog_revision: source.catalog_revision,
                document_id: source.document_id,
                document_revision: source.document_revision,
                title: snapshot.title.clone(),
                summary: snapshot.summary.clone(),
                created_at: source.created.at,
                created_by: source.created.by,
                updated_at: *canonical_at,
                updated_by: *actor,
                revision_coordinate,
                revision_event_id,
                source_event_id: source.source_event_id,
            },
            DocumentRevision::Deleted {
                actor,
                canonical_at,
                ..
            } => DocumentHeadProjection::Deleted {
                schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
                projection_type: buzz_project_document::DocumentProjectionType::DocumentHead,
                project_id: *context.community_id.as_uuid(),
                projection_generation: context.target_generation,
                catalog_revision: source.catalog_revision,
                document_id: source.document_id,
                document_revision: source.document_revision,
                created_at: source.created.at,
                created_by: source.created.by,
                deleted_at: *canonical_at,
                deleted_by: *actor,
                revision_coordinate,
                revision_event_id,
                source_event_id: source.source_event_id,
            },
        }
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn managed_community_writer_does_not_require_assignment_or_runtime() {
        let scratch = ScratchDatabase::create("buzz_pd_managed_community_writer").await;
        let db = Db::from_pool(scratch.pool.clone());
        let owner = Keys::generate();
        let agent = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &owner).await;
        seed_managed_agent(&scratch.pool, community_id, &owner, &agent).await;
        bootstrap(&db, community_id, &relay).await;
        enable_for_storage_test(&scratch.pool, community_id).await;

        let document_id = Uuid::new_v4();
        let create = create_command(document_id);
        assert_eq!(create.acting_assignment_id, None);
        assert_eq!(create.runtime_fence, None);
        let create_event = command_event(&create, &agent);
        let mut write = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin ordinary managed Document create");
        assert_eq!(
            write
                .prepare_command(&create_event, &create)
                .await
                .expect("authorize ordinary managed Document create"),
            ProjectDocumentPrepareOutcome::New
        );
        let context = write
            .load_current(document_id)
            .await
            .expect("load ordinary managed Document target");
        let prepared =
            prepare(&context, create, create_event, &relay).expect("prepare managed projections");
        let committed = write
            .commit(prepared)
            .await
            .expect("commit ordinary managed Document create");
        assert_eq!(committed.receipt.actor, agent.public_key());
        assert_eq!(committed.receipt.acting_assignment_id, None);
        assert_eq!(committed.receipt.document_revision, 1);

        let explicit_assignment = Uuid::new_v4();
        let explicit_runtime = buzz_core::RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 1,
        };
        let claimed = update_command(document_id, 1, "claimed")
            .with_runtime_fence(explicit_assignment, explicit_runtime);
        let claimed_event = command_event(&claimed, &agent);
        let mut claimed_write = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin explicit managed attribution check");
        assert!(matches!(
            claimed_write
                .prepare_command(&claimed_event, &claimed)
                .await,
            Err(ProjectDocumentWriteError::ActingAssignmentInvalid)
        ));
        claimed_write
            .rollback()
            .await
            .expect("release explicit managed attribution check");

        let human_claim = update_command(document_id, 1, "human claim")
            .with_runtime_fence(explicit_assignment, explicit_runtime);
        let human_claim_event = command_event(&human_claim, &owner);
        let mut human_claim_write = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin Human attribution check");
        assert!(matches!(
            human_claim_write
                .prepare_command(&human_claim_event, &human_claim)
                .await,
            Err(ProjectDocumentWriteError::ActingAssignmentInvalid)
        ));
        human_claim_write
            .rollback()
            .await
            .expect("release Human attribution check");

        sqlx::query("DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2")
            .bind(community_id.as_uuid())
            .bind(owner.public_key().to_hex())
            .execute(&scratch.pool)
            .await
            .expect("remove managed Agent owner from Community");
        let update = update_command(document_id, 1, "after owner removal");
        let (write, prepared) = prepare_for_commit(&db, community_id, update, &agent, &relay)
            .await
            .expect("prepare update after owner removal");
        assert!(matches!(
            write.commit(prepared).await,
            Err(ProjectDocumentWriteError::NotAuthorized)
        ));

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn inactive_generation_reproject_rotates_all_history_atomically() {
        let scratch = ScratchDatabase::create("buzz_pd_reproject").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let old_relay = Keys::generate();
        let new_relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap(&db, community_id, &old_relay).await;
        enable_for_storage_test(&scratch.pool, community_id).await;

        let document_id = Uuid::new_v4();
        let (write, create) = prepare_for_commit(
            &db,
            community_id,
            create_command(document_id),
            &actor,
            &old_relay,
        )
        .await
        .expect("prepare create");
        write.commit(create).await.expect("commit create");
        let (write, update) = prepare_for_commit(
            &db,
            community_id,
            update_command(document_id, 1, "rotated"),
            &actor,
            &old_relay,
        )
        .await
        .expect("prepare update");
        write.commit(update).await.expect("commit update");
        db.set_project_document_enabled_checked(community_id, false, None)
            .await
            .expect("disable before rotation");
        let historical_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT projection_event_id FROM project_document_revisions \
             WHERE community_id = $1 AND document_id = $2 AND document_revision = 1",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("load historical projection pointer");
        sqlx::query(
            "UPDATE events SET deleted_at = clock_timestamp() \
             WHERE community_id = $1 AND id = $2 AND deleted_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(historical_event_id)
        .execute(&scratch.pool)
        .await
        .expect("simulate a repairable historical projection loss");
        assert!(
            !db.project_document_preflight(community_id, &old_relay.public_key())
                .await
                .expect("detect historical projection mismatch")
                .projection_parity
        );

        // This storage-kernel test isolates Document reprojection from the
        // already-covered Project View v2 cutover. Bypass cross-domain fixture
        // triggers only in this disposable database after Document writes end.
        let mut fixture_tx = scratch
            .pool
            .begin()
            .await
            .expect("begin fixture schema switch");
        sqlx::query("SET LOCAL session_replication_role = 'replica'")
            .execute(&mut *fixture_tx)
            .await
            .expect("suspend fixture triggers");
        sqlx::query("UPDATE communities SET project_view_schema_version = 2 WHERE id = $1")
            .bind(community_id.as_uuid())
            .execute(&mut *fixture_tx)
            .await
            .expect("use supported Project View schema");
        fixture_tx
            .commit()
            .await
            .expect("commit fixture schema switch");

        let context = db
            .begin_project_document_reproject(community_id, new_relay.public_key())
            .await
            .expect("begin inactive generation");
        assert_eq!(context.source_generation, 1);
        assert_eq!(context.target_generation, 2);
        assert_eq!(context.revision_count, 2);
        let revisions = db
            .project_document_reproject_revision_page(&context, 0, 100)
            .await
            .expect("load immutable history");
        let mut staged = Vec::new();
        for source in &revisions {
            let projection = expected_reproject_revision_projection(
                community_id,
                context.target_generation,
                source,
            );
            let revision_event = build_document_revision_reprojection(&projection)
                .expect("build replacement revision")
                .sign_with_keys(&new_relay)
                .expect("sign replacement revision");
            staged.push(PreparedProjectDocumentReprojectEvent {
                projection_type: ProjectDocumentReprojectEventType::Revision,
                document_id: Some(source.document_id),
                document_revision: Some(source.document_revision),
                event: revision_event.clone(),
            });
            if source.is_current {
                let head = test_reproject_head(&context, source, revision_event.id);
                staged.push(PreparedProjectDocumentReprojectEvent {
                    projection_type: ProjectDocumentReprojectEventType::Head,
                    document_id: Some(source.document_id),
                    document_revision: Some(source.document_revision),
                    event: build_document_head_reprojection(&head, &revision_event)
                        .expect("build replacement head")
                        .sign_with_keys(&new_relay)
                        .expect("sign replacement head"),
                });
            }
        }
        db.stage_project_document_reproject_events(&context, &staged)
            .await
            .expect("stage revision and head events");
        let catalog = DocumentCatalog::from_snapshot(
            community_id,
            context.catalog_revision,
            context.active_document_count,
            context.target_generation,
            context.initialized_at,
            context.updated_at,
        )
        .expect("reconstruct catalog");
        let meta_plan =
            DocumentProjectionPlan::for_reprojection(&catalog).expect("reset projection plan");
        let meta = build_document_meta_projection(&meta_plan, &[])
            .expect("build reset metadata")
            .sign_with_keys(&new_relay)
            .expect("sign reset metadata");
        db.stage_project_document_reproject_events(
            &context,
            &[PreparedProjectDocumentReprojectEvent {
                projection_type: ProjectDocumentReprojectEventType::Meta,
                document_id: None,
                document_revision: None,
                event: meta,
            }],
        )
        .await
        .expect("stage reset metadata");
        db.ready_project_document_reproject(&context)
            .await
            .expect("close inactive generation");
        db.activate_project_document_reproject(&context)
            .await
            .expect("activate target generation");

        let report = db
            .project_document_preflight(community_id, &new_relay.public_key())
            .await
            .expect("verify target generation");
        assert!(report.signer_matches);
        assert!(report.projection_parity);
        let old_report = db
            .project_document_preflight(community_id, &old_relay.public_key())
            .await
            .expect("old signer fails closed");
        assert!(!old_report.signer_matches);
        let canonical: (i64, String) = sqlx::query_as(
            "SELECT current_revision, state FROM project_documents \
             WHERE community_id = $1 AND document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read unchanged canonical Document");
        assert_eq!(canonical, (2, "active".to_owned()));
        let live_by_signer: Vec<(Vec<u8>, i64)> = sqlx::query_as(
            "SELECT pubkey, count(*)::bigint FROM events \
             WHERE community_id = $1 AND kind IN ($2, $3, $4) AND deleted_at IS NULL \
             GROUP BY pubkey",
        )
        .bind(community_id.as_uuid())
        .bind(KIND_PROJECT_DOCUMENT_HEAD as i32)
        .bind(KIND_PROJECT_DOCUMENT_REVISION as i32)
        .bind(KIND_PROJECT_DOCUMENT_META as i32)
        .fetch_all(&scratch.pool)
        .await
        .expect("count active generation events");
        assert_eq!(live_by_signer.len(), 1);
        assert_eq!(live_by_signer[0].0, new_relay.public_key().to_bytes());
        assert_eq!(live_by_signer[0].1, 4);
        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn bootstrap_atomic_lifecycle_replay_and_append_only_history() {
        let scratch = ScratchDatabase::create("buzz_pd_lifecycle").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;

        let mut fence_policy_tx = scratch
            .pool
            .begin()
            .await
            .expect("begin fence policy check");
        let unbound_assignment = Uuid::new_v4();
        crate::project_runtime::validate_runtime_command_fence_in_tx(
            &mut fence_policy_tx,
            community_id,
            Some(unbound_assignment),
            None,
            crate::project_runtime::RuntimeCommandFencePolicy::LegacyOptionalSupervision,
        )
        .await
        .expect("legacy v2 keeps an unbound Assignment optional");
        assert!(matches!(
            crate::project_runtime::validate_runtime_command_fence_in_tx(
                &mut fence_policy_tx,
                community_id,
                Some(unbound_assignment),
                None,
                crate::project_runtime::RuntimeCommandFencePolicy::RequireSupervisedRuntime,
            )
            .await,
            Err(crate::project_runtime::RuntimeSupervisionError::CommandFence)
        ));
        fence_policy_tx
            .rollback()
            .await
            .expect("release fence policy check");

        let default_enabled: bool =
            sqlx::query_scalar("SELECT project_document_enabled FROM communities WHERE id = $1")
                .bind(community_id.as_uuid())
                .fetch_one(&scratch.pool)
                .await
                .expect("read flag default");
        assert!(!default_enabled);
        bootstrap(&db, community_id, &relay).await;
        let preflight = db
            .project_document_preflight(community_id, &relay.public_key())
            .await
            .expect("run Document preflight");
        assert!(preflight.schema_ready);
        assert!(preflight.bootstrapped);
        assert!(preflight.signer_matches);
        assert!(preflight.projection_parity);
        assert!(!preflight.project_view_schema_ready);
        assert!(!preflight.ready);
        let wrong_signer = Keys::generate();
        let wrong_signer_preflight = db
            .project_document_preflight(community_id, &wrong_signer.public_key())
            .await
            .expect("run wrong-signer Document preflight");
        assert!(!wrong_signer_preflight.signer_matches);
        assert!(!wrong_signer_preflight.projection_parity);
        assert!(!wrong_signer_preflight.ready);
        assert!(matches!(
            db.begin_project_document_write(community_id, relay.public_key())
                .await,
            Err(ProjectDocumentWriteError::Unavailable { .. })
        ));
        enable_for_storage_test(&scratch.pool, community_id).await;

        let document_id = Uuid::new_v4();
        let create = create_command(document_id);
        let (tx, prepared_create) = prepare_for_commit(&db, community_id, create, &actor, &relay)
            .await
            .expect("prepare create");
        let created = tx
            .commit(prepared_create.clone())
            .await
            .expect("commit create");
        assert!(!created.replayed);
        assert_eq!(created.receipt.document_revision, 1);

        let mut wrong_signer_tx =
            begin_storage_test_write(&db, community_id, wrong_signer.public_key())
                .await
                .expect("begin wrong-signer replay attempt");
        assert!(matches!(
            wrong_signer_tx.load_current(document_id).await,
            Err(ProjectDocumentWriteError::Unavailable { .. })
        ));
        wrong_signer_tx
            .rollback()
            .await
            .expect("rollback wrong-signer replay attempt");

        let mut replay_tx = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin replay");
        replay_tx
            .load_current(document_id)
            .await
            .expect("load current for replay");
        let replay = replay_tx
            .commit(prepared_create.clone())
            .await
            .expect("replay accepted create");
        assert!(replay.replayed);
        assert_eq!(replay.receipt, created.receipt);

        sqlx::query("DELETE FROM relay_members WHERE community_id = $1 AND pubkey = $2")
            .bind(community_id.as_uuid())
            .bind(actor.public_key().to_hex())
            .execute(&scratch.pool)
            .await
            .expect("revoke actor before receipt retry");
        let mut revoked_replay_tx = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin revoked replay");
        revoked_replay_tx
            .load_current(document_id)
            .await
            .expect("load current before revoked replay");
        assert!(matches!(
            revoked_replay_tx.commit(prepared_create.clone()).await,
            Err(ProjectDocumentWriteError::NotAuthorized)
        ));
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) VALUES ($1, $2, 'member')",
        )
        .bind(community_id.as_uuid())
        .bind(actor.public_key().to_hex())
        .execute(&scratch.pool)
        .await
        .expect("restore actor for remaining lifecycle assertions");

        sqlx::query(
            "INSERT INTO community_bans \
                (community_id, pubkey, muted_until, mute_reason, actor_pubkey) \
             VALUES ($1, $2, clock_timestamp() + interval '1 hour', 'test timeout', $3)",
        )
        .bind(community_id.as_uuid())
        .bind(actor.public_key().as_bytes())
        .bind(relay.public_key().as_bytes())
        .execute(&scratch.pool)
        .await
        .expect("time out actor before receipt retry");
        let mut timed_out_replay_tx =
            begin_storage_test_write(&db, community_id, relay.public_key())
                .await
                .expect("begin timed-out replay");
        timed_out_replay_tx
            .load_current(document_id)
            .await
            .expect("load current before timed-out replay");
        assert!(matches!(
            timed_out_replay_tx.commit(prepared_create).await,
            Err(ProjectDocumentWriteError::NotAuthorized)
        ));
        sqlx::query("DELETE FROM community_bans WHERE community_id = $1 AND pubkey = $2")
            .bind(community_id.as_uuid())
            .bind(actor.public_key().as_bytes())
            .execute(&scratch.pool)
            .await
            .expect("clear actor timeout for remaining lifecycle assertions");

        let update = update_command(document_id, 1, "second");
        let collision_command = command_event(&update, &actor);

        // Force a failure after old head/meta retirement has occurred inside
        // the transaction. The pre-existing command makes event insertion fail;
        // rollback must restore both old live pointers and all canonical rows.
        crate::event::insert_event(&scratch.pool, community_id, &collision_command, None)
            .await
            .expect("preinsert collision command");
        let collision_event_id = collision_command.id;
        let old_head: Vec<u8> = sqlx::query_scalar(
            "SELECT current_head_event_id FROM project_documents \
             WHERE community_id = $1 AND document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read old head");
        let mut tx = begin_storage_test_write(&db, community_id, relay.public_key())
            .await
            .expect("begin colliding update");
        let context = tx
            .load_current(document_id)
            .await
            .expect("load colliding update target");
        let prepared_update =
            prepare(&context, update, collision_command, &relay).expect("prepare colliding update");
        assert!(tx.commit(prepared_update).await.is_err());
        let head_still_live: bool = sqlx::query_scalar(
            "SELECT deleted_at IS NULL FROM events WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&old_head)
        .fetch_one(&scratch.pool)
        .await
        .expect("old head survives rollback");
        assert!(head_still_live);
        let revision_after_rollback: i64 = sqlx::query_scalar(
            "SELECT current_revision FROM project_documents \
             WHERE community_id = $1 AND document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read revision after rollback");
        assert_eq!(revision_after_rollback, 1);
        // Remove the deliberately orphaned collision event by its own ID.
        sqlx::query("DELETE FROM events WHERE community_id = $1 AND id = $2")
            .bind(community_id.as_uuid())
            .bind(collision_event_id.as_bytes().as_slice())
            .execute(&scratch.pool)
            .await
            .expect("remove collision command");

        let update = update_command(document_id, 1, "second");
        let (tx, prepared_update) = prepare_for_commit(&db, community_id, update, &actor, &relay)
            .await
            .expect("prepare retry update");
        tx.commit(prepared_update).await.expect("commit update");

        let delete = ProjectDocumentCommand::new(2, DocumentCommandRequest::Delete { document_id });
        let (tx, prepared_delete) = prepare_for_commit(&db, community_id, delete, &actor, &relay)
            .await
            .expect("prepare delete");
        tx.commit(prepared_delete).await.expect("commit delete");

        let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT document_revision, state, content_markdown \
             FROM project_document_revisions \
             WHERE community_id = $1 AND document_id = $2 ORDER BY document_revision",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_all(&scratch.pool)
        .await
        .expect("read immutable history");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].2.as_deref(), Some("# Runbook\n\nInitial"));
        assert_eq!(rows[1].2.as_deref(), Some("# Runbook\n\nsecond"));
        assert_eq!(rows[2], (3, "deleted".to_owned(), None));
        assert!(sqlx::query(
            "DELETE FROM project_document_revisions \
                 WHERE community_id = $1 AND document_id = $2 AND document_revision = 1"
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .execute(&scratch.pool)
        .await
        .is_err());
        assert!(sqlx::query(
            "UPDATE project_document_revisions SET content_markdown = 'rewrite' \
                 WHERE community_id = $1 AND document_id = $2 AND document_revision = 1"
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .execute(&scratch.pool)
        .await
        .is_err());

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn concurrent_same_document_updates_only_commit_once() {
        let scratch = ScratchDatabase::create("buzz_pd_race").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap(&db, community_id, &relay).await;
        enable_for_storage_test(&scratch.pool, community_id).await;
        let document_id = Uuid::new_v4();
        let (tx, create) = prepare_for_commit(
            &db,
            community_id,
            create_command(document_id),
            &actor,
            &relay,
        )
        .await
        .expect("prepare race fixture");
        tx.commit(create).await.expect("commit race fixture");

        let first_db = db.clone();
        let second_db = db.clone();
        let first_actor = actor.clone();
        let second_actor = actor.clone();
        let first_relay = relay.clone();
        let second_relay = relay.clone();
        let first = async move {
            let (tx, prepared) = prepare_for_commit(
                &first_db,
                community_id,
                update_command(document_id, 1, "racer-a"),
                &first_actor,
                &first_relay,
            )
            .await?;
            tx.commit(prepared).await
        };
        let second = async move {
            let (tx, prepared) = prepare_for_commit(
                &second_db,
                community_id,
                update_command(document_id, 1, "racer-b"),
                &second_actor,
                &second_relay,
            )
            .await?;
            tx.commit(prepared).await
        };
        let (first, second) = tokio::join!(first, second);
        assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
        assert!(first.is_err() || second.is_err());
        let current_revision: i64 = sqlx::query_scalar(
            "SELECT current_revision FROM project_documents \
             WHERE community_id = $1 AND document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read winning revision");
        assert_eq!(current_revision, 2);

        scratch.cleanup().await;
    }

    #[test]
    fn active_snapshot_test_fixture_is_closed() {
        let snapshot = DocumentSnapshot {
            title: "Title".to_owned(),
            summary: None,
            content_markdown: "Body".to_owned(),
        };
        snapshot.validate().expect("valid closed snapshot");
    }
}
