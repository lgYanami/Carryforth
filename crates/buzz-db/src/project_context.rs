//! Project Context Edge canonical state, durable receipts, and restricted writes.
//!
//! The storage coordinator holds the shared Community/Project advisory lock,
//! reconstructs the pure reducer basis, validates Relay-signed projections,
//! and commits command/event/canonical rows atomically. Signing and fan-out
//! remain Relay responsibilities.

use std::collections::{BTreeMap, BTreeSet};

use buzz_audit::{AuditAction, NewAuditEntry};
use buzz_core::kind::{
    KIND_PROJECT_CONTEXT_COMMAND, KIND_PROJECT_CONTEXT_EDGE_BINDING, KIND_PROJECT_CONTEXT_META,
};
use buzz_core::{CommunityId, EventId, PublicKey, StoredEvent};
use buzz_project_context::{
    reduce_project_context, EdgeKey, ProjectContextBindingProjection, ProjectContextBindingState,
    ProjectContextCatalog, ProjectContextCommand, ProjectContextCoordinate, ProjectContextEdge,
    ProjectContextError, ProjectContextOperation, ProjectContextReceipt, ProjectContextTransition,
    MAX_SAFE_REVISION, PROJECT_CONTEXT_SCHEMA_VERSION,
};
use buzz_project_view::ProjectViewObjectType;
use buzz_sdk::project_context::{
    parse_project_context_binding, parse_project_context_command, parse_project_context_meta,
    verify_project_context_binding_observation, verify_project_context_projection_bundle,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde_json::Value;
use sqlx::{Postgres, QueryBuilder, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

/// Failures from the restricted Project Context storage coordinator.
#[derive(Debug, thiserror::Error)]
pub enum ProjectContextWriteError {
    /// Database abstraction failure.
    #[error(transparent)]
    Database(#[from] DbError),
    /// Direct SQL failure.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Tamper-evident operator-control audit append failed.
    #[error(transparent)]
    Audit(#[from] buzz_audit::AuditError),
    /// The pure Project Context kernel rejected the transition.
    #[error(transparent)]
    Domain(#[from] ProjectContextError),
    /// Community prerequisites or the configured projection signer are unavailable.
    #[error("Project Context is unavailable for community {community_id}")]
    Unavailable {
        /// Host-derived Community/Project identity.
        community_id: CommunityId,
    },
    /// The command signer or its managed owner is no longer authorized.
    #[error("Project Context actor is no longer authorized by the Community")]
    NotAuthorized,
    /// An explicitly claimed Assignment is no longer active for the signer.
    #[error("Project Context acting Assignment is no longer valid")]
    ActingAssignmentInvalid,
    /// An explicitly claimed managed Runtime fence is missing or stale.
    #[error("Project Context Runtime fence is missing or stale")]
    RuntimeFence,
    /// A prepared command/projection bundle does not match the locked basis.
    #[error("invalid prepared Project Context commit: {0}")]
    InvalidCommit(String),
}

/// Convenient Project Context write result.
pub type ProjectContextWriteResult<T> = Result<T, ProjectContextWriteError>;

/// Durable Project-scoped catalog metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextStateMetadata {
    /// Current global Context revision.
    pub context_revision: u64,
    /// Number of active coordinate-set edges.
    pub active_edge_count: u64,
    /// Number of active Context Document bindings.
    pub bound_document_count: u64,
    /// Latest accepted member command, absent at empty bootstrap.
    pub last_change_id: Option<EventId>,
    /// Latest command signer, absent at empty bootstrap.
    pub last_actor_pubkey: Option<PublicKey>,
    /// Stable signer for this projection generation.
    pub projection_pubkey: PublicKey,
    /// Positive projection generation.
    pub projection_generation: u64,
    /// Current Relay-signed metadata event.
    pub meta_projection_event_id: EventId,
    /// Canonical catalog initialization time.
    pub initialized_at: DateTime<Utc>,
    /// Canonical current observation time.
    pub updated_at: DateTime<Utc>,
}

/// Locked canonical basis and transaction-derived liveness facts.
#[derive(Debug, Clone)]
pub struct ProjectContextWriteContext {
    /// Current Project-scoped catalog.
    pub catalog: ProjectContextCatalog,
    /// Active exact edge, absent for an unused or currently deleted exact set.
    pub current_edge: Option<ProjectContextEdge>,
    /// Active edge currently owning the Context Document, if any.
    pub active_document_edge: Option<EdgeKey>,
    /// Monotonic PostgreSQL time used by the pure reducer.
    pub canonical_time: DateTime<Utc>,
    /// Whether every coordinate is active at this transition boundary.
    pub all_coordinates_active: bool,
    /// Whether the Context Document is active at this transition boundary.
    pub context_document_active: bool,
}

/// Complete signed inputs to one atomic Project Context business commit.
#[derive(Debug, Clone)]
pub struct PreparedProjectContextCommit {
    /// Accepted member command event.
    pub command_event: Event,
    /// Strict command parsed before transaction entry.
    pub command: ProjectContextCommand,
    /// Pure transition derived from the locked write context.
    pub transition: ProjectContextTransition,
    /// Relay-signed current binding projection.
    pub binding_projection: Event,
    /// Relay-signed incremental catalog metadata projection.
    pub meta_projection: Event,
}

/// Result of a new commit or exact accepted-event replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextCommitOutcome {
    /// Stable business receipt without projection event identifiers.
    pub receipt: ProjectContextReceipt,
    /// Whether no canonical state changed because the command was replayed.
    pub replayed: bool,
}

/// Security-first command preparation result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectContextPrepareOutcome {
    /// The exact signed command has already been accepted.
    Replayed(ProjectContextReceipt),
    /// No durable receipt exists and the caller may load the reducer basis.
    New,
}

/// Signed revision-zero empty catalog prepared for explicit bootstrap.
#[derive(Debug, Clone)]
pub struct PreparedProjectContextBootstrap {
    /// Pure empty generation-one catalog.
    pub catalog: ProjectContextCatalog,
    /// Relay-signed reset metadata projection.
    pub meta_projection: Event,
}

/// Empty-catalog bootstrap result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectContextBootstrapOutcome {
    /// Whether the exact initialized state already existed.
    pub replayed: bool,
}

/// Operator-facing storage status for one Community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextFeatureStatus {
    /// Community identity.
    pub community_id: CommunityId,
    /// Normalized host.
    pub host: String,
    /// Whether the Community is archived.
    pub archived: bool,
    /// Context Edge capability flag.
    pub enabled: bool,
    /// Project View schema major.
    pub project_view_schema_version: i16,
    /// Project View capability flag.
    pub project_view_enabled: bool,
    /// Project Document capability flag.
    pub project_document_enabled: bool,
    /// Durable Project View maintenance state.
    pub maintenance_state: String,
    /// Current Context revision when initialized.
    pub context_revision: Option<u64>,
    /// Canonical active Edge count when initialized.
    pub active_edge_count: Option<u64>,
    /// Canonical active binding count when initialized.
    pub bound_document_count: Option<u64>,
    /// Projection generation when initialized.
    pub projection_generation: Option<u64>,
    /// Stored projection signer when initialized.
    pub projection_pubkey: Option<PublicKey>,
    /// Durable edge identity row count, including deleted rows.
    pub edge_row_count: u64,
    /// Durable binding transport row count, including deleted rows.
    pub binding_row_count: u64,
    /// Accepted change count.
    pub change_count: u64,
}

/// Read-only readiness result used by bootstrap and future operator surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextPreflight {
    /// Community identity.
    pub community_id: CommunityId,
    /// Whether all migration 0049 objects exist.
    pub schema_ready: bool,
    /// Whether Project View v3 is enabled, structurally valid, and unfrozen.
    pub project_view_ready: bool,
    /// Whether Project Document is enabled and structurally valid.
    pub project_document_ready: bool,
    /// Whether the Context catalog has been initialized.
    pub initialized: bool,
    /// Whether the stored Context signer matches the expected Relay.
    pub signer_matches: bool,
    /// Whether canonical rows and signed Context projections agree.
    pub projection_parity: bool,
    /// Whether every canonical projection pointer resolves and no live orphan remains.
    pub integrity_ready: bool,
    /// Whether the Context Edge capability flag is on.
    pub enabled: bool,
    /// Whether all structural read prerequisites pass.
    pub structural_read_ready: bool,
    /// Whether the capability may currently be advertised.
    pub advertised_ready: bool,
}

/// Indexed diagnostics for current Project Context projections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectContextIntegrityStatus {
    /// Live current-generation projections not named by canonical pointers.
    pub orphan_projection_count: u64,
    /// Canonical pointers whose event envelope is missing or mismatched.
    pub pointer_mismatch_count: u64,
}

/// Locked canonical basis for one disabled-only signer reprojection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextReprojectContext {
    /// Catalog observation rebuilt at the next projection generation.
    pub catalog: ProjectContextCatalog,
    /// Projection generation visible before this maintenance operation.
    pub source_generation: u64,
    /// Projection signer visible before this maintenance operation.
    pub source_pubkey: PublicKey,
    /// Exact current active and deleted binding heads rebuilt for the target generation.
    pub bindings: Vec<ProjectContextBindingProjection>,
}

/// Fully signed replacement generation prepared by operator tooling.
#[derive(Debug, Clone)]
pub struct PreparedProjectContextReprojection {
    /// One new Relay-signed head for every durable binding identity.
    pub binding_projections: Vec<Event>,
    /// New Relay-signed reset metadata head.
    pub meta_projection: Event,
}

/// Result of one atomically committed Project Context reprojection.
#[derive(Debug, Clone)]
pub struct ProjectContextReprojectOutcome {
    /// New binding heads followed by the reset metadata head.
    pub events: Vec<Event>,
    /// Projection generation replaced by this operation.
    pub source_generation: u64,
    /// Newly committed projection generation.
    pub projection_generation: u64,
    /// Unchanged business Context revision.
    pub context_revision: u64,
}

/// Caller-owned transaction holding the Community exclusive advisory lock.
pub struct ProjectContextWriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    expected_projection_pubkey: PublicKey,
    operation: ProjectContextOperation,
    loaded: Option<LoadedBasis>,
}

/// Caller-owned disabled-only reprojection transaction.
pub struct ProjectContextReprojectTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    target_pubkey: PublicKey,
    loaded: Option<ProjectContextReprojectContext>,
}

#[derive(Debug, Clone)]
struct LoadedBasis {
    command_edge_key: EdgeKey,
    catalog: ProjectContextCatalog,
    current_edge: Option<ProjectContextEdge>,
    active_document_edge: Option<EdgeKey>,
    projection_pubkey: PublicKey,
    canonical_time: DateTime<Utc>,
    all_coordinates_active: bool,
    context_document_active: bool,
    edge_last_context_revision: Option<u64>,
    binding_context_revision: Option<u64>,
    binding_projection_event_id: Option<EventId>,
}

impl std::fmt::Debug for ProjectContextWriteTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextWriteTx")
            .field("community_id", &self.community_id)
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ProjectContextReprojectTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectContextReprojectTx")
            .field("community_id", &self.community_id)
            .field("target_pubkey", &self.target_pubkey)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Probe live migration objects instead of trusting only the ledger.
    pub async fn project_context_schema_ready(&self) -> crate::Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT \
                EXISTS (SELECT 1 FROM pg_attribute \
                        WHERE attrelid = 'communities'::regclass \
                          AND attname = 'project_context_edge_enabled' AND NOT attisdropped) \
                AND to_regclass('project_context_edge_state') IS NOT NULL \
                AND to_regclass('project_context_edges') IS NOT NULL \
                AND to_regclass('project_context_edge_coordinates') IS NOT NULL \
                AND to_regclass('project_context_document_bindings') IS NOT NULL \
                AND to_regclass('project_context_edge_changes') IS NOT NULL \
                AND to_regclass('idx_project_context_edge_coordinates_lookup') IS NOT NULL \
                AND to_regclass('idx_project_context_bindings_edge') IS NOT NULL \
                AND to_regclass('idx_project_context_bindings_active_document') IS NOT NULL \
                AND to_regprocedure('project_context_validate_community(uuid)') IS NOT NULL \
                AND to_regprocedure('project_context_compute_edge_key(uuid,bytea)') IS NOT NULL",
        )
        .fetch_one(&self.pool)
        .await?)
    }

    /// Deployment-global rolling-start readiness.
    ///
    /// Pre-migration and all-disabled deployments remain ready. Once any
    /// active Community enables Context Edge, the full schema and configured
    /// stable signer are mandatory.
    pub async fn project_context_deployment_ready(
        &self,
        stable_signer_configured: bool,
    ) -> crate::Result<bool> {
        let column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
             WHERE attrelid = 'communities'::regclass \
               AND attname = 'project_context_edge_enabled' AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !column_exists {
            return Ok(true);
        }
        let any_enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities \
             WHERE project_context_edge_enabled AND archived_at IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !any_enabled {
            return Ok(true);
        }
        Ok(stable_signer_configured && self.project_context_schema_ready().await?)
    }

    /// Context readers use the same current Human / managed-owner and active
    /// ban policy as Project View and Project Document.
    pub async fn project_context_authorized_pubkey(
        &self,
        community_id: CommunityId,
        pubkey: &[u8],
    ) -> crate::Result<bool> {
        self.project_view_authorized_pubkey(community_id, pubkey)
            .await
    }

    /// Set-based form used by local and Redis fan-out recipient filtering.
    pub async fn project_context_authorized_pubkeys(
        &self,
        community_id: CommunityId,
        pubkeys: &[Vec<u8>],
    ) -> crate::Result<std::collections::HashSet<Vec<u8>>> {
        self.project_view_authorized_pubkeys(community_id, pubkeys)
            .await
    }

    /// Return a PostgreSQL-derived timestamp for empty-catalog signing.
    pub async fn project_context_canonical_now(&self) -> crate::Result<DateTime<Utc>> {
        Ok(sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&self.pool)
            .await?)
    }

    /// List basic storage status for all Communities in stable UUID order.
    pub async fn list_project_context_statuses(
        &self,
    ) -> crate::Result<Vec<ProjectContextFeatureStatus>> {
        if !self.project_context_schema_ready().await? {
            return Ok(Vec::new());
        }
        let mut query = QueryBuilder::<Postgres>::new(PROJECT_CONTEXT_STATUS_SQL);
        query.push(" ORDER BY c.id");
        let rows = query.build().fetch_all(&self.pool).await?;
        rows.into_iter().map(status_from_row).collect()
    }

    /// Read basic storage status for one exact Community.
    pub async fn project_context_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<Option<ProjectContextFeatureStatus>> {
        if !self.project_context_schema_ready().await? {
            return Ok(None);
        }
        let mut query = QueryBuilder::<Postgres>::new(PROJECT_CONTEXT_STATUS_SQL);
        query
            .push(" WHERE c.id = ")
            .push_bind(community_id.as_uuid());
        let row = query.build().fetch_optional(&self.pool).await?;
        row.map(status_from_row).transpose()
    }

    /// Verify prerequisites and signed current projection parity without writes.
    pub async fn project_context_preflight(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<ProjectContextPreflight> {
        let schema_ready = self.project_context_schema_ready().await?;
        if !schema_ready {
            return Ok(ProjectContextPreflight {
                community_id,
                schema_ready: false,
                project_view_ready: false,
                project_document_ready: false,
                initialized: false,
                signer_matches: false,
                projection_parity: false,
                integrity_ready: false,
                enabled: false,
                structural_read_ready: false,
                advertised_ready: false,
            });
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let row = sqlx::query(
            "SELECT c.archived_at IS NULL AS active, c.project_view_enabled, \
                    c.project_document_enabled, c.project_context_edge_enabled, \
                    c.project_view_schema_version, maintenance.state AS maintenance_state, \
                    document_state.schema_version AS document_schema_version, \
                    document_state.projection_pubkey AS document_projection_pubkey, \
                    state.projection_pubkey \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             LEFT JOIN project_document_state document_state \
               ON document_state.community_id = c.id \
             LEFT JOIN project_context_edge_state state ON state.community_id = c.id \
             WHERE c.id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            tx.rollback().await?;
            return Ok(ProjectContextPreflight {
                community_id,
                schema_ready: true,
                project_view_ready: false,
                project_document_ready: false,
                initialized: false,
                signer_matches: false,
                projection_parity: false,
                integrity_ready: false,
                enabled: false,
                structural_read_ready: false,
                advertised_ready: false,
            });
        };
        let active: bool = row.try_get("active")?;
        let view_enabled: bool = row.try_get("project_view_enabled")?;
        let document_enabled: bool = row.try_get("project_document_enabled")?;
        let enabled: bool = row.try_get("project_context_edge_enabled")?;
        let schema_version: i16 = row.try_get("project_view_schema_version")?;
        let maintenance_state: String = row.try_get("maintenance_state")?;
        let stored_pubkey: Option<Vec<u8>> = row.try_get("projection_pubkey")?;
        let initialized = stored_pubkey.is_some();
        let signer_matches = stored_pubkey
            .as_deref()
            .is_some_and(|bytes| bytes == expected_pubkey.as_bytes());
        let project_view_ready = active
            && view_enabled
            && schema_version == 3
            && maintenance_state == "normal"
            && Db::project_view_v3_structural_ready_in_tx(&mut tx, community_id, expected_pubkey)
                .await?;
        let document_schema_version: Option<i16> = row.try_get("document_schema_version")?;
        let document_projection_pubkey: Option<Vec<u8>> =
            row.try_get("document_projection_pubkey")?;
        let document_basis_ready = active
            && document_enabled
            && schema_version == 3
            && document_schema_version == Some(1)
            && document_projection_pubkey.as_deref() == Some(expected_pubkey.as_bytes());
        let project_document_ready = if document_basis_ready {
            sqlx::query("SELECT project_document_validate_community($1)")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
            crate::project_document::document_projection_parity(
                &mut tx,
                community_id,
                expected_pubkey,
                None,
                None,
            )
            .await?
        } else {
            false
        };
        let projection_parity = if signer_matches {
            context_projection_parity(&mut tx, community_id, expected_pubkey).await?
        } else {
            false
        };
        let integrity_ready = context_integrity_status_in_tx(&mut tx, community_id)
            .await?
            .is_some_and(|status| {
                status.pointer_mismatch_count == 0 && status.orphan_projection_count == 0
            });
        let structural_read_ready = project_view_ready
            && project_document_ready
            && initialized
            && signer_matches
            && projection_parity
            && integrity_ready;
        let result = ProjectContextPreflight {
            community_id,
            schema_ready,
            project_view_ready,
            project_document_ready,
            initialized,
            signer_matches,
            projection_parity,
            integrity_ready,
            enabled,
            structural_read_ready,
            advertised_ready: enabled && structural_read_ready,
        };
        tx.commit().await?;
        Ok(result)
    }

    /// Return structural read readiness independently of the attach gate.
    pub async fn project_context_structural_read_ready(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        Ok(self
            .project_context_preflight(community_id, expected_pubkey)
            .await?
            .structural_read_ready)
    }

    /// Return whether NIP-11 may advertise the Context Edge capability.
    pub async fn project_context_advertised_ready(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<bool> {
        Ok(self
            .project_context_preflight(community_id, expected_pubkey)
            .await?
            .advertised_ready)
    }

    /// Return indexed pointer/orphan diagnostics without mutating state.
    pub async fn project_context_integrity_status(
        &self,
        community_id: CommunityId,
    ) -> crate::Result<Option<ProjectContextIntegrityStatus>> {
        if !self.project_context_schema_ready().await? {
            return Ok(None);
        }
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        let status = context_integrity_status_in_tx(&mut tx, community_id).await?;
        tx.commit().await?;
        Ok(status)
    }

    /// Run SQL commit invariants and full cryptographic current-projection parity.
    pub async fn verify_project_context_storage(
        &self,
        community_id: CommunityId,
        expected_pubkey: &PublicKey,
    ) -> crate::Result<ProjectContextIntegrityStatus> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, true).await?;
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        if !context_projection_parity(&mut tx, community_id, expected_pubkey).await? {
            return Err(DbError::InvalidData(
                "Project Context canonical/projection parity failed".to_owned(),
            ));
        }
        let status = context_integrity_status_in_tx(&mut tx, community_id)
            .await?
            .ok_or_else(|| DbError::InvalidData("Project Context is not initialized".to_owned()))?;
        if status.pointer_mismatch_count != 0 || status.orphan_projection_count != 0 {
            return Err(DbError::InvalidData(
                "Project Context projection pointers contain mismatches or live orphans".to_owned(),
            ));
        }
        tx.commit().await?;
        Ok(status)
    }

    /// Enable or disable Context Edge under the shared Community writer lock.
    ///
    /// Disable preserves every canonical row and projection. Enable requires
    /// an active Community, matching stable signer, healthy Project View v3
    /// and Project Document prerequisites, and full Context projection parity.
    pub async fn set_project_context_edge_enabled_checked(
        &self,
        community_id: CommunityId,
        enabled: bool,
        expected_pubkey: Option<&PublicKey>,
    ) -> ProjectContextWriteResult<bool> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let community = sqlx::query(
            "SELECT archived_at IS NULL AS active, project_view_schema_version, \
                    project_view_enabled, project_document_enabled \
             FROM communities WHERE id = $1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        let Some(community) = community else {
            tx.rollback().await?;
            return Ok(false);
        };
        if enabled {
            let expected_pubkey = expected_pubkey.ok_or_else(|| {
                DbError::InvalidData(
                    "a stable Relay signer is required to enable Project Context Edge".to_owned(),
                )
            })?;
            let active: bool = community.try_get("active")?;
            let schema_version: i16 = community.try_get("project_view_schema_version")?;
            let view_enabled: bool = community.try_get("project_view_enabled")?;
            let document_enabled: bool = community.try_get("project_document_enabled")?;
            if !active || schema_version != 3 || !view_enabled || !document_enabled {
                return Err(DbError::InvalidData(
                    "Project Context Edge requires active Project View v3 and Project Document"
                        .to_owned(),
                )
                .into());
            }
            let signer: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT projection_pubkey FROM project_context_edge_state \
                 WHERE community_id = $1 FOR UPDATE",
            )
            .bind(community_id.as_uuid())
            .fetch_optional(&mut *tx)
            .await?;
            if signer.as_deref() != Some(expected_pubkey.as_bytes()) {
                return Err(DbError::InvalidData(
                    "Project Context Edge stable signer does not match initialized state"
                        .to_owned(),
                )
                .into());
            }
            require_bootstrap_prerequisites(&mut tx, community_id, expected_pubkey).await?;
            sqlx::query("SELECT project_context_validate_community($1)")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
            if !context_projection_parity(&mut tx, community_id, expected_pubkey).await? {
                return Err(DbError::InvalidData(
                    "Project Context Edge canonical/projection parity is not ready".to_owned(),
                )
                .into());
            }
            let integrity = context_integrity_status_in_tx(&mut tx, community_id)
                .await?
                .ok_or_else(|| {
                    DbError::InvalidData(
                        "Project Context Edge must be initialized before enable".to_owned(),
                    )
                })?;
            if integrity.pointer_mismatch_count != 0 || integrity.orphan_projection_count != 0 {
                return Err(DbError::InvalidData(
                    "Project Context Edge projections contain mismatches or live orphans"
                        .to_owned(),
                )
                .into());
            }
        }
        let result =
            sqlx::query("UPDATE communities SET project_context_edge_enabled = $2 WHERE id = $1")
                .bind(community_id.as_uuid())
                .bind(enabled)
                .execute(&mut *tx)
                .await?;
        if result.rows_affected() == 1 {
            append_context_control_audit(
                &mut tx,
                community_id,
                if enabled { "enable" } else { "disable" },
            )
            .await?;
        }
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically store a signed generation-one, revision-zero empty catalog.
    ///
    /// An exact retry is idempotent. Any occupied but non-identical state is
    /// rejected rather than overwritten.
    pub async fn bootstrap_empty_project_context_catalog(
        &self,
        prepared: PreparedProjectContextBootstrap,
    ) -> ProjectContextWriteResult<ProjectContextBootstrapOutcome> {
        validate_bootstrap(&prepared)?;
        let community_id = prepared.catalog.project_id();
        let expected_pubkey = prepared.meta_projection.pubkey;
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        require_bootstrap_prerequisites(&mut tx, community_id, &expected_pubkey).await?;
        let outcome = store_empty_project_context_catalog_in_tx(&mut tx, &prepared).await?;
        if !outcome.replayed {
            append_context_control_audit(&mut tx, community_id, "bootstrap").await?;
        }
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// Begin one disabled-only signer reprojection under the Community lock.
    ///
    /// Project View v3 and Project Document must already be enabled, healthy,
    /// and materialized by `target_pubkey`. The Context capability itself must
    /// remain disabled until the replacement generation has been verified.
    pub async fn begin_project_context_reproject(
        &self,
        community_id: CommunityId,
        target_pubkey: PublicKey,
    ) -> ProjectContextWriteResult<ProjectContextReprojectTx> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let enabled: Option<bool> = sqlx::query_scalar(
            "SELECT project_context_edge_enabled FROM communities \
             WHERE id = $1 AND archived_at IS NULL FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        match enabled {
            Some(false) => {}
            Some(true) => {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "Project Context Edge must be disabled before reprojection".to_owned(),
                ));
            }
            None => return Err(ProjectContextWriteError::Unavailable { community_id }),
        }
        require_bootstrap_prerequisites(&mut tx, community_id, &target_pubkey).await?;
        Ok(ProjectContextReprojectTx {
            tx,
            community_id,
            target_pubkey,
            loaded: None,
        })
    }

    /// Begin one operation-aware business write under the Community lock.
    ///
    /// Attach requires the capability flag. Detach remains available while
    /// disabled when all structural prerequisites are healthy, preserving the
    /// explicit recovery semantics from the design.
    pub async fn begin_project_context_write(
        &self,
        community_id: CommunityId,
        expected_projection_pubkey: PublicKey,
        operation: ProjectContextOperation,
    ) -> ProjectContextWriteResult<ProjectContextWriteTx> {
        let mut tx = self.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let available: Option<bool> = sqlx::query_scalar(
            "SELECT c.archived_at IS NULL \
                    AND c.project_view_schema_version = 3 \
                    AND c.project_view_enabled \
                    AND c.project_document_enabled \
                    AND maintenance.state = 'normal' \
                    AND view_state.schema_version = 3 \
                    AND document_state.schema_version = 1 \
                    AND context_state.schema_version = 1 \
                    AND view_state.projection_pubkey = $2 \
                    AND document_state.projection_pubkey = $2 \
                    AND context_state.projection_pubkey = $2 \
                    AND ($3 = 'detach' OR c.project_context_edge_enabled) \
             FROM communities c \
             JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
             JOIN project_view_state view_state ON view_state.community_id = c.id \
             JOIN project_document_state document_state ON document_state.community_id = c.id \
             JOIN project_context_edge_state context_state ON context_state.community_id = c.id \
             WHERE c.id = $1 FOR UPDATE OF c, maintenance, view_state, document_state, context_state",
        )
        .bind(community_id.as_uuid())
        .bind(expected_projection_pubkey.as_bytes())
        .bind(operation.as_str())
        .fetch_optional(&mut *tx)
        .await?;
        if available != Some(true) {
            return Err(ProjectContextWriteError::Unavailable { community_id });
        }
        if !Db::project_view_v3_structural_ready_in_tx(
            &mut tx,
            community_id,
            &expected_projection_pubkey,
        )
        .await?
            || !crate::project_document::document_projection_parity(
                &mut tx,
                community_id,
                &expected_projection_pubkey,
                None,
                None,
            )
            .await?
            || !context_projection_parity(&mut tx, community_id, &expected_projection_pubkey)
                .await?
        {
            return Err(ProjectContextWriteError::Unavailable { community_id });
        }
        sqlx::query("SELECT project_document_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        Ok(ProjectContextWriteTx {
            tx,
            community_id,
            expected_projection_pubkey,
            operation,
            loaded: None,
        })
    }
}

async fn store_empty_project_context_catalog_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    prepared: &PreparedProjectContextBootstrap,
) -> ProjectContextWriteResult<ProjectContextBootstrapOutcome> {
    validate_bootstrap(prepared)?;
    let community_id = prepared.catalog.project_id();
    let expected_pubkey = prepared.meta_projection.pubkey;
    if let Some(row) = sqlx::query(
        "SELECT context_revision, active_edge_count, bound_document_count, \
                last_change_id, last_actor_pubkey, projection_pubkey, \
                projection_generation, meta_projection_event_id, initialized_at, updated_at \
         FROM project_context_edge_state WHERE community_id = $1 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut **tx)
    .await?
    {
        let metadata = state_metadata_from_row(&row)?;
        let empty_rows: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (SELECT 1 FROM project_context_edges WHERE community_id = $1) \
                 AND NOT EXISTS (SELECT 1 FROM project_context_document_bindings WHERE community_id = $1) \
                 AND NOT EXISTS (SELECT 1 FROM project_context_edge_changes WHERE community_id = $1)",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&mut **tx)
        .await?;
        let exact = metadata.context_revision == 0
            && metadata.active_edge_count == 0
            && metadata.bound_document_count == 0
            && metadata.last_change_id.is_none()
            && metadata.last_actor_pubkey.is_none()
            && metadata.projection_pubkey == expected_pubkey
            && metadata.projection_generation == prepared.catalog.projection_generation()
            && metadata.meta_projection_event_id == prepared.meta_projection.id
            && metadata.initialized_at == prepared.catalog.initialized_at()
            && metadata.updated_at == prepared.catalog.updated_at()
            && empty_rows;
        let stored =
            context_event_by_id(tx, community_id, prepared.meta_projection.id.as_bytes()).await?;
        if !exact || stored.as_ref().map(|event| &event.event) != Some(&prepared.meta_projection) {
            return Err(ProjectContextWriteError::InvalidCommit(
                "occupied Project Context bootstrap state is not the exact signed empty catalog"
                    .to_owned(),
            ));
        }
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut **tx)
            .await?;
        return Ok(ProjectContextBootstrapOutcome { replayed: true });
    }

    let occupied: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM project_context_edges WHERE community_id = $1) \
             OR EXISTS (SELECT 1 FROM project_context_document_bindings WHERE community_id = $1) \
             OR EXISTS (SELECT 1 FROM project_context_edge_changes WHERE community_id = $1)",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    let capability_enabled: bool = sqlx::query_scalar(
        "SELECT project_context_edge_enabled FROM communities WHERE id = $1 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .fetch_one(&mut **tx)
    .await?;
    if occupied || capability_enabled {
        return Err(ProjectContextWriteError::InvalidCommit(
            "new Project Context bootstrap requires disabled, completely empty state".to_owned(),
        ));
    }

    let (_, inserted) =
        crate::event::insert_event_in_tx(tx, community_id, &prepared.meta_projection, None).await?;
    if !inserted {
        return Err(ProjectContextWriteError::InvalidCommit(
            "bootstrap metadata event already exists without canonical state".to_owned(),
        ));
    }
    sqlx::query(
        "INSERT INTO project_context_edge_state \
            (community_id, schema_version, context_revision, active_edge_count, \
             bound_document_count, projection_pubkey, projection_generation, \
             meta_projection_event_id, initialized_at, updated_at) \
         VALUES ($1, 1, 0, 0, 0, $2, 1, $3, $4, $4)",
    )
    .bind(community_id.as_uuid())
    .bind(expected_pubkey.as_bytes())
    .bind(prepared.meta_projection.id.as_bytes().as_slice())
    .bind(prepared.catalog.initialized_at())
    .execute(&mut **tx)
    .await?;
    Ok(ProjectContextBootstrapOutcome { replayed: false })
}

const PROJECT_CONTEXT_STATUS_SQL: &str =
    "SELECT c.id, c.host, c.archived_at IS NOT NULL AS archived, \
            c.project_context_edge_enabled, c.project_view_schema_version, \
            c.project_view_enabled, c.project_document_enabled, \
            maintenance.state AS maintenance_state, \
            state.context_revision, state.active_edge_count, state.bound_document_count, \
            state.projection_generation, state.projection_pubkey, \
            (SELECT count(*)::bigint FROM project_context_edges edge \
             WHERE edge.community_id = c.id) AS edge_row_count, \
            (SELECT count(*)::bigint FROM project_context_document_bindings binding \
             WHERE binding.community_id = c.id) AS binding_row_count, \
            (SELECT count(*)::bigint FROM project_context_edge_changes change \
             WHERE change.community_id = c.id) AS change_count \
     FROM communities c \
     JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
     LEFT JOIN project_context_edge_state state ON state.community_id = c.id";

impl ProjectContextReprojectTx {
    /// Return the Community protected by this maintenance transaction.
    #[must_use]
    pub const fn community_id(&self) -> CommunityId {
        self.community_id
    }

    /// Explicitly roll back and release the Community lock.
    pub async fn rollback(self) -> ProjectContextWriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Reconstruct every durable binding head at the next projection generation.
    ///
    /// This reads canonical rows rather than trusting the current signed event
    /// pointers, allowing the subsequent commit to recover missing pointers or
    /// live orphan projections while still rejecting canonical corruption.
    pub async fn load_current(
        &mut self,
    ) -> ProjectContextWriteResult<ProjectContextReprojectContext> {
        if self.loaded.is_some() {
            return Err(ProjectContextWriteError::InvalidCommit(
                "this transaction already loaded a Project Context reprojection basis".to_owned(),
            ));
        }
        let state_row = sqlx::query(
            "SELECT context_revision, active_edge_count, bound_document_count, \
                    last_change_id, last_actor_pubkey, projection_pubkey, \
                    projection_generation, meta_projection_event_id, initialized_at, updated_at \
             FROM project_context_edge_state WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?
        .ok_or_else(|| {
            ProjectContextWriteError::InvalidCommit(
                "Project Context Edge must be initialized before reprojection".to_owned(),
            )
        })?;
        let metadata = state_metadata_from_row(&state_row)?;
        let projection_generation = metadata
            .projection_generation
            .checked_add(1)
            .filter(|value| *value <= MAX_SAFE_REVISION)
            .ok_or_else(|| {
                ProjectContextWriteError::InvalidCommit(
                    "projection generation overflow during Project Context reprojection".to_owned(),
                )
            })?;
        let catalog = ProjectContextCatalog::from_snapshot(
            self.community_id,
            metadata.context_revision,
            metadata.active_edge_count,
            metadata.bound_document_count,
            projection_generation,
            metadata.initialized_at,
            metadata.updated_at,
        )?;

        let edge_rows = sqlx::query(
            "SELECT edge_key, state, canonical_coordinates \
             FROM project_context_edges WHERE community_id = $1 \
             ORDER BY edge_key FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_all(&mut *self.tx)
        .await?;
        let mut edges = BTreeMap::new();
        for row in edge_rows {
            let edge_key = edge_key_from_bytes(&row.try_get::<Vec<u8>, _>("edge_key")?)?;
            let state: String = row.try_get("state")?;
            if !matches!(state.as_str(), "active" | "deleted") {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "canonical Edge has an unsupported lifecycle state".to_owned(),
                ));
            }
            let coordinates: Vec<ProjectContextCoordinate> = serde_json::from_value(
                row.try_get::<Value, _>("canonical_coordinates")?,
            )
            .map_err(|error| {
                ProjectContextWriteError::InvalidCommit(format!(
                    "invalid canonical Project Context coordinates: {error}"
                ))
            })?;
            if EdgeKey::derive(*self.community_id.as_uuid(), &coordinates)? != edge_key {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "canonical Edge key does not match its coordinate set".to_owned(),
                ));
            }
            let normalized =
                load_normalized_coordinates(&mut self.tx, self.community_id, edge_key).await?;
            if normalized != coordinates {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "canonical Edge JSON and normalized coordinates disagree".to_owned(),
                ));
            }
            if edges.insert(edge_key, (state, coordinates)).is_some() {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "duplicate canonical Project Context Edge identity".to_owned(),
                ));
            }
        }

        let binding_rows = sqlx::query(
            "SELECT context_document_id, edge_key, state, binding_context_revision, \
                    current_source_change_id, updated_at \
             FROM project_context_document_bindings WHERE community_id = $1 \
             ORDER BY context_document_id FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_all(&mut *self.tx)
        .await?;
        let mut bindings = Vec::with_capacity(binding_rows.len());
        let mut binding_ids = BTreeSet::new();
        let mut active_members_by_edge: BTreeMap<EdgeKey, u64> = BTreeMap::new();
        let mut active_binding_count = 0_u64;
        for row in binding_rows {
            let context_document_id: Uuid = row.try_get("context_document_id")?;
            if !binding_ids.insert(context_document_id) {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "duplicate canonical binding for Document {context_document_id}"
                )));
            }
            let edge_key = edge_key_from_bytes(&row.try_get::<Vec<u8>, _>("edge_key")?)?;
            let (edge_state, coordinates) = edges.get(&edge_key).ok_or_else(|| {
                ProjectContextWriteError::InvalidCommit(format!(
                    "binding for Document {context_document_id} references a missing Edge"
                ))
            })?;
            let state_text: String = row.try_get("state")?;
            let state = binding_state_from_str(&state_text).ok_or_else(|| {
                ProjectContextWriteError::InvalidCommit(format!(
                    "binding for Document {context_document_id} has an unsupported state"
                ))
            })?;
            if state == ProjectContextBindingState::Active {
                if edge_state != "active" {
                    return Err(ProjectContextWriteError::InvalidCommit(format!(
                        "active binding for Document {context_document_id} belongs to a deleted Edge"
                    )));
                }
                active_binding_count = active_binding_count.checked_add(1).ok_or_else(|| {
                    ProjectContextWriteError::InvalidCommit(
                        "active Project Context binding count overflow".to_owned(),
                    )
                })?;
                let count = active_members_by_edge.entry(edge_key).or_default();
                *count = count.checked_add(1).ok_or_else(|| {
                    ProjectContextWriteError::InvalidCommit(
                        "Project Context Edge membership count overflow".to_owned(),
                    )
                })?;
            }
            let projection = ProjectContextBindingProjection {
                schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
                projection_type:
                    buzz_project_context::ProjectContextProjectionType::ContextEdgeBinding,
                project_id: *self.community_id.as_uuid(),
                projection_generation,
                context_revision: db_positive_revision(
                    row.try_get("binding_context_revision")?,
                    "binding_context_revision",
                )?,
                edge_key,
                coordinates: coordinates.clone(),
                context_document_id,
                state,
                source_event_id: event_id_from_bytes(
                    &row.try_get::<Vec<u8>, _>("current_source_change_id")?,
                    "current_source_change_id",
                )?,
                updated_at: row.try_get("updated_at")?,
            };
            projection.validate()?;
            if projection.context_revision > catalog.context_revision() {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "binding for Document {context_document_id} is ahead of the catalog"
                )));
            }
            bindings.push(projection);
        }

        let active_edge_count = edges
            .iter()
            .filter(|(_, (state, _))| state == "active")
            .count();
        let edge_lifecycles_valid = edges.iter().all(|(edge_key, (state, _))| {
            let active_members = active_members_by_edge.get(edge_key).copied().unwrap_or(0);
            (state == "active" && active_members > 0) || (state == "deleted" && active_members == 0)
        });
        if !edge_lifecycles_valid
            || u64::try_from(active_edge_count).ok() != Some(catalog.active_edge_count())
            || active_binding_count != catalog.bound_document_count()
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "canonical Project Context counts or Edge lifecycles disagree".to_owned(),
            ));
        }

        let context = ProjectContextReprojectContext {
            catalog,
            source_generation: metadata.projection_generation,
            source_pubkey: metadata.projection_pubkey,
            bindings,
        };
        self.loaded = Some(context.clone());
        Ok(context)
    }

    /// Atomically replace every current binding and metadata projection head.
    ///
    /// The commit retires every live Context projection, including orphans,
    /// then rebinds canonical pointers under the migration's narrow reproject
    /// guard. No business revision or domain row is modified.
    pub async fn commit_reprojection(
        mut self,
        prepared: PreparedProjectContextReprojection,
    ) -> ProjectContextWriteResult<ProjectContextReprojectOutcome> {
        let loaded = self.loaded.as_ref().ok_or_else(|| {
            ProjectContextWriteError::InvalidCommit(
                "reprojection must be prepared from load_current on the same transaction"
                    .to_owned(),
            )
        })?;
        let meta = parse_project_context_meta(
            &prepared.meta_projection,
            &self.target_pubkey,
            self.community_id,
        )
        .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
        if meta.projection.project_id != *self.community_id.as_uuid()
            || meta.projection.projection_generation != loaded.catalog.projection_generation()
            || meta.projection.context_revision != loaded.catalog.context_revision()
            || meta.projection.active_edge_count != loaded.catalog.active_edge_count()
            || meta.projection.bound_document_count != loaded.catalog.bound_document_count()
            || !meta.projection.reset
            || !meta.projection.changed_bindings.is_empty()
            || meta.projection.source_event_id.is_some()
            || meta.projection.updated_at != loaded.catalog.updated_at()
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "replacement metadata does not exactly represent the locked reset catalog"
                    .to_owned(),
            ));
        }

        let expected_bindings = loaded
            .bindings
            .iter()
            .map(|binding| (binding.context_document_id, binding))
            .collect::<BTreeMap<_, _>>();
        let mut prepared_bindings = BTreeMap::new();
        for event in &prepared.binding_projections {
            let verified =
                parse_project_context_binding(event, &self.target_pubkey, self.community_id)
                    .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
            let document_id = verified.projection.context_document_id;
            if expected_bindings.get(&document_id).copied() != Some(&verified.projection) {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "replacement binding for Document {document_id} differs from canonical state"
                )));
            }
            verify_project_context_binding_observation(&meta, &verified)
                .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
            if prepared_bindings
                .insert(document_id, event.clone())
                .is_some()
            {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "duplicate replacement binding for Document {document_id}"
                )));
            }
        }
        if prepared_bindings.keys().copied().collect::<BTreeSet<_>>()
            != expected_bindings.keys().copied().collect::<BTreeSet<_>>()
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "reprojection must exactly cover every active and deleted binding head".to_owned(),
            ));
        }

        sqlx::query("SELECT set_config('buzz.project_context_reproject', 'on', true)")
            .execute(&mut *self.tx)
            .await?;
        sqlx::query(
            "UPDATE events SET deleted_at = clock_timestamp() \
             WHERE community_id = $1 AND kind = ANY($2) AND deleted_at IS NULL",
        )
        .bind(self.community_id.as_uuid())
        .bind([
            KIND_PROJECT_CONTEXT_EDGE_BINDING as i32,
            KIND_PROJECT_CONTEXT_META as i32,
        ])
        .execute(&mut *self.tx)
        .await?;

        let mut published_events = Vec::with_capacity(prepared_bindings.len() + 1);
        for (document_id, event) in prepared_bindings {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, &event, None)
                    .await?;
            if !inserted {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "replacement binding for Document {document_id} already exists"
                )));
            }
            let update = sqlx::query(
                "UPDATE project_context_document_bindings \
                 SET current_projection_event_id = $3 \
                 WHERE community_id = $1 AND context_document_id = $2",
            )
            .bind(self.community_id.as_uuid())
            .bind(document_id)
            .bind(event.id.as_bytes().as_slice())
            .execute(&mut *self.tx)
            .await?;
            if update.rows_affected() != 1 {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "canonical binding pointer for Document {document_id} was not updated"
                )));
            }
            published_events.push(event);
        }

        let (_, meta_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &prepared.meta_projection,
            None,
        )
        .await?;
        if !meta_inserted {
            return Err(ProjectContextWriteError::InvalidCommit(
                "replacement Project Context metadata already exists".to_owned(),
            ));
        }
        let state_update = sqlx::query(
            "UPDATE project_context_edge_state \
             SET projection_pubkey = $2, projection_generation = $3, \
                 meta_projection_event_id = $4 \
             WHERE community_id = $1 AND projection_pubkey = $5 \
               AND projection_generation = $6 AND context_revision = $7",
        )
        .bind(self.community_id.as_uuid())
        .bind(self.target_pubkey.as_bytes())
        .bind(revision_to_i64(
            loaded.catalog.projection_generation(),
            "projection_generation",
        )?)
        .bind(prepared.meta_projection.id.as_bytes().as_slice())
        .bind(loaded.source_pubkey.as_bytes())
        .bind(revision_to_i64(
            loaded.source_generation,
            "source_projection_generation",
        )?)
        .bind(revision_to_i64(
            loaded.catalog.context_revision(),
            "context_revision",
        )?)
        .execute(&mut *self.tx)
        .await?;
        if state_update.rows_affected() != 1 {
            return Err(ProjectContextWriteError::InvalidCommit(
                "canonical Project Context state changed during reprojection".to_owned(),
            ));
        }

        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *self.tx)
            .await?;
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(self.community_id.as_uuid())
            .execute(&mut *self.tx)
            .await?;
        if !context_projection_parity(&mut self.tx, self.community_id, &self.target_pubkey).await? {
            return Err(ProjectContextWriteError::InvalidCommit(
                "replacement Project Context generation failed cryptographic parity".to_owned(),
            ));
        }
        let integrity = context_integrity_status_in_tx(&mut self.tx, self.community_id)
            .await?
            .ok_or_else(|| {
                ProjectContextWriteError::InvalidCommit(
                    "replacement Project Context generation is not initialized".to_owned(),
                )
            })?;
        if integrity.pointer_mismatch_count != 0 || integrity.orphan_projection_count != 0 {
            return Err(ProjectContextWriteError::InvalidCommit(
                "replacement Project Context generation contains pointer mismatches or live orphans"
                    .to_owned(),
            ));
        }
        append_context_control_audit(&mut self.tx, self.community_id, "reproject").await?;
        published_events.push(prepared.meta_projection);
        let outcome = ProjectContextReprojectOutcome {
            events: published_events,
            source_generation: loaded.source_generation,
            projection_generation: loaded.catalog.projection_generation(),
            context_revision: loaded.catalog.context_revision(),
        };
        self.tx.commit().await?;
        Ok(outcome)
    }
}

impl ProjectContextWriteTx {
    /// Explicitly roll back and release the Community lock.
    pub async fn rollback(self) -> ProjectContextWriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Reauthorize the signer and perform replay lookup before loading state.
    pub async fn prepare_command(
        &mut self,
        command_event: &Event,
        command: &ProjectContextCommand,
    ) -> ProjectContextWriteResult<ProjectContextPrepareOutcome> {
        let parsed = parse_project_context_command(command_event, self.community_id)
            .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
        if &parsed != command || command.operation() != self.operation {
            return Err(ProjectContextWriteError::InvalidCommit(
                "command event does not carry the supplied operation-bound command".to_owned(),
            ));
        }
        validate_actor_in_tx(
            &mut self.tx,
            self.community_id,
            command_event.pubkey,
            command,
        )
        .await?;
        if let Some(receipt) =
            find_receipt(&mut self.tx, self.community_id, command_event.id.as_bytes()).await?
        {
            validate_replayed_receipt(command_event, command, &receipt, self.community_id)?;
            return Ok(ProjectContextPrepareOutcome::Replayed(receipt));
        }
        Ok(ProjectContextPrepareOutcome::New)
    }

    /// Lock and reconstruct the exact reducer basis for one strict command.
    pub async fn load_current(
        &mut self,
        command: &ProjectContextCommand,
    ) -> ProjectContextWriteResult<ProjectContextWriteContext> {
        if self.loaded.is_some() {
            return Err(ProjectContextWriteError::InvalidCommit(
                "this transaction already loaded a Project Context basis".to_owned(),
            ));
        }
        if command.operation() != self.operation {
            return Err(ProjectContextWriteError::InvalidCommit(
                "loaded command operation differs from the transaction operation".to_owned(),
            ));
        }
        command.validate_for_project(*self.community_id.as_uuid())?;
        let state_row = sqlx::query(
            "SELECT context_revision, active_edge_count, bound_document_count, \
                    last_change_id, last_actor_pubkey, projection_pubkey, \
                    projection_generation, meta_projection_event_id, initialized_at, updated_at \
             FROM project_context_edge_state WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some(state_row) = state_row else {
            return Err(ProjectContextWriteError::Unavailable {
                community_id: self.community_id,
            });
        };
        let metadata = state_metadata_from_row(&state_row)?;
        if metadata.projection_pubkey != self.expected_projection_pubkey {
            return Err(ProjectContextWriteError::Unavailable {
                community_id: self.community_id,
            });
        }
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(self.community_id.as_uuid())
            .execute(&mut *self.tx)
            .await?;
        let catalog = ProjectContextCatalog::from_snapshot(
            self.community_id,
            metadata.context_revision,
            metadata.active_edge_count,
            metadata.bound_document_count,
            metadata.projection_generation,
            metadata.initialized_at,
            metadata.updated_at,
        )?;
        let edge_key = EdgeKey::derive(*self.community_id.as_uuid(), command.coordinates())?;
        let edge_row = sqlx::query(
            "SELECT state, canonical_coordinates, last_context_revision \
             FROM project_context_edges \
             WHERE community_id = $1 AND edge_key = $2 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(edge_key.as_bytes().as_slice())
        .fetch_optional(&mut *self.tx)
        .await?;
        let mut edge_last_context_revision = None;
        let current_edge = if let Some(row) = edge_row {
            let stored_coordinates: Vec<ProjectContextCoordinate> = serde_json::from_value(
                row.try_get::<Value, _>("canonical_coordinates")?,
            )
            .map_err(|error| {
                ProjectContextWriteError::InvalidCommit(format!(
                    "invalid canonical edge coordinates: {error}"
                ))
            })?;
            if stored_coordinates != command.coordinates()
                || EdgeKey::derive(*self.community_id.as_uuid(), &stored_coordinates)? != edge_key
            {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "edge-key collision or canonical coordinate drift detected".to_owned(),
                ));
            }
            edge_last_context_revision = Some(db_positive_revision(
                row.try_get("last_context_revision")?,
                "last_context_revision",
            )?);
            let state: String = row.try_get("state")?;
            let document_ids: Vec<Uuid> = sqlx::query_scalar(
                "SELECT context_document_id FROM project_context_document_bindings \
                 WHERE community_id = $1 AND edge_key = $2 AND state = 'active' \
                 ORDER BY context_document_id FOR UPDATE",
            )
            .bind(self.community_id.as_uuid())
            .bind(edge_key.as_bytes().as_slice())
            .fetch_all(&mut *self.tx)
            .await?;
            match state.as_str() {
                "active" => Some(ProjectContextEdge::from_snapshot(
                    *self.community_id.as_uuid(),
                    stored_coordinates,
                    document_ids,
                )?),
                "deleted" if document_ids.is_empty() => None,
                "deleted" => {
                    return Err(ProjectContextWriteError::InvalidCommit(
                        "deleted edge retains active bindings".to_owned(),
                    ));
                }
                _ => {
                    return Err(ProjectContextWriteError::InvalidCommit(format!(
                        "unknown Project Context edge state {state}"
                    )));
                }
            }
        } else {
            None
        };

        let binding_row = sqlx::query(
            "SELECT edge_key, state, binding_context_revision, current_projection_event_id \
             FROM project_context_document_bindings \
             WHERE community_id = $1 AND context_document_id = $2 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(command.context_document_id())
        .fetch_optional(&mut *self.tx)
        .await?;
        let mut active_document_edge = None;
        let mut binding_context_revision = None;
        let mut binding_projection_event_id = None;
        if let Some(row) = binding_row {
            let stored_key = edge_key_from_bytes(&row.try_get::<Vec<u8>, _>("edge_key")?)?;
            let state: String = row.try_get("state")?;
            if state == "active" {
                active_document_edge = Some(stored_key);
            } else if state != "deleted" {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "unknown Project Context binding state {state}"
                )));
            }
            binding_context_revision = Some(db_positive_revision(
                row.try_get("binding_context_revision")?,
                "binding_context_revision",
            )?);
            binding_projection_event_id = Some(event_id_from_bytes(
                &row.try_get::<Vec<u8>, _>("current_projection_event_id")?,
                "current_projection_event_id",
            )?);
        }

        let mut all_coordinates_active = true;
        for coordinate in command.coordinates() {
            let active =
                coordinate_active_in_tx(&mut self.tx, self.community_id, coordinate).await?;
            all_coordinates_active &= active;
        }
        let context_document_active: bool = sqlx::query_scalar(
            "SELECT state = 'active' FROM project_documents \
             WHERE community_id = $1 AND document_id = $2 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(command.context_document_id())
        .fetch_optional(&mut *self.tx)
        .await?
        .unwrap_or(false);
        let canonical_time: DateTime<Utc> = sqlx::query_scalar(
            "SELECT GREATEST(clock_timestamp(), $1::timestamptz + interval '1 microsecond')",
        )
        .bind(metadata.updated_at)
        .fetch_one(&mut *self.tx)
        .await?;
        self.loaded = Some(LoadedBasis {
            command_edge_key: edge_key,
            catalog: catalog.clone(),
            current_edge: current_edge.clone(),
            active_document_edge,
            projection_pubkey: metadata.projection_pubkey,
            canonical_time,
            all_coordinates_active,
            context_document_active,
            edge_last_context_revision,
            binding_context_revision,
            binding_projection_event_id,
        });
        Ok(ProjectContextWriteContext {
            catalog,
            current_edge,
            active_document_edge,
            canonical_time,
            all_coordinates_active,
            context_document_active,
        })
    }

    /// Commit one command, receipt, current rows, and both signed projections.
    pub async fn commit(
        mut self,
        prepared: PreparedProjectContextCommit,
    ) -> ProjectContextWriteResult<ProjectContextCommitOutcome> {
        let parsed = parse_project_context_command(&prepared.command_event, self.community_id)
            .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
        if parsed != prepared.command
            || prepared.command.operation() != self.operation
            || u32::from(prepared.command_event.kind.as_u16()) != KIND_PROJECT_CONTEXT_COMMAND
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "command event does not carry the supplied operation-bound command".to_owned(),
            ));
        }
        validate_actor_in_tx(
            &mut self.tx,
            self.community_id,
            prepared.command_event.pubkey,
            &prepared.command,
        )
        .await?;
        if let Some(receipt) = find_receipt(
            &mut self.tx,
            self.community_id,
            prepared.command_event.id.as_bytes(),
        )
        .await?
        {
            validate_replayed_receipt(
                &prepared.command_event,
                &prepared.command,
                &receipt,
                self.community_id,
            )?;
            self.tx.commit().await?;
            return Ok(ProjectContextCommitOutcome {
                receipt,
                replayed: true,
            });
        }
        let loaded = self.loaded.as_ref().ok_or_else(|| {
            ProjectContextWriteError::InvalidCommit(
                "commit must be derived from load_current on the same transaction".to_owned(),
            )
        })?;
        if loaded.command_edge_key
            != EdgeKey::derive(*self.community_id.as_uuid(), prepared.command.coordinates())?
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "loaded exact edge differs from the command".to_owned(),
            ));
        }
        let derived = reduce_project_context(
            &loaded.catalog,
            loaded.current_edge.as_ref(),
            loaded.active_document_edge,
            &prepared.command,
            buzz_project_context::ProjectContextChangeContext::active(
                prepared.command_event.pubkey,
                prepared.command_event.id,
                loaded.canonical_time,
            )
            .with_coordinates_active(loaded.all_coordinates_active)
            .with_context_document_active(loaded.context_document_active),
        )?;
        if derived != prepared.transition {
            return Err(ProjectContextWriteError::InvalidCommit(
                "prepared transition was not derived from the locked canonical basis".to_owned(),
            ));
        }
        verify_project_context_projection_bundle(
            prepared.transition.projection_plan(),
            &prepared.binding_projection,
            &prepared.meta_projection,
            &loaded.projection_pubkey,
        )
        .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
        validate_projection_kinds(&prepared)?;

        let old_meta_event_id: Vec<u8> = sqlx::query_scalar(
            "SELECT meta_projection_event_id FROM project_context_edge_state \
             WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_one(&mut *self.tx)
        .await?;
        if !crate::event::retire_projection_head_in_tx(
            &mut self.tx,
            self.community_id,
            &old_meta_event_id,
            KIND_PROJECT_CONTEXT_META,
        )
        .await?
        {
            return Err(ProjectContextWriteError::InvalidCommit(
                "stored Context metadata pointer is not live".to_owned(),
            ));
        }
        if let Some(old_binding_event_id) = loaded.binding_projection_event_id {
            if !crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                old_binding_event_id.as_bytes(),
                KIND_PROJECT_CONTEXT_EDGE_BINDING,
            )
            .await?
            {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "stored Context binding pointer is not live".to_owned(),
                ));
            }
        }
        for (label, event) in [
            ("command", &prepared.command_event),
            ("binding", &prepared.binding_projection),
            ("metadata", &prepared.meta_projection),
        ] {
            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, event, None)
                    .await?;
            if !inserted {
                return Err(ProjectContextWriteError::InvalidCommit(format!(
                    "{label} event already exists without a canonical receipt"
                )));
            }
        }

        let receipt = prepared.transition.receipt().clone();
        let receipt_result = serde_json::to_value(&receipt).map_err(|error| {
            ProjectContextWriteError::InvalidCommit(format!("serialize receipt: {error}"))
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
        write_edge(
            &mut self.tx,
            self.community_id,
            &prepared,
            loaded.edge_last_context_revision,
        )
        .await?;
        write_binding(
            &mut self.tx,
            self.community_id,
            &prepared,
            loaded.binding_context_revision,
        )
        .await?;

        let catalog = prepared.transition.catalog();
        let update = sqlx::query(
            "UPDATE project_context_edge_state \
             SET context_revision = $2, active_edge_count = $3, bound_document_count = $4, \
                 last_change_id = $5, last_actor_pubkey = $6, updated_at = $7, \
                 meta_projection_event_id = $8 \
             WHERE community_id = $1 AND context_revision = $9 \
               AND projection_generation = $10 AND projection_pubkey = $11",
        )
        .bind(self.community_id.as_uuid())
        .bind(revision_to_i64(
            catalog.context_revision(),
            "context_revision",
        )?)
        .bind(revision_to_i64(
            catalog.active_edge_count(),
            "active_edge_count",
        )?)
        .bind(revision_to_i64(
            catalog.bound_document_count(),
            "bound_document_count",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(prepared.command_event.pubkey.as_bytes())
        .bind(catalog.updated_at())
        .bind(prepared.meta_projection.id.as_bytes().as_slice())
        .bind(revision_to_i64(
            loaded.catalog.context_revision(),
            "expected context_revision",
        )?)
        .bind(revision_to_i64(
            loaded.catalog.projection_generation(),
            "projection_generation",
        )?)
        .bind(loaded.projection_pubkey.as_bytes())
        .execute(&mut *self.tx)
        .await?;
        if update.rows_affected() != 1 {
            return Err(ProjectContextWriteError::InvalidCommit(
                "Project Context catalog changed while committing".to_owned(),
            ));
        }
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *self.tx)
            .await?;
        self.tx.commit().await?;
        Ok(ProjectContextCommitOutcome {
            receipt,
            replayed: false,
        })
    }
}

async fn load_normalized_coordinates(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    edge_key: EdgeKey,
) -> ProjectContextWriteResult<Vec<ProjectContextCoordinate>> {
    let rows = sqlx::query(
        "SELECT ordinal, coordinate_type, coordinate_subtype, coordinate_id, canonical_key \
         FROM project_context_edge_coordinates \
         WHERE community_id = $1 AND edge_key = $2 ORDER BY ordinal FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(edge_key.as_bytes().as_slice())
    .fetch_all(&mut **tx)
    .await?;
    let mut coordinates = Vec::with_capacity(rows.len());
    for (expected_ordinal, row) in rows.into_iter().enumerate() {
        let ordinal: i32 = row.try_get("ordinal")?;
        if usize::try_from(ordinal).ok() != Some(expected_ordinal) {
            return Err(ProjectContextWriteError::InvalidCommit(
                "normalized Project Context coordinate ordinals are not contiguous".to_owned(),
            ));
        }
        let coordinate_type: String = row.try_get("coordinate_type")?;
        let coordinate_subtype: Option<String> = row.try_get("coordinate_subtype")?;
        let coordinate_id: Uuid = row.try_get("coordinate_id")?;
        let coordinate = match (coordinate_type.as_str(), coordinate_subtype.as_deref()) {
            ("project_view_object", Some(subtype)) => {
                let object_type = project_view_object_type_from_str(subtype).ok_or_else(|| {
                    ProjectContextWriteError::InvalidCommit(format!(
                        "unsupported Project View coordinate subtype '{subtype}'"
                    ))
                })?;
                ProjectContextCoordinate::ProjectViewObject {
                    object_type,
                    object_id: coordinate_id,
                }
            }
            ("document", None) => ProjectContextCoordinate::Document {
                document_id: coordinate_id,
            },
            _ => {
                return Err(ProjectContextWriteError::InvalidCommit(
                    "normalized Project Context coordinate has an invalid closed shape".to_owned(),
                ));
            }
        };
        let canonical_key: String = row.try_get("canonical_key")?;
        if canonical_key != coordinate.tag_value(*community_id.as_uuid()) {
            return Err(ProjectContextWriteError::InvalidCommit(
                "normalized Project Context coordinate key is not canonical".to_owned(),
            ));
        }
        coordinates.push(coordinate);
    }
    Ok(coordinates)
}

async fn coordinate_active_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    coordinate: &ProjectContextCoordinate,
) -> ProjectContextWriteResult<bool> {
    match coordinate {
        ProjectContextCoordinate::ProjectViewObject {
            object_type,
            object_id,
        } => Ok(sqlx::query_scalar(
            "SELECT deleted_at IS NULL AND object_type = $3 \
             FROM project_view_objects \
             WHERE community_id = $1 AND object_id = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(object_id)
        .bind(object_type.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(false)),
        ProjectContextCoordinate::Document { document_id } => Ok(sqlx::query_scalar(
            "SELECT state = 'active' FROM project_documents \
             WHERE community_id = $1 AND document_id = $2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(document_id)
        .fetch_optional(&mut **tx)
        .await?
        .unwrap_or(false)),
    }
}

async fn require_bootstrap_prerequisites(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    expected_pubkey: &PublicKey,
) -> ProjectContextWriteResult<()> {
    let ready: Option<bool> = sqlx::query_scalar(
        "SELECT c.archived_at IS NULL \
                AND c.project_view_schema_version = 3 \
                AND c.project_view_enabled \
                AND c.project_document_enabled \
                AND maintenance.state = 'normal' \
                AND view_state.schema_version = 3 \
                AND document_state.schema_version = 1 \
                AND view_state.projection_pubkey = $2 \
                AND document_state.projection_pubkey = $2 \
         FROM communities c \
         JOIN project_view_maintenance maintenance ON maintenance.community_id = c.id \
         JOIN project_view_state view_state ON view_state.community_id = c.id \
         JOIN project_document_state document_state ON document_state.community_id = c.id \
         WHERE c.id = $1 \
         FOR UPDATE OF c, maintenance, view_state, document_state",
    )
    .bind(community_id.as_uuid())
    .bind(expected_pubkey.as_bytes())
    .fetch_optional(&mut **tx)
    .await?;
    if ready != Some(true)
        || !Db::project_view_v3_structural_ready_in_tx(tx, community_id, expected_pubkey).await?
    {
        return Err(ProjectContextWriteError::Unavailable { community_id });
    }
    sqlx::query("SELECT project_document_validate_community($1)")
        .bind(community_id.as_uuid())
        .execute(&mut **tx)
        .await?;
    if !crate::project_document::document_projection_parity(
        tx,
        community_id,
        expected_pubkey,
        None,
        None,
    )
    .await?
    {
        return Err(ProjectContextWriteError::Unavailable { community_id });
    }
    Ok(())
}

async fn validate_actor_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    actor: PublicKey,
    command: &ProjectContextCommand,
) -> ProjectContextWriteResult<()> {
    crate::project_document::validate_actor_in_tx(
        tx,
        community_id,
        actor,
        command.acting_assignment_id,
        command.runtime_fence,
    )
    .await
    .map_err(|error| match error {
        crate::project_document::ProjectDocumentWriteError::Database(error) => {
            ProjectContextWriteError::Database(error)
        }
        crate::project_document::ProjectDocumentWriteError::Sqlx(error) => {
            ProjectContextWriteError::Sqlx(error)
        }
        crate::project_document::ProjectDocumentWriteError::NotAuthorized => {
            ProjectContextWriteError::NotAuthorized
        }
        crate::project_document::ProjectDocumentWriteError::ActingAssignmentInvalid => {
            ProjectContextWriteError::ActingAssignmentInvalid
        }
        crate::project_document::ProjectDocumentWriteError::RuntimeFence => {
            ProjectContextWriteError::RuntimeFence
        }
        other => ProjectContextWriteError::InvalidCommit(format!(
            "unexpected Project Context actor validation failure: {other}"
        )),
    })
}

fn validate_bootstrap(prepared: &PreparedProjectContextBootstrap) -> ProjectContextWriteResult<()> {
    prepared.catalog.validate()?;
    if prepared.catalog.context_revision() != 0
        || prepared.catalog.active_edge_count() != 0
        || prepared.catalog.bound_document_count() != 0
        || prepared.catalog.projection_generation() != 1
        || prepared.catalog.initialized_at() != prepared.catalog.updated_at()
    {
        return Err(ProjectContextWriteError::InvalidCommit(
            "bootstrap catalog must be an empty generation-one revision-zero catalog".to_owned(),
        ));
    }
    let verified = parse_project_context_meta(
        &prepared.meta_projection,
        &prepared.meta_projection.pubkey,
        prepared.catalog.project_id(),
    )
    .map_err(|error| ProjectContextWriteError::InvalidCommit(error.to_string()))?;
    let projection = verified.projection;
    if projection.project_id != *prepared.catalog.project_id().as_uuid()
        || projection.projection_generation != 1
        || projection.context_revision != 0
        || projection.active_edge_count != 0
        || projection.bound_document_count != 0
        || !projection.reset
        || !projection.changed_bindings.is_empty()
        || projection.source_event_id.is_some()
        || projection.updated_at != prepared.catalog.updated_at()
    {
        return Err(ProjectContextWriteError::InvalidCommit(
            "bootstrap metadata does not exactly represent the empty catalog".to_owned(),
        ));
    }
    Ok(())
}

fn state_metadata_from_row(
    row: &sqlx::postgres::PgRow,
) -> ProjectContextWriteResult<ProjectContextStateMetadata> {
    let last_change: Option<Vec<u8>> = row.try_get("last_change_id")?;
    let last_actor: Option<Vec<u8>> = row.try_get("last_actor_pubkey")?;
    let projection_pubkey: Vec<u8> = row.try_get("projection_pubkey")?;
    let meta_event_id: Vec<u8> = row.try_get("meta_projection_event_id")?;
    Ok(ProjectContextStateMetadata {
        context_revision: db_nonnegative_revision(
            row.try_get::<i64, _>("context_revision")?,
            "context_revision",
        )?,
        active_edge_count: db_nonnegative_revision(
            row.try_get::<i64, _>("active_edge_count")?,
            "active_edge_count",
        )?,
        bound_document_count: db_nonnegative_revision(
            row.try_get::<i64, _>("bound_document_count")?,
            "bound_document_count",
        )?,
        last_change_id: last_change
            .map(|bytes| event_id_from_bytes(&bytes, "last_change_id"))
            .transpose()?,
        last_actor_pubkey: last_actor
            .map(|bytes| public_key_from_bytes(&bytes, "last_actor_pubkey"))
            .transpose()?,
        projection_pubkey: public_key_from_bytes(&projection_pubkey, "projection_pubkey")?,
        projection_generation: db_positive_revision(
            row.try_get::<i64, _>("projection_generation")?,
            "projection_generation",
        )?,
        meta_projection_event_id: event_id_from_bytes(&meta_event_id, "meta_projection_event_id")?,
        initialized_at: row.try_get("initialized_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn status_from_row(row: sqlx::postgres::PgRow) -> crate::Result<ProjectContextFeatureStatus> {
    let optional_nonnegative = |field: &str| -> crate::Result<Option<u64>> {
        row.try_get::<Option<i64>, _>(field)?
            .map(|value| db_nonnegative_revision_db(value, field))
            .transpose()
    };
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
    Ok(ProjectContextFeatureStatus {
        community_id: CommunityId::from_uuid(row.try_get("id")?),
        host: row.try_get("host")?,
        archived: row.try_get("archived")?,
        enabled: row.try_get("project_context_edge_enabled")?,
        project_view_schema_version: row.try_get("project_view_schema_version")?,
        project_view_enabled: row.try_get("project_view_enabled")?,
        project_document_enabled: row.try_get("project_document_enabled")?,
        maintenance_state: row.try_get("maintenance_state")?,
        context_revision: optional_nonnegative("context_revision")?,
        active_edge_count: optional_nonnegative("active_edge_count")?,
        bound_document_count: optional_nonnegative("bound_document_count")?,
        projection_generation,
        projection_pubkey,
        edge_row_count: db_nonnegative_revision_db(
            row.try_get::<i64, _>("edge_row_count")?,
            "edge_row_count",
        )?,
        binding_row_count: db_nonnegative_revision_db(
            row.try_get::<i64, _>("binding_row_count")?,
            "binding_row_count",
        )?,
        change_count: db_nonnegative_revision_db(
            row.try_get::<i64, _>("change_count")?,
            "change_count",
        )?,
    })
}

async fn context_integrity_status_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
) -> crate::Result<Option<ProjectContextIntegrityStatus>> {
    let row = sqlx::query(
        "WITH state AS ( \
             SELECT projection_pubkey, meta_projection_event_id \
             FROM project_context_edge_state WHERE community_id = $1), \
         mismatches AS ( \
             SELECT 1 FROM state s WHERE NOT EXISTS ( \
                 SELECT 1 FROM events event WHERE event.community_id = $1 \
                   AND event.id = s.meta_projection_event_id AND event.kind = $2 \
                   AND event.pubkey = s.projection_pubkey AND event.deleted_at IS NULL) \
             UNION ALL \
             SELECT 1 FROM project_context_document_bindings binding CROSS JOIN state s \
             WHERE binding.community_id = $1 AND NOT EXISTS ( \
                 SELECT 1 FROM events event \
                 WHERE event.community_id = binding.community_id \
                   AND event.id = binding.current_projection_event_id AND event.kind = $3 \
                   AND event.pubkey = s.projection_pubkey AND event.deleted_at IS NULL)), \
         active_pointers AS ( \
             SELECT meta_projection_event_id AS event_id FROM state \
             UNION SELECT current_projection_event_id \
             FROM project_context_document_bindings WHERE community_id = $1) \
         SELECT EXISTS (SELECT 1 FROM state) AS initialized, \
                (SELECT count(*)::bigint FROM mismatches) AS pointer_mismatch_count, \
                (SELECT count(*)::bigint FROM events event \
                 WHERE event.community_id = $1 AND event.kind IN ($2, $3) \
                   AND event.deleted_at IS NULL \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM active_pointers pointer WHERE pointer.event_id = event.id)) \
                   AS orphan_projection_count",
    )
    .bind(community_id.as_uuid())
    .bind(KIND_PROJECT_CONTEXT_META as i32)
    .bind(KIND_PROJECT_CONTEXT_EDGE_BINDING as i32)
    .fetch_one(&mut **tx)
    .await?;
    if !row.try_get::<bool, _>("initialized")? {
        return Ok(None);
    }
    Ok(Some(ProjectContextIntegrityStatus {
        orphan_projection_count: db_nonnegative_revision_db(
            row.try_get("orphan_projection_count")?,
            "orphan_projection_count",
        )?,
        pointer_mismatch_count: db_nonnegative_revision_db(
            row.try_get("pointer_mismatch_count")?,
            "pointer_mismatch_count",
        )?,
    }))
}

async fn append_context_control_audit(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    operation: &'static str,
) -> ProjectContextWriteResult<()> {
    buzz_audit::append_in_transaction(
        tx,
        NewAuditEntry {
            community_id,
            action: AuditAction::ProjectContextEdgeControl,
            actor_pubkey: None,
            object_id: Some(community_id.to_string()),
            detail: serde_json::json!({ "operation": operation }),
        },
    )
    .await?;
    Ok(())
}

fn public_key_from_bytes(bytes: &[u8], field: &str) -> ProjectContextWriteResult<PublicKey> {
    PublicKey::from_slice(bytes).map_err(|error| {
        ProjectContextWriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn event_id_from_bytes(bytes: &[u8], field: &str) -> ProjectContextWriteResult<EventId> {
    EventId::from_slice(bytes).map_err(|error| {
        ProjectContextWriteError::InvalidCommit(format!("invalid {field}: {error}"))
    })
}

fn edge_key_from_bytes(bytes: &[u8]) -> ProjectContextWriteResult<EdgeKey> {
    if bytes.len() != 32 {
        return Err(ProjectContextWriteError::InvalidCommit(
            "edge_key must contain 32 bytes".to_owned(),
        ));
    }
    EdgeKey::from_hex(&hex::encode(bytes)).map_err(ProjectContextWriteError::Domain)
}

fn revision_to_i64(value: u64, field: &str) -> ProjectContextWriteResult<i64> {
    if value > MAX_SAFE_REVISION {
        return Err(ProjectContextWriteError::InvalidCommit(format!(
            "{field} exceeds the JSON-safe revision limit"
        )));
    }
    i64::try_from(value).map_err(|_| {
        ProjectContextWriteError::InvalidCommit(format!("{field} does not fit PostgreSQL BIGINT"))
    })
}

fn db_nonnegative_revision(value: i64, field: &str) -> ProjectContextWriteResult<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value <= MAX_SAFE_REVISION)
        .ok_or_else(|| {
            ProjectContextWriteError::InvalidCommit(format!(
                "{field} is outside the JSON-safe revision range"
            ))
        })
}

fn db_positive_revision(value: i64, field: &str) -> ProjectContextWriteResult<u64> {
    db_nonnegative_revision(value, field).and_then(|value| {
        if value == 0 {
            Err(ProjectContextWriteError::InvalidCommit(format!(
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

async fn find_receipt(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    change_id: &[u8],
) -> ProjectContextWriteResult<Option<ProjectContextReceipt>> {
    let row = sqlx::query(
        "SELECT source_type, source_event_id, actor_pubkey, acting_assignment_id, \
                operation, expected_context_revision, context_revision, edge_key, \
                edge_state, edge_document_count, context_document_id, \
                canonical_coordinates, result, accepted_at \
         FROM project_context_edge_changes \
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
    let source_event_id: Vec<u8> = row.try_get("source_event_id")?;
    if source_type != "nostr_event" || source_event_id.as_slice() != change_id {
        return Err(ProjectContextWriteError::InvalidCommit(
            "stored Project Context receipt has an invalid source shape".to_owned(),
        ));
    }
    let receipt: ProjectContextReceipt =
        serde_json::from_value(row.try_get("result")?).map_err(|error| {
            ProjectContextWriteError::InvalidCommit(format!(
                "invalid stored Project Context receipt: {error}"
            ))
        })?;
    receipt.validate()?;
    let actor: Vec<u8> = row.try_get("actor_pubkey")?;
    let acting_assignment_id: Option<Uuid> = row.try_get("acting_assignment_id")?;
    let operation: String = row.try_get("operation")?;
    let expected_context_revision = db_nonnegative_revision(
        row.try_get::<i64, _>("expected_context_revision")?,
        "expected_context_revision",
    )?;
    let context_revision = db_positive_revision(
        row.try_get::<i64, _>("context_revision")?,
        "context_revision",
    )?;
    let edge_key_bytes: Vec<u8> = row.try_get("edge_key")?;
    let edge_key = edge_key_from_bytes(&edge_key_bytes)?;
    let edge_state: String = row.try_get("edge_state")?;
    let edge_document_count = db_nonnegative_revision(
        row.try_get::<i64, _>("edge_document_count")?,
        "edge_document_count",
    )?;
    let context_document_id: Uuid = row.try_get("context_document_id")?;
    let coordinates: Vec<ProjectContextCoordinate> = serde_json::from_value(
        row.try_get::<Value, _>("canonical_coordinates")?,
    )
    .map_err(|error| {
        ProjectContextWriteError::InvalidCommit(format!(
            "invalid stored Project Context coordinates: {error}"
        ))
    })?;
    let accepted_at: DateTime<Utc> = row.try_get("accepted_at")?;
    if EdgeKey::derive(*community_id.as_uuid(), &coordinates)? != edge_key
        || receipt.change_id.as_bytes() != change_id
        || receipt.actor.as_bytes() != actor.as_slice()
        || receipt.acting_assignment_id != acting_assignment_id
        || receipt.operation.as_str() != operation
        || receipt.expected_context_revision != expected_context_revision
        || receipt.context_revision != context_revision
        || receipt.edge_key != edge_key
        || receipt.edge_state.as_str() != edge_state
        || receipt.edge_document_count != edge_document_count
        || receipt.context_document_id != context_document_id
        || receipt.accepted_at != accepted_at
    {
        return Err(ProjectContextWriteError::InvalidCommit(
            "stored Project Context receipt columns and closed JSON result disagree".to_owned(),
        ));
    }
    Ok(Some(receipt))
}

fn validate_replayed_receipt(
    command_event: &Event,
    command: &ProjectContextCommand,
    receipt: &ProjectContextReceipt,
    community_id: CommunityId,
) -> ProjectContextWriteResult<()> {
    command.validate_for_project(*community_id.as_uuid())?;
    let edge_key = EdgeKey::derive(*community_id.as_uuid(), command.coordinates())?;
    if receipt.change_id != command_event.id
        || receipt.actor != command_event.pubkey
        || receipt.acting_assignment_id != command.acting_assignment_id
        || receipt.operation != command.operation()
        || receipt.expected_context_revision != command.expected_context_revision
        || receipt.edge_key != edge_key
        || receipt.context_document_id != command.context_document_id()
    {
        return Err(ProjectContextWriteError::InvalidCommit(
            "replayed command does not match its durable Project Context receipt".to_owned(),
        ));
    }
    Ok(())
}

async fn insert_change(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    command_event: &Event,
    command: &ProjectContextCommand,
    receipt: &ProjectContextReceipt,
    receipt_result: &Value,
) -> ProjectContextWriteResult<()> {
    let coordinates = serde_json::to_value(command.coordinates()).map_err(|error| {
        ProjectContextWriteError::InvalidCommit(format!(
            "serialize canonical Project Context coordinates: {error}"
        ))
    })?;
    sqlx::query(
        "INSERT INTO project_context_edge_changes \
            (community_id, change_id, source_type, source_event_id, actor_pubkey, \
             acting_assignment_id, operation, expected_context_revision, context_revision, \
             edge_key, edge_state, edge_document_count, context_document_id, \
             canonical_coordinates, result, accepted_at) \
         VALUES ($1, $2, 'nostr_event', $2, $3, $4, $5, $6, $7, $8, $9, $10, \
                 $11, $12, $13, $14)",
    )
    .bind(community_id.as_uuid())
    .bind(command_event.id.as_bytes().as_slice())
    .bind(command_event.pubkey.as_bytes())
    .bind(command.acting_assignment_id)
    .bind(command.operation().as_str())
    .bind(revision_to_i64(
        command.expected_context_revision,
        "expected_context_revision",
    )?)
    .bind(revision_to_i64(
        receipt.context_revision,
        "context_revision",
    )?)
    .bind(receipt.edge_key.as_bytes().as_slice())
    .bind(receipt.edge_state.as_str())
    .bind(revision_to_i64(
        receipt.edge_document_count,
        "edge_document_count",
    )?)
    .bind(receipt.context_document_id)
    .bind(coordinates)
    .bind(receipt_result)
    .bind(receipt.accepted_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn write_edge(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    prepared: &PreparedProjectContextCommit,
    previous_revision: Option<u64>,
) -> ProjectContextWriteResult<()> {
    let transition = &prepared.transition;
    let binding = transition.binding();
    let edge_state = if transition.edge().is_some() {
        ProjectContextBindingState::Active
    } else {
        ProjectContextBindingState::Deleted
    };
    let coordinates = serde_json::to_value(&binding.coordinates).map_err(|error| {
        ProjectContextWriteError::InvalidCommit(format!(
            "serialize canonical Project Context coordinates: {error}"
        ))
    })?;
    let actor = prepared.command_event.pubkey.to_bytes();
    if let Some(previous_revision) = previous_revision {
        let result = sqlx::query(
            "UPDATE project_context_edges \
             SET state = $3, last_context_revision = $4, current_source_change_id = $5, \
                 updated_at = $6, updated_by = $7 \
             WHERE community_id = $1 AND edge_key = $2 AND last_context_revision = $8",
        )
        .bind(community_id.as_uuid())
        .bind(binding.edge_key.as_bytes().as_slice())
        .bind(edge_state.as_str())
        .bind(revision_to_i64(
            transition.catalog().context_revision(),
            "context_revision",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(binding.updated_at)
        .bind(actor.as_slice())
        .bind(revision_to_i64(
            previous_revision,
            "previous edge revision",
        )?)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectContextWriteError::InvalidCommit(
                "Project Context edge changed while committing".to_owned(),
            ));
        }
    } else {
        if transition.edge().is_none() || binding.state != ProjectContextBindingState::Active {
            return Err(ProjectContextWriteError::InvalidCommit(
                "a new Project Context edge must be created by an active attach".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO project_context_edges \
                (community_id, edge_key, state, canonical_coordinates, \
                 last_context_revision, current_source_change_id, updated_at, updated_by) \
             VALUES ($1, $2, 'active', $3, $4, $5, $6, $7)",
        )
        .bind(community_id.as_uuid())
        .bind(binding.edge_key.as_bytes().as_slice())
        .bind(coordinates)
        .bind(revision_to_i64(
            transition.catalog().context_revision(),
            "context_revision",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(binding.updated_at)
        .bind(actor.as_slice())
        .execute(&mut **tx)
        .await?;
        insert_coordinates(tx, community_id, binding.edge_key, &binding.coordinates).await?;
    }
    Ok(())
}

async fn insert_coordinates(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    edge_key: EdgeKey,
    coordinates: &[ProjectContextCoordinate],
) -> ProjectContextWriteResult<()> {
    for (ordinal, coordinate) in coordinates.iter().enumerate() {
        let ordinal = i32::try_from(ordinal).map_err(|_| {
            ProjectContextWriteError::InvalidCommit(
                "Project Context coordinate ordinal does not fit INTEGER".to_owned(),
            )
        })?;
        let (coordinate_type, coordinate_subtype, coordinate_id) = match coordinate {
            ProjectContextCoordinate::ProjectViewObject {
                object_type,
                object_id,
            } => (
                "project_view_object",
                Some(object_type.as_str()),
                *object_id,
            ),
            ProjectContextCoordinate::Document { document_id } => ("document", None, *document_id),
        };
        sqlx::query(
            "INSERT INTO project_context_edge_coordinates \
                (community_id, edge_key, ordinal, coordinate_type, coordinate_subtype, \
                 coordinate_id, canonical_key) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(community_id.as_uuid())
        .bind(edge_key.as_bytes().as_slice())
        .bind(ordinal)
        .bind(coordinate_type)
        .bind(coordinate_subtype)
        .bind(coordinate_id)
        .bind(coordinate.tag_value(*community_id.as_uuid()))
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

async fn write_binding(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    prepared: &PreparedProjectContextCommit,
    previous_revision: Option<u64>,
) -> ProjectContextWriteResult<()> {
    let binding = prepared.transition.binding();
    let actor = prepared.command_event.pubkey.to_bytes();
    if let Some(previous_revision) = previous_revision {
        let result = sqlx::query(
            "UPDATE project_context_document_bindings \
             SET edge_key = $3, state = $4, binding_context_revision = $5, \
                 current_source_change_id = $6, current_projection_event_id = $7, \
                 updated_at = $8, updated_by = $9 \
             WHERE community_id = $1 AND context_document_id = $2 \
               AND binding_context_revision = $10",
        )
        .bind(community_id.as_uuid())
        .bind(binding.context_document_id)
        .bind(binding.edge_key.as_bytes().as_slice())
        .bind(binding.state.as_str())
        .bind(revision_to_i64(
            binding.context_revision,
            "binding_context_revision",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(prepared.binding_projection.id.as_bytes().as_slice())
        .bind(binding.updated_at)
        .bind(actor.as_slice())
        .bind(revision_to_i64(
            previous_revision,
            "previous binding revision",
        )?)
        .execute(&mut **tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(ProjectContextWriteError::InvalidCommit(
                "Project Context binding changed while committing".to_owned(),
            ));
        }
    } else {
        if binding.state != ProjectContextBindingState::Active {
            return Err(ProjectContextWriteError::InvalidCommit(
                "a new Project Context binding must be active".to_owned(),
            ));
        }
        sqlx::query(
            "INSERT INTO project_context_document_bindings \
                (community_id, context_document_id, edge_key, state, \
                 binding_context_revision, current_source_change_id, \
                 current_projection_event_id, updated_at, updated_by) \
             VALUES ($1, $2, $3, 'active', $4, $5, $6, $7, $8)",
        )
        .bind(community_id.as_uuid())
        .bind(binding.context_document_id)
        .bind(binding.edge_key.as_bytes().as_slice())
        .bind(revision_to_i64(
            binding.context_revision,
            "binding_context_revision",
        )?)
        .bind(prepared.command_event.id.as_bytes().as_slice())
        .bind(prepared.binding_projection.id.as_bytes().as_slice())
        .bind(binding.updated_at)
        .bind(actor.as_slice())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

fn validate_projection_kinds(
    prepared: &PreparedProjectContextCommit,
) -> ProjectContextWriteResult<()> {
    for (label, event, expected_kind) in [
        (
            "binding",
            &prepared.binding_projection,
            KIND_PROJECT_CONTEXT_EDGE_BINDING,
        ),
        (
            "metadata",
            &prepared.meta_projection,
            KIND_PROJECT_CONTEXT_META,
        ),
    ] {
        if u32::from(event.kind.as_u16()) != expected_kind {
            return Err(ProjectContextWriteError::InvalidCommit(format!(
                "{label} projection kind must be {expected_kind}"
            )));
        }
    }
    Ok(())
}

pub(crate) async fn context_projection_parity(
    connection: &mut sqlx::PgConnection,
    community_id: CommunityId,
    expected_pubkey: &PublicKey,
) -> crate::Result<bool> {
    let state = sqlx::query(
        "SELECT context_revision, active_edge_count, bound_document_count, \
                last_change_id, projection_generation, meta_projection_event_id, updated_at \
         FROM project_context_edge_state WHERE community_id = $1",
    )
    .bind(community_id.as_uuid())
    .fetch_optional(&mut *connection)
    .await?;
    let Some(state) = state else {
        return Ok(false);
    };
    let context_revision: i64 = state.try_get("context_revision")?;
    let active_edge_count: i64 = state.try_get("active_edge_count")?;
    let bound_document_count: i64 = state.try_get("bound_document_count")?;
    let last_change_id: Option<Vec<u8>> = state.try_get("last_change_id")?;
    let projection_generation: i64 = state.try_get("projection_generation")?;
    let meta_event_id: Vec<u8> = state.try_get("meta_projection_event_id")?;
    let updated_at: DateTime<Utc> = state.try_get("updated_at")?;
    if context_revision < 0
        || active_edge_count < 0
        || bound_document_count < 0
        || projection_generation <= 0
        || meta_event_id.len() != 32
    {
        return Ok(false);
    }
    let Some(meta_event) = context_event_by_id(connection, community_id, &meta_event_id).await?
    else {
        return Ok(false);
    };
    let Ok(meta) = parse_project_context_meta(&meta_event.event, expected_pubkey, community_id)
    else {
        return Ok(false);
    };
    if i64::try_from(meta.projection.context_revision).ok() != Some(context_revision)
        || i64::try_from(meta.projection.active_edge_count).ok() != Some(active_edge_count)
        || i64::try_from(meta.projection.bound_document_count).ok() != Some(bound_document_count)
        || i64::try_from(meta.projection.projection_generation).ok() != Some(projection_generation)
        || meta.projection.updated_at != updated_at
        || (context_revision == 0 && !meta.projection.reset)
        || (!meta.projection.reset
            && meta
                .projection
                .source_event_id
                .as_ref()
                .map(|event_id| event_id.as_bytes().as_slice())
                != last_change_id.as_deref())
    {
        return Ok(false);
    }

    let edge_rows = sqlx::query(
        "SELECT edge_key, state, canonical_coordinates \
         FROM project_context_edges WHERE community_id = $1 ORDER BY edge_key",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut *connection)
    .await?;
    let mut edges = BTreeMap::new();
    for row in edge_rows {
        let key_bytes: Vec<u8> = row.try_get("edge_key")?;
        let Ok(edge_key) = edge_key_from_bytes(&key_bytes) else {
            return Ok(false);
        };
        let state: String = row.try_get("state")?;
        if !matches!(state.as_str(), "active" | "deleted") {
            return Ok(false);
        }
        let Ok(coordinates) = serde_json::from_value::<Vec<ProjectContextCoordinate>>(
            row.try_get::<Value, _>("canonical_coordinates")?,
        ) else {
            return Ok(false);
        };
        if EdgeKey::derive(*community_id.as_uuid(), &coordinates).ok() != Some(edge_key) {
            return Ok(false);
        }
        let coordinate_rows = sqlx::query(
            "SELECT ordinal, coordinate_type, coordinate_subtype, coordinate_id, canonical_key \
             FROM project_context_edge_coordinates \
             WHERE community_id = $1 AND edge_key = $2 ORDER BY ordinal",
        )
        .bind(community_id.as_uuid())
        .bind(edge_key.as_bytes().as_slice())
        .fetch_all(&mut *connection)
        .await?;
        let mut normalized = Vec::with_capacity(coordinate_rows.len());
        for (expected_ordinal, coordinate_row) in coordinate_rows.into_iter().enumerate() {
            let ordinal: i32 = coordinate_row.try_get("ordinal")?;
            if usize::try_from(ordinal).ok() != Some(expected_ordinal) {
                return Ok(false);
            }
            let coordinate_type: String = coordinate_row.try_get("coordinate_type")?;
            let coordinate_subtype: Option<String> =
                coordinate_row.try_get("coordinate_subtype")?;
            let coordinate_id: Uuid = coordinate_row.try_get("coordinate_id")?;
            let coordinate = match (coordinate_type.as_str(), coordinate_subtype.as_deref()) {
                ("project_view_object", Some(subtype)) => {
                    let Some(object_type) = project_view_object_type_from_str(subtype) else {
                        return Ok(false);
                    };
                    ProjectContextCoordinate::ProjectViewObject {
                        object_type,
                        object_id: coordinate_id,
                    }
                }
                ("document", None) => ProjectContextCoordinate::Document {
                    document_id: coordinate_id,
                },
                _ => return Ok(false),
            };
            let canonical_key: String = coordinate_row.try_get("canonical_key")?;
            if canonical_key != coordinate.tag_value(*community_id.as_uuid()) {
                return Ok(false);
            }
            normalized.push(coordinate);
        }
        if normalized != coordinates || edges.insert(edge_key, (state, coordinates)).is_some() {
            return Ok(false);
        }
    }

    let binding_rows = sqlx::query(
        "SELECT context_document_id, edge_key, state, binding_context_revision, \
                current_source_change_id, current_projection_event_id, updated_at \
         FROM project_context_document_bindings \
         WHERE community_id = $1 ORDER BY context_document_id",
    )
    .bind(community_id.as_uuid())
    .fetch_all(&mut *connection)
    .await?;
    let mut active_bindings = 0_i64;
    let mut active_members_by_edge: BTreeMap<EdgeKey, u64> = BTreeMap::new();
    let mut current_incremental_binding_verified = meta.projection.reset;
    for row in binding_rows {
        let context_document_id: Uuid = row.try_get("context_document_id")?;
        let key_bytes: Vec<u8> = row.try_get("edge_key")?;
        let Ok(edge_key) = edge_key_from_bytes(&key_bytes) else {
            return Ok(false);
        };
        let Some((edge_state, coordinates)) = edges.get(&edge_key) else {
            return Ok(false);
        };
        let state_text: String = row.try_get("state")?;
        let Some(binding_state) = binding_state_from_str(&state_text) else {
            return Ok(false);
        };
        if binding_state == ProjectContextBindingState::Active {
            if edge_state != "active" {
                return Ok(false);
            }
            active_bindings += 1;
            *active_members_by_edge.entry(edge_key).or_default() += 1;
        }
        let binding_context_revision: i64 = row.try_get("binding_context_revision")?;
        let source_event_id: Vec<u8> = row.try_get("current_source_change_id")?;
        let projection_event_id: Vec<u8> = row.try_get("current_projection_event_id")?;
        let binding_updated_at: DateTime<Utc> = row.try_get("updated_at")?;
        let Some(binding_event) =
            context_event_by_id(connection, community_id, &projection_event_id).await?
        else {
            return Ok(false);
        };
        let Ok(binding) =
            parse_project_context_binding(&binding_event.event, expected_pubkey, community_id)
        else {
            return Ok(false);
        };
        if binding.projection.context_document_id != context_document_id
            || binding.projection.edge_key != edge_key
            || &binding.projection.coordinates != coordinates
            || binding.projection.state != binding_state
            || i64::try_from(binding.projection.context_revision).ok()
                != Some(binding_context_revision)
            || binding.projection.source_event_id.as_bytes() != source_event_id.as_slice()
            || binding.projection.updated_at != binding_updated_at
            || i64::try_from(binding.projection.projection_generation).ok()
                != Some(projection_generation)
            || verify_project_context_binding_observation(&meta, &binding).is_err()
        {
            return Ok(false);
        }
        if !meta.projection.reset
            && binding.projection.context_revision == meta.projection.context_revision
        {
            current_incremental_binding_verified = true;
        }
    }
    let actual_active_edges = edges
        .iter()
        .filter(|(edge_key, (state, _))| {
            if state.as_str() == "active" {
                active_members_by_edge.get(edge_key).copied().unwrap_or(0) > 0
            } else {
                active_members_by_edge.get(edge_key).copied().unwrap_or(0) == 0
            }
        })
        .filter(|(_, (state, _))| state.as_str() == "active")
        .count();
    let all_edge_lifecycles_valid = edges.iter().all(|(edge_key, (state, _))| {
        let active_members = active_members_by_edge.get(edge_key).copied().unwrap_or(0);
        (state == "active" && active_members > 0) || (state == "deleted" && active_members == 0)
    });
    if !all_edge_lifecycles_valid
        || i64::try_from(actual_active_edges).ok() != Some(active_edge_count)
        || active_bindings != bound_document_count
        || !current_incremental_binding_verified
    {
        return Ok(false);
    }
    Ok(true)
}

async fn context_event_by_id(
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

fn binding_state_from_str(value: &str) -> Option<ProjectContextBindingState> {
    match value {
        "active" => Some(ProjectContextBindingState::Active),
        "deleted" => Some(ProjectContextBindingState::Deleted),
        _ => None,
    }
}

fn project_view_object_type_from_str(value: &str) -> Option<ProjectViewObjectType> {
    match value {
        "project_profile" => Some(ProjectViewObjectType::ProjectProfile),
        "goal" => Some(ProjectViewObjectType::Goal),
        "role" => Some(ProjectViewObjectType::Role),
        "plan" => Some(ProjectViewObjectType::Plan),
        "stage" => Some(ProjectViewObjectType::Stage),
        "requirement" => Some(ProjectViewObjectType::Requirement),
        "issue" => Some(ProjectViewObjectType::Issue),
        "work" => Some(ProjectViewObjectType::Work),
        "resource" => Some(ProjectViewObjectType::Resource),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use buzz_project_context::{ProjectContextChangeContext, ProjectContextProjectionPlan};
    use buzz_project_document::{
        reduce_document, DocumentCatalog, DocumentChangeContext, DocumentCommandRequest,
        DocumentError, DocumentProjectionPlan, ProjectDocumentCommand,
    };
    use buzz_sdk::project_context::{
        build_project_context_binding_projection, build_project_context_binding_reprojection,
        build_project_context_command, build_project_context_meta_projection,
        changed_project_context_binding_for,
    };
    use buzz_sdk::project_document::{
        build_document_command, build_document_head_projection, build_document_meta_projection,
        build_document_revision_projection, changed_head_for,
    };
    use nostr::{EventBuilder, Keys, Kind};
    use sqlx::{Executor, PgPool};

    use crate::project_document::{
        PreparedProjectDocumentBootstrap, PreparedProjectDocumentCommit,
        ProjectDocumentCommitOutcome, ProjectDocumentWriteContext, ProjectDocumentWriteTx,
    };

    struct ScratchDatabase {
        admin: PgPool,
        pool: PgPool,
        name: String,
    }

    impl ScratchDatabase {
        async fn create(prefix: &str) -> Self {
            assert!(
                prefix.starts_with("buzz_"),
                "Project Context scratch database prefixes must start with buzz_"
            );
            let admin_url = std::env::var("BUZZ_TEST_DATABASE_URL").expect(
                "Project Context database tests require an explicit BUZZ_TEST_DATABASE_URL",
            );
            let admin_database_name = admin_url
                .rsplit('/')
                .next()
                .and_then(|tail| tail.split(['?', '#']).next())
                .filter(|name| !name.is_empty())
                .expect("BUZZ_TEST_DATABASE_URL must include a database name");
            assert!(
                admin_database_name.starts_with("buzz_"),
                "Project Context database tests require a disposable buzz_ administrative database; refused {admin_database_name}"
            );
            let admin = PgPool::connect(&admin_url)
                .await
                .expect("connect test database server");
            let name = format!("{prefix}_{}", Uuid::new_v4().simple());
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
                .execute(&admin)
                .await
                .expect("create Project Context scratch database");
            let slash = admin_url.rfind('/').expect("database URL has path");
            let database_url = format!("{}/{}", &admin_url[..slash], name);
            let pool = PgPool::connect(&database_url)
                .await
                .expect("connect Project Context scratch database");
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate Project Context scratch database");
            Self { admin, pool, name }
        }

        async fn cleanup(self) {
            let actual_database_name: String = sqlx::query_scalar("SELECT current_database()")
                .fetch_one(&self.pool)
                .await
                .expect("read Project Context scratch database name before cleanup");
            assert_eq!(actual_database_name, self.name);
            assert!(actual_database_name.starts_with("buzz_"));
            self.pool.close().await;
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DROP DATABASE {} WITH (FORCE)",
                self.name
            )))
            .execute(&self.admin)
            .await
            .expect("drop Project Context scratch database");
            self.admin.close().await;
        }
    }

    fn whole_second_now() -> DateTime<Utc> {
        DateTime::from_timestamp(Utc::now().timestamp(), 0).expect("current timestamp")
    }

    async fn seed_community(pool: &PgPool, actor: &Keys) -> CommunityId {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities (id, host) VALUES ($1, $2)")
            .bind(community_id.as_uuid())
            .bind(format!("project-context-{}.test", community_id.as_uuid()))
            .execute(pool)
            .await
            .expect("seed Project Context Community");
        sqlx::query(
            "INSERT INTO relay_members (community_id, pubkey, role) VALUES ($1, $2, 'member')",
        )
        .bind(community_id.as_uuid())
        .bind(actor.public_key().to_hex())
        .execute(pool)
        .await
        .expect("seed Project Context actor");
        community_id
    }

    async fn bootstrap_documents(db: &Db, community_id: CommunityId, relay: &Keys) {
        let catalog = DocumentCatalog::empty(community_id, 1, whole_second_now())
            .expect("empty Document catalog");
        let plan =
            DocumentProjectionPlan::for_bootstrap(&catalog).expect("Document bootstrap plan");
        let meta_projection = build_document_meta_projection(&plan, &[])
            .expect("build Document bootstrap metadata")
            .sign_with_keys(relay)
            .expect("sign Document bootstrap metadata");
        db.bootstrap_empty_project_document_catalog(PreparedProjectDocumentBootstrap {
            catalog,
            meta_projection,
        })
        .await
        .expect("bootstrap Document catalog");
        sqlx::query(
            "UPDATE communities \
             SET project_document_enabled = TRUE \
             WHERE id = $1",
        )
        .bind(community_id.as_uuid())
        .execute(&db.pool)
        .await
        .expect("enable Document storage fixture");
    }

    async fn create_document(
        db: &Db,
        community_id: CommunityId,
        document_id: Uuid,
        actor: &Keys,
        relay: &Keys,
    ) {
        let command = ProjectDocumentCommand::new(
            0,
            DocumentCommandRequest::Create {
                document_id,
                title: format!("Context fixture {document_id}"),
                summary: Some("Project Context storage fixture".to_owned()),
                content_markdown: "# Context\n\nCanonical fixture.".to_owned(),
            },
        );
        commit_document_command(db, community_id, command, actor, relay).await;
    }

    fn document_command_event(command: &ProjectDocumentCommand, actor: &Keys) -> Event {
        build_document_command(command.clone())
            .expect("build Document command")
            .sign_with_keys(actor)
            .expect("sign Document command")
    }

    fn prepare_document_commit(
        context: &ProjectDocumentWriteContext,
        command: ProjectDocumentCommand,
        command_event: Event,
        relay: &Keys,
    ) -> Result<PreparedProjectDocumentCommit, DocumentError> {
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
            .expect("build Document revision")
            .sign_with_keys(relay)
            .expect("sign Document revision");
        let head_projection =
            build_document_head_projection(transition.projection_plan(), &revision_projection)
                .expect("build Document head")
                .sign_with_keys(relay)
                .expect("sign Document head");
        let changed = changed_head_for(
            transition.projection_plan(),
            &head_projection,
            &revision_projection,
        )
        .expect("bind changed Document head");
        let meta_projection =
            build_document_meta_projection(transition.projection_plan(), &[changed])
                .expect("build Document metadata")
                .sign_with_keys(relay)
                .expect("sign Document metadata");
        Ok(PreparedProjectDocumentCommit {
            command_event,
            command,
            transition,
            revision_projection,
            head_projection,
            meta_projection,
        })
    }

    async fn prepare_document_storage_commit(
        db: &Db,
        community_id: CommunityId,
        command: ProjectDocumentCommand,
        actor: &Keys,
        relay: &Keys,
    ) -> (ProjectDocumentWriteTx, PreparedProjectDocumentCommit) {
        let command_event = document_command_event(&command, actor);
        let mut write = crate::project_document::begin_project_document_storage_test_write(
            db,
            community_id,
            relay.public_key(),
        )
        .await
        .expect("begin Document fixture write");
        let context = write
            .load_current(command.document_id())
            .await
            .expect("load Document fixture identity");
        let prepared = prepare_document_commit(&context, command, command_event, relay)
            .expect("reduce Document fixture command");
        (write, prepared)
    }

    async fn commit_document_command(
        db: &Db,
        community_id: CommunityId,
        command: ProjectDocumentCommand,
        actor: &Keys,
        relay: &Keys,
    ) -> ProjectDocumentCommitOutcome {
        let (write, prepared) =
            prepare_document_storage_commit(db, community_id, command, actor, relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit Document fixture")
    }

    fn context_bootstrap(
        community_id: CommunityId,
        relay: &Keys,
        initialized_at: DateTime<Utc>,
    ) -> PreparedProjectContextBootstrap {
        let catalog = ProjectContextCatalog::empty(community_id, 1, initialized_at)
            .expect("empty Context catalog");
        let plan = ProjectContextProjectionPlan::for_reset(&catalog)
            .expect("Context reset projection plan");
        let meta_projection = build_project_context_meta_projection(&plan, &[])
            .expect("build Context bootstrap metadata")
            .sign_with_keys(relay)
            .expect("sign Context bootstrap metadata");
        PreparedProjectContextBootstrap {
            catalog,
            meta_projection,
        }
    }

    async fn store_context_bootstrap(
        db: &Db,
        prepared: &PreparedProjectContextBootstrap,
    ) -> ProjectContextBootstrapOutcome {
        let mut tx = db.pool.begin().await.expect("begin Context bootstrap");
        crate::community_lock::acquire(&mut tx, prepared.catalog.project_id(), false)
            .await
            .expect("lock Context bootstrap");
        let outcome = store_empty_project_context_catalog_in_tx(&mut tx, prepared)
            .await
            .expect("store Context bootstrap");
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await
            .expect("validate Context bootstrap");
        tx.commit().await.expect("commit Context bootstrap");
        outcome
    }

    async fn begin_storage_test_write(
        db: &Db,
        community_id: CommunityId,
        expected_projection_pubkey: PublicKey,
        operation: ProjectContextOperation,
    ) -> ProjectContextWriteResult<ProjectContextWriteTx> {
        let mut tx = db.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let signer: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT projection_pubkey FROM project_context_edge_state \
             WHERE community_id = $1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if signer.as_deref() != Some(expected_projection_pubkey.as_bytes()) {
            return Err(ProjectContextWriteError::Unavailable { community_id });
        }
        sqlx::query("SELECT project_context_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *tx)
            .await?;
        if !context_projection_parity(&mut tx, community_id, &expected_projection_pubkey).await? {
            return Err(ProjectContextWriteError::Unavailable { community_id });
        }
        Ok(ProjectContextWriteTx {
            tx,
            community_id,
            expected_projection_pubkey,
            operation,
            loaded: None,
        })
    }

    async fn begin_reproject_test_write(
        db: &Db,
        community_id: CommunityId,
        target_pubkey: PublicKey,
    ) -> ProjectContextWriteResult<ProjectContextReprojectTx> {
        let mut tx = db.pool.begin().await?;
        crate::community_lock::acquire(&mut tx, community_id, false).await?;
        let enabled: Option<bool> = sqlx::query_scalar(
            "SELECT project_context_edge_enabled FROM communities \
             WHERE id = $1 AND archived_at IS NULL FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
        if enabled != Some(false) {
            return Err(ProjectContextWriteError::Unavailable { community_id });
        }
        Ok(ProjectContextReprojectTx {
            tx,
            community_id,
            target_pubkey,
            loaded: None,
        })
    }

    fn prepare_context_reprojection(
        context: &ProjectContextReprojectContext,
        relay: &Keys,
    ) -> PreparedProjectContextReprojection {
        let binding_projections = context
            .bindings
            .iter()
            .map(|binding| {
                build_project_context_binding_reprojection(binding)
                    .expect("build Context binding reprojection")
                    .sign_with_keys(relay)
                    .expect("sign Context binding reprojection")
            })
            .collect();
        let plan = ProjectContextProjectionPlan::for_reset(&context.catalog)
            .expect("Context reprojection reset plan");
        let meta_projection = build_project_context_meta_projection(&plan, &[])
            .expect("build Context reprojection metadata")
            .sign_with_keys(relay)
            .expect("sign Context reprojection metadata");
        PreparedProjectContextReprojection {
            binding_projections,
            meta_projection,
        }
    }

    fn context_command_event(
        community_id: CommunityId,
        command: &ProjectContextCommand,
        actor: &Keys,
    ) -> Event {
        build_project_context_command(community_id, command.clone())
            .expect("build Context command")
            .sign_with_keys(actor)
            .expect("sign Context command")
    }

    fn prepare_context_commit(
        context: &ProjectContextWriteContext,
        command: ProjectContextCommand,
        command_event: Event,
        relay: &Keys,
    ) -> PreparedProjectContextCommit {
        let transition = reduce_project_context(
            &context.catalog,
            context.current_edge.as_ref(),
            context.active_document_edge,
            &command,
            ProjectContextChangeContext::active(
                command_event.pubkey,
                command_event.id,
                context.canonical_time,
            )
            .with_coordinates_active(context.all_coordinates_active)
            .with_context_document_active(context.context_document_active),
        )
        .expect("reduce Context command");
        let binding_projection =
            build_project_context_binding_projection(transition.projection_plan())
                .expect("build Context binding")
                .sign_with_keys(relay)
                .expect("sign Context binding");
        let changed =
            changed_project_context_binding_for(transition.projection_plan(), &binding_projection)
                .expect("bind changed Context binding");
        let meta_projection =
            build_project_context_meta_projection(transition.projection_plan(), &[changed])
                .expect("build Context metadata")
                .sign_with_keys(relay)
                .expect("sign Context metadata");
        PreparedProjectContextCommit {
            command_event,
            command,
            transition,
            binding_projection,
            meta_projection,
        }
    }

    async fn prepare_storage_commit(
        db: &Db,
        community_id: CommunityId,
        command: ProjectContextCommand,
        actor: &Keys,
        relay: &Keys,
    ) -> (ProjectContextWriteTx, PreparedProjectContextCommit) {
        let command_event = context_command_event(community_id, &command, actor);
        let mut write =
            begin_storage_test_write(db, community_id, relay.public_key(), command.operation())
                .await
                .expect("begin Context storage write");
        assert_eq!(
            write
                .prepare_command(&command_event, &command)
                .await
                .expect("prepare Context command"),
            ProjectContextPrepareOutcome::New
        );
        let context = write
            .load_current(&command)
            .await
            .expect("load Context reducer basis");
        let prepared = prepare_context_commit(&context, command, command_event, relay);
        (write, prepared)
    }

    async fn assert_inactive_coordinate_attach(
        db: &Db,
        community_id: CommunityId,
        expected_revision: u64,
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
        actor: &Keys,
        relay: &Keys,
    ) {
        let command = ProjectContextCommand::new(
            expected_revision,
            ProjectContextOperation::Attach,
            coordinates,
            context_document_id,
        )
        .expect("build inactive-coordinate attach");
        let command_event = context_command_event(community_id, &command, actor);
        let mut write = begin_storage_test_write(
            db,
            community_id,
            relay.public_key(),
            ProjectContextOperation::Attach,
        )
        .await
        .expect("begin inactive-coordinate attach");
        assert_eq!(
            write
                .prepare_command(&command_event, &command)
                .await
                .expect("prepare inactive-coordinate attach"),
            ProjectContextPrepareOutcome::New
        );
        let context = write
            .load_current(&command)
            .await
            .expect("load inactive-coordinate basis");
        assert!(!context.all_coordinates_active);
        assert!(matches!(
            reduce_project_context(
                &context.catalog,
                context.current_edge.as_ref(),
                context.active_document_edge,
                &command,
                ProjectContextChangeContext::active(
                    command_event.pubkey,
                    command_event.id,
                    context.canonical_time,
                )
                .with_coordinates_active(context.all_coordinates_active)
                .with_context_document_active(context.context_document_active),
            ),
            Err(ProjectContextError::InactiveCoordinate)
        ));
        write
            .rollback()
            .await
            .expect("rollback inactive-coordinate attach");
    }

    async fn context_business_snapshot(pool: &PgPool, community_id: CommunityId) -> Value {
        sqlx::query_scalar(
            "SELECT jsonb_build_object( \
                'context_state', ( \
                    SELECT to_jsonb(state) - 'projection_pubkey' \
                           - 'projection_generation' - 'meta_projection_event_id' \
                    FROM project_context_edge_state state WHERE community_id = $1), \
                'edges', COALESCE(( \
                    SELECT jsonb_agg(to_jsonb(edge) ORDER BY encode(edge.edge_key, 'hex')) \
                    FROM project_context_edges edge WHERE community_id = $1), '[]'::jsonb), \
                'coordinates', COALESCE(( \
                    SELECT jsonb_agg(to_jsonb(coordinate) \
                        ORDER BY encode(coordinate.edge_key, 'hex'), coordinate.ordinal) \
                    FROM project_context_edge_coordinates coordinate \
                    WHERE community_id = $1), '[]'::jsonb), \
                'bindings', COALESCE(( \
                    SELECT jsonb_agg( \
                        to_jsonb(binding) - 'current_projection_event_id' \
                        ORDER BY binding.context_document_id) \
                    FROM project_context_document_bindings binding \
                    WHERE community_id = $1), '[]'::jsonb), \
                'changes', COALESCE(( \
                    SELECT jsonb_agg(to_jsonb(change) ORDER BY change.context_revision) \
                    FROM project_context_edge_changes change \
                    WHERE community_id = $1), '[]'::jsonb), \
                'document_state', ( \
                    SELECT to_jsonb(state) FROM project_document_state state \
                    WHERE community_id = $1), \
                'documents', COALESCE(( \
                    SELECT jsonb_agg(to_jsonb(document) ORDER BY document.document_id) \
                    FROM project_documents document WHERE community_id = $1), '[]'::jsonb) \
             )",
        )
        .bind(community_id.as_uuid())
        .fetch_one(pool)
        .await
        .expect("snapshot Project Context business state")
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn canonical_storage_bootstrap_lifecycle_replay_and_guards() {
        let scratch = ScratchDatabase::create("buzz_project_context_storage").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap_documents(&db, community_id, &relay).await;
        let coordinate_a = Uuid::new_v4();
        let coordinate_b = Uuid::new_v4();
        let context_document_id = Uuid::new_v4();
        for document_id in [coordinate_a, coordinate_b, context_document_id] {
            create_document(&db, community_id, document_id, &actor, &relay).await;
        }

        let initialized_at = whole_second_now();
        let bootstrap = context_bootstrap(community_id, &relay, initialized_at);
        let production_rejection = db
            .bootstrap_empty_project_context_catalog(bootstrap.clone())
            .await;
        assert!(matches!(
            production_rejection,
            Err(ProjectContextWriteError::Unavailable { .. })
        ));
        let mut rollback_bootstrap = db.pool.begin().await.expect("begin rollback bootstrap");
        crate::community_lock::acquire(&mut rollback_bootstrap, community_id, false)
            .await
            .expect("lock rollback bootstrap");
        assert_eq!(
            store_empty_project_context_catalog_in_tx(&mut rollback_bootstrap, &bootstrap)
                .await
                .expect("stage rollback bootstrap"),
            ProjectContextBootstrapOutcome { replayed: false }
        );
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *rollback_bootstrap)
            .await
            .expect("validate staged rollback bootstrap");
        rollback_bootstrap
            .rollback()
            .await
            .expect("roll back atomic bootstrap");
        let rolled_back: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS (SELECT 1 FROM project_context_edge_state WHERE community_id = $1) \
                 AND NOT EXISTS (SELECT 1 FROM events WHERE community_id = $1 AND id = $2)",
        )
        .bind(community_id.as_uuid())
        .bind(bootstrap.meta_projection.id.as_bytes().as_slice())
        .fetch_one(&scratch.pool)
        .await
        .expect("verify bootstrap rollback");
        assert!(rolled_back);
        assert_eq!(
            store_context_bootstrap(&db, &bootstrap).await,
            ProjectContextBootstrapOutcome { replayed: false }
        );
        assert_eq!(
            store_context_bootstrap(&db, &bootstrap).await,
            ProjectContextBootstrapOutcome { replayed: true }
        );
        let other_relay = Keys::generate();
        let mismatched = context_bootstrap(community_id, &other_relay, initialized_at);
        let mut mismatch_tx = db.pool.begin().await.expect("begin mismatched bootstrap");
        crate::community_lock::acquire(&mut mismatch_tx, community_id, false)
            .await
            .expect("lock mismatched bootstrap");
        assert!(matches!(
            store_empty_project_context_catalog_in_tx(&mut mismatch_tx, &mismatched).await,
            Err(ProjectContextWriteError::InvalidCommit(_))
        ));
        mismatch_tx
            .rollback()
            .await
            .expect("rollback mismatched bootstrap");

        let coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_b,
            },
        ];
        let attach = ProjectContextCommand::new(
            0,
            ProjectContextOperation::Attach,
            coordinates.clone(),
            context_document_id,
        )
        .expect("attach command");
        let (write, prepared_attach) =
            prepare_storage_commit(&db, community_id, attach, &actor, &relay).await;
        let expected_receipt = prepared_attach.transition.receipt().clone();
        let attach_outcome = write
            .commit(prepared_attach.clone())
            .await
            .expect("commit first attach");
        assert_eq!(attach_outcome.receipt, expected_receipt);
        assert!(!attach_outcome.replayed);

        let replay_write = begin_storage_test_write(
            &db,
            community_id,
            relay.public_key(),
            ProjectContextOperation::Attach,
        )
        .await
        .expect("begin attach replay");
        let replay = replay_write
            .commit(prepared_attach)
            .await
            .expect("replay exact attach");
        assert!(replay.replayed);
        assert_eq!(replay.receipt, expected_receipt);

        let edge_key = expected_receipt.edge_key;
        let sql_edge_key: Vec<u8> =
            sqlx::query_scalar("SELECT project_context_compute_edge_key($1, $2)")
                .bind(community_id.as_uuid())
                .bind(edge_key.as_bytes().as_slice())
                .fetch_one(&scratch.pool)
                .await
                .expect("derive SQL edge key");
        assert_eq!(sql_edge_key.as_slice(), edge_key.as_bytes());
        assert_eq!(
            db.verify_project_context_storage(community_id, &relay.public_key())
                .await
                .expect("verify attached storage"),
            ProjectContextIntegrityStatus {
                orphan_projection_count: 0,
                pointer_mismatch_count: 0,
            }
        );

        let delete_error = sqlx::query(
            "UPDATE project_documents SET state = 'deleted' \
             WHERE community_id = $1 AND document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(context_document_id)
        .execute(&scratch.pool)
        .await
        .expect_err("active Context Document tombstone must fail");
        assert!(delete_error
            .to_string()
            .contains("active Context Document must be detached before deletion"));

        let detach = ProjectContextCommand::new(
            1,
            ProjectContextOperation::Detach,
            coordinates.clone(),
            context_document_id,
        )
        .expect("detach command");
        let (write, prepared_detach) =
            prepare_storage_commit(&db, community_id, detach, &actor, &relay).await;
        let detached = write
            .commit(prepared_detach)
            .await
            .expect("commit final detach");
        assert_eq!(
            detached.receipt.edge_state,
            ProjectContextBindingState::Deleted
        );
        assert_eq!(detached.receipt.edge_document_count, 0);

        let reattach = ProjectContextCommand::new(
            2,
            ProjectContextOperation::Attach,
            coordinates.clone(),
            context_document_id,
        )
        .expect("reattach command");
        let (write, prepared_reattach) =
            prepare_storage_commit(&db, community_id, reattach, &actor, &relay).await;
        let reattached = write
            .commit(prepared_reattach)
            .await
            .expect("reattach deleted transport rows");
        assert_eq!(reattached.receipt.edge_key, edge_key);
        assert_eq!(reattached.receipt.context_revision, 3);

        let counts = sqlx::query(
            "SELECT \
                (SELECT count(*)::bigint FROM project_context_edges \
                 WHERE community_id = $1) AS edges, \
                (SELECT count(*)::bigint FROM project_context_edge_coordinates \
                 WHERE community_id = $1) AS coordinates, \
                (SELECT count(*)::bigint FROM project_context_document_bindings \
                 WHERE community_id = $1) AS bindings, \
                (SELECT count(*)::bigint FROM project_context_edge_changes \
                 WHERE community_id = $1) AS changes",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("load Context storage counts");
        assert_eq!(counts.try_get::<i64, _>("edges").expect("edge count"), 1);
        assert_eq!(
            counts
                .try_get::<i64, _>("coordinates")
                .expect("coordinate count"),
            2
        );
        assert_eq!(
            counts.try_get::<i64, _>("bindings").expect("binding count"),
            1
        );
        assert_eq!(
            counts.try_get::<i64, _>("changes").expect("change count"),
            3
        );

        let missing_identity = ProjectContextCommand::new(
            3,
            ProjectContextOperation::Attach,
            vec![
                ProjectContextCoordinate::Document {
                    document_id: coordinate_b,
                },
                ProjectContextCoordinate::Document {
                    document_id: Uuid::new_v4(),
                },
            ],
            coordinate_a,
        )
        .expect("missing-coordinate attach command");
        let mut missing_write = begin_storage_test_write(
            &db,
            community_id,
            relay.public_key(),
            ProjectContextOperation::Attach,
        )
        .await
        .expect("begin missing-coordinate proof");
        let missing_context = missing_write
            .load_current(&missing_identity)
            .await
            .expect("load missing-coordinate basis");
        assert!(!missing_context.all_coordinates_active);
        let missing_event = context_command_event(community_id, &missing_identity, &actor);
        assert!(matches!(
            reduce_project_context(
                &missing_context.catalog,
                missing_context.current_edge.as_ref(),
                missing_context.active_document_edge,
                &missing_identity,
                ProjectContextChangeContext::active(
                    missing_event.pubkey,
                    missing_event.id,
                    missing_context.canonical_time,
                )
                .with_coordinates_active(missing_context.all_coordinates_active)
                .with_context_document_active(missing_context.context_document_active),
            ),
            Err(ProjectContextError::InactiveCoordinate)
        ));
        missing_write
            .rollback()
            .await
            .expect("rollback missing-coordinate proof");

        let different_coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: context_document_id,
            },
        ];
        let already_bound = ProjectContextCommand::new(
            3,
            ProjectContextOperation::Attach,
            different_coordinates,
            context_document_id,
        )
        .expect("second-edge attach command");
        let mut conflict_write = begin_storage_test_write(
            &db,
            community_id,
            relay.public_key(),
            ProjectContextOperation::Attach,
        )
        .await
        .expect("begin single-binding conflict");
        let conflict_context = conflict_write
            .load_current(&already_bound)
            .await
            .expect("load single-binding conflict basis");
        let conflict_event = context_command_event(community_id, &already_bound, &actor);
        let conflict = reduce_project_context(
            &conflict_context.catalog,
            conflict_context.current_edge.as_ref(),
            conflict_context.active_document_edge,
            &already_bound,
            ProjectContextChangeContext::active(
                conflict_event.pubkey,
                conflict_event.id,
                conflict_context.canonical_time,
            ),
        );
        assert!(matches!(
            conflict,
            Err(ProjectContextError::DocumentAlreadyBound { .. })
        ));
        conflict_write
            .rollback()
            .await
            .expect("rollback single-binding conflict");

        let mut corruption = scratch
            .pool
            .begin()
            .await
            .expect("begin collision simulation");
        corruption
            .execute("SET LOCAL session_replication_role = replica")
            .await
            .expect("disable guards for collision simulation");
        let corrupt_coordinates = serde_json::to_value([
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: context_document_id,
            },
        ])
        .expect("serialize corrupt coordinates");
        sqlx::query(
            "UPDATE project_context_edges SET canonical_coordinates = $3 \
             WHERE community_id = $1 AND edge_key = $2",
        )
        .bind(community_id.as_uuid())
        .bind(edge_key.as_bytes().as_slice())
        .bind(corrupt_coordinates)
        .execute(&mut *corruption)
        .await
        .expect("simulate hash collision");
        corruption
            .execute("SET LOCAL session_replication_role = origin")
            .await
            .expect("restore constraint triggers");
        let collision_error = sqlx::query("SELECT project_context_validate_community($1)")
            .bind(community_id.as_uuid())
            .execute(&mut *corruption)
            .await
            .expect_err("hash/coordinate drift must fail closed");
        assert!(collision_error
            .to_string()
            .contains("JSON, normalized coordinates, and edge key disagree"));
        corruption
            .rollback()
            .await
            .expect("rollback collision simulation");

        let status = db
            .project_context_status(community_id)
            .await
            .expect("load Context status")
            .expect("Context status exists");
        assert_eq!(status.context_revision, Some(3));
        assert_eq!(status.active_edge_count, Some(1));
        assert_eq!(status.bound_document_count, Some(1));
        assert_eq!(status.edge_row_count, 1);
        assert_eq!(status.binding_row_count, 1);
        assert_eq!(status.change_count, 3);
        db.verify_project_context_storage(community_id, &relay.public_key())
            .await
            .expect("verify final Context storage");

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn signer_reproject_rebuilds_all_heads_and_preserves_business_state() {
        let scratch = ScratchDatabase::create("buzz_project_context_reproject").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let source_relay = Keys::generate();
        let target_relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap_documents(&db, community_id, &source_relay).await;
        let bootstrap = context_bootstrap(community_id, &source_relay, whole_second_now());
        assert_eq!(
            store_context_bootstrap(&db, &bootstrap).await,
            ProjectContextBootstrapOutcome { replayed: false }
        );

        let coordinate_a = Uuid::new_v4();
        let coordinate_b = Uuid::new_v4();
        let context_document_a = Uuid::new_v4();
        let context_document_b = Uuid::new_v4();
        for document_id in [
            coordinate_a,
            coordinate_b,
            context_document_a,
            context_document_b,
        ] {
            create_document(&db, community_id, document_id, &actor, &source_relay).await;
        }
        let coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_b,
            },
        ];
        for (revision, document_id) in [(0, context_document_a), (1, context_document_b)] {
            let attach = ProjectContextCommand::new(
                revision,
                ProjectContextOperation::Attach,
                coordinates.clone(),
                document_id,
            )
            .expect("build reproject attach");
            let (write, prepared) =
                prepare_storage_commit(&db, community_id, attach, &actor, &source_relay).await;
            write
                .commit(prepared)
                .await
                .expect("commit reproject attach");
        }
        let detach = ProjectContextCommand::new(
            2,
            ProjectContextOperation::Detach,
            coordinates,
            context_document_a,
        )
        .expect("build deleted-head fixture");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, detach, &actor, &source_relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit deleted-head fixture");

        let orphan =
            EventBuilder::new(Kind::Custom(KIND_PROJECT_CONTEXT_EDGE_BINDING as u16), "{}")
                .sign_with_keys(&source_relay)
                .expect("sign orphan Context projection fixture");
        let (_, inserted) = crate::event::insert_event(&scratch.pool, community_id, &orphan, None)
            .await
            .expect("insert orphan Context projection fixture");
        assert!(inserted);
        let unhealthy = db
            .project_context_integrity_status(community_id)
            .await
            .expect("read unhealthy Context integrity")
            .expect("initialized Context integrity");
        assert_eq!(unhealthy.orphan_projection_count, 1);
        let business_before = context_business_snapshot(&scratch.pool, community_id).await;

        let mut write = begin_reproject_test_write(&db, community_id, target_relay.public_key())
            .await
            .expect("begin Context signer reprojection");
        let context = write
            .load_current()
            .await
            .expect("load Context signer reprojection");
        assert_eq!(context.source_generation, 1);
        assert_eq!(context.catalog.projection_generation(), 2);
        assert_eq!(context.catalog.context_revision(), 3);
        assert_eq!(context.bindings.len(), 2);
        assert_eq!(
            context
                .bindings
                .iter()
                .filter(|binding| binding.state == ProjectContextBindingState::Active)
                .count(),
            1
        );
        assert_eq!(
            context
                .bindings
                .iter()
                .filter(|binding| binding.state == ProjectContextBindingState::Deleted)
                .count(),
            1
        );
        let prepared = prepare_context_reprojection(&context, &target_relay);
        let outcome = write
            .commit_reprojection(prepared)
            .await
            .expect("commit Context signer reprojection");
        assert_eq!(outcome.source_generation, 1);
        assert_eq!(outcome.projection_generation, 2);
        assert_eq!(outcome.context_revision, 3);
        assert_eq!(outcome.events.len(), 3);

        assert_eq!(
            context_business_snapshot(&scratch.pool, community_id).await,
            business_before,
            "reprojection changed Project Context or Document business state"
        );
        assert_eq!(
            db.verify_project_context_storage(community_id, &target_relay.public_key())
                .await
                .expect("verify reprojected Context storage"),
            ProjectContextIntegrityStatus {
                orphan_projection_count: 0,
                pointer_mismatch_count: 0,
            }
        );
        let generation: (i64, Vec<u8>, i64) = sqlx::query_as(
            "SELECT projection_generation, projection_pubkey, context_revision \
             FROM project_context_edge_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read reprojected Context generation");
        assert_eq!(generation.0, 2);
        assert_eq!(generation.1, target_relay.public_key().to_bytes());
        assert_eq!(generation.2, 3);
        let live_projection_state: (i64, bool) = sqlx::query_as(
            "SELECT count(*)::bigint, bool_and(pubkey = $2) \
             FROM events WHERE community_id = $1 AND kind = ANY($3) \
               AND deleted_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(target_relay.public_key().as_bytes())
        .bind([
            KIND_PROJECT_CONTEXT_EDGE_BINDING as i32,
            KIND_PROJECT_CONTEXT_META as i32,
        ])
        .fetch_one(&scratch.pool)
        .await
        .expect("read live replacement projections");
        assert_eq!(live_projection_state, (3, true));
        let reproject_audits: i64 = sqlx::query_scalar(
            "SELECT count(*)::bigint FROM audit_log \
             WHERE community_id = $1 AND action = 'project_context_edge_control' \
               AND detail->>'operation' = 'reproject'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read Context reproject audit");
        assert_eq!(reproject_audits, 1);

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn document_and_coordinate_lifecycles_are_independent_and_non_cascading() {
        let scratch = ScratchDatabase::create("buzz_project_context_lifecycle").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap_documents(&db, community_id, &relay).await;
        let bootstrap = context_bootstrap(community_id, &relay, whole_second_now());
        assert_eq!(
            store_context_bootstrap(&db, &bootstrap).await,
            ProjectContextBootstrapOutcome { replayed: false }
        );

        let coordinate_a = Uuid::new_v4();
        let coordinate_b = Uuid::new_v4();
        let context_document_id = Uuid::new_v4();
        let rejected_candidate_id = Uuid::new_v4();
        for document_id in [
            coordinate_a,
            coordinate_b,
            context_document_id,
            rejected_candidate_id,
        ] {
            create_document(&db, community_id, document_id, &actor, &relay).await;
        }
        let coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_b,
            },
        ];
        let attach = ProjectContextCommand::new(
            0,
            ProjectContextOperation::Attach,
            coordinates.clone(),
            context_document_id,
        )
        .expect("build lifecycle attach");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, attach, &actor, &relay).await;
        let attached = write
            .commit(prepared)
            .await
            .expect("commit lifecycle attach");
        assert_eq!(attached.receipt.context_revision, 1);

        // The user-facing Document reducer sees the active Context binding,
        // instead of relying on the lower commit-time trigger to surface an
        // internal database error.
        let delete = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: context_document_id,
            },
        );
        let delete_event = document_command_event(&delete, &actor);
        let mut document_write =
            crate::project_document::begin_project_document_storage_test_write(
                &db,
                community_id,
                relay.public_key(),
            )
            .await
            .expect("begin protected Document delete");
        let delete_context = document_write
            .load_current(context_document_id)
            .await
            .expect("load protected Context Document");
        assert!(delete_context.deletion_blocked);
        assert!(matches!(
            prepare_document_commit(&delete_context, delete, delete_event, &relay),
            Err(DocumentError::StillReferenced { document_id })
                if document_id == context_document_id
        ));
        document_write
            .rollback()
            .await
            .expect("rollback protected Document delete");

        let binding_projection_before: Vec<u8> = sqlx::query_scalar(
            "SELECT current_projection_event_id \
             FROM project_context_document_bindings \
             WHERE community_id = $1 AND context_document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(context_document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read binding projection before Document update");
        let update = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: context_document_id,
                title: "Corrected Context".to_owned(),
                summary: Some("Semantic correction".to_owned()),
                content_markdown: "# Corrected Context\n\nThe Edge is unchanged.".to_owned(),
            },
        );
        let updated = commit_document_command(&db, community_id, update, &actor, &relay).await;
        assert_eq!(updated.receipt.document_revision, 2);
        let after_update: (i64, Vec<u8>) = sqlx::query_as(
            "SELECT state.context_revision, binding.current_projection_event_id \
             FROM project_context_edge_state state \
             JOIN project_context_document_bindings binding \
               ON binding.community_id = state.community_id \
             WHERE state.community_id = $1 AND binding.context_document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(context_document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read Context state after Document update");
        assert_eq!(after_update.0, 1);
        assert_eq!(after_update.1, binding_projection_before);

        // A Document used only as a coordinate can tombstone without changing
        // the retained Edge, binding head, normalized coordinate rows, or the
        // independent Context revision.
        let delete_coordinate = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: coordinate_a,
            },
        );
        commit_document_command(&db, community_id, delete_coordinate, &actor, &relay).await;
        let retained: (i64, String, String, i64) = sqlx::query_as(
            "SELECT state.context_revision, edge.state, binding.state, \
                    (SELECT count(*) FROM project_context_edge_coordinates coordinate \
                     WHERE coordinate.community_id = state.community_id) \
             FROM project_context_edge_state state \
             JOIN project_context_edges edge ON edge.community_id = state.community_id \
             JOIN project_context_document_bindings binding \
               ON binding.community_id = edge.community_id AND binding.edge_key = edge.edge_key \
             WHERE state.community_id = $1 AND binding.context_document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(context_document_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read retained Edge after coordinate tombstone");
        assert_eq!(retained, (1, "active".to_owned(), "active".to_owned(), 2));
        assert_inactive_coordinate_attach(
            &db,
            community_id,
            1,
            coordinates.clone(),
            rejected_candidate_id,
            &actor,
            &relay,
        )
        .await;

        let detach = ProjectContextCommand::new(
            1,
            ProjectContextOperation::Detach,
            coordinates,
            context_document_id,
        )
        .expect("build lifecycle detach");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, detach, &actor, &relay).await;
        let detached = write.commit(prepared).await.expect("detach retained Edge");
        assert_eq!(detached.receipt.context_revision, 2);
        assert_eq!(detached.receipt.edge_document_count, 0);
        let delete_context_document = ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: context_document_id,
            },
        );
        let deleted =
            commit_document_command(&db, community_id, delete_context_document, &actor, &relay)
                .await;
        assert_eq!(deleted.receipt.document_revision, 3);

        // One Document may independently be an Edge coordinate and that same
        // Edge's Context Document, while also remaining a coordinate of a
        // second overlapping Edge. Detaching one relation never rewrites the
        // other relation.
        let shared_document = Uuid::new_v4();
        let coordinate_c = Uuid::new_v4();
        let coordinate_d = Uuid::new_v4();
        let second_context_document = Uuid::new_v4();
        let overlap_candidate = Uuid::new_v4();
        for document_id in [
            shared_document,
            coordinate_c,
            coordinate_d,
            second_context_document,
            overlap_candidate,
        ] {
            create_document(&db, community_id, document_id, &actor, &relay).await;
        }
        let first_overlap = vec![
            ProjectContextCoordinate::Document {
                document_id: shared_document,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_c,
            },
        ];
        let second_overlap = vec![
            ProjectContextCoordinate::Document {
                document_id: shared_document,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_d,
            },
        ];
        let attach_shared = ProjectContextCommand::new(
            2,
            ProjectContextOperation::Attach,
            first_overlap.clone(),
            shared_document,
        )
        .expect("attach same-role Document");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, attach_shared, &actor, &relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit same-role Document attach");
        let attach_overlap = ProjectContextCommand::new(
            3,
            ProjectContextOperation::Attach,
            second_overlap.clone(),
            second_context_document,
        )
        .expect("attach overlapping Edge");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, attach_overlap, &actor, &relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit overlapping Edge");

        let delete_coordinate_c = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: coordinate_c,
            },
        );
        commit_document_command(&db, community_id, delete_coordinate_c, &actor, &relay).await;
        let update_shared = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: shared_document,
                title: "Shared Context corrected".to_owned(),
                summary: None,
                content_markdown: "# Shared Context\n\nCorrected after tombstone.".to_owned(),
            },
        );
        commit_document_command(&db, community_id, update_shared, &actor, &relay).await;
        let context_revision_after_independent_writes: i64 = sqlx::query_scalar(
            "SELECT context_revision FROM project_context_edge_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read independent Context revision");
        assert_eq!(context_revision_after_independent_writes, 4);
        assert_inactive_coordinate_attach(
            &db,
            community_id,
            4,
            first_overlap.clone(),
            overlap_candidate,
            &actor,
            &relay,
        )
        .await;

        let detach_shared = ProjectContextCommand::new(
            4,
            ProjectContextOperation::Detach,
            first_overlap,
            shared_document,
        )
        .expect("detach same-role Document");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, detach_shared, &actor, &relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit same-role detach");
        let delete_shared = ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: shared_document,
            },
        );
        commit_document_command(&db, community_id, delete_shared, &actor, &relay).await;
        let retained_overlap: (i64, i64, String) = sqlx::query_as(
            "SELECT state.context_revision, \
                    (SELECT count(*) FROM project_context_edges edge \
                     WHERE edge.community_id = state.community_id AND edge.state = 'active'), \
                    binding.state \
             FROM project_context_edge_state state \
             JOIN project_context_document_bindings binding \
               ON binding.community_id = state.community_id \
             WHERE state.community_id = $1 AND binding.context_document_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(second_context_document)
        .fetch_one(&scratch.pool)
        .await
        .expect("read unaffected overlapping Edge");
        assert_eq!(retained_overlap, (5, 1, "active".to_owned()));

        let update_second_context = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Update {
                document_id: second_context_document,
                title: "Second Context corrected".to_owned(),
                summary: Some("Tombstoned coordinate retained".to_owned()),
                content_markdown: "# Second Context\n\nStill editable.".to_owned(),
            },
        );
        commit_document_command(&db, community_id, update_second_context, &actor, &relay).await;
        assert_inactive_coordinate_attach(
            &db,
            community_id,
            5,
            second_overlap.clone(),
            overlap_candidate,
            &actor,
            &relay,
        )
        .await;
        let detach_second = ProjectContextCommand::new(
            5,
            ProjectContextOperation::Detach,
            second_overlap,
            second_context_document,
        )
        .expect("detach second overlapping Edge");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, detach_second, &actor, &relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit second overlap detach");
        let delete_second_context = ProjectDocumentCommand::new(
            2,
            DocumentCommandRequest::Delete {
                document_id: second_context_document,
            },
        );
        commit_document_command(&db, community_id, delete_second_context, &actor, &relay).await;

        let final_context: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "SELECT context_revision, active_edge_count, bound_document_count, \
                    (SELECT count(*) FROM project_context_edges edge \
                     WHERE edge.community_id = state.community_id), \
                    (SELECT count(*) FROM project_context_edge_coordinates coordinate \
                     WHERE coordinate.community_id = state.community_id), \
                    (SELECT count(*) FROM project_context_document_bindings binding \
                     WHERE binding.community_id = state.community_id) \
             FROM project_context_edge_state state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read final lifecycle Context state");
        assert_eq!(final_context, (6, 0, 0, 3, 6, 3));
        db.verify_project_context_storage(community_id, &relay.public_key())
            .await
            .expect("verify final lifecycle parity");

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn context_attach_and_document_delete_share_one_community_lock() {
        let scratch = ScratchDatabase::create("buzz_project_context_delete_race").await;
        let db = Db::from_pool(scratch.pool.clone());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let community_id = seed_community(&scratch.pool, &actor).await;
        bootstrap_documents(&db, community_id, &relay).await;
        let bootstrap = context_bootstrap(community_id, &relay, whole_second_now());
        assert_eq!(
            store_context_bootstrap(&db, &bootstrap).await,
            ProjectContextBootstrapOutcome { replayed: false }
        );

        let coordinate_a = Uuid::new_v4();
        let coordinate_b = Uuid::new_v4();
        let context_document_id = Uuid::new_v4();
        for document_id in [coordinate_a, coordinate_b, context_document_id] {
            create_document(&db, community_id, document_id, &actor, &relay).await;
        }
        let coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_b,
            },
        ];

        // Hold the shared lock in attach. The competing delete cannot observe
        // an unbound Document and commit before the binding becomes visible.
        let attach = ProjectContextCommand::new(
            0,
            ProjectContextOperation::Attach,
            coordinates.clone(),
            context_document_id,
        )
        .expect("build attach-wins command");
        let (attach_write, prepared_attach) =
            prepare_storage_commit(&db, community_id, attach, &actor, &relay).await;
        let delete_db = db.clone();
        let delete_actor = actor.clone();
        let delete_relay = relay.clone();
        let mut delete_task = tokio::spawn(async move {
            let command = ProjectDocumentCommand::new(
                1,
                DocumentCommandRequest::Delete {
                    document_id: context_document_id,
                },
            );
            let event = document_command_event(&command, &delete_actor);
            let mut write = crate::project_document::begin_project_document_storage_test_write(
                &delete_db,
                community_id,
                delete_relay.public_key(),
            )
            .await
            .expect("begin delete behind attach");
            let context = write
                .load_current(context_document_id)
                .await
                .expect("load delete behind attach");
            let result = prepare_document_commit(&context, command, event, &delete_relay);
            write
                .rollback()
                .await
                .expect("rollback delete behind attach");
            result.map(|_| ())
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut delete_task)
                .await
                .is_err(),
            "Document delete bypassed the held Context attach lock"
        );
        attach_write
            .commit(prepared_attach)
            .await
            .expect("commit attach before competing delete");
        assert!(matches!(
            delete_task.await.expect("join attach-wins delete"),
            Err(DocumentError::StillReferenced { document_id })
                if document_id == context_document_id
        ));

        let detach = ProjectContextCommand::new(
            1,
            ProjectContextOperation::Detach,
            coordinates,
            context_document_id,
        )
        .expect("build race cleanup detach");
        let (write, prepared) =
            prepare_storage_commit(&db, community_id, detach, &actor, &relay).await;
        write
            .commit(prepared)
            .await
            .expect("commit race cleanup detach");

        // Reverse the order: once Document delete owns the same lock, attach
        // waits and then observes the committed tombstone. Both operations can
        // never succeed against the same active Document snapshot.
        let doomed_document = Uuid::new_v4();
        create_document(&db, community_id, doomed_document, &actor, &relay).await;
        let delete = ProjectDocumentCommand::new(
            1,
            DocumentCommandRequest::Delete {
                document_id: doomed_document,
            },
        );
        let (delete_write, prepared_delete) =
            prepare_document_storage_commit(&db, community_id, delete, &actor, &relay).await;
        let attach_db = db.clone();
        let attach_actor = actor.clone();
        let attach_relay = relay.clone();
        let second_coordinates = vec![
            ProjectContextCoordinate::Document {
                document_id: coordinate_a,
            },
            ProjectContextCoordinate::Document {
                document_id: coordinate_b,
            },
        ];
        let mut attach_task = tokio::spawn(async move {
            let command = ProjectContextCommand::new(
                2,
                ProjectContextOperation::Attach,
                second_coordinates,
                doomed_document,
            )
            .expect("build delete-wins attach");
            let event = context_command_event(community_id, &command, &attach_actor);
            let mut write = begin_storage_test_write(
                &attach_db,
                community_id,
                attach_relay.public_key(),
                ProjectContextOperation::Attach,
            )
            .await
            .expect("begin attach behind delete");
            assert_eq!(
                write
                    .prepare_command(&event, &command)
                    .await
                    .expect("prepare attach behind delete"),
                ProjectContextPrepareOutcome::New
            );
            let context = write
                .load_current(&command)
                .await
                .expect("load attach behind delete");
            let result = reduce_project_context(
                &context.catalog,
                context.current_edge.as_ref(),
                context.active_document_edge,
                &command,
                ProjectContextChangeContext::active(event.pubkey, event.id, context.canonical_time)
                    .with_coordinates_active(context.all_coordinates_active)
                    .with_context_document_active(context.context_document_active),
            );
            write
                .rollback()
                .await
                .expect("rollback attach behind delete");
            result.map(|_| ())
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), &mut attach_task)
                .await
                .is_err(),
            "Context attach bypassed the held Document delete lock"
        );
        delete_write
            .commit(prepared_delete)
            .await
            .expect("commit delete before competing attach");
        assert!(matches!(
            attach_task.await.expect("join delete-wins attach"),
            Err(ProjectContextError::InactiveContextDocument { document_id })
                if document_id == doomed_document
        ));
        let final_context: (i64, i64, i64) = sqlx::query_as(
            "SELECT context_revision, active_edge_count, bound_document_count \
             FROM project_context_edge_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read final race Context state");
        assert_eq!(final_context, (2, 0, 0));

        scratch.cleanup().await;
    }
}
