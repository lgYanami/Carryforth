//! Project Context semantic-index deployment probes.
//!
//! This module starts with the database prerequisite probe. Later phases add
//! only derived semantic state here; canonical Project View, Document, Meeting,
//! and Project Context ownership remains in their existing modules.

use buzz_core::CommunityId;
use buzz_project_document::{DocumentRevision, DocumentState};
use buzz_project_view::v3::{ProjectViewEntryV3, ProjectViewObjectDataV3};
use buzz_semantic::{
    CanonicalSemanticSourceObservation, Digest32, EncodedSemanticUnit, IneligibilityReason,
    MeetingSourceBasis, ProjectDocumentSourceBasis, ProjectViewSemanticType,
    ProjectViewSourceBasis, SemanticCoverage, SemanticDistanceMetric, SemanticEligibility,
    SemanticFilterMetadata, SemanticLifecycleClass, SemanticModelContract, SemanticNormalization,
    SemanticProviderBoundary, SemanticSourceBasis, SemanticSourceIdentity, SemanticSourceKind,
    SemanticUnit, SemanticUnitKind,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use pgvector::Vector;
use sqlx::Row;
use uuid::Uuid;

use crate::{Db, DbError, Result};

/// Canonical source family selected by a rebuild scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticScanFamily {
    /// Current Project View identities.
    ProjectView,
    /// Current Project Document identities.
    ProjectDocument,
    /// Current Meeting identities.
    Meeting,
}

/// Canonical source scope covered by one durable rebuild operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRebuildScope {
    /// Every source family required by a generation cutover.
    All,
    /// One diagnostic or repair family; this does not satisfy the generation
    /// full-rebuild fence.
    Family(SemanticScanFamily),
}

/// Closed lifecycle of a durable rebuild cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticRebuildState {
    /// The operation can be resumed from its persisted cursor.
    Running,
    /// Every family in scope was scanned.
    Completed,
    /// An operator explicitly cancelled the operation.
    Cancelled,
}

/// Durable, resumable canonical-source rebuild cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticRebuildOperation {
    /// Tenant boundary.
    pub community_id: CommunityId,
    /// Stable operation identity.
    pub operation_id: Uuid,
    /// Generation whose cutover fence this rebuild may satisfy.
    pub generation_id: Uuid,
    /// Full or source-family scope.
    pub scope: SemanticRebuildScope,
    /// Family currently being scanned.
    pub current_family: SemanticScanFamily,
    /// Last durable canonical source cursor.
    pub cursor: Option<SemanticSourceScanCursor>,
    /// Operation lifecycle.
    pub state: SemanticRebuildState,
}

/// Stable keyset cursor for one source-family scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSourceScanCursor {
    /// Stable subtype spelling; `document` and `meeting` are used for the
    /// single-subtype source families.
    pub source_subtype: String,
    /// Last source identity returned on the previous page.
    pub source_id: Uuid,
}

/// One bounded canonical source page used by rebuild and reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticSourcePage {
    /// Verified current observations in canonical key order.
    pub observations: Vec<CanonicalSemanticSourceObservation>,
    /// Cursor to continue after the final returned identity.
    pub next_cursor: Option<SemanticSourceScanCursor>,
}

/// Operator-supplied immutable generation creation values.
pub struct CreateSemanticGeneration<'a> {
    /// Tenant that owns the generation and controls provider authorization.
    pub community_id: CommunityId,
    /// Stable generation identity.
    pub generation_id: Uuid,
    /// Frozen extractor version.
    pub extractor_version: &'a str,
    /// Frozen model and data-boundary contract.
    pub model_contract: &'a SemanticModelContract,
    /// Auditable operator identity without source content.
    pub created_by: &'a str,
}

/// Durable semantic generation state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGenerationRecord {
    /// Tenant boundary.
    pub community_id: CommunityId,
    /// Stable generation identity.
    pub generation_id: Uuid,
    /// Lifecycle spelling from the closed database contract.
    pub lifecycle: String,
    /// Extractor version.
    pub extractor_version: String,
    /// Frozen model contract.
    pub model_contract: SemanticModelContract,
    /// Domain-separated model contract digest.
    pub model_contract_digest: Digest32,
    /// Completion fence for a full canonical-source rebuild.
    pub rebuild_completed_at: Option<DateTime<Utc>>,
    /// Creation time.
    pub created_at: DateTime<Utc>,
}

/// One fenced semantic worker claim.
pub struct SemanticJobLease {
    /// Canonical source identity to observe.
    pub source: SemanticSourceIdentity,
    /// Model generation to produce.
    pub generation_id: Uuid,
    /// Source invalidation epoch this claim is allowed to complete.
    pub desired_invalidation_epoch: u64,
    /// Unique claim fence.
    pub claim_id: Uuid,
    /// Lease deadline.
    pub lease_until: DateTime<Utc>,
    /// Number of attempts including this claim.
    pub attempts: u32,
    /// Frozen generation extractor.
    pub extractor_version: String,
    /// Frozen generation model contract.
    pub model_contract: SemanticModelContract,
    /// Frozen model contract digest.
    pub model_contract_digest: Digest32,
}

/// Result of a claim step whose source may have changed concurrently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticClaimObservationOutcome {
    /// Observation was recorded and encoding may continue.
    Ready,
    /// Source is currently ineligible; the claim completed without encoding.
    Ineligible,
    /// A newer source epoch or claim superseded this worker.
    Superseded,
}

/// Result of atomic unit-set activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticActivationOutcome {
    /// A complete current source-generation head was committed.
    Activated,
    /// A newer source epoch or claim superseded this worker.
    Superseded,
}

/// Generation coverage used by operator verification and cutover gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGenerationCoverage {
    /// Whether a durable all-family canonical rebuild completed for this
    /// generation. This prevents an unscanned empty catalog from passing 0/0
    /// coverage.
    pub rebuild_complete: bool,
    /// Sources currently eligible in the derived currentness catalog.
    pub eligible_sources: u64,
    /// Eligible sources with a complete current head for this generation.
    pub current_heads: u64,
    /// Pending or retryable jobs.
    pub queued_jobs: u64,
    /// Jobs currently leased by workers.
    pub claimed_jobs: u64,
    /// Permanently failed jobs.
    pub poison_jobs: u64,
}

/// Content-free aggregate worker/coverage observations for Relay metrics.
#[derive(Debug, Clone, PartialEq)]
pub struct SemanticRuntimeMetrics {
    /// Eligible sources in enabled Communities.
    pub eligible_sources: u64,
    /// Source catalog entries with at least one current activated head.
    pub current_sources: u64,
    /// Pending and retry jobs.
    pub queued_jobs: u64,
    /// Currently leased jobs.
    pub claimed_jobs: u64,
    /// Poison jobs requiring operator action.
    pub poison_jobs: u64,
    /// Age of the oldest due queued job, in seconds.
    pub oldest_due_seconds: f64,
}

impl SemanticGenerationCoverage {
    /// Whether this generation is complete enough to become ready or active.
    pub const fn complete(&self) -> bool {
        self.rebuild_complete
            && self.eligible_sources == self.current_heads
            && self.queued_jobs == 0
            && self.claimed_jobs == 0
            && self.poison_jobs == 0
    }
}

/// A reusable, already validated database embedding.
pub struct ReusableSemanticEmbedding {
    /// Provider-resolved model stored with the vector.
    pub response_model: String,
    /// Full-precision finite values.
    pub values: Vec<f32>,
}

/// PostgreSQL and pgvector compatibility contract for the first semantic
/// generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticPgvectorPreflight {
    /// Numeric PostgreSQL server version (`170000` means PostgreSQL 17.0).
    pub server_version_num: i32,
    /// Human-readable PostgreSQL server version.
    pub server_version: String,
    /// pgvector version available to `CREATE EXTENSION`, when installed on the
    /// host image.
    pub available_vector_version: Option<String>,
    /// pgvector version installed in the current database.
    pub installed_vector_version: Option<String>,
    /// Whether the `vector` PostgreSQL type resolves in the current database.
    pub vector_type_available: bool,
    /// Whether the `halfvec` PostgreSQL type resolves in the current database.
    pub halfvec_type_available: bool,
    /// Whether a typed vector can be constructed and inspected.
    pub vector_roundtrip_ok: bool,
    /// Whether cosine distance produces the expected result.
    pub cosine_distance_ok: bool,
    /// Whether a full-precision vector can be cast to `halfvec`, which is
    /// required for the 2048-dimensional ANN access path.
    pub halfvec_cast_ok: bool,
    /// Whether the selected Rust pgvector adapter can bind and decode the
    /// frozen 2048-dimensional full-precision representation through SQLx.
    pub sqlx_2048_roundtrip_ok: bool,
}

impl SemanticPgvectorPreflight {
    /// Return whether the live database satisfies the frozen Phase 0 contract.
    pub fn ready(&self) -> bool {
        self.server_version_num >= 170_000
            && self.server_version_num < 180_000
            && self
                .installed_vector_version
                .as_deref()
                .is_some_and(vector_version_supported)
            && self.vector_type_available
            && self.halfvec_type_available
            && self.vector_roundtrip_ok
            && self.cosine_distance_ok
            && self.halfvec_cast_ok
            && self.sqlx_2048_roundtrip_ok
    }

    /// Return stable, content-free reason codes for every failed prerequisite.
    pub fn failure_codes(&self) -> Vec<&'static str> {
        let mut failures = Vec::new();
        if !(170_000..180_000).contains(&self.server_version_num) {
            failures.push("unsupported_postgres_version");
        }
        if self.available_vector_version.is_none() {
            failures.push("pgvector_not_available_on_host");
        }
        match self.installed_vector_version.as_deref() {
            None => failures.push("pgvector_not_installed_in_database"),
            Some(version) if !vector_version_supported(version) => {
                failures.push("unsupported_pgvector_version");
            }
            Some(_) => {}
        }
        if !self.vector_type_available {
            failures.push("vector_type_unavailable");
        }
        if !self.halfvec_type_available {
            failures.push("halfvec_type_unavailable");
        }
        if !self.vector_roundtrip_ok {
            failures.push("vector_roundtrip_failed");
        }
        if !self.cosine_distance_ok {
            failures.push("cosine_distance_failed");
        }
        if !self.halfvec_cast_ok {
            failures.push("halfvec_cast_failed");
        }
        if !self.sqlx_2048_roundtrip_ok {
            failures.push("sqlx_2048_roundtrip_failed");
        }
        failures
    }
}

impl Db {
    /// Inspect the authoritative writer database without installing or changing
    /// extensions.
    ///
    /// The operator must install pgvector before semantic migrations. This
    /// probe deliberately never attempts `CREATE EXTENSION` and therefore does
    /// not pretend that catalog visibility proves installation authority.
    pub async fn semantic_pgvector_preflight(&self) -> Result<SemanticPgvectorPreflight> {
        let server = sqlx::query(
            "SELECT current_setting('server_version_num')::int4 AS version_num, \
                    current_setting('server_version') AS version_text",
        )
        .fetch_one(&self.pool)
        .await?;
        let server_version_num: i32 = server.try_get("version_num")?;
        let server_version: String = server.try_get("version_text")?;

        let available_vector_version: Option<String> = sqlx::query_scalar(
            "SELECT default_version FROM pg_available_extensions WHERE name = 'vector'",
        )
        .fetch_optional(&self.pool)
        .await?;
        let installed_vector_version: Option<String> =
            sqlx::query_scalar("SELECT extversion FROM pg_extension WHERE extname = 'vector'")
                .fetch_optional(&self.pool)
                .await?;
        let types = sqlx::query(
            "SELECT to_regtype('vector') IS NOT NULL AS vector_available, \
                    to_regtype('halfvec') IS NOT NULL AS halfvec_available",
        )
        .fetch_one(&self.pool)
        .await?;
        let vector_type_available: bool = types.try_get("vector_available")?;
        let halfvec_type_available: bool = types.try_get("halfvec_available")?;

        let (vector_roundtrip_ok, cosine_distance_ok, halfvec_cast_ok, sqlx_2048_roundtrip_ok) =
            if installed_vector_version.is_some() && vector_type_available && halfvec_type_available
            {
                let probe = sqlx::query(
                    "SELECT vector_dims('[1,2,3]'::vector) = 3 AS roundtrip_ok, \
                            abs(('[1,0]'::vector <=> '[0,1]'::vector) - 1.0) < 1e-12 \
                                AS cosine_ok, \
                            vector_dims(('[1,2,3]'::vector)::halfvec) = 3 \
                                AS halfvec_ok",
                )
                .fetch_one(&self.pool)
                .await?;
                let expected = Vector::from(vec![0.25_f32; 2_048]);
                let observed: Vector = sqlx::query_scalar("SELECT $1::vector")
                    .bind(expected.clone())
                    .fetch_one(&self.pool)
                    .await?;
                (
                    probe.try_get("roundtrip_ok")?,
                    probe.try_get("cosine_ok")?,
                    probe.try_get("halfvec_ok")?,
                    observed == expected,
                )
            } else {
                (false, false, false, false)
            };

        Ok(SemanticPgvectorPreflight {
            server_version_num,
            server_version,
            available_vector_version,
            installed_vector_version,
            vector_type_available,
            halfvec_type_available,
            vector_roundtrip_ok,
            cosine_distance_ok,
            halfvec_cast_ok,
            sqlx_2048_roundtrip_ok,
        })
    }

    /// Check the live semantic catalog without consulting SQLx's migration
    /// ledger. This also supports ledger-less desired-schema installations.
    pub async fn semantic_schema_ready(&self) -> Result<bool> {
        let ready: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_extension WHERE extname = 'vector') \
                 AND to_regtype('vector') IS NOT NULL \
                 AND to_regtype('halfvec') IS NOT NULL \
                 AND to_regclass('semantic_index_generations') IS NOT NULL \
                 AND to_regclass('semantic_sources') IS NOT NULL \
                 AND to_regclass('semantic_unit_sets') IS NOT NULL \
                 AND to_regclass('semantic_units') IS NOT NULL \
                 AND to_regclass('semantic_embeddings') IS NOT NULL \
                 AND to_regclass('semantic_source_generation_heads') IS NOT NULL \
                 AND to_regclass('semantic_index_jobs') IS NOT NULL \
                 AND to_regclass('semantic_rebuild_operations') IS NOT NULL \
                 AND to_regclass('semantic_provider_rate_gates') IS NOT NULL \
                 AND EXISTS (SELECT 1 FROM pg_attribute \
                             WHERE attrelid = 'communities'::regclass \
                               AND attname = 'semantic_index_enabled' \
                               AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(ready)
    }

    /// Deployment readiness for capability-gated semantic indexing.
    /// Pre-migration and all-disabled deployments remain ready. Enabled
    /// Communities require the derived schema and a running provider worker;
    /// any published active pointer must resolve to an active generation.
    pub async fn semantic_deployment_ready(&self, worker_ready: bool) -> Result<bool> {
        let column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
             WHERE attrelid='communities'::regclass \
               AND attname='semantic_index_enabled' AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !column_exists {
            return Ok(true);
        }
        let any_enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities \
             WHERE semantic_index_enabled AND archived_at IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !any_enabled {
            return Ok(true);
        }
        if !worker_ready || !self.semantic_schema_ready().await? {
            return Ok(false);
        }
        let invalid_pointer: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities community \
             LEFT JOIN semantic_index_generations generation \
               ON generation.community_id=community.id \
              AND generation.generation_id=community.semantic_active_generation_id \
             WHERE community.semantic_index_enabled \
               AND community.semantic_active_generation_id IS NOT NULL \
               AND (generation.generation_id IS NULL OR generation.lifecycle<>'active'))",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(!invalid_pointer)
    }

    /// Create one immutable, capability-off model generation.
    pub async fn create_semantic_generation(
        &self,
        input: CreateSemanticGeneration<'_>,
    ) -> Result<SemanticGenerationRecord> {
        input
            .model_contract
            .validate()
            .map_err(|error| semantic_contract_error("model_contract", error))?;
        if input.generation_id.is_nil() {
            return Err(DbError::InvalidData(
                "semantic generation id must not be nil".to_string(),
            ));
        }
        if input.extractor_version.trim().is_empty() || input.created_by.trim().is_empty() {
            return Err(DbError::InvalidData(
                "semantic generation extractor and creator must not be blank".to_string(),
            ));
        }
        let digest = input
            .model_contract
            .digest()
            .map_err(|error| semantic_contract_error("model_contract", error))?;
        let dimensions = i32::try_from(input.model_contract.dimensions).map_err(|_| {
            DbError::InvalidData("semantic generation dimensions exceed PostgreSQL int".to_string())
        })?;
        let row = sqlx::query(
            "INSERT INTO semantic_index_generations (\
                 community_id, generation_id, lifecycle, extractor_version, \
                 input_contract_version, provider, model, dimensions, \
                 distance_metric, normalization, provider_boundary, \
                 model_contract_digest, created_by) \
             VALUES ($1, $2, 'building', $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             RETURNING *",
        )
        .bind(input.community_id.as_uuid())
        .bind(input.generation_id)
        .bind(input.extractor_version)
        .bind(&input.model_contract.input_contract_version)
        .bind(&input.model_contract.provider)
        .bind(&input.model_contract.model)
        .bind(dimensions)
        .bind(distance_metric_db(input.model_contract.distance_metric))
        .bind(normalization_db(input.model_contract.normalization))
        .bind(provider_boundary_db(
            &input.model_contract.provider_boundary,
        ))
        .bind(digest.as_bytes().as_slice())
        .bind(input.created_by)
        .fetch_one(&self.pool)
        .await?;
        semantic_generation_from_row(&row)
    }

    /// List model generations for one trusted Community.
    pub async fn list_semantic_generations(
        &self,
        community_id: CommunityId,
    ) -> Result<Vec<SemanticGenerationRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM semantic_index_generations \
             WHERE community_id = $1 ORDER BY created_at, generation_id",
        )
        .bind(community_id.as_uuid())
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(semantic_generation_from_row).collect()
    }

    /// Enable or disable provider/worker/query eligibility for one Community.
    /// Currentness triggers remain active while disabled.
    pub async fn set_semantic_community_enabled(
        &self,
        community_id: CommunityId,
        enabled: bool,
    ) -> Result<()> {
        let affected =
            sqlx::query("UPDATE communities SET semantic_index_enabled = $2 WHERE id = $1")
                .bind(community_id.as_uuid())
                .bind(enabled)
                .execute(&self.pool)
                .await?
                .rows_affected();
        if affected != 1 {
            return Err(DbError::NotFound("semantic Community".to_string()));
        }
        Ok(())
    }

    /// Return the Community gate and active generation pointer.
    pub async fn semantic_community_state(
        &self,
        community_id: CommunityId,
    ) -> Result<(bool, Option<Uuid>)> {
        let row = sqlx::query(
            "SELECT semantic_index_enabled, semantic_active_generation_id \
             FROM communities WHERE id=$1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Community".to_string()))?;
        Ok((
            row.try_get("semantic_index_enabled")?,
            row.try_get("semantic_active_generation_id")?,
        ))
    }

    /// Aggregate content-free semantic queue and coverage metrics over only
    /// explicitly enabled Communities.
    pub async fn semantic_runtime_metrics(&self) -> Result<SemanticRuntimeMetrics> {
        let row = sqlx::query(
            "SELECT \
                 (SELECT count(*) FROM semantic_sources source \
                  JOIN communities community ON community.id=source.community_id \
                  WHERE community.semantic_index_enabled \
                    AND source.eligibility='eligible') AS eligible_sources, \
                 (SELECT count(*) FROM semantic_sources source \
                  JOIN communities community ON community.id=source.community_id \
                  WHERE community.semantic_index_enabled \
                    AND source.eligibility='eligible' \
                    AND source.coverage_state='current') AS current_sources, \
                 count(*) FILTER (WHERE job.state IN ('pending','retry')) AS queued_jobs, \
                 count(*) FILTER (WHERE job.state='claimed') AS claimed_jobs, \
                 count(*) FILTER (WHERE job.state='poison') AS poison_jobs, \
                 COALESCE(EXTRACT(EPOCH FROM \
                     (clock_timestamp()-min(job.next_attempt_at) FILTER (\
                         WHERE job.state IN ('pending','retry') \
                           AND job.next_attempt_at<=clock_timestamp()))),0)::float8 \
                     AS oldest_due_seconds \
             FROM semantic_index_jobs job \
             JOIN communities community ON community.id=job.community_id \
             WHERE community.semantic_index_enabled",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(SemanticRuntimeMetrics {
            eligible_sources: nonnegative_u64(
                row.try_get("eligible_sources")?,
                "eligible_sources",
            )?,
            current_sources: nonnegative_u64(row.try_get("current_sources")?, "current_sources")?,
            queued_jobs: nonnegative_u64(row.try_get("queued_jobs")?, "queued_jobs")?,
            claimed_jobs: nonnegative_u64(row.try_get("claimed_jobs")?, "claimed_jobs")?,
            poison_jobs: nonnegative_u64(row.try_get("poison_jobs")?, "poison_jobs")?,
            oldest_due_seconds: row.try_get::<f64, _>("oldest_due_seconds")?.max(0.0),
        })
    }

    /// Reserve the next Community/provider request time in the writer DB so
    /// horizontally scaled workers share one conservative rate gate.
    pub async fn reserve_semantic_provider_slot(
        &self,
        community_id: CommunityId,
        provider: &str,
        interval: std::time::Duration,
    ) -> Result<std::time::Duration> {
        if provider.trim().is_empty() || provider.len() > 255 {
            return Err(DbError::InvalidData(
                "semantic provider rate-gate identity is invalid".to_string(),
            ));
        }
        if interval < std::time::Duration::from_millis(100)
            || interval > std::time::Duration::from_secs(60)
        {
            return Err(DbError::InvalidData(
                "semantic provider interval must be between 100ms and 60s".to_string(),
            ));
        }
        let row = sqlx::query(
            "INSERT INTO semantic_provider_rate_gates (\
                 community_id,provider,next_request_at,updated_at) \
             VALUES ($1,$2,clock_timestamp()+make_interval(secs=>$3),clock_timestamp()) \
             ON CONFLICT (community_id,provider) DO UPDATE SET \
                 next_request_at=GREATEST(semantic_provider_rate_gates.next_request_at, \
                                          clock_timestamp())+make_interval(secs=>$3), \
                 updated_at=clock_timestamp() \
             RETURNING next_request_at-make_interval(secs=>$3) AS scheduled_at",
        )
        .bind(community_id.as_uuid())
        .bind(provider)
        .bind(interval.as_secs_f64())
        .fetch_one(&self.pool)
        .await?;
        let scheduled_at: DateTime<Utc> = row.try_get("scheduled_at")?;
        let wait_millis = (scheduled_at - Utc::now()).num_milliseconds().max(0);
        Ok(std::time::Duration::from_millis(
            u64::try_from(wait_millis).unwrap_or(u64::MAX),
        ))
    }

    /// Start a durable canonical-source rebuild. Reusing an operation UUID is
    /// idempotent only when its Community, generation, and scope are exact.
    pub async fn start_semantic_rebuild(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
        operation_id: Uuid,
        scope: SemanticRebuildScope,
    ) -> Result<SemanticRebuildOperation> {
        if operation_id.is_nil() {
            return Err(DbError::InvalidData(
                "semantic rebuild operation id must not be nil".to_string(),
            ));
        }
        let scope_db = rebuild_scope_db(scope);
        let current_family = match scope {
            SemanticRebuildScope::All => SemanticScanFamily::ProjectView,
            SemanticRebuildScope::Family(family) => family,
        };
        let row = sqlx::query(
            "INSERT INTO semantic_rebuild_operations (\
                 community_id,operation_id,generation_id,scope_family,current_family) \
             SELECT $1,$2,$3,$4,$5 FROM semantic_index_generations generation \
             WHERE generation.community_id=$1 AND generation.generation_id=$3 \
               AND generation.lifecycle='building' \
             ON CONFLICT (community_id,operation_id) DO NOTHING \
             RETURNING *",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .bind(generation_id)
        .bind(scope_db)
        .bind(scan_family_db(current_family))
        .fetch_optional(&self.pool)
        .await?;
        if let Some(row) = row {
            return semantic_rebuild_from_row(&row);
        }
        let existing = self
            .semantic_rebuild_operation(community_id, operation_id)
            .await?;
        if existing.generation_id != generation_id || existing.scope != scope {
            return Err(DbError::InvalidData(
                "semantic rebuild operation id belongs to another contract".to_string(),
            ));
        }
        Ok(existing)
    }

    /// Load one durable rebuild operation for explicit resume.
    pub async fn semantic_rebuild_operation(
        &self,
        community_id: CommunityId,
        operation_id: Uuid,
    ) -> Result<SemanticRebuildOperation> {
        let row = sqlx::query(
            "SELECT * FROM semantic_rebuild_operations \
             WHERE community_id=$1 AND operation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic rebuild operation".to_string()))?;
        semantic_rebuild_from_row(&row)
    }

    /// Persist a successful page cursor, advance to the next family, or
    /// atomically complete the rebuild and its full-generation scan fence.
    pub async fn checkpoint_semantic_rebuild(
        &self,
        operation: &SemanticRebuildOperation,
        next_cursor: Option<&SemanticSourceScanCursor>,
        family_complete: bool,
    ) -> Result<SemanticRebuildOperation> {
        if operation.state != SemanticRebuildState::Running {
            return Err(DbError::InvalidData(
                "semantic rebuild operation is not running".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        let row = sqlx::query(
            "SELECT * FROM semantic_rebuild_operations \
             WHERE community_id=$1 AND operation_id=$2 FOR UPDATE",
        )
        .bind(operation.community_id.as_uuid())
        .bind(operation.operation_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic rebuild operation".to_string()))?;
        let current = semantic_rebuild_from_row(&row)?;
        if current.state != SemanticRebuildState::Running
            || current.generation_id != operation.generation_id
            || current.current_family != operation.current_family
            || current.cursor != operation.cursor
        {
            return Err(DbError::InvalidData(
                "semantic rebuild cursor changed concurrently".to_string(),
            ));
        }

        let next_family = if family_complete {
            next_rebuild_family(current.scope, current.current_family)
        } else {
            Some(current.current_family)
        };
        let completed = next_family.is_none();
        let next_family = next_family.unwrap_or(current.current_family);
        let cursor = if family_complete { None } else { next_cursor };
        if !family_complete && cursor.is_none() {
            return Err(DbError::InvalidData(
                "semantic rebuild page checkpoint requires a cursor".to_string(),
            ));
        }
        if let Some(cursor) = cursor {
            validate_rebuild_cursor(current.current_family, cursor)?;
        }
        let updated = sqlx::query(
            "UPDATE semantic_rebuild_operations SET current_family=$3, \
                    after_source_subtype=$4,after_source_id=$5, \
                    state=CASE WHEN $6 THEN 'completed' ELSE 'running' END, \
                    completed_at=CASE WHEN $6 THEN clock_timestamp() ELSE NULL END, \
                    updated_at=clock_timestamp() \
             WHERE community_id=$1 AND operation_id=$2 RETURNING *",
        )
        .bind(current.community_id.as_uuid())
        .bind(current.operation_id)
        .bind(scan_family_db(next_family))
        .bind(cursor.map(|value| value.source_subtype.as_str()))
        .bind(cursor.map(|value| value.source_id))
        .bind(completed)
        .fetch_one(&mut *tx)
        .await?;
        if completed && current.scope == SemanticRebuildScope::All {
            let affected = sqlx::query(
                "UPDATE semantic_index_generations \
                 SET rebuild_completed_at=clock_timestamp() \
                 WHERE community_id=$1 AND generation_id=$2 AND lifecycle='building'",
            )
            .bind(current.community_id.as_uuid())
            .bind(current.generation_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if affected != 1 {
                return Err(DbError::InvalidData(
                    "semantic rebuild generation is no longer building".to_string(),
                ));
            }
        }
        tx.commit().await?;
        semantic_rebuild_from_row(&updated)
    }

    /// Explicitly cancel one running rebuild without satisfying the generation
    /// scan fence. Already completed operations cannot be cancelled.
    pub async fn cancel_semantic_rebuild(
        &self,
        community_id: CommunityId,
        operation_id: Uuid,
    ) -> Result<SemanticRebuildOperation> {
        let row = sqlx::query(
            "UPDATE semantic_rebuild_operations SET state='cancelled', \
                    completed_at=clock_timestamp(),updated_at=clock_timestamp() \
             WHERE community_id=$1 AND operation_id=$2 AND state='running' RETURNING *",
        )
        .bind(community_id.as_uuid())
        .bind(operation_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| {
            DbError::InvalidData("semantic rebuild operation is not cancellable".to_string())
        })?;
        semantic_rebuild_from_row(&row)
    }

    /// Reconcile one verified canonical observation into the derived
    /// currentness catalog and coalesce work for every maintained generation.
    pub async fn reconcile_semantic_observation(
        &self,
        observation: &CanonicalSemanticSourceObservation,
    ) -> Result<u64> {
        observation
            .identity
            .validate()
            .map_err(|error| semantic_contract_error("source_identity", error))?;
        let (family, subtype) = semantic_source_db_key(observation.identity.kind);
        let basis = serde_json::to_value(&observation.basis)?;
        let snapshot = observation.snapshot_digest.as_bytes().as_slice();
        let eligibility = eligibility_db(observation.eligibility);
        let ineligibility_reason = ineligibility_reason_db(observation.eligibility);
        let lifecycle = lifecycle_db(observation.filter.lifecycle);
        let mut tx = self.pool.begin().await?;
        let existing = sqlx::query(
            "SELECT eligibility, lifecycle_class, source_status, source_basis, \
                    snapshot_digest, invalidation_epoch, coverage_state \
             FROM semantic_sources \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4 \
             FOR UPDATE",
        )
        .bind(observation.identity.community_id)
        .bind(family)
        .bind(subtype)
        .bind(observation.identity.source_id)
        .fetch_optional(&mut *tx)
        .await?;

        let same_observation = existing.as_ref().is_some_and(|row| {
            row.try_get::<String, _>("eligibility").ok().as_deref() == Some(eligibility)
                && row.try_get::<String, _>("lifecycle_class").ok().as_deref() == Some(lifecycle)
                && row
                    .try_get::<Option<String>, _>("source_status")
                    .ok()
                    .flatten()
                    == observation.filter.source_status
                && row
                    .try_get::<Option<serde_json::Value>, _>("source_basis")
                    .ok()
                    .flatten()
                    == Some(basis.clone())
                && row
                    .try_get::<Option<Vec<u8>>, _>("snapshot_digest")
                    .ok()
                    .flatten()
                    == Some(snapshot.to_vec())
        });
        let trigger_dirty = existing.as_ref().is_some_and(|row| {
            row.try_get::<Option<serde_json::Value>, _>("source_basis")
                .ok()
                .flatten()
                .is_none()
                && row
                    .try_get::<String, _>("coverage_state")
                    .ok()
                    .is_some_and(|state| state == "dirty" || state == "ineligible")
        });

        if existing.is_none() || (!same_observation && !trigger_dirty) {
            sqlx::query("SELECT semantic_mark_source_changed($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind(observation.identity.community_id)
                .bind(family)
                .bind(subtype)
                .bind(observation.identity.source_id)
                .bind(matches!(
                    observation.eligibility,
                    SemanticEligibility::Eligible
                ))
                .bind(lifecycle)
                .bind(observation.filter.source_status.as_deref())
                .bind(ineligibility_reason)
                .execute(&mut *tx)
                .await?;
        }

        let epoch: i64 = sqlx::query_scalar(
            "UPDATE semantic_sources SET eligibility=$5, ineligibility_reason=$6, \
                    lifecycle_class=$7, source_status=$8, source_basis=$9, \
                    snapshot_digest=$10, coverage_state=$11, observed_at=clock_timestamp(), \
                    updated_at=clock_timestamp() \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4 \
             RETURNING invalidation_epoch",
        )
        .bind(observation.identity.community_id)
        .bind(family)
        .bind(subtype)
        .bind(observation.identity.source_id)
        .bind(eligibility)
        .bind(ineligibility_reason)
        .bind(lifecycle)
        .bind(observation.filter.source_status.as_deref())
        .bind(basis)
        .bind(snapshot)
        .bind(
            if matches!(observation.eligibility, SemanticEligibility::Eligible) {
                if same_observation {
                    "current"
                } else {
                    "missing"
                }
            } else {
                "ineligible"
            },
        )
        .fetch_one(&mut *tx)
        .await?;

        if matches!(observation.eligibility, SemanticEligibility::Eligible) {
            enqueue_semantic_jobs_tx(
                &mut tx,
                observation.identity.community_id,
                family,
                subtype,
                observation.identity.source_id,
                epoch,
            )
            .await?;
        }
        tx.commit().await?;
        positive_u64(epoch, "invalidation_epoch")
    }

    /// Claim one due semantic job from an explicitly enabled Community.
    pub async fn claim_due_semantic_job(
        &self,
        lease_seconds: u16,
    ) -> Result<Option<SemanticJobLease>> {
        if !(10..=300).contains(&lease_seconds) {
            return Err(DbError::InvalidData(
                "semantic job lease must be between 10 and 300 seconds".to_string(),
            ));
        }
        let claim_id = Uuid::new_v4();
        let lease_until = Utc::now() + ChronoDuration::seconds(i64::from(lease_seconds));
        let row = sqlx::query(
            "WITH candidate AS (\
                 SELECT job.community_id, job.generation_id, job.source_family, \
                        job.source_subtype, job.source_id \
                 FROM semantic_index_jobs job \
                 JOIN communities community ON community.id = job.community_id \
                 JOIN semantic_index_generations generation \
                   ON generation.community_id = job.community_id \
                  AND generation.generation_id = job.generation_id \
                 WHERE community.semantic_index_enabled \
                   AND generation.lifecycle IN ('building','ready','active','rollback_ready') \
                   AND ((job.state IN ('pending','retry') AND job.next_attempt_at <= clock_timestamp()) \
                        OR (job.state='claimed' AND job.lease_until < clock_timestamp())) \
                 ORDER BY job.next_attempt_at, job.community_id, job.generation_id, \
                          job.source_family, job.source_subtype, job.source_id \
                 LIMIT 1 FOR UPDATE OF job SKIP LOCKED\
             ) \
             UPDATE semantic_index_jobs job SET \
                 state='claimed', claim_id=$1, lease_until=$2, \
                 claimed_at=clock_timestamp(), completed_at=NULL, \
                 attempts=job.attempts+1, updated_at=clock_timestamp(), \
                 error_code=NULL, error_detail=NULL \
             FROM candidate, semantic_index_generations generation \
             WHERE job.community_id=candidate.community_id \
               AND job.generation_id=candidate.generation_id \
               AND job.source_family=candidate.source_family \
               AND job.source_subtype=candidate.source_subtype \
               AND job.source_id=candidate.source_id \
               AND generation.community_id=job.community_id \
               AND generation.generation_id=job.generation_id \
             RETURNING job.*, generation.extractor_version, \
                       generation.input_contract_version, generation.provider, \
                       generation.model, generation.dimensions, \
                       generation.distance_metric, generation.normalization, \
                       generation.provider_boundary, generation.model_contract_digest, \
                       generation.lifecycle AS generation_lifecycle, \
                       generation.created_at AS generation_created_at",
        )
        .bind(claim_id)
        .bind(lease_until)
        .fetch_optional(&self.pool)
        .await?;
        row.as_ref().map(semantic_job_lease_from_row).transpose()
    }

    /// Bind a claimed job to an exact canonical observation before encoding.
    pub async fn prepare_semantic_claim_observation(
        &self,
        lease: &SemanticJobLease,
        observation: &CanonicalSemanticSourceObservation,
    ) -> Result<SemanticClaimObservationOutcome> {
        if observation.identity != lease.source {
            return Err(DbError::InvalidData(
                "semantic claim observation identity mismatch".to_string(),
            ));
        }
        if !matches!(observation.eligibility, SemanticEligibility::Eligible) {
            self.reconcile_semantic_observation(observation).await?;
            return Ok(SemanticClaimObservationOutcome::Ineligible);
        }
        let (family, subtype) = semantic_source_db_key(lease.source.kind);
        let basis = serde_json::to_value(&observation.basis)?;
        let snapshot = observation.snapshot_digest.as_bytes().as_slice();
        let mut tx = self.pool.begin().await?;
        let source = sqlx::query(
            "SELECT invalidation_epoch, source_basis, snapshot_digest \
             FROM semantic_sources \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4 \
             FOR UPDATE",
        )
        .bind(lease.source.community_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(source) = source else {
            tx.rollback().await?;
            self.reconcile_semantic_observation(observation).await?;
            return Ok(SemanticClaimObservationOutcome::Superseded);
        };
        let epoch = positive_u64(source.try_get("invalidation_epoch")?, "invalidation_epoch")?;
        if epoch != lease.desired_invalidation_epoch {
            tx.rollback().await?;
            return Ok(SemanticClaimObservationOutcome::Superseded);
        }
        let stored_basis: Option<serde_json::Value> = source.try_get("source_basis")?;
        let stored_snapshot: Option<Vec<u8>> = source.try_get("snapshot_digest")?;
        if stored_basis.as_ref().is_some_and(|value| value != &basis)
            || stored_snapshot
                .as_ref()
                .is_some_and(|value| value.as_slice() != snapshot)
        {
            tx.rollback().await?;
            self.reconcile_semantic_observation(observation).await?;
            return Ok(SemanticClaimObservationOutcome::Superseded);
        }

        let claimed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM semantic_index_jobs \
             WHERE community_id=$1 AND generation_id=$2 \
               AND source_family=$3 AND source_subtype=$4 AND source_id=$5 \
               AND desired_invalidation_epoch=$6 AND state='claimed' \
               AND claim_id=$7 AND lease_until >= clock_timestamp() FOR UPDATE)",
        )
        .bind(lease.source.community_id)
        .bind(lease.generation_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(u64_to_i64(
            lease.desired_invalidation_epoch,
            "invalidation_epoch",
        )?)
        .bind(lease.claim_id)
        .fetch_one(&mut *tx)
        .await?;
        if !claimed {
            tx.rollback().await?;
            return Ok(SemanticClaimObservationOutcome::Superseded);
        }
        sqlx::query(
            "UPDATE semantic_sources SET eligibility='eligible', \
                 ineligibility_reason=NULL, lifecycle_class=$5, source_status=$6, \
                 source_basis=$7, snapshot_digest=$8, coverage_state='building', \
                 observed_at=clock_timestamp(), updated_at=clock_timestamp() \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4",
        )
        .bind(lease.source.community_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(lifecycle_db(observation.filter.lifecycle))
        .bind(observation.filter.source_status.as_deref())
        .bind(basis)
        .bind(snapshot)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SemanticClaimObservationOutcome::Ready)
    }

    /// Find a digest- and generation-identical embedding that can be reused
    /// without sending source text to the provider again.
    pub async fn reusable_semantic_embedding(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
        unit: &SemanticUnit,
        model_contract_digest: Digest32,
    ) -> Result<Option<ReusableSemanticEmbedding>> {
        let row = sqlx::query(
            "SELECT embedding.embedding, embedding.response_model \
             FROM semantic_embeddings embedding \
             JOIN semantic_units unit \
               ON unit.community_id=embedding.community_id \
              AND unit.unit_set_id=embedding.unit_set_id \
              AND unit.unit_key=embedding.unit_key \
             WHERE embedding.community_id=$1 AND embedding.generation_id=$2 \
               AND embedding.model_contract_digest=$3 \
               AND unit.semantic_text_digest=$4 \
             ORDER BY embedding.indexed_at DESC LIMIT 1",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .bind(model_contract_digest.as_bytes().as_slice())
        .bind(unit.semantic_text_digest.as_bytes().as_slice())
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            let vector: Vector = row.try_get("embedding")?;
            Ok(ReusableSemanticEmbedding {
                response_model: row.try_get("response_model")?,
                values: vector.to_vec(),
            })
        })
        .transpose()
    }

    /// Atomically publish one complete unit set and its complete generation
    /// embeddings under both the source epoch and worker claim fences.
    pub async fn activate_semantic_claim(
        &self,
        lease: &SemanticJobLease,
        observation: &CanonicalSemanticSourceObservation,
        units: &[SemanticUnit],
        encoded: &[EncodedSemanticUnit],
    ) -> Result<SemanticActivationOutcome> {
        validate_semantic_activation(lease, observation, units, encoded)?;
        let (family, subtype) = semantic_source_db_key(lease.source.kind);
        let epoch = u64_to_i64(
            lease.desired_invalidation_epoch,
            "source_invalidation_epoch",
        )?;
        let basis = serde_json::to_value(&observation.basis)?;
        let snapshot = observation.snapshot_digest.as_bytes().as_slice();
        let complete_count = i32::try_from(units.len()).map_err(|_| {
            DbError::InvalidData("semantic unit count exceeds PostgreSQL int".to_string())
        })?;
        let mut tx = self.pool.begin().await?;
        let source_current: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM semantic_sources \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4 \
               AND invalidation_epoch=$5 AND eligibility='eligible' \
               AND source_basis=$6 AND snapshot_digest=$7 FOR UPDATE)",
        )
        .bind(lease.source.community_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(epoch)
        .bind(&basis)
        .bind(snapshot)
        .fetch_one(&mut *tx)
        .await?;
        if !source_current {
            tx.rollback().await?;
            return Ok(SemanticActivationOutcome::Superseded);
        }
        let claim_current: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM semantic_index_jobs \
             WHERE community_id=$1 AND generation_id=$2 \
               AND source_family=$3 AND source_subtype=$4 AND source_id=$5 \
               AND desired_invalidation_epoch=$6 AND state='claimed' \
               AND claim_id=$7 AND lease_until >= clock_timestamp() FOR UPDATE)",
        )
        .bind(lease.source.community_id)
        .bind(lease.generation_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(epoch)
        .bind(lease.claim_id)
        .fetch_one(&mut *tx)
        .await?;
        if !claim_current {
            tx.rollback().await?;
            return Ok(SemanticActivationOutcome::Superseded);
        }

        let unit_set_id = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO semantic_unit_sets (\
                 community_id, unit_set_id, source_family, source_subtype, source_id, \
                 source_invalidation_epoch, source_basis, source_snapshot_digest, \
                 extractor_version, state, complete_unit_count) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'staging',$10) \
             ON CONFLICT (community_id, source_family, source_subtype, source_id, \
                          source_snapshot_digest, extractor_version) \
             DO UPDATE SET source_invalidation_epoch=EXCLUDED.source_invalidation_epoch \
             RETURNING unit_set_id",
        )
        .bind(lease.source.community_id)
        .bind(Uuid::new_v4())
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(epoch)
        .bind(&basis)
        .bind(snapshot)
        .bind(&lease.extractor_version)
        .bind(complete_count)
        .fetch_one(&mut *tx)
        .await?;

        for unit in units {
            sqlx::query(
                "INSERT INTO semantic_units (\
                     community_id, unit_set_id, unit_key, ordinal, unit_kind, source_path, \
                     semantic_text, semantic_text_digest, summary_coverage, extraction_provenance) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10) ON CONFLICT DO NOTHING",
            )
            .bind(lease.source.community_id)
            .bind(unit_set_id)
            .bind(&unit.identity.key)
            .bind(i32::try_from(unit.identity.ordinal).map_err(|_| {
                DbError::InvalidData("semantic unit ordinal exceeds PostgreSQL int".to_string())
            })?)
            .bind(unit_kind_db(unit.identity.kind))
            .bind(unit.identity.path.as_deref())
            .bind(&unit.text)
            .bind(unit.semantic_text_digest.as_bytes().as_slice())
            .bind(coverage_db(unit.coverage))
            .bind(serde_json::to_value(&unit.identity)?)
            .execute(&mut *tx)
            .await?;
        }
        for result in encoded {
            sqlx::query(
                "INSERT INTO semantic_embeddings (\
                     community_id, unit_set_id, unit_key, generation_id, dimensions, \
                     model_contract_digest, response_model, embedding) \
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT DO NOTHING",
            )
            .bind(lease.source.community_id)
            .bind(unit_set_id)
            .bind(&result.identity().key)
            .bind(lease.generation_id)
            .bind(i32::try_from(lease.model_contract.dimensions).map_err(|_| {
                DbError::InvalidData("semantic dimensions exceed PostgreSQL int".to_string())
            })?)
            .bind(lease.model_contract_digest.as_bytes().as_slice())
            .bind(result.response_model())
            .bind(Vector::from(result.embedding().as_slice().to_vec()))
            .execute(&mut *tx)
            .await?;
        }
        let observed_counts = sqlx::query(
            "SELECT (SELECT count(*) FROM semantic_units \
                     WHERE community_id=$1 AND unit_set_id=$2) AS unit_count, \
                    (SELECT count(*) FROM semantic_embeddings \
                     WHERE community_id=$1 AND unit_set_id=$2 AND generation_id=$3 \
                       AND model_contract_digest=$4) AS embedding_count",
        )
        .bind(lease.source.community_id)
        .bind(unit_set_id)
        .bind(lease.generation_id)
        .bind(lease.model_contract_digest.as_bytes().as_slice())
        .fetch_one(&mut *tx)
        .await?;
        let unit_count: i64 = observed_counts.try_get("unit_count")?;
        let embedding_count: i64 = observed_counts.try_get("embedding_count")?;
        if unit_count != i64::from(complete_count) || embedding_count != i64::from(complete_count) {
            return Err(DbError::InvalidData(
                "semantic unit set or embeddings are incomplete".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE semantic_unit_sets SET state='active', \
                    activated_at=COALESCE(activated_at,clock_timestamp()), retired_at=NULL \
             WHERE community_id=$1 AND unit_set_id=$2",
        )
        .bind(lease.source.community_id)
        .bind(unit_set_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO semantic_source_generation_heads (\
                 community_id, generation_id, source_family, source_subtype, source_id, \
                 unit_set_id, source_invalidation_epoch, source_snapshot_digest, \
                 complete_unit_count, complete_embedding_count) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$9) \
             ON CONFLICT (community_id,generation_id,source_family,source_subtype,source_id) \
             DO UPDATE SET unit_set_id=EXCLUDED.unit_set_id, \
                 source_invalidation_epoch=EXCLUDED.source_invalidation_epoch, \
                 source_snapshot_digest=EXCLUDED.source_snapshot_digest, \
                 complete_unit_count=EXCLUDED.complete_unit_count, \
                 complete_embedding_count=EXCLUDED.complete_embedding_count, \
                 activated_at=clock_timestamp()",
        )
        .bind(lease.source.community_id)
        .bind(lease.generation_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(unit_set_id)
        .bind(epoch)
        .bind(snapshot)
        .bind(complete_count)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_unit_sets unit_set \
             SET state='retired', retired_at=COALESCE(unit_set.retired_at,clock_timestamp()) \
             WHERE unit_set.community_id=$1 \
               AND unit_set.source_family=$2 AND unit_set.source_subtype=$3 \
               AND unit_set.source_id=$4 AND unit_set.state='active' \
               AND NOT EXISTS (SELECT 1 FROM semantic_source_generation_heads head \
                               WHERE head.community_id=unit_set.community_id \
                                 AND head.unit_set_id=unit_set.unit_set_id)",
        )
        .bind(lease.source.community_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_sources SET coverage_state='current', \
                    updated_at=clock_timestamp() \
             WHERE community_id=$1 AND source_family=$2 AND source_subtype=$3 AND source_id=$4",
        )
        .bind(lease.source.community_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_index_jobs SET state='succeeded', claim_id=NULL, \
                    lease_until=NULL, claimed_at=NULL, completed_at=clock_timestamp(), \
                    updated_at=clock_timestamp(), error_code=NULL, error_detail=NULL \
             WHERE community_id=$1 AND generation_id=$2 AND source_family=$3 \
               AND source_subtype=$4 AND source_id=$5 AND claim_id=$6",
        )
        .bind(lease.source.community_id)
        .bind(lease.generation_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(lease.claim_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(SemanticActivationOutcome::Activated)
    }

    /// Release or poison one current claim with bounded, content-free failure
    /// metadata.
    pub async fn retry_semantic_claim(
        &self,
        lease: &SemanticJobLease,
        retry_after_seconds: u32,
        max_attempts: u32,
        error_code: &str,
        error_detail: Option<&str>,
    ) -> Result<bool> {
        if error_code.trim().is_empty() || error_code.len() > 128 {
            return Err(DbError::InvalidData(
                "semantic error code must contain 1..=128 bytes".to_string(),
            ));
        }
        let detail = error_detail.map(|value| {
            let mut bounded = value.replace(['\n', '\r'], " ");
            bounded.truncate(512);
            bounded
        });
        let poison = lease.attempts >= max_attempts.max(1);
        let (family, subtype) = semantic_source_db_key(lease.source.kind);
        let affected = sqlx::query(
            "UPDATE semantic_index_jobs SET state=$8, claim_id=NULL, lease_until=NULL, \
                    claimed_at=NULL, completed_at=CASE WHEN $8='poison' \
                        THEN clock_timestamp() ELSE NULL END, \
                    next_attempt_at=clock_timestamp()+make_interval(secs=>$9), \
                    error_code=$10, error_detail=$11, updated_at=clock_timestamp() \
             WHERE community_id=$1 AND generation_id=$2 AND source_family=$3 \
               AND source_subtype=$4 AND source_id=$5 \
               AND desired_invalidation_epoch=$6 AND claim_id=$7 AND state='claimed'",
        )
        .bind(lease.source.community_id)
        .bind(lease.generation_id)
        .bind(family)
        .bind(subtype)
        .bind(lease.source.source_id)
        .bind(u64_to_i64(
            lease.desired_invalidation_epoch,
            "invalidation_epoch",
        )?)
        .bind(lease.claim_id)
        .bind(if poison { "poison" } else { "retry" })
        .bind(f64::from(retry_after_seconds.min(86_400)))
        .bind(error_code)
        .bind(detail.as_deref())
        .execute(&self.pool)
        .await?
        .rows_affected();
        Ok(affected == 1)
    }

    /// Compute the exact current coverage gate for one generation.
    pub async fn semantic_generation_coverage(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<SemanticGenerationCoverage> {
        let row = sqlx::query(
            "SELECT \
                 EXISTS (SELECT 1 FROM semantic_index_generations generation \
                         WHERE generation.community_id=$1 AND generation.generation_id=$2 \
                           AND generation.rebuild_completed_at IS NOT NULL) AS rebuild_complete, \
                 (SELECT count(*) FROM semantic_sources source \
                  WHERE source.community_id=$1 AND source.eligibility='eligible') AS eligible, \
                 (SELECT count(*) FROM semantic_source_generation_heads head \
                  JOIN semantic_sources source \
                    ON source.community_id=head.community_id \
                   AND source.source_family=head.source_family \
                   AND source.source_subtype=head.source_subtype \
                   AND source.source_id=head.source_id \
                   AND source.invalidation_epoch=head.source_invalidation_epoch \
                   AND source.snapshot_digest=head.source_snapshot_digest \
                  WHERE head.community_id=$1 AND head.generation_id=$2 \
                    AND source.eligibility='eligible') AS current_heads, \
                 (SELECT count(*) FROM semantic_index_jobs \
                  WHERE community_id=$1 AND generation_id=$2 \
                    AND state IN ('pending','retry')) AS queued, \
                 (SELECT count(*) FROM semantic_index_jobs \
                  WHERE community_id=$1 AND generation_id=$2 \
                    AND state='claimed') AS claimed, \
                 (SELECT count(*) FROM semantic_index_jobs \
                  WHERE community_id=$1 AND generation_id=$2 \
                    AND state='poison') AS poison",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(SemanticGenerationCoverage {
            rebuild_complete: row.try_get("rebuild_complete")?,
            eligible_sources: nonnegative_u64(row.try_get("eligible")?, "eligible_sources")?,
            current_heads: nonnegative_u64(row.try_get("current_heads")?, "current_heads")?,
            queued_jobs: nonnegative_u64(row.try_get("queued")?, "queued_jobs")?,
            claimed_jobs: nonnegative_u64(row.try_get("claimed")?, "claimed_jobs")?,
            poison_jobs: nonnegative_u64(row.try_get("poison")?, "poison_jobs")?,
        })
    }

    /// Mark a fully covered building generation ready for cutover.
    pub async fn mark_semantic_generation_ready(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<SemanticGenerationCoverage> {
        let coverage = self
            .semantic_generation_coverage(community_id, generation_id)
            .await?;
        if !coverage.complete() {
            return Err(DbError::InvalidData(
                "semantic generation coverage is incomplete".to_string(),
            ));
        }
        let affected = sqlx::query(
            "UPDATE semantic_index_generations SET lifecycle='ready', ready_at=clock_timestamp() \
             WHERE community_id=$1 AND generation_id=$2 AND lifecycle='building'",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&self.pool)
        .await?
        .rows_affected();
        if affected != 1 {
            return Err(DbError::InvalidData(
                "semantic generation is not building".to_string(),
            ));
        }
        Ok(coverage)
    }

    /// Atomically switch the Community active pointer to a fully current ready
    /// or rollback-ready generation.
    pub async fn activate_semantic_generation(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<SemanticGenerationCoverage> {
        let coverage = self
            .semantic_generation_coverage(community_id, generation_id)
            .await?;
        if !coverage.complete() {
            return Err(DbError::InvalidData(
                "semantic generation coverage is incomplete".to_string(),
            ));
        }
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("buzz_semantic_generation:{community_id}"))
            .execute(&mut *tx)
            .await?;
        let target: Option<String> = sqlx::query_scalar(
            "SELECT lifecycle FROM semantic_index_generations \
             WHERE community_id=$1 AND generation_id=$2 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .fetch_optional(&mut *tx)
        .await?;
        if !target.is_some_and(|value| value == "ready" || value == "rollback_ready") {
            return Err(DbError::InvalidData(
                "semantic target generation is not ready".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE semantic_index_generations SET lifecycle='rollback_ready' \
             WHERE community_id=$1 AND lifecycle='active' AND generation_id<>$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_index_generations SET lifecycle='active', \
                    activated_at=COALESCE(activated_at,clock_timestamp()) \
             WHERE community_id=$1 AND generation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?;
        let switched = sqlx::query(
            "UPDATE communities SET semantic_active_generation_id=$2 \
             WHERE id=$1 AND semantic_index_enabled",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if switched != 1 {
            return Err(DbError::InvalidData(
                "semantic Community is not enabled".to_string(),
            ));
        }
        tx.commit().await?;
        Ok(coverage)
    }

    /// Delete unreferenced staging and retired semantic sets older than a
    /// retention cutoff. Canonical sources and current heads are never touched.
    pub async fn gc_semantic_derived_sets(
        &self,
        community_id: CommunityId,
        older_than: DateTime<Utc>,
        limit: u16,
    ) -> Result<u64> {
        if limit == 0 || limit > 1_000 {
            return Err(DbError::InvalidData(
                "semantic GC limit must be between 1 and 1000".to_string(),
            ));
        }
        let result = sqlx::query(
            "WITH victims AS (\
                 SELECT unit_set.community_id, unit_set.unit_set_id \
                 FROM semantic_unit_sets unit_set \
                 WHERE unit_set.community_id=$1 \
                   AND unit_set.state IN ('staging','retired') \
                   AND COALESCE(unit_set.retired_at,unit_set.created_at) < $2 \
                   AND NOT EXISTS (SELECT 1 FROM semantic_source_generation_heads head \
                                   WHERE head.community_id=unit_set.community_id \
                                     AND head.unit_set_id=unit_set.unit_set_id) \
                 ORDER BY COALESCE(unit_set.retired_at,unit_set.created_at), unit_set.unit_set_id \
                 LIMIT $3 FOR UPDATE SKIP LOCKED\
             ) DELETE FROM semantic_unit_sets unit_set USING victims \
               WHERE unit_set.community_id=victims.community_id \
                 AND unit_set.unit_set_id=victims.unit_set_id",
        )
        .bind(community_id.as_uuid())
        .bind(older_than)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Delete old completed jobs, plus poison metadata only after its
    /// generation is no longer serving or rollback-eligible.
    pub async fn gc_semantic_jobs(
        &self,
        community_id: CommunityId,
        older_than: DateTime<Utc>,
        limit: u16,
    ) -> Result<u64> {
        if limit == 0 || limit > 1_000 {
            return Err(DbError::InvalidData(
                "semantic job GC limit must be between 1 and 1000".to_string(),
            ));
        }
        let result = sqlx::query(
            "WITH victims AS (\
                 SELECT job.community_id,job.generation_id,job.source_family, \
                        job.source_subtype,job.source_id \
                 FROM semantic_index_jobs job \
                 JOIN semantic_index_generations generation \
                   ON generation.community_id=job.community_id \
                  AND generation.generation_id=job.generation_id \
                 WHERE job.community_id=$1 AND job.completed_at<$2 \
                   AND (job.state='succeeded' \
                        OR (job.state='poison' \
                            AND generation.lifecycle IN ('retired','failed'))) \
                 ORDER BY job.completed_at,job.generation_id,job.source_family, \
                          job.source_subtype,job.source_id \
                 LIMIT $3 FOR UPDATE OF job SKIP LOCKED\
             ) DELETE FROM semantic_index_jobs job USING victims \
               WHERE job.community_id=victims.community_id \
                 AND job.generation_id=victims.generation_id \
                 AND job.source_family=victims.source_family \
                 AND job.source_subtype=victims.source_subtype \
                 AND job.source_id=victims.source_id",
        )
        .bind(community_id.as_uuid())
        .bind(older_than)
        .bind(i64::from(limit))
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Requeue poison jobs without changing their desired source epoch.
    pub async fn retry_poison_semantic_jobs(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE semantic_index_jobs SET state='retry',attempts=0, \
                    next_attempt_at=clock_timestamp(),claim_id=NULL,lease_until=NULL, \
                    claimed_at=NULL,completed_at=NULL,error_code=NULL,error_detail=NULL, \
                    updated_at=clock_timestamp() \
             WHERE community_id=$1 AND generation_id=$2 AND state='poison'",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Retire a non-active generation and remove its current-head pointers.
    pub async fn retire_semantic_generation(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query(
            "UPDATE semantic_index_generations SET lifecycle='retired', \
                    retired_at=clock_timestamp() \
             WHERE community_id=$1 AND generation_id=$2 \
               AND lifecycle IN ('rollback_ready','ready')",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if affected != 1 {
            return Err(DbError::InvalidData(
                "semantic generation is not retireable".to_string(),
            ));
        }
        sqlx::query(
            "DELETE FROM semantic_source_generation_heads \
             WHERE community_id=$1 AND generation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE semantic_unit_sets unit_set \
             SET state='retired', retired_at=COALESCE(unit_set.retired_at,clock_timestamp()) \
             WHERE unit_set.community_id=$1 AND unit_set.state='active' \
               AND NOT EXISTS (SELECT 1 FROM semantic_source_generation_heads head \
                               WHERE head.community_id=unit_set.community_id \
                                 AND head.unit_set_id=unit_set.unit_set_id)",
        )
        .bind(community_id.as_uuid())
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM semantic_index_jobs WHERE community_id=$1 AND generation_id=$2")
            .bind(community_id.as_uuid())
            .bind(generation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Permanently remove one retired/failed derived generation. Canonical
    /// sources and Project Context graph rows are never changed.
    pub async fn purge_semantic_generation(
        &self,
        community_id: CommunityId,
        generation_id: Uuid,
    ) -> Result<u64> {
        let mut tx = self.pool.begin().await?;
        let allowed: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM semantic_index_generations \
             WHERE community_id=$1 AND generation_id=$2 \
               AND lifecycle IN ('retired','failed') FOR UPDATE)",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .fetch_one(&mut *tx)
        .await?;
        if !allowed {
            return Err(DbError::InvalidData(
                "semantic generation is not purgeable".to_string(),
            ));
        }
        let embeddings = sqlx::query(
            "DELETE FROM semantic_embeddings WHERE community_id=$1 AND generation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        sqlx::query(
            "DELETE FROM semantic_index_generations WHERE community_id=$1 AND generation_id=$2",
        )
        .bind(community_id.as_uuid())
        .bind(generation_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(embeddings)
    }

    /// Reconstruct and verify one current canonical source observation using
    /// the authoritative writer database.
    pub async fn observe_semantic_source(
        &self,
        identity: &SemanticSourceIdentity,
    ) -> Result<CanonicalSemanticSourceObservation> {
        identity
            .validate()
            .map_err(|error| semantic_contract_error("source_identity", error))?;
        match identity.kind {
            SemanticSourceKind::ProjectView(subtype) => {
                self.observe_project_view_source(identity, subtype).await
            }
            SemanticSourceKind::ProjectDocument => {
                self.observe_project_document_source(identity).await
            }
            SemanticSourceKind::Meeting => self.observe_meeting_source(identity).await,
        }
    }

    /// Scan one canonical source family using a stable subtype/id keyset.
    ///
    /// The page includes ineligible tombstones so reconciliation can remove
    /// obsolete heads. It never scans semantic derived tables as the source of
    /// truth.
    pub async fn scan_current_semantic_sources(
        &self,
        community_id: CommunityId,
        family: SemanticScanFamily,
        cursor: Option<&SemanticSourceScanCursor>,
        limit: u16,
    ) -> Result<SemanticSourcePage> {
        if limit == 0 || limit > 500 {
            return Err(DbError::InvalidData(
                "semantic source scan limit must be between 1 and 500".to_string(),
            ));
        }
        let identities = match family {
            SemanticScanFamily::ProjectView => {
                let after_subtype = cursor.map_or("", |value| value.source_subtype.as_str());
                let after_id = cursor.map_or(Uuid::nil(), |value| value.source_id);
                let rows = sqlx::query(
                    "SELECT object_type, object_id FROM project_view_objects \
                     WHERE community_id = $1 \
                       AND (object_type, object_id) > ($2, $3) \
                     ORDER BY object_type, object_id LIMIT $4",
                )
                .bind(community_id.as_uuid())
                .bind(after_subtype)
                .bind(after_id)
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?;
                rows.into_iter()
                    .map(|row| {
                        let object_type: String = row.try_get("object_type")?;
                        let subtype = project_view_semantic_type(&object_type)?;
                        Ok(SemanticSourceIdentity {
                            community_id: *community_id.as_uuid(),
                            kind: SemanticSourceKind::ProjectView(subtype),
                            source_id: row.try_get("object_id")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            SemanticScanFamily::ProjectDocument => {
                validate_single_subtype_cursor(cursor, "document")?;
                let after_id = cursor.map_or(Uuid::nil(), |value| value.source_id);
                let ids = sqlx::query_scalar::<_, Uuid>(
                    "SELECT document_id FROM project_documents \
                     WHERE community_id = $1 AND document_id > $2 \
                     ORDER BY document_id LIMIT $3",
                )
                .bind(community_id.as_uuid())
                .bind(after_id)
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?;
                ids.into_iter()
                    .map(|source_id| SemanticSourceIdentity {
                        community_id: *community_id.as_uuid(),
                        kind: SemanticSourceKind::ProjectDocument,
                        source_id,
                    })
                    .collect()
            }
            SemanticScanFamily::Meeting => {
                validate_single_subtype_cursor(cursor, "meeting")?;
                let after_id = cursor.map_or(Uuid::nil(), |value| value.source_id);
                let ids = sqlx::query_scalar::<_, Uuid>(
                    "SELECT session_id FROM meeting_sessions \
                     WHERE community_id = $1 AND session_id > $2 \
                     ORDER BY session_id LIMIT $3",
                )
                .bind(community_id.as_uuid())
                .bind(after_id)
                .bind(i64::from(limit))
                .fetch_all(&self.pool)
                .await?;
                ids.into_iter()
                    .map(|source_id| SemanticSourceIdentity {
                        community_id: *community_id.as_uuid(),
                        kind: SemanticSourceKind::Meeting,
                        source_id,
                    })
                    .collect()
            }
        };

        let mut observations = Vec::with_capacity(identities.len());
        for identity in identities {
            observations.push(self.observe_semantic_source(&identity).await?);
        }
        let next_cursor = observations
            .last()
            .map(|observation| SemanticSourceScanCursor {
                source_subtype: semantic_source_subtype(observation.identity.kind).to_string(),
                source_id: observation.identity.source_id,
            });
        Ok(SemanticSourcePage {
            observations,
            next_cursor,
        })
    }

    async fn observe_project_view_source(
        &self,
        identity: &SemanticSourceIdentity,
        requested_subtype: ProjectViewSemanticType,
    ) -> Result<CanonicalSemanticSourceObservation> {
        let row = sqlx::query(
            "SELECT object.object_id, object.object_type, object.schema_version, \
                    object.object_revision, object.project_revision, object.body, \
                    object.under_goal_id, object.under_plan_id, \
                    object.planned_in_stage_id, object.about_object_id, \
                    object.about_object_type, object.handles_object_id, \
                    object.handles_object_type, object.created_at, object.updated_at, \
                    object.created_by, object.updated_by, object.deleted_at, \
                    object.role_level, object.responsible_role_id, \
                    object.projection_event_id, object.source_change_id, \
                    community.project_view_enabled, community.project_view_schema_version \
             FROM project_view_objects object \
             JOIN communities community ON community.id = object.community_id \
             WHERE object.community_id = $1 AND object.object_id = $2",
        )
        .bind(identity.community_id)
        .bind(identity.source_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Project View source".to_string()))?;

        let object_type: String = row.try_get("object_type")?;
        if project_view_semantic_type(&object_type)? != requested_subtype {
            return Err(DbError::NotFound(
                "semantic Project View source".to_string(),
            ));
        }
        let schema_version: i16 = row.try_get("schema_version")?;
        let object_revision = positive_u64(row.try_get("object_revision")?, "object_revision")?;
        let source_change_id =
            digest_from_bytes(row.try_get("source_change_id")?, "source_change_id")?;
        let basis = SemanticSourceBasis::ProjectView(ProjectViewSourceBasis {
            schema_version: u16::try_from(schema_version).map_err(|_| {
                DbError::InvalidData("semantic Project View schema version is invalid".to_string())
            })?,
            object_revision,
            source_change_id,
        });
        let deleted_at = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")?;
        let capability_ready: bool = row.try_get::<bool, _>("project_view_enabled")?
            && row.try_get::<i16, _>("project_view_schema_version")? == 3;

        if schema_version != 3 || !capability_ready || deleted_at.is_some() {
            let (reason, lifecycle) = if deleted_at.is_some() {
                (
                    IneligibilityReason::Tombstone,
                    SemanticLifecycleClass::Tombstone,
                )
            } else {
                (
                    IneligibilityReason::SourceCapabilityUnavailable,
                    SemanticLifecycleClass::Active,
                )
            };
            return semantic_observation(
                identity.clone(),
                basis,
                SemanticEligibility::Ineligible(reason),
                lifecycle,
                None,
                String::new(),
                None,
            );
        }

        let entry = crate::project_view_v3::v3_entry_from_row(row).map_err(|error| {
            DbError::InvalidData(format!("invalid semantic Project View source: {error}"))
        })?;
        let ProjectViewEntryV3::Active(object) = entry else {
            return Err(DbError::InvalidData(
                "active semantic Project View row reconstructed as tombstone".to_string(),
            ));
        };
        let status = object.data.source_status().map(str::to_string);
        let lifecycle = project_view_lifecycle(&object.data);
        semantic_observation(
            identity.clone(),
            basis,
            SemanticEligibility::Eligible,
            lifecycle,
            status,
            object.data.title().to_string(),
            object.data.summary().map(str::to_string),
        )
    }

    async fn observe_project_document_source(
        &self,
        identity: &SemanticSourceIdentity,
    ) -> Result<CanonicalSemanticSourceObservation> {
        let row = sqlx::query(
            "SELECT document.current_revision, document.state, document.created_at, \
                    document.created_by, document.updated_at, document.updated_by, \
                    document.current_source_change_id, revision.title, revision.summary, \
                    revision.content_markdown, revision.actor_pubkey, revision.canonical_at, \
                    community.project_document_enabled \
             FROM project_documents document \
             JOIN project_document_revisions revision \
               ON revision.community_id = document.community_id \
              AND revision.document_id = document.document_id \
              AND revision.document_revision = document.current_revision \
             JOIN communities community ON community.id = document.community_id \
             WHERE document.community_id = $1 AND document.document_id = $2",
        )
        .bind(identity.community_id)
        .bind(identity.source_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Project Document source".to_string()))?;
        let current_revision = positive_u64(row.try_get("current_revision")?, "document_revision")?;
        let source_change_id = digest_from_bytes(
            row.try_get("current_source_change_id")?,
            "current_source_change_id",
        )?;
        let capability_ready: bool = row.try_get("project_document_enabled")?;
        let current = crate::project_document::current_document_from_row(identity.source_id, &row)
            .map_err(|error| {
                DbError::InvalidData(format!("invalid semantic Document source: {error}"))
            })?;
        let basis = SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
            document_revision: current_revision,
            source_change_id,
        });
        match current.revision() {
            DocumentRevision::Active { snapshot, .. } if capability_ready => semantic_observation(
                identity.clone(),
                basis,
                SemanticEligibility::Eligible,
                SemanticLifecycleClass::Active,
                Some(DocumentState::Active.as_str().to_string()),
                snapshot.title.clone(),
                snapshot.summary.clone(),
            ),
            DocumentRevision::Active { .. } => semantic_observation(
                identity.clone(),
                basis,
                SemanticEligibility::Ineligible(IneligibilityReason::SourceCapabilityUnavailable),
                SemanticLifecycleClass::Active,
                Some(DocumentState::Active.as_str().to_string()),
                String::new(),
                None,
            ),
            DocumentRevision::Deleted { .. } => semantic_observation(
                identity.clone(),
                basis,
                SemanticEligibility::Ineligible(IneligibilityReason::Tombstone),
                SemanticLifecycleClass::Tombstone,
                Some(DocumentState::Deleted.as_str().to_string()),
                String::new(),
                None,
            ),
        }
    }

    async fn observe_meeting_source(
        &self,
        identity: &SemanticSourceIdentity,
    ) -> Result<CanonicalSemanticSourceObservation> {
        let row = sqlx::query(
            "SELECT session.create_event_id, session.end_event_id, session.status, \
                    session.summary, channel.name, channel.deleted_at, \
                    runtime.runtime_phase \
             FROM meeting_sessions session \
             JOIN channels channel \
               ON channel.community_id = session.community_id \
              AND channel.id = session.session_id AND channel.room_kind = 'meeting' \
             LEFT JOIN meeting_v2_bootstrap_state runtime \
               ON runtime.community_id = session.community_id \
              AND runtime.session_id = session.session_id \
             WHERE session.community_id = $1 AND session.session_id = $2",
        )
        .bind(identity.community_id)
        .bind(identity.source_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Meeting source".to_string()))?;
        let create_event_id =
            digest_from_bytes(row.try_get("create_event_id")?, "create_event_id")?;
        let end_event_id = row
            .try_get::<Option<Vec<u8>>, _>("end_event_id")?
            .map(|value| digest_from_bytes(value, "end_event_id"))
            .transpose()?;
        let basis = SemanticSourceBasis::Meeting(MeetingSourceBasis {
            create_event_id,
            end_event_id,
        });
        let status: String = row.try_get("status")?;
        let runtime_phase: Option<String> = row.try_get("runtime_phase")?;
        let lifecycle = if status == "ended" {
            SemanticLifecycleClass::Terminal
        } else if runtime_phase.as_deref() == Some("finalizing_actions") {
            SemanticLifecycleClass::Finalizing
        } else {
            SemanticLifecycleClass::Active
        };
        if row
            .try_get::<Option<chrono::DateTime<chrono::Utc>>, _>("deleted_at")?
            .is_some()
        {
            return semantic_observation(
                identity.clone(),
                basis,
                SemanticEligibility::Ineligible(IneligibilityReason::Deleted),
                SemanticLifecycleClass::Deleted,
                Some(status),
                String::new(),
                None,
            );
        }
        semantic_observation(
            identity.clone(),
            basis,
            SemanticEligibility::Eligible,
            lifecycle,
            Some(status),
            row.try_get("name")?,
            row.try_get("summary")?,
        )
    }
}

fn semantic_observation(
    identity: SemanticSourceIdentity,
    basis: SemanticSourceBasis,
    eligibility: SemanticEligibility,
    lifecycle: SemanticLifecycleClass,
    source_status: Option<String>,
    title: String,
    summary: Option<String>,
) -> Result<CanonicalSemanticSourceObservation> {
    CanonicalSemanticSourceObservation::new(
        identity,
        basis,
        eligibility,
        SemanticFilterMetadata {
            lifecycle,
            source_status,
        },
        title,
        summary,
    )
    .map_err(|error| semantic_contract_error("source_observation", error))
}

fn semantic_contract_error(context: &str, error: buzz_semantic::SemanticError) -> DbError {
    DbError::InvalidData(format!("invalid semantic {context}: {error}"))
}

fn digest_from_bytes(value: Vec<u8>, field: &str) -> Result<Digest32> {
    let bytes: [u8; 32] = value
        .try_into()
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must contain 32 bytes")))?;
    Ok(Digest32::from_bytes(bytes))
}

fn positive_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| DbError::InvalidData(format!("semantic {field} must be positive")))
}

fn project_view_semantic_type(value: &str) -> Result<ProjectViewSemanticType> {
    match value {
        "project_profile" => Ok(ProjectViewSemanticType::ProjectProfile),
        "goal" => Ok(ProjectViewSemanticType::Goal),
        "role" => Ok(ProjectViewSemanticType::Role),
        "plan" => Ok(ProjectViewSemanticType::Plan),
        "stage" => Ok(ProjectViewSemanticType::Stage),
        "requirement" => Ok(ProjectViewSemanticType::Requirement),
        "issue" => Ok(ProjectViewSemanticType::Issue),
        "work" => Ok(ProjectViewSemanticType::Work),
        "resource" => Ok(ProjectViewSemanticType::Resource),
        _ => Err(DbError::InvalidData(
            "semantic Project View source has an unknown subtype".to_string(),
        )),
    }
}

fn semantic_source_subtype(kind: SemanticSourceKind) -> &'static str {
    match kind {
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::ProjectProfile) => {
            "project_profile"
        }
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Goal) => "goal",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Role) => "role",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Plan) => "plan",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Stage) => "stage",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Requirement) => "requirement",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Issue) => "issue",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Work) => "work",
        SemanticSourceKind::ProjectView(ProjectViewSemanticType::Resource) => "resource",
        SemanticSourceKind::ProjectDocument => "document",
        SemanticSourceKind::Meeting => "meeting",
    }
}

fn project_view_lifecycle(data: &ProjectViewObjectDataV3) -> SemanticLifecycleClass {
    match data.source_status() {
        Some(
            "completed" | "cancelled" | "satisfied" | "withdrawn" | "resolved" | "closed"
            | "inactive",
        ) => SemanticLifecycleClass::Terminal,
        _ => SemanticLifecycleClass::Active,
    }
}

fn validate_single_subtype_cursor(
    cursor: Option<&SemanticSourceScanCursor>,
    expected: &str,
) -> Result<()> {
    if cursor.is_some_and(|cursor| cursor.source_subtype != expected) {
        return Err(DbError::InvalidData(
            "semantic source scan cursor does not match the selected family".to_string(),
        ));
    }
    Ok(())
}

fn validate_rebuild_cursor(
    family: SemanticScanFamily,
    cursor: &SemanticSourceScanCursor,
) -> Result<()> {
    if cursor.source_id.is_nil() {
        return Err(DbError::InvalidData(
            "semantic rebuild cursor source id must not be nil".to_string(),
        ));
    }
    match family {
        SemanticScanFamily::ProjectView => {
            project_view_semantic_type(&cursor.source_subtype)?;
        }
        SemanticScanFamily::ProjectDocument => {
            validate_single_subtype_cursor(Some(cursor), "document")?;
        }
        SemanticScanFamily::Meeting => {
            validate_single_subtype_cursor(Some(cursor), "meeting")?;
        }
    }
    Ok(())
}

async fn enqueue_semantic_jobs_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    community_id: Uuid,
    family: &str,
    subtype: &str,
    source_id: Uuid,
    epoch: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO semantic_index_jobs (\
             community_id,generation_id,source_family,source_subtype,source_id, \
             desired_invalidation_epoch,state,attempts,next_attempt_at,created_at,updated_at) \
         SELECT $1,generation.generation_id,$2,$3,$4,$5,'pending',0, \
                clock_timestamp(),clock_timestamp(),clock_timestamp() \
         FROM semantic_index_generations generation \
         WHERE generation.community_id=$1 \
           AND generation.lifecycle IN ('building','ready','active','rollback_ready') \
         ON CONFLICT (community_id,generation_id,source_family,source_subtype,source_id) \
         DO UPDATE SET desired_invalidation_epoch=EXCLUDED.desired_invalidation_epoch, \
             state='pending',attempts=0,next_attempt_at=clock_timestamp(),claim_id=NULL, \
             lease_until=NULL,claimed_at=NULL,completed_at=NULL,error_code=NULL,error_detail=NULL, \
             updated_at=clock_timestamp() \
         WHERE semantic_index_jobs.desired_invalidation_epoch \
               <> EXCLUDED.desired_invalidation_epoch",
    )
    .bind(community_id)
    .bind(family)
    .bind(subtype)
    .bind(source_id)
    .bind(epoch)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn semantic_generation_from_row(row: &sqlx::postgres::PgRow) -> Result<SemanticGenerationRecord> {
    let dimensions: i32 = row.try_get("dimensions")?;
    let provider: String = row.try_get("provider")?;
    let boundary: String = row.try_get("provider_boundary")?;
    let model_contract = SemanticModelContract {
        provider: provider.clone(),
        model: row.try_get("model")?,
        dimensions: usize::try_from(dimensions).map_err(|_| {
            DbError::InvalidData("semantic generation dimensions are invalid".to_string())
        })?,
        distance_metric: parse_distance_metric(row.try_get("distance_metric")?)?,
        normalization: parse_normalization(row.try_get("normalization")?)?,
        input_contract_version: row.try_get("input_contract_version")?,
        provider_boundary: parse_provider_boundary(&boundary, &provider)?,
    };
    model_contract
        .validate()
        .map_err(|error| semantic_contract_error("stored_model_contract", error))?;
    let stored_digest = digest_from_bytes(
        row.try_get("model_contract_digest")?,
        "model_contract_digest",
    )?;
    let observed_digest = model_contract
        .digest()
        .map_err(|error| semantic_contract_error("stored_model_contract", error))?;
    if stored_digest != observed_digest {
        return Err(DbError::InvalidData(
            "semantic generation model contract digest mismatch".to_string(),
        ));
    }
    Ok(SemanticGenerationRecord {
        community_id: CommunityId::from_uuid(row.try_get("community_id")?),
        generation_id: row.try_get("generation_id")?,
        lifecycle: row.try_get("lifecycle")?,
        extractor_version: row.try_get("extractor_version")?,
        model_contract,
        model_contract_digest: stored_digest,
        rebuild_completed_at: row.try_get("rebuild_completed_at")?,
        created_at: row.try_get("created_at")?,
    })
}

fn semantic_job_lease_from_row(row: &sqlx::postgres::PgRow) -> Result<SemanticJobLease> {
    let family: String = row.try_get("source_family")?;
    let subtype: String = row.try_get("source_subtype")?;
    let kind = semantic_source_kind_from_db(&family, &subtype)?;
    let dimensions: i32 = row.try_get("dimensions")?;
    let provider: String = row.try_get("provider")?;
    let model_contract = SemanticModelContract {
        provider: provider.clone(),
        model: row.try_get("model")?,
        dimensions: usize::try_from(dimensions).map_err(|_| {
            DbError::InvalidData("semantic generation dimensions are invalid".to_string())
        })?,
        distance_metric: parse_distance_metric(row.try_get("distance_metric")?)?,
        normalization: parse_normalization(row.try_get("normalization")?)?,
        input_contract_version: row.try_get("input_contract_version")?,
        provider_boundary: parse_provider_boundary(
            row.try_get::<String, _>("provider_boundary")?.as_str(),
            &provider,
        )?,
    };
    model_contract
        .validate()
        .map_err(|error| semantic_contract_error("stored_model_contract", error))?;
    let model_contract_digest = digest_from_bytes(
        row.try_get("model_contract_digest")?,
        "model_contract_digest",
    )?;
    if model_contract
        .digest()
        .map_err(|error| semantic_contract_error("stored_model_contract", error))?
        != model_contract_digest
    {
        return Err(DbError::InvalidData(
            "semantic job model contract digest mismatch".to_string(),
        ));
    }
    let community_id = CommunityId::from_uuid(row.try_get("community_id")?);
    let generation_id = row.try_get("generation_id")?;
    Ok(SemanticJobLease {
        source: SemanticSourceIdentity {
            community_id: *community_id.as_uuid(),
            kind,
            source_id: row.try_get("source_id")?,
        },
        generation_id,
        desired_invalidation_epoch: positive_u64(
            row.try_get("desired_invalidation_epoch")?,
            "desired_invalidation_epoch",
        )?,
        claim_id: row.try_get("claim_id")?,
        lease_until: row.try_get("lease_until")?,
        attempts: u32::try_from(row.try_get::<i32, _>("attempts")?)
            .map_err(|_| DbError::InvalidData("semantic job attempts are invalid".to_string()))?,
        extractor_version: row.try_get("extractor_version")?,
        model_contract,
        model_contract_digest,
    })
}

fn validate_semantic_activation(
    lease: &SemanticJobLease,
    observation: &CanonicalSemanticSourceObservation,
    units: &[SemanticUnit],
    encoded: &[EncodedSemanticUnit],
) -> Result<()> {
    if observation.identity != lease.source || units.is_empty() || units.len() != encoded.len() {
        return Err(DbError::InvalidData(
            "semantic activation identity or complete-count mismatch".to_string(),
        ));
    }
    lease
        .model_contract
        .validate()
        .map_err(|error| semantic_contract_error("claim_model_contract", error))?;
    if lease
        .model_contract
        .digest()
        .map_err(|error| semantic_contract_error("claim_model_contract", error))?
        != lease.model_contract_digest
    {
        return Err(DbError::InvalidData(
            "semantic claim model contract digest mismatch".to_string(),
        ));
    }
    for (unit, result) in units.iter().zip(encoded) {
        if unit.identity.source != lease.source
            || unit.identity.source_snapshot_digest != observation.snapshot_digest
            || unit.identity.extractor_version != lease.extractor_version
            || result.identity() != &unit.identity
            || result.semantic_text_digest() != unit.semantic_text_digest
            || result.model_contract_digest() != lease.model_contract_digest
            || result.response_model() != lease.model_contract.model
            || result.embedding().as_slice().len() != lease.model_contract.dimensions
            || result
                .embedding()
                .as_slice()
                .iter()
                .any(|value| !value.is_finite())
        {
            return Err(DbError::InvalidData(
                "semantic activation unit or embedding contract mismatch".to_string(),
            ));
        }
    }
    Ok(())
}

fn semantic_source_db_key(kind: SemanticSourceKind) -> (&'static str, &'static str) {
    match kind {
        SemanticSourceKind::ProjectView(subtype) => (
            "project_view",
            semantic_source_subtype(SemanticSourceKind::ProjectView(subtype)),
        ),
        SemanticSourceKind::ProjectDocument => ("project_document", "document"),
        SemanticSourceKind::Meeting => ("meeting", "meeting"),
    }
}

const fn scan_family_db(family: SemanticScanFamily) -> &'static str {
    match family {
        SemanticScanFamily::ProjectView => "project_view",
        SemanticScanFamily::ProjectDocument => "project_document",
        SemanticScanFamily::Meeting => "meeting",
    }
}

fn parse_scan_family(value: &str) -> Result<SemanticScanFamily> {
    match value {
        "project_view" => Ok(SemanticScanFamily::ProjectView),
        "project_document" => Ok(SemanticScanFamily::ProjectDocument),
        "meeting" => Ok(SemanticScanFamily::Meeting),
        _ => Err(DbError::InvalidData(
            "semantic rebuild source family is invalid".to_string(),
        )),
    }
}

const fn rebuild_scope_db(scope: SemanticRebuildScope) -> &'static str {
    match scope {
        SemanticRebuildScope::All => "all",
        SemanticRebuildScope::Family(family) => scan_family_db(family),
    }
}

fn parse_rebuild_scope(value: &str) -> Result<SemanticRebuildScope> {
    match value {
        "all" => Ok(SemanticRebuildScope::All),
        value => Ok(SemanticRebuildScope::Family(parse_scan_family(value)?)),
    }
}

const fn next_rebuild_family(
    scope: SemanticRebuildScope,
    current: SemanticScanFamily,
) -> Option<SemanticScanFamily> {
    match scope {
        SemanticRebuildScope::Family(_) => None,
        SemanticRebuildScope::All => match current {
            SemanticScanFamily::ProjectView => Some(SemanticScanFamily::ProjectDocument),
            SemanticScanFamily::ProjectDocument => Some(SemanticScanFamily::Meeting),
            SemanticScanFamily::Meeting => None,
        },
    }
}

fn semantic_rebuild_from_row(row: &sqlx::postgres::PgRow) -> Result<SemanticRebuildOperation> {
    let cursor_subtype: Option<String> = row.try_get("after_source_subtype")?;
    let cursor_id: Option<Uuid> = row.try_get("after_source_id")?;
    let cursor = match (cursor_subtype, cursor_id) {
        (Some(source_subtype), Some(source_id)) => Some(SemanticSourceScanCursor {
            source_subtype,
            source_id,
        }),
        (None, None) => None,
        _ => {
            return Err(DbError::InvalidData(
                "semantic rebuild cursor shape is invalid".to_string(),
            ));
        }
    };
    let state = match row.try_get::<String, _>("state")?.as_str() {
        "running" => SemanticRebuildState::Running,
        "completed" => SemanticRebuildState::Completed,
        "cancelled" => SemanticRebuildState::Cancelled,
        _ => {
            return Err(DbError::InvalidData(
                "semantic rebuild state is invalid".to_string(),
            ));
        }
    };
    Ok(SemanticRebuildOperation {
        community_id: CommunityId::from_uuid(row.try_get("community_id")?),
        operation_id: row.try_get("operation_id")?,
        generation_id: row.try_get("generation_id")?,
        scope: parse_rebuild_scope(row.try_get::<String, _>("scope_family")?.as_str())?,
        current_family: parse_scan_family(row.try_get::<String, _>("current_family")?.as_str())?,
        cursor,
        state,
    })
}

fn semantic_source_kind_from_db(family: &str, subtype: &str) -> Result<SemanticSourceKind> {
    match family {
        "project_view" => Ok(SemanticSourceKind::ProjectView(project_view_semantic_type(
            subtype,
        )?)),
        "project_document" if subtype == "document" => Ok(SemanticSourceKind::ProjectDocument),
        "meeting" if subtype == "meeting" => Ok(SemanticSourceKind::Meeting),
        _ => Err(DbError::InvalidData(
            "semantic source family or subtype is invalid".to_string(),
        )),
    }
}

const fn distance_metric_db(metric: SemanticDistanceMetric) -> &'static str {
    match metric {
        SemanticDistanceMetric::Cosine => "cosine",
    }
}

fn parse_distance_metric(value: String) -> Result<SemanticDistanceMetric> {
    match value.as_str() {
        "cosine" => Ok(SemanticDistanceMetric::Cosine),
        _ => Err(DbError::InvalidData(
            "semantic distance metric is invalid".to_string(),
        )),
    }
}

const fn normalization_db(value: SemanticNormalization) -> &'static str {
    match value {
        SemanticNormalization::None => "none",
    }
}

fn parse_normalization(value: String) -> Result<SemanticNormalization> {
    match value.as_str() {
        "none" => Ok(SemanticNormalization::None),
        _ => Err(DbError::InvalidData(
            "semantic normalization is invalid".to_string(),
        )),
    }
}

fn provider_boundary_db(value: &SemanticProviderBoundary) -> &'static str {
    match value {
        SemanticProviderBoundary::External(_) => "external",
        SemanticProviderBoundary::DeterministicFake => "deterministic_fake",
    }
}

fn parse_provider_boundary(value: &str, provider: &str) -> Result<SemanticProviderBoundary> {
    match value {
        "external" => Ok(SemanticProviderBoundary::External(provider.to_string())),
        "deterministic_fake" => Ok(SemanticProviderBoundary::DeterministicFake),
        _ => Err(DbError::InvalidData(
            "semantic provider boundary is invalid".to_string(),
        )),
    }
}

const fn eligibility_db(value: SemanticEligibility) -> &'static str {
    match value {
        SemanticEligibility::Eligible => "eligible",
        SemanticEligibility::Ineligible(_) => "ineligible",
    }
}

const fn ineligibility_reason_db(value: SemanticEligibility) -> Option<&'static str> {
    match value {
        SemanticEligibility::Eligible => None,
        SemanticEligibility::Ineligible(IneligibilityReason::Tombstone) => Some("tombstone"),
        SemanticEligibility::Ineligible(IneligibilityReason::Deleted) => Some("deleted"),
        SemanticEligibility::Ineligible(IneligibilityReason::InvalidCanonicalState) => {
            Some("invalid_canonical_state")
        }
        SemanticEligibility::Ineligible(IneligibilityReason::SourceCapabilityUnavailable) => {
            Some("source_capability_unavailable")
        }
    }
}

const fn lifecycle_db(value: SemanticLifecycleClass) -> &'static str {
    match value {
        SemanticLifecycleClass::Active => "active",
        SemanticLifecycleClass::Finalizing => "finalizing",
        SemanticLifecycleClass::Terminal => "terminal",
        SemanticLifecycleClass::Tombstone => "tombstone",
        SemanticLifecycleClass::Deleted => "deleted",
    }
}

const fn unit_kind_db(value: SemanticUnitKind) -> &'static str {
    match value {
        SemanticUnitKind::Overview => "overview",
        SemanticUnitKind::ContentChunk => "content_chunk",
    }
}

const fn coverage_db(value: SemanticCoverage) -> &'static str {
    match value {
        SemanticCoverage::TitleOnly => "title_only",
        SemanticCoverage::TitleAndSummary => "title_and_summary",
    }
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| DbError::InvalidData(format!("semantic {field} exceeds PostgreSQL bigint")))
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| DbError::InvalidData(format!("semantic {field} must not be negative")))
}

fn vector_version_supported(version: &str) -> bool {
    let mut parts = version.split('.');
    let Some(major) = parts.next().and_then(|part| part.parse::<u16>().ok()) else {
        return false;
    };
    let Some(minor) = parts.next().and_then(|part| part.parse::<u16>().ok()) else {
        return false;
    };
    let Some(patch) = parts
        .next()
        .and_then(|part| part.split(['-', '+']).next())
        .and_then(|part| part.parse::<u16>().ok())
    else {
        return false;
    };
    major == 0 && minor == 8 && patch >= 5
}

#[cfg(test)]
mod tests {
    use buzz_semantic::{
        extract_overview, CanonicalSemanticSourceObservation, DeterministicFakeEncoder, Digest32,
        EncodedSemanticUnit, IneligibilityReason, ProjectDocumentSourceBasis, SemanticEligibility,
        SemanticEncoder, SemanticEncoderInput, SemanticFilterMetadata, SemanticLifecycleClass,
        SemanticSourceBasis, SemanticSourceIdentity, SemanticSourceKind,
        OVERVIEW_EXTRACTOR_VERSION,
    };
    use uuid::Uuid;

    use super::{
        vector_version_supported, CreateSemanticGeneration, SemanticActivationOutcome,
        SemanticClaimObservationOutcome, SemanticPgvectorPreflight, SemanticRebuildScope,
        SemanticRebuildState, SemanticScanFamily,
    };
    use crate::{Db, DbConfig};

    fn ready_report() -> SemanticPgvectorPreflight {
        SemanticPgvectorPreflight {
            server_version_num: 170_006,
            server_version: "17.6".to_string(),
            available_vector_version: Some("0.8.5".to_string()),
            installed_vector_version: Some("0.8.5".to_string()),
            vector_type_available: true,
            halfvec_type_available: true,
            vector_roundtrip_ok: true,
            cosine_distance_ok: true,
            halfvec_cast_ok: true,
            sqlx_2048_roundtrip_ok: true,
        }
    }

    #[test]
    fn supported_version_contract_is_closed_to_pgvector_zero_eight() {
        assert!(vector_version_supported("0.8.5"));
        assert!(vector_version_supported("0.8.9+vendor"));
        assert!(!vector_version_supported("0.8.4"));
        assert!(!vector_version_supported("0.9.0"));
        assert!(!vector_version_supported("invalid"));
    }

    #[test]
    fn preflight_reports_stable_failures() {
        let report = ready_report();
        assert!(report.ready());
        assert!(report.failure_codes().is_empty());

        let failed = SemanticPgvectorPreflight {
            installed_vector_version: None,
            halfvec_type_available: false,
            halfvec_cast_ok: false,
            ..report
        };
        assert!(!failed.ready());
        assert_eq!(
            failed.failure_codes(),
            [
                "pgvector_not_installed_in_database",
                "halfvec_type_unavailable",
                "halfvec_cast_failed",
            ]
        );
    }

    #[tokio::test]
    async fn semantic_pipeline_activates_only_a_complete_fenced_set() {
        let Ok(database_url) = std::env::var("BUZZ_TEST_SEMANTIC_DATABASE_URL") else {
            return;
        };
        let db = Db::new(&DbConfig {
            database_url,
            ..DbConfig::default()
        })
        .await
        .expect("semantic test database");
        db.migrate().await.expect("semantic migrations");
        let community_id = buzz_core::CommunityId::from_uuid(Uuid::new_v4());
        sqlx::query("INSERT INTO communities(id,host) VALUES ($1,$2)")
            .bind(community_id.as_uuid())
            .bind(format!("semantic-{}.invalid", community_id.as_uuid()))
            .execute(&db.pool)
            .await
            .expect("semantic community");
        let encoder = DeterministicFakeEncoder::new(8).expect("fake encoder");
        let generation_id = Uuid::new_v4();
        db.create_semantic_generation(CreateSemanticGeneration {
            community_id,
            generation_id,
            extractor_version: OVERVIEW_EXTRACTOR_VERSION,
            model_contract: encoder.contract(),
            created_by: "semantic-test",
        })
        .await
        .expect("generation");
        db.set_semantic_community_enabled(community_id, true)
            .await
            .expect("enable");
        let first_slot = db
            .reserve_semantic_provider_slot(
                community_id,
                "deterministic_fake",
                std::time::Duration::from_millis(100),
            )
            .await
            .expect("first distributed provider slot");
        let second_slot = db
            .reserve_semantic_provider_slot(
                community_id,
                "deterministic_fake",
                std::time::Duration::from_millis(100),
            )
            .await
            .expect("second distributed provider slot");
        assert!(first_slot < std::time::Duration::from_millis(50));
        assert!(second_slot >= std::time::Duration::from_millis(50));
        let before_rebuild = db
            .semantic_generation_coverage(community_id, generation_id)
            .await
            .expect("coverage before rebuild");
        assert!(!before_rebuild.complete());
        let mut rebuild = db
            .start_semantic_rebuild(
                community_id,
                generation_id,
                Uuid::new_v4(),
                SemanticRebuildScope::All,
            )
            .await
            .expect("start rebuild");
        assert_eq!(rebuild.current_family, SemanticScanFamily::ProjectView);
        for expected_family in [
            SemanticScanFamily::ProjectView,
            SemanticScanFamily::ProjectDocument,
            SemanticScanFamily::Meeting,
        ] {
            assert_eq!(rebuild.current_family, expected_family);
            rebuild = db
                .checkpoint_semantic_rebuild(&rebuild, None, true)
                .await
                .expect("advance rebuild family");
        }
        assert_eq!(rebuild.state, SemanticRebuildState::Completed);
        let observation = CanonicalSemanticSourceObservation::new(
            SemanticSourceIdentity {
                community_id: *community_id.as_uuid(),
                kind: SemanticSourceKind::ProjectDocument,
                source_id: Uuid::new_v4(),
            },
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 1,
                source_change_id: Digest32::from_bytes([7; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Active,
                source_status: Some("active".to_string()),
            },
            "Semantic test".to_string(),
            Some("A source-owned overview".to_string()),
        )
        .expect("observation");
        db.reconcile_semantic_observation(&observation)
            .await
            .expect("reconcile");
        let lease = db
            .claim_due_semantic_job(60)
            .await
            .expect("claim query")
            .expect("claimed job");
        assert_eq!(lease.source, observation.identity);
        assert_eq!(
            db.prepare_semantic_claim_observation(&lease, &observation)
                .await
                .expect("prepare"),
            SemanticClaimObservationOutcome::Ready
        );
        let unit = extract_overview(&observation).expect("overview");
        let encoded = encoder
            .encode(&[SemanticEncoderInput::from_unit(&unit)])
            .await
            .expect("encode");
        assert_eq!(
            db.activate_semantic_claim(&lease, &observation, &[unit], &encoded)
                .await
                .expect("activate set"),
            SemanticActivationOutcome::Activated
        );
        let coverage = db
            .semantic_generation_coverage(community_id, generation_id)
            .await
            .expect("coverage");
        assert!(coverage.complete());
        db.mark_semantic_generation_ready(community_id, generation_id)
            .await
            .expect("first generation ready");
        db.activate_semantic_generation(community_id, generation_id)
            .await
            .expect("first generation active");

        let second_encoder = DeterministicFakeEncoder::new(12).expect("second fake encoder");
        let second_generation_id = Uuid::new_v4();
        db.create_semantic_generation(CreateSemanticGeneration {
            community_id,
            generation_id: second_generation_id,
            extractor_version: OVERVIEW_EXTRACTOR_VERSION,
            model_contract: second_encoder.contract(),
            created_by: "semantic-test",
        })
        .await
        .expect("second generation");
        let mut second_rebuild = db
            .start_semantic_rebuild(
                community_id,
                second_generation_id,
                Uuid::new_v4(),
                SemanticRebuildScope::All,
            )
            .await
            .expect("second rebuild");
        for _family in 0..3 {
            second_rebuild = db
                .checkpoint_semantic_rebuild(&second_rebuild, None, true)
                .await
                .expect("advance second rebuild");
        }
        db.reconcile_semantic_observation(&observation)
            .await
            .expect("enqueue second generation");
        let second_lease = db
            .claim_due_semantic_job(60)
            .await
            .expect("second generation claim query")
            .expect("second generation claim");
        assert_eq!(second_lease.generation_id, second_generation_id);
        assert_eq!(
            db.prepare_semantic_claim_observation(&second_lease, &observation)
                .await
                .expect("prepare second generation"),
            SemanticClaimObservationOutcome::Ready
        );
        let second_unit = extract_overview(&observation).expect("second overview");
        let second_encoded = second_encoder
            .encode(&[SemanticEncoderInput::from_unit(&second_unit)])
            .await
            .expect("second encode");
        assert_eq!(
            db.activate_semantic_claim(
                &second_lease,
                &observation,
                &[second_unit],
                &second_encoded,
            )
            .await
            .expect("activate second set"),
            SemanticActivationOutcome::Activated
        );
        db.mark_semantic_generation_ready(community_id, second_generation_id)
            .await
            .expect("second generation ready");
        db.activate_semantic_generation(community_id, second_generation_id)
            .await
            .expect("second generation active");
        db.activate_semantic_generation(community_id, generation_id)
            .await
            .expect("rollback first generation");
        db.activate_semantic_generation(community_id, second_generation_id)
            .await
            .expect("restore second generation");
        assert_eq!(
            db.semantic_community_state(community_id)
                .await
                .expect("community state")
                .1,
            Some(second_generation_id)
        );

        let terminal_observation = CanonicalSemanticSourceObservation::new(
            observation.identity.clone(),
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 2,
                source_change_id: Digest32::from_bytes([8; 32]),
            }),
            SemanticEligibility::Eligible,
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Terminal,
                source_status: Some("closed".to_string()),
            },
            observation.title.clone(),
            observation.summary.clone(),
        )
        .expect("terminal observation");
        db.reconcile_semantic_observation(&terminal_observation)
            .await
            .expect("terminal reconcile");
        assert!(!db
            .semantic_generation_coverage(community_id, generation_id)
            .await
            .expect("invalidated coverage")
            .complete());
        let terminal_lease = db
            .claim_due_semantic_job(60)
            .await
            .expect("terminal claim query")
            .expect("terminal claim");
        assert_eq!(
            db.prepare_semantic_claim_observation(&terminal_lease, &terminal_observation)
                .await
                .expect("prepare terminal"),
            SemanticClaimObservationOutcome::Ready
        );
        let terminal_unit = extract_overview(&terminal_observation).expect("terminal overview");
        let reusable = db
            .reusable_semantic_embedding(
                community_id,
                terminal_lease.generation_id,
                &terminal_unit,
                terminal_lease.model_contract_digest,
            )
            .await
            .expect("reuse query")
            .expect("same semantic text reuses embedding");
        let reused = EncodedSemanticUnit::new(
            &terminal_unit,
            reusable.response_model,
            reusable.values,
            &terminal_lease.model_contract,
        )
        .expect("reused embedding");
        assert_eq!(
            db.activate_semantic_claim(
                &terminal_lease,
                &terminal_observation,
                &[terminal_unit],
                &[reused],
            )
            .await
            .expect("activate terminal"),
            SemanticActivationOutcome::Activated
        );

        let deleted_observation = CanonicalSemanticSourceObservation::new(
            observation.identity,
            SemanticSourceBasis::ProjectDocument(ProjectDocumentSourceBasis {
                document_revision: 3,
                source_change_id: Digest32::from_bytes([9; 32]),
            }),
            SemanticEligibility::Ineligible(IneligibilityReason::Tombstone),
            SemanticFilterMetadata {
                lifecycle: SemanticLifecycleClass::Tombstone,
                source_status: Some("deleted".to_string()),
            },
            String::new(),
            None,
        )
        .expect("deleted observation");
        db.reconcile_semantic_observation(&deleted_observation)
            .await
            .expect("delete reconcile");
        let deleted_coverage = db
            .semantic_generation_coverage(community_id, generation_id)
            .await
            .expect("deleted coverage");
        assert_eq!(deleted_coverage.eligible_sources, 0);
        assert_eq!(deleted_coverage.current_heads, 0);
        assert!(deleted_coverage.complete());
        db.retire_semantic_generation(community_id, generation_id)
            .await
            .expect("retire rollback generation");
        assert!(
            db.purge_semantic_generation(community_id, generation_id)
                .await
                .expect("purge retired generation")
                > 0
        );
    }
}
