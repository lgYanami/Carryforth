//! Project View canonical state, feature control, and atomic write transaction.
//!
//! Relay protocol handlers never compose Project View SQL directly. They hold a
//! [`ProjectViewWriteTx`], apply a typed mutation through `buzz-project-view`,
//! sign the resulting projections, and hand the prepared commit back here.

use std::collections::{BTreeMap, BTreeSet};

use buzz_core::kind::{
    KIND_PROJECT_VIEW_META, KIND_PROJECT_VIEW_MUTATION, KIND_PROJECT_VIEW_OBJECT,
};
use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::{
    DomainError, Mutation, MutationOutcome, MutationRequest, ProjectViewEntry, ProjectViewObject,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, ProjectViewState,
    ProjectViewTombstone, MUTATION_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::Event;
use serde_json::Value;
use sqlx::{Postgres, Row, Transaction};
use uuid::Uuid;

use crate::{Db, DbError};

const PROJECT_VIEW_LOCK_NAMESPACE: &str = "buzz_project_view:";

/// Errors specific to preparing or committing one Project View mutation.
#[derive(Debug, thiserror::Error)]
pub enum ProjectViewWriteError {
    /// Database storage failed.
    #[error(transparent)]
    Database(#[from] DbError),
    /// SQL execution failed before it could be mapped to a storage result.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// The pure Project View domain rejected the loaded or proposed state.
    #[error(transparent)]
    Domain(#[from] DomainError),
    /// Project View is disabled, archived, or absent for this Community.
    #[error("Project View is unavailable for community {community_id}")]
    Unavailable {
        /// Community whose centralized feature gate is not writable.
        community_id: CommunityId,
    },
    /// The caller based its mutation on a different project revision.
    #[error("Project View revision conflict: expected {expected}, current {current:?}")]
    RevisionConflict {
        /// Revision declared by the signed mutation.
        expected: u64,
        /// Current revision, or `None` when the Project View is uninitialized.
        current: Option<u64>,
    },
    /// The prepared state/projection bundle is internally inconsistent.
    #[error("invalid prepared Project View commit: {0}")]
    InvalidCommit(String),
}

/// Convenient result type for Project View writes.
pub type ProjectViewWriteResult<T> = std::result::Result<T, ProjectViewWriteError>;

/// Durable metadata stored beside one initialized Project View.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewStateMetadata {
    /// Current project-wide optimistic-concurrency revision.
    pub project_revision: u64,
    /// Number of active canonical objects maintained by a database trigger.
    pub active_object_count: u32,
    /// Canonical initialization time.
    pub initialized_at: DateTime<Utc>,
    /// Canonical time of the latest accepted mutation.
    pub updated_at: DateTime<Utc>,
    /// Latest accepted member command event ID.
    pub last_event_id: [u8; 32],
    /// Actor that authored the latest accepted command.
    pub last_actor_pubkey: PublicKey,
    /// Current relay-signed metadata projection event ID.
    pub meta_projection_event_id: [u8; 32],
    /// Relay identity that signed the current projection generation.
    pub projection_pubkey: PublicKey,
    /// Current projection signer generation.
    pub projection_generation: u64,
}

/// Locked canonical state and database-derived time for one mutation attempt.
#[derive(Debug)]
pub struct ProjectViewWriteContext {
    /// Reconstructed pure domain state. Revision zero means uninitialized.
    pub state: ProjectViewState,
    /// Durable projection metadata, absent before initialization.
    pub metadata: Option<ProjectViewStateMetadata>,
    /// Monotonic timestamp that must be supplied to the pure reducer.
    pub canonical_time: DateTime<Utc>,
}

/// One relay-signed object projection associated with a changed object ID.
#[derive(Debug, Clone)]
pub struct PreparedObjectProjection {
    object_id: Uuid,
    event: Event,
}

impl PreparedObjectProjection {
    /// Associate a signed projection event with its canonical object ID.
    #[must_use]
    pub const fn new(object_id: Uuid, event: Event) -> Self {
        Self { object_id, event }
    }

    /// Return the projected object ID.
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        self.object_id
    }

    /// Return the signed Nostr event.
    #[must_use]
    pub const fn event(&self) -> &Event {
        &self.event
    }
}

/// Fully prepared inputs for one atomic Project View database commit.
#[derive(Debug, Clone)]
pub struct PreparedProjectViewCommit {
    /// Accepted member-signed command event.
    pub command_event: Event,
    /// Parsed typed mutation carried by the command.
    pub mutation: Mutation,
    /// Complete canonical state after applying the mutation.
    pub next_state: ProjectViewState,
    /// Pure reducer outcome identifying the changed entries.
    pub outcome: MutationOutcome,
    /// One relay-signed projection for every changed entry.
    pub object_projections: Vec<PreparedObjectProjection>,
    /// Relay-signed metadata head for the new project revision.
    pub meta_projection: Event,
    /// Projection generation used by every prepared head.
    pub projection_generation: u64,
    /// Stable JSON object returned to duplicate retries.
    pub receipt_result: Value,
}

/// Durable idempotency receipt for one accepted Project View mutation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectViewReceipt {
    /// Signed member command event ID.
    pub event_id: [u8; 32],
    /// Project revision allocated to this mutation.
    pub project_revision: u64,
    /// Verified command author.
    pub actor_pubkey: PublicKey,
    /// Stable operation spelling.
    pub operation: String,
    /// Object type for create/update/delete; absent for initialize.
    pub object_type: Option<ProjectViewObjectType>,
    /// Object ID for create/update/delete; absent for initialize.
    pub object_id: Option<Uuid>,
    /// Stable successful command response.
    pub result: Value,
    /// Canonical database acceptance time.
    pub accepted_at: DateTime<Utc>,
}

/// Result of committing or replaying a prepared mutation.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectViewCommitOutcome {
    /// Stored receipt returned to the caller.
    pub receipt: ProjectViewReceipt,
    /// `true` when no write occurred because this event ID was already accepted.
    pub replayed: bool,
}

/// Operator-facing Project View status for one Community.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewFeatureStatus {
    /// Community identifier.
    pub community_id: CommunityId,
    /// Normalized Community host.
    pub host: String,
    /// Whether the Community has been archived.
    pub archived: bool,
    /// Centralized Project View write/capability flag.
    pub enabled: bool,
    /// Current project revision when initialized.
    pub project_revision: Option<u64>,
    /// Current projection generation when initialized.
    pub projection_generation: Option<u64>,
    /// Current projection signer when initialized.
    pub projection_pubkey: Option<PublicKey>,
}

/// Caller-owned Project View transaction holding the Community advisory lock.
///
/// Dropping this value before [`Self::commit_mutation`] rolls the SQL
/// transaction back.
pub struct ProjectViewWriteTx {
    tx: Transaction<'static, Postgres>,
    community_id: CommunityId,
    loaded_basis: Option<ProjectViewLoadedBasis>,
}

#[derive(Debug, Clone)]
struct ProjectViewLoadedBasis {
    state: ProjectViewState,
    canonical_time: DateTime<Utc>,
}

impl std::fmt::Debug for ProjectViewWriteTx {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectViewWriteTx")
            .field("community_id", &self.community_id)
            .finish_non_exhaustive()
    }
}

impl Db {
    /// Begin a Project View writer transaction for one Community.
    ///
    /// This acquires the shared namespace's exclusive advisory lock and checks
    /// the centralized feature flag from the writer database before exposing
    /// any canonical state.
    pub async fn begin_project_view_write(
        &self,
        community_id: CommunityId,
    ) -> ProjectViewWriteResult<ProjectViewWriteTx> {
        let mut tx = self.pool.begin().await?;
        acquire_project_view_lock(&mut tx, community_id, false).await?;

        let enabled = sqlx::query_scalar::<_, bool>(
            "SELECT project_view_enabled FROM communities \
             WHERE id = $1 AND archived_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;

        if enabled != Some(true) {
            return Err(ProjectViewWriteError::Unavailable { community_id });
        }

        Ok(ProjectViewWriteTx {
            tx,
            community_id,
            loaded_basis: None,
        })
    }

    /// Return Project View status for every Community in stable UUID order.
    pub async fn list_project_view_statuses(&self) -> crate::Result<Vec<ProjectViewFeatureStatus>> {
        let rows = sqlx::query(
            "SELECT c.id, c.host, c.archived_at IS NOT NULL AS archived, \
                    c.project_view_enabled, s.project_revision, \
                    s.projection_generation, s.projection_pubkey \
             FROM communities c \
             LEFT JOIN project_view_state s ON s.community_id = c.id \
             ORDER BY c.id",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter().map(status_from_row).collect()
    }

    /// Return Project View status for one normalized Community host.
    pub async fn project_view_status_by_host(
        &self,
        normalized_host: &str,
    ) -> crate::Result<Option<ProjectViewFeatureStatus>> {
        let row = sqlx::query(
            "SELECT c.id, c.host, c.archived_at IS NOT NULL AS archived, \
                    c.project_view_enabled, s.project_revision, \
                    s.projection_generation, s.projection_pubkey \
             FROM communities c \
             LEFT JOIN project_view_state s ON s.community_id = c.id \
             WHERE lower(c.host) = lower($1)",
        )
        .bind(normalized_host)
        .fetch_optional(&self.pool)
        .await?;

        row.map(status_from_row).transpose()
    }

    /// Atomically enable or disable Project View for one Community.
    ///
    /// The same exclusive advisory lock is used by mutation writers, so a
    /// successful disable cannot race with a later commit from an in-flight
    /// transaction.
    pub async fn set_project_view_enabled(
        &self,
        community_id: CommunityId,
        enabled: bool,
    ) -> crate::Result<bool> {
        let mut tx = self.pool.begin().await?;
        acquire_project_view_lock(&mut tx, community_id, false)
            .await
            .map_err(project_view_error_to_db)?;
        let result = sqlx::query(
            "UPDATE communities SET project_view_enabled = $2 \
             WHERE id = $1 AND archived_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically enable or disable Project View for all active Communities.
    ///
    /// Locks are acquired in UUID order inside one transaction to prevent
    /// cross-admin deadlocks and mutation interleaving.
    pub async fn set_all_project_views_enabled(&self, enabled: bool) -> crate::Result<u64> {
        let mut tx = self.pool.begin().await?;
        let community_ids: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM communities WHERE archived_at IS NULL ORDER BY id")
                .fetch_all(&mut *tx)
                .await?;

        for community_id in &community_ids {
            acquire_project_view_lock(&mut tx, CommunityId::from_uuid(*community_id), false)
                .await
                .map_err(project_view_error_to_db)?;
        }

        let result = sqlx::query(
            "UPDATE communities SET project_view_enabled = $1 WHERE archived_at IS NULL",
        )
        .bind(enabled)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(result.rows_affected())
    }
}

impl ProjectViewWriteTx {
    /// Return the Community protected by this transaction.
    #[must_use]
    pub const fn community_id(&self) -> CommunityId {
        self.community_id
    }

    /// Explicitly roll back this transaction and release its advisory lock.
    pub async fn rollback(self) -> ProjectViewWriteResult<()> {
        self.tx.rollback().await?;
        Ok(())
    }

    /// Look up a previously accepted event without changing state.
    pub async fn find_receipt(
        &mut self,
        event_id: &[u8],
    ) -> ProjectViewWriteResult<Option<ProjectViewReceipt>> {
        let row = sqlx::query(
            "SELECT event_id, project_revision, actor_pubkey, operation, \
                    object_type, object_id, result, accepted_at \
             FROM project_view_mutations \
             WHERE community_id = $1 AND event_id = $2",
        )
        .bind(self.community_id.as_uuid())
        .bind(event_id)
        .fetch_optional(&mut *self.tx)
        .await?;

        row.map(receipt_from_row).transpose()
    }

    /// Lock and reconstruct the current canonical state with one set-based query.
    ///
    /// The returned database timestamp is monotonic with respect to the last
    /// committed project timestamp, even if the database wall clock moves
    /// backwards.
    pub async fn load_current(&mut self) -> ProjectViewWriteResult<ProjectViewWriteContext> {
        let state_row = sqlx::query(
            "SELECT project_revision, active_object_count, initialized_at, updated_at, \
                    last_event_id, last_actor_pubkey, meta_projection_event_id, \
                    projection_pubkey, projection_generation \
             FROM project_view_state WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?;

        let Some(state_row) = state_row else {
            let canonical_time: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
                .fetch_one(&mut *self.tx)
                .await?;
            let state = ProjectViewState::new(self.community_id);
            self.loaded_basis = Some(ProjectViewLoadedBasis {
                state: state.clone(),
                canonical_time,
            });
            return Ok(ProjectViewWriteContext {
                state,
                metadata: None,
                canonical_time,
            });
        };

        let metadata = state_metadata_from_row(&state_row)?;
        let rows = sqlx::query(
            "SELECT object_id, object_type, object_revision, project_revision, body, \
                    under_goal_id, under_plan_id, planned_in_stage_id, \
                    about_object_id, about_object_type, handles_object_id, \
                    handles_object_type, created_at, updated_at, created_by, \
                    updated_by, deleted_at \
             FROM project_view_objects \
             WHERE community_id = $1 ORDER BY object_id",
        )
        .bind(self.community_id.as_uuid())
        .fetch_all(&mut *self.tx)
        .await?;

        let entries = rows
            .into_iter()
            .map(entry_from_row)
            .collect::<ProjectViewWriteResult<Vec<_>>>()?;
        let state = ProjectViewState::from_snapshot(
            self.community_id,
            metadata.project_revision,
            Some(metadata.initialized_at),
            Some(metadata.updated_at),
            entries,
        )?;
        let actual_count = u32::try_from(state.active_objects().count()).map_err(|_| {
            ProjectViewWriteError::InvalidCommit(
                "active Project View object count exceeds u32".to_owned(),
            )
        })?;
        if actual_count != metadata.active_object_count {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "active object count mismatch: state {}, objects {actual_count}",
                metadata.active_object_count
            )));
        }

        let canonical_time: DateTime<Utc> = sqlx::query_scalar(
            "SELECT GREATEST(clock_timestamp(), $1::timestamptz + interval '1 microsecond')",
        )
        .bind(metadata.updated_at)
        .fetch_one(&mut *self.tx)
        .await?;

        self.loaded_basis = Some(ProjectViewLoadedBasis {
            state: state.clone(),
            canonical_time,
        });
        Ok(ProjectViewWriteContext {
            state,
            metadata: Some(metadata),
            canonical_time,
        })
    }

    /// Commit a member command, canonical state, receipt, and all projections.
    ///
    /// A duplicate accepted event returns its stored receipt without allocating
    /// another revision. Any error before the final SQL commit rolls every
    /// Project View and event-store write back together.
    pub async fn commit_mutation(
        mut self,
        commit: PreparedProjectViewCommit,
    ) -> ProjectViewWriteResult<ProjectViewCommitOutcome> {
        validate_prepared_commit(&commit)?;
        if commit.next_state.project_id() != self.community_id {
            return Err(ProjectViewWriteError::InvalidCommit(
                "next state belongs to a different Community".to_owned(),
            ));
        }
        commit.next_state.validate()?;

        let command_event_id = commit.command_event.id.as_bytes();
        if let Some(receipt) = self.find_receipt(command_event_id).await? {
            self.tx.commit().await?;
            return Ok(ProjectViewCommitOutcome {
                receipt,
                replayed: true,
            });
        }

        let current_row = sqlx::query(
            "SELECT project_revision, initialized_at, updated_at, \
                    meta_projection_event_id, projection_pubkey, projection_generation \
             FROM project_view_state WHERE community_id = $1 FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .fetch_optional(&mut *self.tx)
        .await?;
        let current_revision = current_row
            .as_ref()
            .map(|row| db_revision_to_u64(row.try_get("project_revision")?, "project_revision"))
            .transpose()?;
        if current_revision != Some(commit.mutation.expected_project_revision)
            && !(current_revision.is_none()
                && commit.mutation.expected_project_revision == 0
                && matches!(&commit.mutation.request, MutationRequest::Initialize(_)))
        {
            return Err(ProjectViewWriteError::RevisionConflict {
                expected: commit.mutation.expected_project_revision,
                current: current_revision,
            });
        }
        if current_revision.is_some()
            && matches!(&commit.mutation.request, MutationRequest::Initialize(_))
        {
            return Err(ProjectViewWriteError::RevisionConflict {
                expected: commit.mutation.expected_project_revision,
                current: current_revision,
            });
        }
        if current_revision.is_none()
            && !matches!(&commit.mutation.request, MutationRequest::Initialize(_))
        {
            return Err(ProjectViewWriteError::RevisionConflict {
                expected: commit.mutation.expected_project_revision,
                current: None,
            });
        }

        let loaded_basis = self.loaded_basis.as_ref().ok_or_else(|| {
            ProjectViewWriteError::InvalidCommit(
                "a new mutation commit must be prepared from load_current on the same transaction"
                    .to_owned(),
            )
        })?;
        if loaded_basis.state.project_revision() != commit.mutation.expected_project_revision {
            return Err(ProjectViewWriteError::InvalidCommit(
                "loaded Project View basis does not match the mutation revision".to_owned(),
            ));
        }
        let (derived_state, derived_outcome) = loaded_basis.state.reduce(
            &commit.mutation,
            commit.command_event.pubkey,
            loaded_basis.canonical_time,
        )?;
        if derived_state != commit.next_state || derived_outcome != commit.outcome {
            return Err(ProjectViewWriteError::InvalidCommit(
                "prepared state or changed entries were not derived from the loaded mutation basis"
                    .to_owned(),
            ));
        }

        let expected_next_revision = commit
            .mutation
            .expected_project_revision
            .checked_add(1)
            .ok_or_else(|| {
                ProjectViewWriteError::InvalidCommit(
                    "project revision overflow while preparing commit".to_owned(),
                )
            })?;
        if commit.next_state.project_revision() != expected_next_revision {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "next project revision must be {expected_next_revision}"
            )));
        }

        let initialized_at = commit.next_state.initialized_at().ok_or_else(|| {
            ProjectViewWriteError::InvalidCommit(
                "committed Project View state must be initialized".to_owned(),
            )
        })?;
        let updated_at = commit.next_state.updated_at().ok_or_else(|| {
            ProjectViewWriteError::InvalidCommit(
                "committed Project View state must have an update time".to_owned(),
            )
        })?;
        if let Some(row) = current_row.as_ref() {
            let stored_initialized_at: DateTime<Utc> = row.try_get("initialized_at")?;
            let stored_updated_at: DateTime<Utc> = row.try_get("updated_at")?;
            if initialized_at != stored_initialized_at {
                return Err(ProjectViewWriteError::InvalidCommit(
                    "initialization time is immutable".to_owned(),
                ));
            }
            if updated_at <= stored_updated_at {
                return Err(ProjectViewWriteError::InvalidCommit(
                    "canonical update time must increase".to_owned(),
                ));
            }

            let stored_pubkey: Vec<u8> = row.try_get("projection_pubkey")?;
            let stored_pubkey = public_key_from_bytes(&stored_pubkey, "projection_pubkey")?;
            let stored_generation = db_revision_to_u64(
                row.try_get("projection_generation")?,
                "projection_generation",
            )?;
            if stored_pubkey != commit.meta_projection.pubkey
                || stored_generation != commit.projection_generation
            {
                return Err(ProjectViewWriteError::InvalidCommit(
                    "prepared signer/generation is not the current ready signer".to_owned(),
                ));
            }
        } else if commit.projection_generation != 1 {
            return Err(ProjectViewWriteError::InvalidCommit(
                "initial projection generation must be one".to_owned(),
            ));
        }

        let command_actor = commit.command_event.pubkey;
        let command_actor_bytes = command_actor.to_bytes();
        for entry in &commit.outcome.changed_entries {
            let actor = match entry {
                ProjectViewEntry::Active(object) => object.updated_by,
                ProjectViewEntry::Tombstone(tombstone) => tombstone.deleted_by,
            };
            if actor != command_actor {
                return Err(ProjectViewWriteError::InvalidCommit(format!(
                    "changed object {} actor differs from command author",
                    entry.id()
                )));
            }
            if entry.project_revision() != commit.outcome.project_revision {
                return Err(ProjectViewWriteError::InvalidCommit(format!(
                    "changed object {} carries the wrong project revision",
                    entry.id()
                )));
            }
        }

        let changed_ids = commit
            .outcome
            .changed_entries
            .iter()
            .map(ProjectViewEntry::id)
            .collect::<Vec<_>>();
        let old_projection_rows = sqlx::query(
            "SELECT object_id, projection_event_id \
             FROM project_view_objects \
             WHERE community_id = $1 AND object_id = ANY($2) \
             FOR UPDATE",
        )
        .bind(self.community_id.as_uuid())
        .bind(&changed_ids)
        .fetch_all(&mut *self.tx)
        .await?;
        let old_projection_ids = old_projection_rows
            .into_iter()
            .map(|row| {
                let object_id: Uuid = row.try_get("object_id")?;
                let event_id: Vec<u8> = row.try_get("projection_event_id")?;
                Ok((object_id, event_id))
            })
            .collect::<std::result::Result<BTreeMap<_, _>, sqlx::Error>>()?;

        match &commit.mutation.request {
            MutationRequest::Initialize(_) | MutationRequest::Create(_) => {
                if !old_projection_ids.is_empty() {
                    return Err(ProjectViewWriteError::InvalidCommit(
                        "create attempted to reuse an occupied object ID".to_owned(),
                    ));
                }
            }
            MutationRequest::Update(update) => {
                if !old_projection_ids.contains_key(&update.object_id()) {
                    return Err(ProjectViewWriteError::InvalidCommit(
                        "update target has no canonical object row".to_owned(),
                    ));
                }
            }
            MutationRequest::Delete(delete) => {
                if !old_projection_ids.contains_key(&delete.object_id) {
                    return Err(ProjectViewWriteError::InvalidCommit(
                        "delete target has no canonical object row".to_owned(),
                    ));
                }
            }
        }

        let projections = projection_map(commit.object_projections);
        let (_, command_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &commit.command_event,
            None,
        )
        .await?;
        if !command_inserted {
            return Err(ProjectViewWriteError::InvalidCommit(
                "command event exists without its Project View receipt".to_owned(),
            ));
        }

        let next_revision_i64 =
            revision_to_i64(commit.next_state.project_revision(), "project_revision")?;
        let generation_i64 =
            revision_to_i64(commit.projection_generation, "projection_generation")?;
        let meta_event_id = commit.meta_projection.id.as_bytes();
        let projection_pubkey = commit.meta_projection.pubkey.to_bytes();

        if current_row.is_none() {
            sqlx::query(
                "INSERT INTO project_view_state \
                    (community_id, project_revision, active_object_count, \
                     initialized_at, updated_at, last_event_id, last_actor_pubkey, \
                     meta_projection_event_id, projection_pubkey, projection_generation) \
                 VALUES ($1, $2, 0, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(self.community_id.as_uuid())
            .bind(next_revision_i64)
            .bind(initialized_at)
            .bind(updated_at)
            .bind(command_event_id.as_slice())
            .bind(command_actor_bytes.as_slice())
            .bind(meta_event_id.as_slice())
            .bind(projection_pubkey.as_slice())
            .bind(generation_i64)
            .execute(&mut *self.tx)
            .await?;
        }

        let (operation, object_type, object_id) = mutation_identity(&commit.mutation);
        sqlx::query(
            "INSERT INTO project_view_mutations \
                (community_id, event_id, project_revision, actor_pubkey, operation, \
                 object_type, object_id, result, accepted_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(self.community_id.as_uuid())
        .bind(command_event_id.as_slice())
        .bind(next_revision_i64)
        .bind(command_actor_bytes.as_slice())
        .bind(operation)
        .bind(object_type.map(ProjectViewObjectType::as_str))
        .bind(object_id)
        .bind(&commit.receipt_result)
        .bind(updated_at)
        .execute(&mut *self.tx)
        .await?;

        for (object_id, old_event_id) in &old_projection_ids {
            let retired = crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                old_event_id,
                KIND_PROJECT_VIEW_OBJECT,
            )
            .await?;
            if !retired {
                return Err(ProjectViewWriteError::InvalidCommit(format!(
                    "stored projection pointer for object {object_id} is not live"
                )));
            }
        }

        if let Some(row) = current_row.as_ref() {
            let old_meta_event_id: Vec<u8> = row.try_get("meta_projection_event_id")?;
            let retired = crate::event::retire_projection_head_in_tx(
                &mut self.tx,
                self.community_id,
                &old_meta_event_id,
                KIND_PROJECT_VIEW_META,
            )
            .await?;
            if !retired {
                return Err(ProjectViewWriteError::InvalidCommit(
                    "stored metadata projection pointer is not live".to_owned(),
                ));
            }
        }

        for entry in &commit.outcome.changed_entries {
            let projection = projections.get(&entry.id()).ok_or_else(|| {
                ProjectViewWriteError::InvalidCommit(format!(
                    "missing projection for changed object {}",
                    entry.id()
                ))
            })?;
            write_project_view_entry(
                &mut self.tx,
                self.community_id,
                command_event_id,
                projection.id.as_bytes(),
                entry,
            )
            .await?;

            let (_, inserted) =
                crate::event::insert_event_in_tx(&mut self.tx, self.community_id, projection, None)
                    .await?;
            if !inserted {
                return Err(ProjectViewWriteError::InvalidCommit(format!(
                    "object projection for {} already exists",
                    entry.id()
                )));
            }
        }

        let (_, meta_inserted) = crate::event::insert_event_in_tx(
            &mut self.tx,
            self.community_id,
            &commit.meta_projection,
            None,
        )
        .await?;
        if !meta_inserted {
            return Err(ProjectViewWriteError::InvalidCommit(
                "metadata projection already exists".to_owned(),
            ));
        }

        if current_row.is_some() {
            let result = sqlx::query(
                "UPDATE project_view_state \
                 SET project_revision = $2, updated_at = $3, last_event_id = $4, \
                     last_actor_pubkey = $5, meta_projection_event_id = $6, \
                     projection_pubkey = $7, projection_generation = $8 \
                 WHERE community_id = $1 AND project_revision = $9",
            )
            .bind(self.community_id.as_uuid())
            .bind(next_revision_i64)
            .bind(updated_at)
            .bind(command_event_id.as_slice())
            .bind(command_actor_bytes.as_slice())
            .bind(meta_event_id.as_slice())
            .bind(projection_pubkey.as_slice())
            .bind(generation_i64)
            .bind(revision_to_i64(
                commit.mutation.expected_project_revision,
                "expected_project_revision",
            )?)
            .execute(&mut *self.tx)
            .await?;
            if result.rows_affected() != 1 {
                return Err(ProjectViewWriteError::RevisionConflict {
                    expected: commit.mutation.expected_project_revision,
                    current: current_revision,
                });
            }
        }

        let stored_count: i32 = sqlx::query_scalar(
            "SELECT active_object_count FROM project_view_state WHERE community_id = $1",
        )
        .bind(self.community_id.as_uuid())
        .fetch_one(&mut *self.tx)
        .await?;
        let expected_count = active_count(&commit.next_state)?;
        if stored_count != expected_count {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "active count trigger produced {stored_count}, expected {expected_count}"
            )));
        }

        // Surface deferred FK/aggregate failures before returning from commit,
        // while every write is still rollback-safe.
        sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *self.tx)
            .await?;

        let receipt = ProjectViewReceipt {
            event_id: *command_event_id,
            project_revision: commit.next_state.project_revision(),
            actor_pubkey: command_actor,
            operation: operation.to_owned(),
            object_type,
            object_id,
            result: commit.receipt_result,
            accepted_at: updated_at,
        };
        self.tx.commit().await?;
        Ok(ProjectViewCommitOutcome {
            receipt,
            replayed: false,
        })
    }
}

async fn write_project_view_entry(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    source_event_id: &[u8],
    projection_event_id: &[u8],
    entry: &ProjectViewEntry,
) -> ProjectViewWriteResult<()> {
    let (
        object_id,
        object_type,
        object_revision,
        project_revision,
        body,
        relations,
        created_at,
        updated_at,
        created_by,
        updated_by,
        deleted_at,
    ) = match entry {
        ProjectViewEntry::Active(object) => (
            object.id,
            object.object_type,
            object.object_revision,
            object.project_revision,
            Some(object_body(&object.data)?),
            object.relations,
            object.created_at,
            object.updated_at,
            object.created_by,
            object.updated_by,
            None,
        ),
        ProjectViewEntry::Tombstone(tombstone) => (
            tombstone.id,
            tombstone.object_type,
            tombstone.object_revision,
            tombstone.project_revision,
            None,
            ProjectViewRelations::default(),
            tombstone.created_at,
            tombstone.deleted_at,
            tombstone.created_by,
            tombstone.deleted_by,
            Some(tombstone.deleted_at),
        ),
    };
    let object_revision = revision_to_i64(object_revision, "object_revision")?;
    let project_revision = revision_to_i64(project_revision, "project_revision")?;
    let created_by = created_by.to_bytes();
    let updated_by = updated_by.to_bytes();
    let about_object_id = relations.about.map(|reference| reference.object_id);
    let about_object_type = relations
        .about
        .map(|reference| reference.object_type.as_str());
    let handles_object_id = relations.handles.map(|reference| reference.object_id);
    let handles_object_type = relations
        .handles
        .map(|reference| reference.object_type.as_str());

    let result = sqlx::query(
        "INSERT INTO project_view_objects \
            (community_id, object_id, object_type, schema_version, \
             object_revision, project_revision, body, under_goal_id, \
             under_plan_id, planned_in_stage_id, about_object_id, \
             about_object_type, handles_object_id, handles_object_type, \
             created_at, updated_at, created_by, updated_by, source_event_id, \
             projection_event_id, deleted_at) \
         VALUES \
            ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, \
             $14, $15, $16, $17, $18, $19, $20, $21) \
         ON CONFLICT (community_id, object_id) DO UPDATE SET \
             object_type = EXCLUDED.object_type, \
             schema_version = EXCLUDED.schema_version, \
             object_revision = EXCLUDED.object_revision, \
             project_revision = EXCLUDED.project_revision, \
             body = EXCLUDED.body, \
             under_goal_id = EXCLUDED.under_goal_id, \
             under_plan_id = EXCLUDED.under_plan_id, \
             planned_in_stage_id = EXCLUDED.planned_in_stage_id, \
             about_object_id = EXCLUDED.about_object_id, \
             about_object_type = EXCLUDED.about_object_type, \
             handles_object_id = EXCLUDED.handles_object_id, \
             handles_object_type = EXCLUDED.handles_object_type, \
             created_at = EXCLUDED.created_at, \
             updated_at = EXCLUDED.updated_at, \
             created_by = EXCLUDED.created_by, \
             updated_by = EXCLUDED.updated_by, \
             source_event_id = EXCLUDED.source_event_id, \
             projection_event_id = EXCLUDED.projection_event_id, \
             deleted_at = EXCLUDED.deleted_at \
         WHERE project_view_objects.deleted_at IS NULL \
           AND project_view_objects.object_type = EXCLUDED.object_type \
           AND project_view_objects.object_revision + 1 = EXCLUDED.object_revision \
           AND project_view_objects.project_revision < EXCLUDED.project_revision",
    )
    .bind(community_id.as_uuid())
    .bind(object_id)
    .bind(object_type.as_str())
    .bind(i16::try_from(MUTATION_SCHEMA_VERSION).map_err(|_| {
        ProjectViewWriteError::InvalidCommit(
            "mutation schema version does not fit PostgreSQL SMALLINT".to_owned(),
        )
    })?)
    .bind(object_revision)
    .bind(project_revision)
    .bind(body)
    .bind(relations.under_goal_id)
    .bind(relations.under_plan_id)
    .bind(relations.planned_in_stage_id)
    .bind(about_object_id)
    .bind(about_object_type)
    .bind(handles_object_id)
    .bind(handles_object_type)
    .bind(created_at)
    .bind(updated_at)
    .bind(created_by.as_slice())
    .bind(updated_by.as_slice())
    .bind(source_event_id)
    .bind(projection_event_id)
    .bind(deleted_at)
    .execute(&mut **tx)
    .await?;

    if result.rows_affected() != 1 {
        return Err(ProjectViewWriteError::InvalidCommit(format!(
            "canonical object {object_id} did not advance exactly one revision"
        )));
    }
    Ok(())
}

async fn acquire_project_view_lock(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    shared: bool,
) -> ProjectViewWriteResult<()> {
    let function = if shared {
        "pg_advisory_xact_lock_shared"
    } else {
        "pg_advisory_xact_lock"
    };
    let sql = format!("SELECT {function}(hashtextextended($1, 0))");
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(format!(
            "{PROJECT_VIEW_LOCK_NAMESPACE}{}",
            community_id.as_uuid()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

fn status_from_row(row: sqlx::postgres::PgRow) -> crate::Result<ProjectViewFeatureStatus> {
    let community_id: Uuid = row.try_get("id")?;
    let project_revision: Option<i64> = row.try_get("project_revision")?;
    let projection_generation: Option<i64> = row.try_get("projection_generation")?;
    let projection_pubkey: Option<Vec<u8>> = row.try_get("projection_pubkey")?;

    Ok(ProjectViewFeatureStatus {
        community_id: CommunityId::from_uuid(community_id),
        host: row.try_get("host")?,
        archived: row.try_get("archived")?,
        enabled: row.try_get("project_view_enabled")?,
        project_revision: project_revision
            .map(|revision| db_revision_to_u64(revision, "project_revision"))
            .transpose()?,
        projection_generation: projection_generation
            .map(|generation| db_revision_to_u64(generation, "projection_generation"))
            .transpose()?,
        projection_pubkey: projection_pubkey
            .map(|bytes| public_key_from_bytes(&bytes, "projection_pubkey"))
            .transpose()?,
    })
}

fn state_metadata_from_row(
    row: &sqlx::postgres::PgRow,
) -> ProjectViewWriteResult<ProjectViewStateMetadata> {
    let active_object_count: i32 = row.try_get("active_object_count")?;
    let active_object_count = u32::try_from(active_object_count).map_err(|_| {
        ProjectViewWriteError::InvalidCommit(
            "negative or oversized active_object_count in Project View state".to_owned(),
        )
    })?;
    let last_event_id: Vec<u8> = row.try_get("last_event_id")?;
    let last_actor_pubkey: Vec<u8> = row.try_get("last_actor_pubkey")?;
    let meta_projection_event_id: Vec<u8> = row.try_get("meta_projection_event_id")?;
    let projection_pubkey: Vec<u8> = row.try_get("projection_pubkey")?;

    Ok(ProjectViewStateMetadata {
        project_revision: db_revision_to_u64(row.try_get("project_revision")?, "project_revision")?,
        active_object_count,
        initialized_at: row.try_get("initialized_at")?,
        updated_at: row.try_get("updated_at")?,
        last_event_id: bytes32(last_event_id, "last_event_id")?,
        last_actor_pubkey: public_key_from_bytes(&last_actor_pubkey, "last_actor_pubkey")?,
        meta_projection_event_id: bytes32(meta_projection_event_id, "meta_projection_event_id")?,
        projection_pubkey: public_key_from_bytes(&projection_pubkey, "projection_pubkey")?,
        projection_generation: db_revision_to_u64(
            row.try_get("projection_generation")?,
            "projection_generation",
        )?,
    })
}

fn entry_from_row(row: sqlx::postgres::PgRow) -> ProjectViewWriteResult<ProjectViewEntry> {
    let object_id: Uuid = row.try_get("object_id")?;
    let object_type_text: String = row.try_get("object_type")?;
    let object_type = parse_object_type(&object_type_text)?;
    let object_revision = db_revision_to_u64(row.try_get("object_revision")?, "object_revision")?;
    let project_revision =
        db_revision_to_u64(row.try_get("project_revision")?, "project_revision")?;
    let created_at: DateTime<Utc> = row.try_get("created_at")?;
    let updated_at: DateTime<Utc> = row.try_get("updated_at")?;
    let created_by_bytes: Vec<u8> = row.try_get("created_by")?;
    let updated_by_bytes: Vec<u8> = row.try_get("updated_by")?;
    let created_by = public_key_from_bytes(&created_by_bytes, "created_by")?;
    let updated_by = public_key_from_bytes(&updated_by_bytes, "updated_by")?;
    let deleted_at: Option<DateTime<Utc>> = row.try_get("deleted_at")?;

    if let Some(deleted_at) = deleted_at {
        return Ok(ProjectViewEntry::Tombstone(ProjectViewTombstone {
            id: object_id,
            object_type,
            object_revision,
            project_revision,
            created_at,
            deleted_at,
            created_by,
            deleted_by: updated_by,
        }));
    }

    let body: Value = row.try_get("body")?;
    let data = object_data_from_body(object_type, body)?;
    let about_object_id: Option<Uuid> = row.try_get("about_object_id")?;
    let about_object_type: Option<String> = row.try_get("about_object_type")?;
    let handles_object_id: Option<Uuid> = row.try_get("handles_object_id")?;
    let handles_object_type: Option<String> = row.try_get("handles_object_type")?;
    let relations = ProjectViewRelations {
        under_goal_id: row.try_get("under_goal_id")?,
        under_plan_id: row.try_get("under_plan_id")?,
        planned_in_stage_id: row.try_get("planned_in_stage_id")?,
        about: typed_reference(about_object_id, about_object_type, "about")?,
        handles: typed_reference(handles_object_id, handles_object_type, "handles")?,
    };

    Ok(ProjectViewEntry::Active(ProjectViewObject {
        id: object_id,
        object_type,
        object_revision,
        project_revision,
        created_at,
        updated_at,
        created_by,
        updated_by,
        data,
        relations,
    }))
}

fn receipt_from_row(row: sqlx::postgres::PgRow) -> ProjectViewWriteResult<ProjectViewReceipt> {
    let event_id: Vec<u8> = row.try_get("event_id")?;
    let actor_pubkey: Vec<u8> = row.try_get("actor_pubkey")?;
    let object_type: Option<String> = row.try_get("object_type")?;
    Ok(ProjectViewReceipt {
        event_id: bytes32(event_id, "event_id")?,
        project_revision: db_revision_to_u64(row.try_get("project_revision")?, "project_revision")?,
        actor_pubkey: public_key_from_bytes(&actor_pubkey, "actor_pubkey")?,
        operation: row.try_get("operation")?,
        object_type: object_type
            .map(|value| parse_object_type(&value))
            .transpose()?,
        object_id: row.try_get("object_id")?,
        result: row.try_get("result")?,
        accepted_at: row.try_get("accepted_at")?,
    })
}

fn typed_reference(
    object_id: Option<Uuid>,
    object_type: Option<String>,
    field: &'static str,
) -> ProjectViewWriteResult<Option<buzz_project_view::ObjectRef>> {
    match (object_id, object_type) {
        (None, None) => Ok(None),
        (Some(object_id), Some(object_type)) => Ok(Some(buzz_project_view::ObjectRef {
            object_type: parse_object_type(&object_type)?,
            object_id,
        })),
        _ => Err(ProjectViewWriteError::InvalidCommit(format!(
            "stored {field} relation has an incomplete id/type pair"
        ))),
    }
}

fn object_data_from_body(
    object_type: ProjectViewObjectType,
    body: Value,
) -> ProjectViewWriteResult<ProjectViewObjectData> {
    serde_json::from_value(serde_json::json!({
        "object_type": object_type.as_str(),
        "data": body,
    }))
    .map_err(|error| {
        ProjectViewWriteError::Database(DbError::InvalidData(format!(
            "invalid stored {} body: {error}",
            object_type.as_str()
        )))
    })
}

fn parse_object_type(value: &str) -> ProjectViewWriteResult<ProjectViewObjectType> {
    let object_type = match value {
        "project_profile" => ProjectViewObjectType::ProjectProfile,
        "goal" => ProjectViewObjectType::Goal,
        "role" => ProjectViewObjectType::Role,
        "plan" => ProjectViewObjectType::Plan,
        "stage" => ProjectViewObjectType::Stage,
        "requirement" => ProjectViewObjectType::Requirement,
        "issue" => ProjectViewObjectType::Issue,
        "work" => ProjectViewObjectType::Work,
        "resource" => ProjectViewObjectType::Resource,
        other => {
            return Err(ProjectViewWriteError::Database(DbError::InvalidData(
                format!("unknown Project View object type: {other}"),
            )));
        }
    };
    Ok(object_type)
}

fn public_key_from_bytes(bytes: &[u8], field: &str) -> crate::Result<PublicKey> {
    PublicKey::from_slice(bytes)
        .map_err(|error| DbError::InvalidData(format!("invalid {field}: {error}")))
}

fn bytes32(bytes: Vec<u8>, field: &str) -> crate::Result<[u8; 32]> {
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        DbError::InvalidData(format!(
            "{field} must contain 32 bytes, got {}",
            bytes.len()
        ))
    })
}

fn db_revision_to_u64(value: i64, field: &str) -> crate::Result<u64> {
    u64::try_from(value)
        .map_err(|_| DbError::InvalidData(format!("{field} must be non-negative, got {value}")))
}

fn revision_to_i64(value: u64, field: &str) -> ProjectViewWriteResult<i64> {
    i64::try_from(value).map_err(|_| {
        ProjectViewWriteError::InvalidCommit(format!("{field} does not fit in PostgreSQL BIGINT"))
    })
}

fn project_view_error_to_db(error: ProjectViewWriteError) -> DbError {
    match error {
        ProjectViewWriteError::Database(error) => error,
        ProjectViewWriteError::Sqlx(error) => DbError::Sqlx(error),
        other => DbError::InvalidData(other.to_string()),
    }
}

fn mutation_identity(
    mutation: &Mutation,
) -> (&'static str, Option<ProjectViewObjectType>, Option<Uuid>) {
    match &mutation.request {
        MutationRequest::Initialize(_) => ("initialize", None, None),
        MutationRequest::Create(create) => (
            "create",
            Some(create.object.object_type()),
            Some(create.object.id()),
        ),
        MutationRequest::Update(update) => (
            "update",
            Some(update.object_type()),
            Some(update.object_id()),
        ),
        MutationRequest::Delete(delete) => {
            ("delete", Some(delete.object_type), Some(delete.object_id))
        }
    }
}

fn object_body(data: &ProjectViewObjectData) -> ProjectViewWriteResult<Value> {
    let mut value = serde_json::to_value(data).map_err(DbError::from)?;
    value.get_mut("data").map(Value::take).ok_or_else(|| {
        ProjectViewWriteError::InvalidCommit(
            "serialized Project View body is missing its data field".to_owned(),
        )
    })
}

fn active_count(state: &ProjectViewState) -> ProjectViewWriteResult<i32> {
    i32::try_from(state.active_objects().count()).map_err(|_| {
        ProjectViewWriteError::InvalidCommit(
            "active Project View object count exceeds PostgreSQL INTEGER".to_owned(),
        )
    })
}

fn validate_prepared_commit(commit: &PreparedProjectViewCommit) -> ProjectViewWriteResult<()> {
    if commit.command_event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_MUTATION {
        return Err(ProjectViewWriteError::InvalidCommit(format!(
            "command kind must be {KIND_PROJECT_VIEW_MUTATION}"
        )));
    }
    if commit.meta_projection.kind.as_u16() as u32 != KIND_PROJECT_VIEW_META {
        return Err(ProjectViewWriteError::InvalidCommit(format!(
            "meta projection kind must be {KIND_PROJECT_VIEW_META}"
        )));
    }
    let parsed_mutation = Mutation::from_json(&commit.command_event.content)?;
    if parsed_mutation != commit.mutation {
        return Err(ProjectViewWriteError::InvalidCommit(
            "command content differs from the prepared typed mutation".to_owned(),
        ));
    }
    if commit.mutation.schema_version != MUTATION_SCHEMA_VERSION {
        return Err(ProjectViewWriteError::InvalidCommit(
            "mutation schema version does not match the domain crate".to_owned(),
        ));
    }
    if commit.outcome.project_revision != commit.next_state.project_revision() {
        return Err(ProjectViewWriteError::InvalidCommit(
            "mutation outcome revision differs from next state".to_owned(),
        ));
    }
    if !commit.receipt_result.is_object() {
        return Err(ProjectViewWriteError::InvalidCommit(
            "receipt result must be a JSON object".to_owned(),
        ));
    }
    if commit.projection_generation == 0 {
        return Err(ProjectViewWriteError::InvalidCommit(
            "projection generation must start at one".to_owned(),
        ));
    }

    let changed_ids = commit
        .outcome
        .changed_entries
        .iter()
        .map(ProjectViewEntry::id)
        .collect::<BTreeSet<_>>();
    if changed_ids.len() != commit.outcome.changed_entries.len() {
        return Err(ProjectViewWriteError::InvalidCommit(
            "mutation outcome contains duplicate changed object IDs".to_owned(),
        ));
    }

    for entry in &commit.outcome.changed_entries {
        if commit.next_state.entry(entry.id()) != Some(entry) {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "changed entry {} does not match next state",
                entry.id()
            )));
        }
    }

    let mut projection_ids = BTreeSet::new();
    for projection in &commit.object_projections {
        if projection.event.kind.as_u16() as u32 != KIND_PROJECT_VIEW_OBJECT {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "object projection kind must be {KIND_PROJECT_VIEW_OBJECT}"
            )));
        }
        if projection.event.pubkey != commit.meta_projection.pubkey {
            return Err(ProjectViewWriteError::InvalidCommit(
                "object and meta projections must share one relay signer".to_owned(),
            ));
        }
        if !projection_ids.insert(projection.object_id) {
            return Err(ProjectViewWriteError::InvalidCommit(format!(
                "duplicate projection for object {}",
                projection.object_id
            )));
        }
    }
    if projection_ids != changed_ids {
        return Err(ProjectViewWriteError::InvalidCommit(
            "prepared projections must exactly cover changed object IDs".to_owned(),
        ));
    }
    Ok(())
}

fn projection_map(projections: Vec<PreparedObjectProjection>) -> BTreeMap<Uuid, Event> {
    projections
        .into_iter()
        .map(|projection| (projection.object_id, projection.event))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use buzz_project_view::{
        CreateMutation, DeleteMutation, Goal, InitializeGoal, InitializeMutation,
        NewProjectViewObject, ProjectProfile,
    };
    use nostr::{EventBuilder, Keys, Kind, Tag, Timestamp};
    use sqlx::PgPool;

    const TEST_DB_URL: &str = "postgres://buzz:buzz_dev@localhost:5432/buzz";

    struct ScratchDatabase {
        admin: PgPool,
        pool: PgPool,
        name: String,
    }

    impl ScratchDatabase {
        async fn create(prefix: &str) -> Self {
            let admin_url =
                std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| TEST_DB_URL.to_owned());
            let admin = PgPool::connect(&admin_url)
                .await
                .expect("connect test database server");
            let name = format!("{prefix}_{}", Uuid::new_v4().simple());
            sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
                .execute(&admin)
                .await
                .expect("create Project View scratch database");
            let slash = admin_url.rfind('/').expect("database URL has path");
            let database_url = format!("{}/{}", &admin_url[..slash], name);
            let pool = PgPool::connect(&database_url)
                .await
                .expect("connect Project View scratch database");
            crate::migration::run_migrations(&pool)
                .await
                .expect("migrate Project View scratch database");
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
            .expect("drop Project View scratch database");
            self.admin.close().await;
        }
    }

    async fn seed_community(pool: &PgPool, enabled: bool) -> CommunityId {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities (id, host, project_view_enabled) VALUES ($1, $2, $3)")
            .bind(community_id.as_uuid())
            .bind(format!("project-view-{}.test", community_id.as_uuid()))
            .bind(enabled)
            .execute(pool)
            .await
            .expect("seed Project View community");
        community_id
    }

    fn initialize_mutation() -> Mutation {
        Mutation::new(
            0,
            MutationRequest::Initialize(InitializeMutation {
                profile: ProjectProfile {
                    name: "Project View".to_owned(),
                    positioning: "Canonical project state".to_owned(),
                    purpose: "Coordinate humans and agents".to_owned(),
                    problem: "Project context is fragmented".to_owned(),
                    scope: "Backend v0".to_owned(),
                },
                goals: vec![InitializeGoal {
                    id: Uuid::new_v4(),
                    title: "Ship the backend".to_owned(),
                    desired_outcome: "A transactionally consistent view".to_owned(),
                    directions: vec!["Preserve atomicity".to_owned()],
                }],
            }),
        )
    }

    fn create_goal_mutation(expected_revision: u64, title: &str) -> Mutation {
        create_goal_mutation_with_id(expected_revision, Uuid::new_v4(), title)
    }

    fn create_goal_mutation_with_id(
        expected_revision: u64,
        object_id: Uuid,
        title: &str,
    ) -> Mutation {
        Mutation::new(
            expected_revision,
            MutationRequest::Create(CreateMutation {
                object: NewProjectViewObject::Goal {
                    id: object_id,
                    title: title.to_owned(),
                    desired_outcome: format!("{title} is complete"),
                    directions: Vec::new(),
                },
            }),
        )
    }

    fn signed_event(
        keys: &Keys,
        kind: u32,
        content: String,
        tags: Vec<Tag>,
        canonical_time: DateTime<Utc>,
    ) -> Event {
        let timestamp =
            u64::try_from(canonical_time.timestamp()).expect("test timestamp is positive");
        EventBuilder::new(Kind::Custom(kind as u16), content)
            .tags(tags)
            .custom_created_at(Timestamp::from(timestamp))
            .sign_with_keys(keys)
            .expect("sign Project View test event")
    }

    fn prepare_commit(
        community_id: CommunityId,
        actor: &Keys,
        relay: &Keys,
        mutation: Mutation,
        next_state: ProjectViewState,
        outcome: MutationOutcome,
        canonical_time: DateTime<Utc>,
    ) -> PreparedProjectViewCommit {
        let command_event = signed_event(
            actor,
            KIND_PROJECT_VIEW_MUTATION,
            serde_json::to_string(&mutation).expect("serialize mutation"),
            vec![
                Tag::parse(["-"]).expect("protected tag"),
                Tag::parse(["t", "buzz-project-view-mutation"]).expect("type tag"),
            ],
            canonical_time,
        );
        let object_projections = outcome
            .changed_entries
            .iter()
            .map(|entry| {
                let object_id = entry.id();
                let coordinate = format!(
                    "project-view:{}:{}:{object_id}",
                    community_id.as_uuid(),
                    entry.object_type().as_str()
                );
                let event = signed_event(
                    relay,
                    KIND_PROJECT_VIEW_OBJECT,
                    serde_json::json!({
                        "object_id": object_id,
                        "project_revision": outcome.project_revision,
                    })
                    .to_string(),
                    vec![Tag::parse(["d", coordinate.as_str()]).expect("object d tag")],
                    canonical_time,
                );
                PreparedObjectProjection::new(object_id, event)
            })
            .collect();
        let meta_coordinate = format!("project-view:{}:meta", community_id.as_uuid());
        let meta_projection = signed_event(
            relay,
            KIND_PROJECT_VIEW_META,
            serde_json::json!({
                "project_revision": outcome.project_revision,
                "active_object_count": next_state.active_objects().count(),
            })
            .to_string(),
            vec![Tag::parse(["d", meta_coordinate.as_str()]).expect("meta d tag")],
            canonical_time,
        );
        let project_revision = next_state.project_revision();

        PreparedProjectViewCommit {
            command_event,
            mutation,
            next_state,
            outcome,
            object_projections,
            meta_projection,
            projection_generation: 1,
            receipt_result: serde_json::json!({
                "project_revision": project_revision,
            }),
        }
    }

    async fn initialize(
        db: &Db,
        community_id: CommunityId,
        actor: &Keys,
        relay: &Keys,
    ) -> ProjectViewCommitOutcome {
        commit_for_test(db, community_id, actor, relay, initialize_mutation()).await
    }

    async fn commit_for_test(
        db: &Db,
        community_id: CommunityId,
        actor: &Keys,
        relay: &Keys,
        mutation: Mutation,
    ) -> ProjectViewCommitOutcome {
        let mut tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin test mutation");
        let context = tx.load_current().await.expect("load test mutation basis");
        let (next_state, outcome) = context
            .state
            .reduce(&mutation, actor.public_key(), context.canonical_time)
            .expect("reduce test mutation");
        let prepared = prepare_commit(
            community_id,
            actor,
            relay,
            mutation,
            next_state,
            outcome,
            context.canonical_time,
        );
        tx.commit_mutation(prepared)
            .await
            .expect("commit test mutation")
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn initialize_commits_one_bundle_and_duplicate_replays_receipt() {
        let scratch = ScratchDatabase::create("buzz_pv_atomic").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();

        let mut tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin initialization");
        let context = tx.load_current().await.expect("load uninitialized state");
        let mutation = initialize_mutation();
        let (next_state, outcome) = context
            .state
            .reduce(&mutation, actor.public_key(), context.canonical_time)
            .expect("reduce initialization");
        let prepared = prepare_commit(
            community_id,
            &actor,
            &relay,
            mutation,
            next_state.clone(),
            outcome,
            context.canonical_time,
        );
        let first = tx
            .commit_mutation(prepared.clone())
            .await
            .expect("commit initialization");
        assert!(!first.replayed);
        assert_eq!(first.receipt.project_revision, 1);

        let replay_tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin duplicate retry");
        let replay = replay_tx
            .commit_mutation(prepared)
            .await
            .expect("replay duplicate receipt");
        assert!(replay.replayed);
        assert_eq!(replay.receipt, first.receipt);

        let state: (i64, i32) = sqlx::query_as(
            "SELECT project_revision, active_object_count \
             FROM project_view_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read committed state");
        assert_eq!(state, (1, 2));
        let object_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM project_view_objects WHERE community_id = $1")
                .bind(community_id.as_uuid())
                .fetch_one(&scratch.pool)
                .await
                .expect("count canonical objects");
        assert_eq!(object_count, 2);
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM project_view_mutations WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("count receipts");
        assert_eq!(receipt_count, 1);
        let event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id = $1")
                .bind(community_id.as_uuid())
                .fetch_one(&scratch.pool)
                .await
                .expect("count protocol events");
        assert_eq!(event_count, 4);

        let mut read_tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin canonical reload");
        let loaded = read_tx
            .load_current()
            .await
            .expect("reload canonical state");
        assert_eq!(loaded.state, next_state);
        read_tx.rollback().await.expect("release read transaction");

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn projection_insert_failure_rolls_back_command_state_objects_and_receipt() {
        let scratch = ScratchDatabase::create("buzz_pv_rollback").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();

        let mut tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin initialization");
        let context = tx.load_current().await.expect("load uninitialized state");
        let mutation = initialize_mutation();
        let (next_state, outcome) = context
            .state
            .reduce(&mutation, actor.public_key(), context.canonical_time)
            .expect("reduce initialization");
        let prepared = prepare_commit(
            community_id,
            &actor,
            &relay,
            mutation,
            next_state,
            outcome,
            context.canonical_time,
        );
        let duplicate_projection = prepared
            .object_projections
            .first()
            .expect("initialization has projections")
            .event()
            .clone();
        crate::event::insert_event(&scratch.pool, community_id, &duplicate_projection, None)
            .await
            .expect("seed conflicting projection");

        let error = tx
            .commit_mutation(prepared)
            .await
            .expect_err("duplicate projection must abort the transaction");
        assert!(matches!(error, ProjectViewWriteError::InvalidCommit(_)));

        for table in [
            "project_view_state",
            "project_view_objects",
            "project_view_mutations",
        ] {
            let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT count(*) FROM {table} WHERE community_id = $1"
            )))
            .bind(community_id.as_uuid())
            .fetch_one(&scratch.pool)
            .await
            .unwrap_or_else(|error| panic!("count {table}: {error}"));
            assert_eq!(count, 0, "{table} must roll back");
        }
        let command_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND kind = $2",
        )
        .bind(community_id.as_uuid())
        .bind(KIND_PROJECT_VIEW_MUTATION as i32)
        .fetch_one(&scratch.pool)
        .await
        .expect("count rolled-back command");
        assert_eq!(command_count, 0);
        let all_event_count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE community_id = $1")
                .bind(community_id.as_uuid())
                .fetch_one(&scratch.pool)
                .await
                .expect("count surviving seed event");
        assert_eq!(all_event_count, 1);

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn prepared_bundle_must_be_derived_from_the_locked_mutation_basis() {
        let scratch = ScratchDatabase::create("buzz_pv_derived_bundle").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();
        initialize(&db, community_id, &actor, &relay).await;

        let mut tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin forged bundle attempt");
        let context = tx.load_current().await.expect("load forged bundle basis");
        let signed_mutation = create_goal_mutation(1, "Signed goal");
        let different_mutation = create_goal_mutation(1, "Different derived goal");
        let (different_state, different_outcome) = context
            .state
            .reduce(
                &different_mutation,
                actor.public_key(),
                context.canonical_time,
            )
            .expect("reduce different mutation");
        let forged = prepare_commit(
            community_id,
            &actor,
            &relay,
            signed_mutation,
            different_state,
            different_outcome,
            context.canonical_time,
        );
        let error = tx
            .commit_mutation(forged)
            .await
            .expect_err("prepared bundle must match the signed mutation");
        assert!(matches!(error, ProjectViewWriteError::InvalidCommit(_)));

        let durable: (i64, i32, i64, i64) = sqlx::query_as(
            "SELECT s.project_revision, s.active_object_count, \
                    (SELECT count(*) FROM project_view_objects o \
                     WHERE o.community_id = s.community_id), \
                    (SELECT count(*) FROM project_view_mutations m \
                     WHERE m.community_id = s.community_id) \
             FROM project_view_state s WHERE s.community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("inspect state after forged bundle");
        assert_eq!(durable, (1, 2, 2, 1));
        let command_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events \
             WHERE community_id = $1 AND kind = $2",
        )
        .bind(community_id.as_uuid())
        .bind(KIND_PROJECT_VIEW_MUTATION as i32)
        .fetch_one(&scratch.pool)
        .await
        .expect("count accepted commands after forged bundle");
        assert_eq!(command_count, 1);

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn concurrent_same_revision_mutations_have_exactly_one_winner() {
        let scratch = ScratchDatabase::create("buzz_pv_cas").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();
        initialize(&db, community_id, &actor, &relay).await;

        let mut prepare_tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin preparation");
        let context = prepare_tx.load_current().await.expect("load revision one");
        let mutation_a = create_goal_mutation(1, "Concurrent A");
        let mutation_b = create_goal_mutation(1, "Concurrent B");
        let (state_a, outcome_a) = context
            .state
            .reduce(&mutation_a, actor.public_key(), context.canonical_time)
            .expect("reduce A");
        let (state_b, outcome_b) = context
            .state
            .reduce(&mutation_b, actor.public_key(), context.canonical_time)
            .expect("reduce B");
        let prepared_a = prepare_commit(
            community_id,
            &actor,
            &relay,
            mutation_a,
            state_a,
            outcome_a,
            context.canonical_time,
        );
        let prepared_b = prepare_commit(
            community_id,
            &actor,
            &relay,
            mutation_b,
            state_b,
            outcome_b,
            context.canonical_time,
        );
        let db_b = db.clone();
        let (result_a, result_b) =
            tokio::join!(prepare_tx.commit_mutation(prepared_a), async move {
                let mut tx = db_b
                    .begin_project_view_write(community_id)
                    .await
                    .expect("begin concurrent B");
                tx.load_current()
                    .await
                    .expect("load concurrent B commit basis");
                tx.commit_mutation(prepared_b).await
            });
        let results = [result_a, result_b];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(
                    result,
                    Err(ProjectViewWriteError::RevisionConflict {
                        expected: 1,
                        current: Some(2)
                    })
                ))
                .count(),
            1
        );

        let state: (i64, i32) = sqlx::query_as(
            "SELECT project_revision, active_object_count \
             FROM project_view_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read winning state");
        assert_eq!(state, (2, 3));
        let receipt_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM project_view_mutations WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("count accepted mutations");
        assert_eq!(receipt_count, 2);

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn tenant_keys_allow_same_object_id_but_reject_cross_community_reference() {
        let scratch = ScratchDatabase::create("buzz_pv_tenant_keys").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_a = seed_community(&scratch.pool, true).await;
        let community_b = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();
        initialize(&db, community_a, &actor, &relay).await;
        initialize(&db, community_b, &actor, &relay).await;

        let shared_object_id = Uuid::new_v4();
        commit_for_test(
            &db,
            community_a,
            &actor,
            &relay,
            create_goal_mutation_with_id(1, shared_object_id, "Community A goal"),
        )
        .await;
        commit_for_test(
            &db,
            community_b,
            &actor,
            &relay,
            create_goal_mutation_with_id(1, shared_object_id, "Community B goal"),
        )
        .await;
        let shared_rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM project_view_objects WHERE object_id = $1")
                .bind(shared_object_id)
                .fetch_one(&scratch.pool)
                .await
                .expect("count tenant-scoped duplicate IDs");
        assert_eq!(shared_rows, 2);

        let goal_only_in_a: Uuid = sqlx::query_scalar(
            "SELECT object_id FROM project_view_objects \
             WHERE community_id = $1 AND object_type = 'goal' \
               AND object_id <> $2 AND deleted_at IS NULL \
             ORDER BY object_id LIMIT 1",
        )
        .bind(community_a.as_uuid())
        .bind(shared_object_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read Community A-only relation target");
        let cross_community_plan = Uuid::new_v4();
        let actor_bytes = actor.public_key().to_bytes();
        let source_event_id = [0x11_u8; 32];
        let projection_event_id = [0x22_u8; 32];
        let mut tx = scratch
            .pool
            .begin()
            .await
            .expect("begin cross-tenant insert");
        sqlx::query(
            "INSERT INTO project_view_objects \
                (community_id, object_id, object_type, schema_version, \
                 object_revision, project_revision, body, under_goal_id, \
                 created_at, updated_at, created_by, updated_by, \
                 source_event_id, projection_event_id) \
             VALUES ($1, $2, 'plan', 1, 1, 2, $3, $4, \
                     clock_timestamp(), clock_timestamp(), $5, $5, $6, $7)",
        )
        .bind(community_b.as_uuid())
        .bind(cross_community_plan)
        .bind(serde_json::json!({
            "title": "Cross-tenant plan",
            "description": "Must not resolve through Community A",
            "status": "draft",
        }))
        .bind(goal_only_in_a)
        .bind(actor_bytes.as_slice())
        .bind(source_event_id.as_slice())
        .bind(projection_event_id.as_slice())
        .execute(&mut *tx)
        .await
        .expect("stage cross-Community relation");
        let error = sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await
            .expect_err("cross-Community relation must fail deferred validation");
        assert_eq!(
            error
                .as_database_error()
                .and_then(|error| error.code())
                .as_deref(),
            Some("23503")
        );
        tx.rollback()
            .await
            .expect("roll back cross-Community relation");
        let leaked_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM project_view_objects \
             WHERE community_id = $1 AND object_id = $2",
        )
        .bind(community_b.as_uuid())
        .bind(cross_community_plan)
        .fetch_one(&scratch.pool)
        .await
        .expect("check rejected cross-Community row");
        assert_eq!(leaked_rows, 0);

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn tombstone_clears_body_decrements_count_and_retires_exact_old_head() {
        let scratch = ScratchDatabase::create("buzz_pv_tombstone").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();
        initialize(&db, community_id, &actor, &relay).await;

        let deleted_goal_id: Uuid = sqlx::query_scalar(
            "SELECT object_id FROM project_view_objects \
             WHERE community_id = $1 AND object_type = 'goal' AND deleted_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read initial goal");
        commit_for_test(
            &db,
            community_id,
            &actor,
            &relay,
            create_goal_mutation(1, "Replacement goal"),
        )
        .await;

        let old_projection_id: Vec<u8> = sqlx::query_scalar(
            "SELECT projection_event_id FROM project_view_objects \
             WHERE community_id = $1 AND object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(deleted_goal_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read old goal projection pointer");
        let delete = Mutation::new(
            2,
            MutationRequest::Delete(DeleteMutation {
                object_type: ProjectViewObjectType::Goal,
                object_id: deleted_goal_id,
            }),
        );
        let deleted = commit_for_test(&db, community_id, &actor, &relay, delete).await;
        assert_eq!(deleted.receipt.project_revision, 3);

        let row: (Option<Value>, Option<DateTime<Utc>>, i64, i64, Vec<u8>) = sqlx::query_as(
            "SELECT body, deleted_at, object_revision, project_revision, projection_event_id \
             FROM project_view_objects WHERE community_id = $1 AND object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(deleted_goal_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("read canonical tombstone");
        assert!(row.0.is_none(), "a tombstone must not retain its body");
        assert!(row.1.is_some(), "the deletion timestamp must be durable");
        assert_eq!((row.2, row.3), (2, 3));
        assert_ne!(row.4, old_projection_id);

        let old_head_retired: bool = sqlx::query_scalar(
            "SELECT deleted_at IS NOT NULL FROM events \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&old_projection_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("inspect retired projection");
        assert!(old_head_retired);
        let new_head_live: bool = sqlx::query_scalar(
            "SELECT deleted_at IS NULL FROM events \
             WHERE community_id = $1 AND id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(&row.4)
        .fetch_one(&scratch.pool)
        .await
        .expect("inspect tombstone projection");
        assert!(new_head_live);

        let state: (i64, i32) = sqlx::query_as(
            "SELECT project_revision, active_object_count \
             FROM project_view_state WHERE community_id = $1",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read state after tombstone");
        assert_eq!(state, (3, 2));

        let mut read_tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("begin tombstone reload");
        let loaded = read_tx.load_current().await.expect("reload tombstone");
        assert!(matches!(
            loaded.state.entry(deleted_goal_id),
            Some(ProjectViewEntry::Tombstone(_))
        ));
        read_tx.rollback().await.expect("release tombstone reload");

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn deferred_database_guard_rejects_direct_last_goal_tombstone() {
        let scratch = ScratchDatabase::create("buzz_pv_last_goal_guard").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, true).await;
        let actor = Keys::generate();
        let relay = Keys::generate();
        initialize(&db, community_id, &actor, &relay).await;

        let goal_id: Uuid = sqlx::query_scalar(
            "SELECT object_id FROM project_view_objects \
             WHERE community_id = $1 AND object_type = 'goal' AND deleted_at IS NULL",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&scratch.pool)
        .await
        .expect("read only goal");
        let mut tx = scratch.pool.begin().await.expect("begin direct corruption");
        let changed = sqlx::query(
            "WITH canonical_time AS (SELECT clock_timestamp() AS value) \
             UPDATE project_view_objects \
             SET body = NULL, under_goal_id = NULL, under_plan_id = NULL, \
                 planned_in_stage_id = NULL, about_object_id = NULL, \
                 about_object_type = NULL, handles_object_id = NULL, \
                 handles_object_type = NULL, object_revision = object_revision + 1, \
                 updated_at = canonical_time.value, deleted_at = canonical_time.value \
             FROM canonical_time \
             WHERE community_id = $1 AND object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(goal_id)
        .execute(&mut *tx)
        .await
        .expect("stage invalid last-goal tombstone");
        assert_eq!(changed.rows_affected(), 1);

        let error = sqlx::query("SET CONSTRAINTS ALL IMMEDIATE")
            .execute(&mut *tx)
            .await
            .expect_err("deferred aggregate guard must reject the last-goal tombstone");
        let database_error = error
            .as_database_error()
            .expect("constraint failure is a database error");
        assert_eq!(database_error.code().as_deref(), Some("23514"));
        assert!(database_error
            .message()
            .contains("at least one active Goal"));
        tx.rollback()
            .await
            .expect("roll back rejected direct corruption");

        let persisted: (bool, i32) = sqlx::query_as(
            "SELECT o.deleted_at IS NULL, s.active_object_count \
             FROM project_view_objects o \
             JOIN project_view_state s ON s.community_id = o.community_id \
             WHERE o.community_id = $1 AND o.object_id = $2",
        )
        .bind(community_id.as_uuid())
        .bind(goal_id)
        .fetch_one(&scratch.pool)
        .await
        .expect("verify rejected tombstone rolled back");
        assert_eq!(persisted, (true, 2));

        scratch.cleanup().await;
    }

    #[tokio::test]
    #[ignore = "requires Postgres and CREATE DATABASE"]
    async fn centralized_feature_flag_gates_writer_transactions() {
        let scratch = ScratchDatabase::create("buzz_pv_flag").await;
        let db = Db::from_pool(scratch.pool.clone());
        let community_id = seed_community(&scratch.pool, false).await;
        let other_active_id = seed_community(&scratch.pool, false).await;
        let archived_id = seed_community(&scratch.pool, false).await;
        sqlx::query("UPDATE communities SET archived_at = clock_timestamp() WHERE id = $1")
            .bind(archived_id.as_uuid())
            .execute(&scratch.pool)
            .await
            .expect("archive Project View test community");

        let error = db
            .begin_project_view_write(community_id)
            .await
            .expect_err("disabled Project View must reject writes");
        assert!(matches!(
            error,
            ProjectViewWriteError::Unavailable {
                community_id: rejected
            } if rejected == community_id
        ));

        assert!(db
            .set_project_view_enabled(community_id, true)
            .await
            .expect("enable Project View"));
        let tx = db
            .begin_project_view_write(community_id)
            .await
            .expect("enabled Project View accepts writer transaction");
        tx.rollback().await.expect("release enabled transaction");

        assert!(db
            .set_project_view_enabled(community_id, false)
            .await
            .expect("disable Project View"));
        assert!(db.begin_project_view_write(community_id).await.is_err());

        let changed = db
            .set_all_project_views_enabled(true)
            .await
            .expect("enable every active Project View");
        assert_eq!(changed, 2);
        let flags: Vec<(Uuid, bool)> =
            sqlx::query_as("SELECT id, project_view_enabled FROM communities ORDER BY id")
                .fetch_all(&scratch.pool)
                .await
                .expect("read centralized Project View flags");
        assert!(flags
            .iter()
            .any(|(id, enabled)| { *id == *community_id.as_uuid() && *enabled }));
        assert!(flags
            .iter()
            .any(|(id, enabled)| { *id == *other_active_id.as_uuid() && *enabled }));
        assert!(flags
            .iter()
            .any(|(id, enabled)| *id == *archived_id.as_uuid() && !*enabled));

        scratch.cleanup().await;
    }

    #[test]
    fn canonical_body_storage_round_trips_every_active_variant_shape() {
        let data = ProjectViewObjectData::Goal(Goal {
            title: "Goal".to_owned(),
            desired_outcome: "Outcome".to_owned(),
            directions: vec!["Direction".to_owned()],
        });
        let body = object_body(&data).expect("serialize canonical body");
        assert_eq!(
            body,
            serde_json::json!({
                "title": "Goal",
                "desired_outcome": "Outcome",
                "directions": ["Direction"],
            })
        );
        assert_eq!(
            object_data_from_body(ProjectViewObjectType::Goal, body).expect("parse canonical body"),
            data
        );
    }

    #[test]
    fn prepared_command_content_must_match_the_typed_mutation() {
        let community_id = CommunityId::from_uuid(Uuid::new_v4());
        let actor = Keys::generate();
        let relay = Keys::generate();
        let canonical_time = Utc::now();
        let mutation = initialize_mutation();
        let (next_state, outcome) = ProjectViewState::new(community_id)
            .reduce(&mutation, actor.public_key(), canonical_time)
            .expect("reduce initialization");
        let mut prepared = prepare_commit(
            community_id,
            &actor,
            &relay,
            mutation,
            next_state,
            outcome,
            canonical_time,
        );
        prepared.mutation = create_goal_mutation(0, "Different command");

        let error = validate_prepared_commit(&prepared)
            .expect_err("signed command content and typed mutation must agree");
        assert!(matches!(error, ProjectViewWriteError::InvalidCommit(_)));
    }
}
