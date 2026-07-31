//! Project Document canonical state, immutable history, and restricted writes.
//!
//! Stage 1 deliberately exposes no Relay routing. The only mutation seam is a
//! caller-owned transaction that holds the shared Community advisory lock,
//! re-derives the pure transition, verifies the complete signed projection
//! bundle, and commits command/event/history/pointers atomically.

use buzz_core::kind::{
    KIND_PROJECT_DOCUMENT_COMMAND, KIND_PROJECT_DOCUMENT_HEAD, KIND_PROJECT_DOCUMENT_META,
    KIND_PROJECT_DOCUMENT_REVISION,
};
use buzz_core::{CommunityId, EventId, PublicKey};
use buzz_project_document::{
    reduce_document, CurrentDocument, DocumentAttribution, DocumentCatalog, DocumentChangeContext,
    DocumentCommandRequest, DocumentError, DocumentRevision, DocumentState, DocumentTransition,
    ProjectDocument, ProjectDocumentCommand, ProjectDocumentReceipt, MAX_SAFE_REVISION,
};
use buzz_sdk::project_document::{
    parse_document_command, parse_document_head, parse_document_meta, parse_document_revision,
    verify_document_projection_bundle, VerifiedCurrentDocument,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
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
    /// Actor membership, Assignment, or Runtime authority changed.
    #[error("Project Document actor is no longer authorized")]
    Unauthorized,
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
    /// Whether a locked incoming Live/Guide reference blocks deletion.
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
    /// Capability flag. Stage 1 has no API that can turn this on.
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
                AND to_regclass('idx_project_documents_active') IS NOT NULL \
                AND to_regclass('idx_project_document_revisions_history') IS NOT NULL",
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
                    count(r.document_revision)::bigint AS revision_count \
             FROM communities c \
             LEFT JOIN project_document_state s ON s.community_id = c.id \
             LEFT JOIN project_document_revisions r ON r.community_id = c.id \
             GROUP BY c.id, c.host, c.archived_at, c.project_document_enabled, \
                      c.project_view_schema_version, s.catalog_revision, \
                      s.active_document_count, s.projection_generation, s.projection_pubkey \
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
                    count(r.document_revision)::bigint AS revision_count \
             FROM communities c \
             LEFT JOIN project_document_state s ON s.community_id = c.id \
             LEFT JOIN project_document_revisions r ON r.community_id = c.id \
             WHERE c.id = $1 \
             GROUP BY c.id, c.host, c.archived_at, c.project_document_enabled, \
                      c.project_view_schema_version, s.catalog_revision, \
                      s.active_document_count, s.projection_generation, s.projection_pubkey",
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
            document_projection_parity(
                &self.pool,
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

    /// Commit a signed revision-zero reset catalog while the capability is off.
    ///
    /// No admin command invokes this in stage 1; it exists for isolated DB
    /// builders/tests and the later controlled enable workflow.
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
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
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

impl ProjectDocumentWriteTx {
    /// Explicitly roll back and release the Community lock.
    pub async fn rollback(self) -> ProjectDocumentWriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
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

        // Resource/Context tables do not exist until later stages. Holding the
        // Community exclusive lock now freezes the deletion-proof seam without
        // pretending those future reference sources are implemented.
        let deletion_blocked = false;
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

async fn validate_actor_in_tx(
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
        return Err(ProjectDocumentWriteError::Unauthorized);
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
            return Err(ProjectDocumentWriteError::Unauthorized);
        }
        if acting_assignment_id.is_none() {
            return Err(ProjectDocumentWriteError::Unauthorized);
        }
    } else {
        if !direct_member {
            return Err(ProjectDocumentWriteError::Unauthorized);
        }
        // Human Document commands never borrow Role/Runtime authority. This
        // keeps attribution unambiguous and prevents a stale optional v2 fence
        // from changing the meaning of a direct-member write.
        if acting_assignment_id.is_some() || runtime_fence.is_some() {
            return Err(ProjectDocumentWriteError::Unauthorized);
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
            return Err(ProjectDocumentWriteError::Unauthorized);
        }
    }
    if managed {
        crate::project_runtime::validate_runtime_command_fence_in_tx(
            tx,
            community_id,
            acting_assignment_id,
            runtime_fence,
            crate::project_runtime::RuntimeCommandFencePolicy::RequireSupervisedRuntime,
        )
        .await
        .map_err(|_| ProjectDocumentWriteError::Unauthorized)?;
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

async fn document_projection_parity(
    pool: &sqlx::PgPool,
    community_id: CommunityId,
    expected_pubkey: &PublicKey,
    meta_event_id: Option<&[u8]>,
    active_count: Option<i64>,
) -> crate::Result<bool> {
    let (Some(meta_event_id), Some(active_count)) = (meta_event_id, active_count) else {
        return Ok(false);
    };
    if meta_event_id.len() != 32 || active_count < 0 {
        return Ok(false);
    }
    let state = sqlx::query(
        "SELECT catalog_revision, projection_generation, initialized_at, updated_at \
         FROM project_document_state WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(pool)
    .await?;
    let Some(state) = state else {
        return Ok(false);
    };
    let catalog_revision: i64 = state.try_get("catalog_revision")?;
    let generation: i64 = state.try_get("projection_generation")?;
    let updated_at: DateTime<Utc> = state.try_get("updated_at")?;
    let meta = crate::event::get_event_by_id(pool, community_id, meta_event_id).await?;
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
    .fetch_one(pool)
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
    .fetch_all(pool)
    .await?;
    for row in rows {
        let document_id: Uuid = row.try_get("document_id")?;
        let document_revision: i64 = row.try_get("current_revision")?;
        let state: String = row.try_get("state")?;
        let source: Vec<u8> = row.try_get("current_source_change_id")?;
        let head_id: Vec<u8> = row.try_get("current_head_event_id")?;
        let revision_id: Vec<u8> = row.try_get("current_revision_event_id")?;
        let head = crate::event::get_event_by_id(pool, community_id, &head_id).await?;
        let revision = crate::event::get_event_by_id(pool, community_id, &revision_id).await?;
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
    Ok(true)
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
        build_document_command, build_document_head_projection, build_document_meta_projection,
        build_document_revision_projection, changed_head_for,
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
            Err(ProjectDocumentWriteError::Unauthorized)
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
            Err(ProjectDocumentWriteError::Unauthorized)
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
