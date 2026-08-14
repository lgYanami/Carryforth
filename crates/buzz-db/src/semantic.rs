//! Project Context semantic-index deployment probes.
//!
//! This module starts with the database prerequisite probe. Later phases add
//! only derived semantic state here; canonical Project View, Document, Meeting,
//! and Project Context ownership remains in their existing modules.

use buzz_core::CommunityId;
use buzz_project_document::{DocumentRevision, DocumentState};
use buzz_project_view::v3::{ProjectContextReference, ProjectViewEntryV3, ProjectViewObjectDataV3};
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
use sqlx::{PgConnection, Postgres, Row, Transaction};
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

/// Closed provider workload lanes sharing one physical Community/provider gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticProviderWorkload {
    /// Latency-bounded graph-query encoding.
    InteractiveQuery,
    /// Durable overview indexing and generation rebuilds.
    BackgroundIndex,
}

/// Outcome of one deadline-aware distributed provider admission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticProviderReservation {
    /// Exactly one physical slot was consumed; wait this long before transport.
    Reserved {
        /// Delay remaining before the reserved transport slot starts.
        wait: std::time::Duration,
    },
    /// No usable slot existed before the deadline; no gate row was changed.
    Busy,
}

/// Single-use proof that a background overview input was current at the
/// provider-egress linearization point.
///
/// The permit deliberately cannot be cloned. It carries no source content and
/// is useful only to make the worker's final database fence explicit before
/// handing the already-built input to the configured provider.
#[derive(Debug)]
#[must_use = "a semantic worker egress permit must be consumed at provider handoff"]
pub struct SemanticWorkerEgressPermit {
    _private: (),
}

/// Closed result of the background worker's final provider-egress fence.
#[derive(Debug)]
#[must_use = "semantic provider egress is allowed only by the permitted outcome"]
pub enum SemanticWorkerEgressConfirmation {
    /// Every Community, generation, claim, and exact source-currentness fence
    /// passed in one writer transaction.
    Permitted(SemanticWorkerEgressPermit),
    /// At least one fence no longer matched; no source text may leave the
    /// process for this claim.
    Unavailable,
}

/// Content-free database prerequisites for enabling semantic graph queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticGraphQueryReadiness {
    /// All additive query schema objects and constraints exist.
    pub schema_ready: bool,
    /// Foundation indexing is enabled for the Community.
    pub index_enabled: bool,
    /// The query egress gate is currently enabled.
    pub query_enabled: bool,
    /// The canonical Project Context graph gate is enabled.
    pub project_context_enabled: bool,
    /// Published active generation, if any.
    pub active_generation_id: Option<Uuid>,
    /// The active pointer resolves to a generation in `active` lifecycle.
    pub active_generation_ready: bool,
    /// Current heads without one exact, model-matching, non-zero overview.
    pub non_queryable_current_heads: u64,
    /// Persisted rows using the response-only virtual kind.
    pub persisted_virtual_events: u64,
}

/// Closed, content-free outcome of repairing historical zero query vectors.
///
/// The repair is scoped to the Community's currently published active
/// generation. It removes only exact current heads whose overview embedding
/// has zero cosine norm and schedules the same source epoch for rebuilding;
/// it never advances or rewrites canonical source currentness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticQueryVectorRepairReport {
    /// Tenant boundary selected by the operator.
    pub community_id: CommunityId,
    /// Active generation inspected and repaired under the generation fence.
    pub active_generation_id: Uuid,
    /// Every head inspected in the active generation.
    pub current_heads_scanned: u64,
    /// Heads with an exact, model-matching, non-zero overview embedding.
    pub queryable_current_heads: u64,
    /// Exact current heads invalidated because their overview vector was zero.
    pub zero_vector_current_heads: u64,
    /// Heads that failed a non-zero-queryability fence for another reason.
    pub other_nonqueryable_current_heads: u64,
    /// Historical zero overview embedding rows observed behind repaired heads.
    pub zero_vector_embeddings: u64,
    /// Zero-vector heads removed from query-current eligibility.
    pub heads_invalidated: u64,
    /// Missing source-generation jobs created for the exact current epoch.
    pub jobs_created: u64,
    /// Existing source-generation jobs reset to immediately pending.
    pub jobs_requeued: u64,
}

impl SemanticGraphQueryReadiness {
    /// Whether database-owned enable prerequisites currently pass.
    ///
    /// Deployment master, routing policy, provider compatibility, signer, and
    /// Project Context projection checks remain Relay/operator fences.
    pub const fn database_ready(&self) -> bool {
        self.schema_ready
            && self.index_enabled
            && self.project_context_enabled
            && self.active_generation_ready
            && self.non_queryable_current_heads == 0
            && self.persisted_virtual_events == 0
    }
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

/// Reserve one provider slot inside a caller-owned transaction.
///
/// This is the single physical/lane capacity-admission implementation used by
/// both the background worker and graph queries. It is never an egress
/// authorization or currentness proof. `Busy` performs no durable write;
/// callers must end the transaction to release its advisory and row locks. A
/// successful reservation remains consumed when the caller commits, even if a
/// later final egress fence rejects the request or the network call fails.
pub(crate) async fn reserve_semantic_provider_slot_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    community_id: CommunityId,
    provider: &str,
    workload: SemanticProviderWorkload,
    interval: std::time::Duration,
    latest_start_at: DateTime<Utc>,
) -> Result<SemanticProviderReservation> {
    if provider.trim().is_empty() || provider.trim() != provider || provider.len() > 255 {
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
    let interval_chrono = ChronoDuration::from_std(interval)
        .map_err(|_| DbError::InvalidData("semantic provider interval is invalid".to_string()))?;
    let lane_interval = interval.checked_mul(2).ok_or_else(|| {
        DbError::InvalidData("semantic provider lane interval overflow".to_string())
    })?;
    let lane_interval_chrono = ChronoDuration::from_std(lane_interval).map_err(|_| {
        DbError::InvalidData("semantic provider lane interval is invalid".to_string())
    })?;
    let workload = semantic_provider_workload_db(workload);

    // This closes the absent-row race before the physical/lane rows exist;
    // established rows are additionally selected FOR UPDATE below.
    let lock_identity = format!(
        "buzz.semantic-provider:{}:{provider}",
        community_id.as_uuid()
    );
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(lock_identity)
        .execute(&mut **tx)
        .await?;

    let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
        .fetch_one(&mut **tx)
        .await?;
    if latest_start_at > now + ChronoDuration::minutes(5) {
        return Err(DbError::InvalidData(
            "semantic provider reservation horizon exceeds five minutes".to_string(),
        ));
    }
    let physical_next: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_request_at FROM semantic_provider_rate_gates \
         WHERE community_id=$1 AND provider=$2 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(provider)
    .fetch_optional(&mut **tx)
    .await?;
    let lane_next: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT next_admission_at FROM semantic_query_provider_admission \
         WHERE community_id=$1 AND provider=$2 AND workload=$3 FOR UPDATE",
    )
    .bind(community_id.as_uuid())
    .bind(provider)
    .bind(workload)
    .fetch_optional(&mut **tx)
    .await?;

    let physical_gate_idle = physical_next.is_none_or(|next| next <= now);
    let scheduled_at = physical_next.map_or(now, |next| next.max(now));
    if scheduled_at > latest_start_at
        || (!physical_gate_idle && lane_next.is_some_and(|next| next > scheduled_at))
    {
        return Ok(SemanticProviderReservation::Busy);
    }

    let next_request_at = scheduled_at + interval_chrono;
    let next_admission_at = scheduled_at + lane_interval_chrono;
    sqlx::query(
        "INSERT INTO semantic_provider_rate_gates (\
             community_id,provider,next_request_at,updated_at) \
         VALUES ($1,$2,$3,$4) \
         ON CONFLICT (community_id,provider) DO UPDATE SET \
             next_request_at=EXCLUDED.next_request_at, \
             updated_at=EXCLUDED.updated_at",
    )
    .bind(community_id.as_uuid())
    .bind(provider)
    .bind(next_request_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    sqlx::query(
        "INSERT INTO semantic_query_provider_admission (\
             community_id,provider,workload,next_admission_at,updated_at) \
         VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (community_id,provider,workload) DO UPDATE SET \
             next_admission_at=EXCLUDED.next_admission_at, \
             updated_at=EXCLUDED.updated_at",
    )
    .bind(community_id.as_uuid())
    .bind(provider)
    .bind(workload)
    .bind(next_admission_at)
    .bind(now)
    .execute(&mut **tx)
    .await?;

    let wait_millis = (scheduled_at - now).num_milliseconds().max(0);
    Ok(SemanticProviderReservation::Reserved {
        wait: std::time::Duration::from_millis(u64::try_from(wait_millis).unwrap_or(u64::MAX)),
    })
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

    /// Check the additive graph-query schema without trusting the migration
    /// ledger. An upgraded database may retain the historical-vector
    /// constraint as `NOT VALID`; current-head readiness is checked separately.
    pub async fn semantic_graph_query_schema_ready(&self) -> Result<bool> {
        let ready: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
                             WHERE attrelid='communities'::regclass \
                               AND attname='semantic_graph_query_enabled' \
                               AND NOT attisdropped) \
                 AND to_regclass('semantic_query_provider_admission') IS NOT NULL \
                 AND to_regclass('semantic_graph_http_fleet_attestations') IS NOT NULL \
                 AND EXISTS (SELECT 1 FROM pg_constraint \
                             WHERE conrelid=to_regclass('communities') \
                               AND conname='communities_semantic_graph_query_requires_index') \
                 AND EXISTS (SELECT 1 FROM pg_constraint \
                             WHERE conrelid=to_regclass('events') \
                               AND conname='events_kind_not_semantic_graph_query_result' \
                               AND convalidated) \
                 AND EXISTS (SELECT 1 FROM pg_constraint \
                             WHERE conrelid=to_regclass('events') \
                               AND conname='events_kind_not_project_context_coordinate_search_result' \
                               AND convalidated) \
                 AND EXISTS (SELECT 1 FROM pg_constraint \
                             WHERE conrelid=to_regclass('events') \
                               AND conname='events_kind_not_project_context_one_hop_semantic_search_result' \
                               AND convalidated) \
                 AND EXISTS (SELECT 1 FROM pg_constraint \
                             WHERE conrelid=to_regclass('semantic_embeddings') \
                               AND conname='semantic_embeddings_nonzero_cosine')",
        )
        .fetch_one(&self.pool)
        .await?;
        if !ready {
            return Ok(false);
        }
        self.semantic_schema_ready().await
    }

    /// Inspect content-free database prerequisites for the Community query
    /// gate. This does not replace Relay runtime/fleet/provider readiness.
    pub async fn semantic_graph_query_readiness(
        &self,
        community_id: CommunityId,
    ) -> Result<SemanticGraphQueryReadiness> {
        let schema_ready = self.semantic_graph_query_schema_ready().await?;
        if !schema_ready {
            return Ok(SemanticGraphQueryReadiness {
                schema_ready: false,
                index_enabled: false,
                query_enabled: false,
                project_context_enabled: false,
                active_generation_id: None,
                active_generation_ready: false,
                non_queryable_current_heads: 0,
                persisted_virtual_events: 0,
            });
        }

        let row = sqlx::query(
            "SELECT community.semantic_index_enabled, \
                    community.semantic_graph_query_enabled, \
                    community.project_context_edge_enabled, \
                    community.semantic_active_generation_id, \
                    EXISTS (SELECT 1 FROM semantic_index_generations generation \
                            WHERE generation.community_id=community.id \
                              AND generation.generation_id=\
                                  community.semantic_active_generation_id \
                              AND generation.lifecycle='active') \
                        AS active_generation_ready, \
                    (SELECT count(*) FROM semantic_source_generation_heads head \
                     JOIN semantic_index_generations generation \
                       ON generation.community_id=head.community_id \
                      AND generation.generation_id=head.generation_id \
                     WHERE head.community_id=community.id \
                       AND head.generation_id=community.semantic_active_generation_id \
                       AND NOT EXISTS ( \
                           SELECT 1 FROM semantic_unit_sets unit_set \
                           JOIN semantic_units unit \
                             ON unit.community_id=unit_set.community_id \
                            AND unit.unit_set_id=unit_set.unit_set_id \
                            AND unit.unit_kind='overview' \
                           JOIN semantic_embeddings embedding \
                             ON embedding.community_id=unit.community_id \
                            AND embedding.unit_set_id=unit.unit_set_id \
                            AND embedding.unit_key=unit.unit_key \
                            AND embedding.generation_id=generation.generation_id \
                           WHERE unit_set.community_id=head.community_id \
                             AND unit_set.unit_set_id=head.unit_set_id \
                             AND unit_set.state='active' \
                             AND unit_set.extractor_version=generation.extractor_version \
                             AND embedding.dimensions=generation.dimensions \
                             AND embedding.model_contract_digest=\
                                 generation.model_contract_digest \
                             AND embedding.response_model=generation.model \
                             AND vector_norm(embedding.embedding)>0 \
                       )) AS non_queryable_current_heads, \
                    (SELECT count(*) FROM events WHERE kind IN (40912, 40913, 40914)) \
                        AS persisted_virtual_events \
             FROM communities community WHERE community.id=$1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Community".to_string()))?;

        Ok(SemanticGraphQueryReadiness {
            schema_ready,
            index_enabled: row.try_get("semantic_index_enabled")?,
            query_enabled: row.try_get("semantic_graph_query_enabled")?,
            project_context_enabled: row.try_get("project_context_edge_enabled")?,
            active_generation_id: row.try_get("semantic_active_generation_id")?,
            active_generation_ready: row.try_get("active_generation_ready")?,
            non_queryable_current_heads: nonnegative_u64(
                row.try_get("non_queryable_current_heads")?,
                "non_queryable_current_heads",
            )?,
            persisted_virtual_events: nonnegative_u64(
                row.try_get("persisted_virtual_events")?,
                "persisted_virtual_events",
            )?,
        })
    }

    /// Invalidate active-generation heads backed by historical zero overview
    /// vectors and immediately schedule their exact source epochs for rebuild.
    ///
    /// The complete scan, head invalidation, and job coalescing commit in one
    /// transaction. The Community active-generation pointer is fenced for the
    /// duration, source rows are locked before their jobs and heads (the same
    /// order used by worker activation), and neither the canonical source basis
    /// nor its invalidation epoch is changed. Rollback-ready and other
    /// generations are never selected.
    pub async fn repair_active_semantic_query_vectors(
        &self,
        community_id: CommunityId,
    ) -> Result<SemanticQueryVectorRepairReport> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("buzz_semantic_generation:{community_id}"))
            .execute(&mut *tx)
            .await?;

        let community = sqlx::query(
            "SELECT semantic_active_generation_id FROM communities \
             WHERE id=$1 FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Community".to_string()))?;
        let active_generation_id: Uuid = community
            .try_get::<Option<Uuid>, _>("semantic_active_generation_id")?
            .ok_or_else(|| {
                DbError::InvalidData(
                    "semantic query-vector repair requires an active generation".to_string(),
                )
            })?;
        let generation_is_active: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM semantic_index_generations \
             WHERE community_id=$1 AND generation_id=$2 AND lifecycle='active' \
             FOR UPDATE)",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_one(&mut *tx)
        .await?;
        if !generation_is_active {
            return Err(DbError::InvalidData(
                "semantic active generation pointer is not query-current".to_string(),
            ));
        }

        // Preserve worker activation's source -> job -> head lock order. This
        // also makes a concurrently completing non-zero rebuild win before it
        // can be mistaken for a historical zero head.
        sqlx::query(
            "SELECT source.source_id \
             FROM semantic_sources source \
             JOIN semantic_source_generation_heads head \
               ON head.community_id=source.community_id \
              AND head.source_family=source.source_family \
              AND head.source_subtype=source.source_subtype \
              AND head.source_id=source.source_id \
             WHERE head.community_id=$1 AND head.generation_id=$2 \
             ORDER BY source.source_family,source.source_subtype,source.source_id \
             FOR UPDATE OF source",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT job.source_id \
             FROM semantic_index_jobs job \
             JOIN semantic_source_generation_heads head \
               ON head.community_id=job.community_id \
              AND head.generation_id=job.generation_id \
              AND head.source_family=job.source_family \
              AND head.source_subtype=job.source_subtype \
              AND head.source_id=job.source_id \
             WHERE job.community_id=$1 AND job.generation_id=$2 \
             ORDER BY job.source_family,job.source_subtype,job.source_id \
             FOR UPDATE OF job",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_all(&mut *tx)
        .await?;
        sqlx::query(
            "SELECT source_id FROM semantic_source_generation_heads \
             WHERE community_id=$1 AND generation_id=$2 \
             ORDER BY source_family,source_subtype,source_id FOR UPDATE",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_all(&mut *tx)
        .await?;

        let classification = sqlx::query(
            "WITH classified AS ( \
                 SELECT head.source_family,head.source_subtype,head.source_id, \
                        COALESCE(bool_or(candidate.cosine_norm=0),FALSE) AS has_zero, \
                        COALESCE(bool_or(candidate.cosine_norm>0),FALSE) AS has_nonzero, \
                        count(candidate.cosine_norm) FILTER ( \
                            WHERE candidate.cosine_norm=0 \
                        ) AS zero_embeddings \
                 FROM semantic_source_generation_heads head \
                 LEFT JOIN LATERAL ( \
                     SELECT vector_norm(embedding.embedding) AS cosine_norm \
                     FROM semantic_index_generations generation \
                     JOIN semantic_sources source \
                       ON source.community_id=head.community_id \
                      AND source.source_family=head.source_family \
                      AND source.source_subtype=head.source_subtype \
                      AND source.source_id=head.source_id \
                      AND source.invalidation_epoch=head.source_invalidation_epoch \
                      AND source.snapshot_digest=head.source_snapshot_digest \
                      AND source.eligibility='eligible' \
                     JOIN semantic_unit_sets unit_set \
                       ON unit_set.community_id=head.community_id \
                      AND unit_set.unit_set_id=head.unit_set_id \
                      AND unit_set.source_invalidation_epoch=\
                          head.source_invalidation_epoch \
                      AND unit_set.source_snapshot_digest=head.source_snapshot_digest \
                      AND unit_set.state='active' \
                      AND unit_set.extractor_version=generation.extractor_version \
                     JOIN semantic_units unit \
                       ON unit.community_id=unit_set.community_id \
                      AND unit.unit_set_id=unit_set.unit_set_id \
                      AND unit.unit_kind='overview' AND unit.unit_key='overview' \
                     JOIN semantic_embeddings embedding \
                       ON embedding.community_id=unit.community_id \
                      AND embedding.unit_set_id=unit.unit_set_id \
                      AND embedding.unit_key=unit.unit_key \
                      AND embedding.generation_id=generation.generation_id \
                      AND embedding.dimensions=generation.dimensions \
                      AND embedding.model_contract_digest=\
                          generation.model_contract_digest \
                      AND embedding.response_model=generation.model \
                     WHERE generation.community_id=head.community_id \
                       AND generation.generation_id=head.generation_id \
                       AND generation.lifecycle='active' \
                 ) candidate ON TRUE \
                 WHERE head.community_id=$1 AND head.generation_id=$2 \
                 GROUP BY head.source_family,head.source_subtype,head.source_id \
             ) \
             SELECT count(*) AS scanned, \
                    count(*) FILTER (WHERE NOT has_zero AND has_nonzero) AS queryable, \
                    count(*) FILTER (WHERE has_zero) AS zero_heads, \
                    count(*) FILTER (WHERE NOT has_zero AND NOT has_nonzero) AS other, \
                    COALESCE(sum(zero_embeddings),0)::bigint AS zero_embeddings \
             FROM classified",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_one(&mut *tx)
        .await?;
        let current_heads_scanned = nonnegative_u64(
            classification.try_get("scanned")?,
            "repair current_heads_scanned",
        )?;
        let queryable_current_heads = nonnegative_u64(
            classification.try_get("queryable")?,
            "repair queryable_current_heads",
        )?;
        let zero_vector_current_heads = nonnegative_u64(
            classification.try_get("zero_heads")?,
            "repair zero_vector_current_heads",
        )?;
        let other_nonqueryable_current_heads = nonnegative_u64(
            classification.try_get("other")?,
            "repair other_nonqueryable_current_heads",
        )?;
        let zero_vector_embeddings = nonnegative_u64(
            classification.try_get("zero_embeddings")?,
            "repair zero_vector_embeddings",
        )?;
        if current_heads_scanned
            != queryable_current_heads
                .saturating_add(zero_vector_current_heads)
                .saturating_add(other_nonqueryable_current_heads)
        {
            return Err(DbError::InvalidData(
                "semantic query-vector repair classification is not closed".to_string(),
            ));
        }

        let existing_jobs: i64 = sqlx::query_scalar(
            "SELECT count(*) \
             FROM semantic_index_jobs job \
             JOIN semantic_source_generation_heads head \
               ON head.community_id=job.community_id \
              AND head.generation_id=job.generation_id \
              AND head.source_family=job.source_family \
              AND head.source_subtype=job.source_subtype \
              AND head.source_id=job.source_id \
             JOIN semantic_index_generations generation \
               ON generation.community_id=head.community_id \
              AND generation.generation_id=head.generation_id \
              AND generation.lifecycle='active' \
             JOIN semantic_sources source \
               ON source.community_id=head.community_id \
              AND source.source_family=head.source_family \
              AND source.source_subtype=head.source_subtype \
              AND source.source_id=head.source_id \
              AND source.invalidation_epoch=head.source_invalidation_epoch \
              AND source.snapshot_digest=head.source_snapshot_digest \
              AND source.eligibility='eligible' \
             JOIN semantic_unit_sets unit_set \
               ON unit_set.community_id=head.community_id \
              AND unit_set.unit_set_id=head.unit_set_id \
              AND unit_set.source_invalidation_epoch=head.source_invalidation_epoch \
              AND unit_set.source_snapshot_digest=head.source_snapshot_digest \
              AND unit_set.state='active' \
              AND unit_set.extractor_version=generation.extractor_version \
             JOIN semantic_units unit \
               ON unit.community_id=unit_set.community_id \
              AND unit.unit_set_id=unit_set.unit_set_id \
              AND unit.unit_kind='overview' AND unit.unit_key='overview' \
             JOIN semantic_embeddings embedding \
               ON embedding.community_id=unit.community_id \
              AND embedding.unit_set_id=unit.unit_set_id \
              AND embedding.unit_key=unit.unit_key \
              AND embedding.generation_id=generation.generation_id \
              AND embedding.dimensions=generation.dimensions \
              AND embedding.model_contract_digest=generation.model_contract_digest \
              AND embedding.response_model=generation.model \
             WHERE head.community_id=$1 AND head.generation_id=$2 \
               AND vector_norm(embedding.embedding)=0",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_one(&mut *tx)
        .await?;
        let jobs_requeued = nonnegative_u64(existing_jobs, "repair existing_jobs")?;

        let repaired = sqlx::query(
            "WITH victims AS ( \
                 DELETE FROM semantic_source_generation_heads head \
                 USING semantic_index_generations generation, \
                       semantic_sources source, semantic_unit_sets unit_set, \
                       semantic_units unit, semantic_embeddings embedding \
                 WHERE head.community_id=$1 AND head.generation_id=$2 \
                   AND generation.community_id=head.community_id \
                   AND generation.generation_id=head.generation_id \
                   AND generation.lifecycle='active' \
                   AND source.community_id=head.community_id \
                   AND source.source_family=head.source_family \
                   AND source.source_subtype=head.source_subtype \
                   AND source.source_id=head.source_id \
                   AND source.invalidation_epoch=head.source_invalidation_epoch \
                   AND source.snapshot_digest=head.source_snapshot_digest \
                   AND source.eligibility='eligible' \
                   AND unit_set.community_id=head.community_id \
                   AND unit_set.unit_set_id=head.unit_set_id \
                   AND unit_set.source_invalidation_epoch=head.source_invalidation_epoch \
                   AND unit_set.source_snapshot_digest=head.source_snapshot_digest \
                   AND unit_set.state='active' \
                   AND unit_set.extractor_version=generation.extractor_version \
                   AND unit.community_id=unit_set.community_id \
                   AND unit.unit_set_id=unit_set.unit_set_id \
                   AND unit.unit_kind='overview' AND unit.unit_key='overview' \
                   AND embedding.community_id=unit.community_id \
                   AND embedding.unit_set_id=unit.unit_set_id \
                   AND embedding.unit_key=unit.unit_key \
                   AND embedding.generation_id=generation.generation_id \
                   AND embedding.dimensions=generation.dimensions \
                   AND embedding.model_contract_digest=generation.model_contract_digest \
                   AND embedding.response_model=generation.model \
                   AND vector_norm(embedding.embedding)=0 \
                 RETURNING head.community_id,head.generation_id,head.source_family, \
                           head.source_subtype,head.source_id, \
                           head.source_invalidation_epoch \
             ), scheduled AS ( \
                 INSERT INTO semantic_index_jobs ( \
                     community_id,generation_id,source_family,source_subtype,source_id, \
                     desired_invalidation_epoch,state,attempts,next_attempt_at, \
                     created_at,updated_at \
                 ) \
                 SELECT community_id,generation_id,source_family,source_subtype,source_id, \
                        source_invalidation_epoch,'pending',0,clock_timestamp(), \
                        clock_timestamp(),clock_timestamp() \
                 FROM victims \
                 ON CONFLICT ( \
                     community_id,generation_id,source_family,source_subtype,source_id \
                 ) DO UPDATE SET \
                     desired_invalidation_epoch=EXCLUDED.desired_invalidation_epoch, \
                     state='pending',attempts=0,next_attempt_at=clock_timestamp(), \
                     claim_id=NULL,lease_until=NULL,claimed_at=NULL,completed_at=NULL, \
                     error_code=NULL,error_detail=NULL,updated_at=clock_timestamp() \
                 RETURNING source_id \
             ) \
             SELECT (SELECT count(*) FROM victims) AS invalidated, \
                    (SELECT count(*) FROM scheduled) AS scheduled",
        )
        .bind(community_id.as_uuid())
        .bind(active_generation_id)
        .fetch_one(&mut *tx)
        .await?;
        let heads_invalidated =
            nonnegative_u64(repaired.try_get("invalidated")?, "repair heads_invalidated")?;
        let jobs_scheduled =
            nonnegative_u64(repaired.try_get("scheduled")?, "repair jobs_scheduled")?;
        if heads_invalidated != zero_vector_current_heads || jobs_scheduled != heads_invalidated {
            return Err(DbError::InvalidData(
                "semantic query-vector repair lost its transactional victim set".to_string(),
            ));
        }
        let jobs_created = jobs_scheduled.checked_sub(jobs_requeued).ok_or_else(|| {
            DbError::InvalidData(
                "semantic query-vector repair job accounting is invalid".to_string(),
            )
        })?;

        tx.commit().await?;
        Ok(SemanticQueryVectorRepairReport {
            community_id,
            active_generation_id,
            current_heads_scanned,
            queryable_current_heads,
            zero_vector_current_heads,
            other_nonqueryable_current_heads,
            zero_vector_embeddings,
            heads_invalidated,
            jobs_created,
            jobs_requeued,
        })
    }

    /// Deployment readiness for capability-gated semantic indexing.
    /// Pre-migration and all-disabled deployments remain ready. Enabled
    /// Communities require the derived schema and a running provider worker;
    /// any published active pointer must resolve to an active generation.
    pub async fn semantic_deployment_ready(
        &self,
        worker_ready: bool,
        graph_query_runtime_ready: bool,
    ) -> Result<bool> {
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
        if !worker_ready
            || !self.semantic_schema_ready().await?
            || !self.semantic_graph_query_schema_ready().await?
        {
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
        if invalid_pointer {
            return Ok(false);
        }
        let query_column_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_attribute \
             WHERE attrelid='communities'::regclass \
               AND attname='semantic_graph_query_enabled' AND NOT attisdropped)",
        )
        .fetch_one(&self.pool)
        .await?;
        if !query_column_exists {
            return Ok(true);
        }
        let any_query_enabled: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM communities \
             WHERE semantic_graph_query_enabled AND archived_at IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(!any_query_enabled
            || (graph_query_runtime_ready && self.semantic_graph_query_schema_ready().await?))
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
        let mut tx = self.pool.begin().await?;
        let locked: Option<Uuid> =
            sqlx::query_scalar("SELECT id FROM communities WHERE id=$1 FOR UPDATE")
                .bind(community_id.as_uuid())
                .fetch_optional(&mut *tx)
                .await?;
        if locked.is_none() {
            tx.rollback().await?;
            return Err(DbError::NotFound("semantic Community".to_string()));
        }
        if !enabled {
            // 0058 makes query imply index. Close egress first in the same
            // row-locked transaction so no committed state can violate it.
            sqlx::query("UPDATE communities SET semantic_graph_query_enabled=FALSE WHERE id=$1")
                .bind(community_id.as_uuid())
                .execute(&mut *tx)
                .await?;
        }
        let affected = sqlx::query("UPDATE communities SET semantic_index_enabled=$2 WHERE id=$1")
            .bind(community_id.as_uuid())
            .bind(enabled)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if affected != 1 {
            tx.rollback().await?;
            return Err(DbError::NotFound("semantic Community".to_string()));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Disable the Community graph-query egress gate.
    ///
    /// Enabling deliberately has no symmetric setter: callers must use the
    /// fleet-fenced atomic operation in `semantic_fleet`.
    pub async fn disable_semantic_graph_query(&self, community_id: CommunityId) -> Result<()> {
        let affected =
            sqlx::query("UPDATE communities SET semantic_graph_query_enabled=FALSE WHERE id=$1")
                .bind(community_id.as_uuid())
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

    /// Return both semantic gates and the active generation pointer.
    pub async fn semantic_community_query_state(
        &self,
        community_id: CommunityId,
    ) -> Result<(bool, bool, Option<Uuid>)> {
        let row = sqlx::query(
            "SELECT semantic_index_enabled, semantic_graph_query_enabled, \
                    semantic_active_generation_id FROM communities WHERE id=$1",
        )
        .bind(community_id.as_uuid())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Community".to_string()))?;
        Ok((
            row.try_get("semantic_index_enabled")?,
            row.try_get("semantic_graph_query_enabled")?,
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

    /// Try to reserve exactly one deadline-usable provider slot.
    ///
    /// Both workload lanes share the existing physical gate. A lane may only
    /// consume the next physical slot when its cadence permits, so it cannot
    /// jump over and erase a slot reserved for the other workload. Expired,
    /// physically idle capacity may be borrowed without creating future queue
    /// debt. `Busy` rolls back without changing either gate table.
    pub async fn try_reserve_semantic_provider_slot_until(
        &self,
        community_id: CommunityId,
        provider: &str,
        workload: SemanticProviderWorkload,
        interval: std::time::Duration,
        latest_start_at: DateTime<Utc>,
    ) -> Result<SemanticProviderReservation> {
        let mut tx = self.pool.begin().await?;
        let reservation = reserve_semantic_provider_slot_in_tx(
            &mut tx,
            community_id,
            provider,
            workload,
            interval,
            latest_start_at,
        )
        .await?;
        match reservation {
            SemanticProviderReservation::Reserved { .. } => tx.commit().await?,
            SemanticProviderReservation::Busy => tx.rollback().await?,
        }
        Ok(reservation)
    }

    /// Revalidate one background claim immediately before external provider
    /// handoff.
    ///
    /// Provider-slot reservation is capacity admission only. This separate,
    /// short writer `REPEATABLE READ` transaction is the authorization and
    /// currentness linearization point after any reservation wait. It locks in
    /// canonical-trigger order (`semantic_sources` before jobs) and fails
    /// closed if the Community, generation contract/lifecycle, exact claim
    /// lease, or exact source basis/snapshot changed.
    pub async fn confirm_semantic_worker_egress(
        &self,
        lease: &SemanticJobLease,
        observation: &CanonicalSemanticSourceObservation,
    ) -> Result<SemanticWorkerEgressConfirmation> {
        validate_semantic_worker_egress_expectation(lease, observation)?;
        let (family, subtype) = semantic_source_db_key(lease.source.kind);
        let basis = serde_json::to_value(&observation.basis)?;
        let epoch = u64_to_i64(
            lease.desired_invalidation_epoch,
            "semantic worker egress invalidation_epoch",
        )?;
        let dimensions = i32::try_from(lease.model_contract.dimensions).map_err(|_| {
            DbError::InvalidData(
                "semantic worker egress dimensions exceed PostgreSQL int".to_string(),
            )
        })?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;

        let current: Result<bool> = async {
            let community_enabled = sqlx::query_scalar::<_, bool>(
                "SELECT semantic_index_enabled FROM communities WHERE id=$1 FOR SHARE",
            )
            .bind(lease.source.community_id)
            .fetch_optional(&mut *tx)
            .await?;
            if community_enabled != Some(true) {
                return Ok(false);
            }

            let generation_current: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM semantic_index_generations generation \
                 WHERE generation.community_id=$1 AND generation.generation_id=$2 \
                   AND generation.lifecycle IN ('building','ready','active','rollback_ready') \
                   AND generation.extractor_version=$3 \
                   AND generation.input_contract_version=$4 \
                   AND generation.provider=$5 AND generation.model=$6 \
                   AND generation.dimensions=$7 AND generation.distance_metric=$8 \
                   AND generation.normalization=$9 AND generation.provider_boundary=$10 \
                   AND generation.model_contract_digest=$11 \
                 FOR SHARE OF generation)",
            )
            .bind(lease.source.community_id)
            .bind(lease.generation_id)
            .bind(&lease.extractor_version)
            .bind(&lease.model_contract.input_contract_version)
            .bind(&lease.model_contract.provider)
            .bind(&lease.model_contract.model)
            .bind(dimensions)
            .bind(distance_metric_db(lease.model_contract.distance_metric))
            .bind(normalization_db(lease.model_contract.normalization))
            .bind(provider_boundary_db(
                &lease.model_contract.provider_boundary,
            ))
            .bind(lease.model_contract_digest.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await?;
            if !generation_current {
                return Ok(false);
            }

            // Canonical-source triggers take this row before coalescing the
            // job. Keeping the same order avoids a source/job lock inversion.
            let source_current: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM semantic_sources source \
                 WHERE source.community_id=$1 AND source.source_family=$2 \
                   AND source.source_subtype=$3 AND source.source_id=$4 \
                   AND source.eligibility='eligible' AND source.invalidation_epoch=$5 \
                   AND source.source_basis=$6 AND source.snapshot_digest=$7 \
                 FOR SHARE OF source)",
            )
            .bind(lease.source.community_id)
            .bind(family)
            .bind(subtype)
            .bind(lease.source.source_id)
            .bind(epoch)
            .bind(basis)
            .bind(observation.snapshot_digest.as_bytes().as_slice())
            .fetch_one(&mut *tx)
            .await?;
            if !source_current {
                return Ok(false);
            }

            let claim_current: bool = sqlx::query_scalar(
                "SELECT EXISTS (SELECT 1 FROM semantic_index_jobs job \
                 WHERE job.community_id=$1 AND job.generation_id=$2 \
                   AND job.source_family=$3 AND job.source_subtype=$4 AND job.source_id=$5 \
                   AND job.desired_invalidation_epoch=$6 AND job.state='claimed' \
                   AND job.claim_id=$7 AND job.lease_until=$8 \
                   AND job.lease_until >= clock_timestamp() \
                 FOR SHARE OF job)",
            )
            .bind(lease.source.community_id)
            .bind(lease.generation_id)
            .bind(family)
            .bind(subtype)
            .bind(lease.source.source_id)
            .bind(epoch)
            .bind(lease.claim_id)
            .bind(lease.lease_until)
            .fetch_one(&mut *tx)
            .await?;
            Ok(claim_current)
        }
        .await;

        match current {
            Ok(true) => {
                tx.commit().await?;
                Ok(SemanticWorkerEgressConfirmation::Permitted(
                    SemanticWorkerEgressPermit { _private: () },
                ))
            }
            Ok(false) => {
                tx.rollback().await?;
                Ok(SemanticWorkerEgressConfirmation::Unavailable)
            }
            Err(error) if semantic_worker_egress_serialization_failure(&error) => {
                // A writer that committed after this REPEATABLE READ snapshot
                // cannot leave us holding an old row version: PostgreSQL
                // raises 40001 at the conflicting row lock. Treat that normal
                // writer-first race as the same closed no-egress outcome.
                tx.rollback().await?;
                Ok(SemanticWorkerEgressConfirmation::Unavailable)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
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
             JOIN semantic_index_generations generation \
               ON generation.community_id=embedding.community_id \
              AND generation.generation_id=embedding.generation_id \
             JOIN semantic_units unit \
               ON unit.community_id=embedding.community_id \
              AND unit.unit_set_id=embedding.unit_set_id \
              AND unit.unit_key=embedding.unit_key \
             WHERE embedding.community_id=$1 AND embedding.generation_id=$2 \
               AND embedding.model_contract_digest=$3 \
               AND embedding.dimensions=generation.dimensions \
               AND embedding.model_contract_digest=generation.model_contract_digest \
               AND embedding.response_model=generation.model \
               AND vector_norm(embedding.embedding)>0 \
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
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8) \
                 ON CONFLICT (community_id,unit_set_id,unit_key,generation_id) \
                 DO UPDATE SET response_model=EXCLUDED.response_model, \
                               embedding=EXCLUDED.embedding, \
                               indexed_at=clock_timestamp() \
                 WHERE vector_norm(semantic_embeddings.embedding)=0",
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

    /// Release a provider-admission-blocked claim without consuming a failure attempt.
    ///
    /// Claiming increments `attempts` before the worker reaches the shared provider
    /// gate. A healthy lane-cadence conflict is capacity backpressure rather than an
    /// encoder/provider failure, so this transition reverses exactly that increment
    /// and can never poison the job. The exact claim/source epoch fence prevents an
    /// obsolete worker from changing a newer durable job state.
    pub async fn defer_semantic_claim_for_provider_admission(
        &self,
        lease: &SemanticJobLease,
        retry_after: std::time::Duration,
    ) -> Result<bool> {
        if retry_after < std::time::Duration::from_millis(100)
            || retry_after > std::time::Duration::from_secs(60)
        {
            return Err(DbError::InvalidData(
                "semantic provider admission retry must be between 100ms and 60s".to_string(),
            ));
        }
        let (family, subtype) = semantic_source_db_key(lease.source.kind);
        let affected = sqlx::query(
            "UPDATE semantic_index_jobs SET state='retry',attempts=GREATEST(attempts-1,0), \
                    claim_id=NULL,lease_until=NULL,claimed_at=NULL,completed_at=NULL, \
                    next_attempt_at=clock_timestamp()+make_interval(secs=>$8), \
                    error_code='provider_busy',error_detail=NULL,updated_at=clock_timestamp() \
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
        .bind(retry_after.as_secs_f64())
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
        let mut connection = self.pool.acquire().await?;
        observe_semantic_source_in_connection(&mut connection, identity).await
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
}

/// Reconstruct one canonical source through its source-owned tables and typed
/// parser using the caller's existing database snapshot.
///
/// Semantic query code uses this adapter to avoid reading projections, caches,
/// or a second transaction while hydrating current exact hits.
pub(crate) async fn observe_semantic_source_in_connection(
    connection: &mut PgConnection,
    identity: &SemanticSourceIdentity,
) -> Result<CanonicalSemanticSourceObservation> {
    identity
        .validate()
        .map_err(|error| semantic_contract_error("source_identity", error))?;
    match identity.kind {
        SemanticSourceKind::ProjectView(subtype) => {
            observe_project_view_source_in_connection(connection, identity, subtype).await
        }
        SemanticSourceKind::ProjectDocument => {
            observe_project_document_source_in_connection(connection, identity).await
        }
        SemanticSourceKind::Meeting => {
            observe_meeting_source_in_connection(connection, identity).await
        }
    }
}

/// Source-owned fields needed by a signed one-hop candidate preview.
///
/// The semantic observation remains the existing Foundation currentness
/// contract. `description` is read separately from the same canonical writer
/// snapshot and never enters the extractor or embedding text.
pub(crate) struct SemanticCanonicalPreviewObservation {
    /// Existing typed Foundation observation.
    pub observation: CanonicalSemanticSourceObservation,
    /// Canonical description when this source family owns one.
    pub description: Option<String>,
}

/// Reconstruct one current semantic observation plus its source-owned optional
/// description through the caller's existing database snapshot.
pub(crate) async fn observe_semantic_source_preview_in_connection(
    connection: &mut PgConnection,
    identity: &SemanticSourceIdentity,
) -> Result<SemanticCanonicalPreviewObservation> {
    let observation = observe_semantic_source_in_connection(connection, identity).await?;
    let description = match identity.kind {
        SemanticSourceKind::ProjectView(expected_subtype) => {
            let (object_type, body): (String, serde_json::Value) = sqlx::query_as(
                "SELECT object_type, body FROM project_view_objects \
                 WHERE community_id=$1 AND object_id=$2 AND deleted_at IS NULL",
            )
            .bind(identity.community_id)
            .bind(identity.source_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| DbError::NotFound("semantic Project View preview".to_string()))?;
            let data = project_view_preview_data(expected_subtype, &object_type, body)?;
            project_view_description(&data).map(str::to_owned)
        }
        SemanticSourceKind::ProjectDocument => None,
        SemanticSourceKind::Meeting => sqlx::query_scalar::<_, Option<String>>(
            "SELECT description FROM channels \
             WHERE community_id=$1 AND id=$2 AND room_kind='meeting' AND deleted_at IS NULL",
        )
        .bind(identity.community_id)
        .bind(identity.source_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| DbError::NotFound("semantic Meeting preview".to_string()))?
        .filter(|value| !value.trim().is_empty()),
    };
    Ok(SemanticCanonicalPreviewObservation {
        observation,
        description,
    })
}

fn project_view_preview_data(
    expected_subtype: ProjectViewSemanticType,
    object_type: &str,
    mut body: serde_json::Value,
) -> Result<ProjectViewObjectDataV3> {
    if project_view_semantic_type(object_type)? != expected_subtype {
        return Err(DbError::InvalidData(
            "semantic Project View preview subtype changed".to_string(),
        ));
    }
    let object = body.as_object_mut().ok_or_else(|| {
        DbError::InvalidData("semantic Project View preview body must be an object".to_string())
    })?;
    let context_references = object.remove("context_references").ok_or_else(|| {
        DbError::InvalidData(
            "semantic Project View preview body has no context_references".to_string(),
        )
    })?;
    let _: Vec<ProjectContextReference> =
        serde_json::from_value(context_references).map_err(|error| {
            DbError::InvalidData(format!(
                "invalid semantic Project View preview context_references: {error}"
            ))
        })?;
    if object_type == "role" {
        object.remove("level");
    }
    serde_json::from_value(serde_json::json!({
        "object_type": object_type,
        "data": body,
    }))
    .map_err(|error| {
        DbError::InvalidData(format!(
            "invalid semantic Project View preview body: {error}"
        ))
    })
}

fn project_view_description(data: &buzz_project_view::v3::ProjectViewObjectDataV3) -> Option<&str> {
    use buzz_project_view::v3::ProjectViewObjectDataV3;

    match data {
        ProjectViewObjectDataV3::Plan(value) => Some(&value.description),
        ProjectViewObjectDataV3::Stage(value) => Some(&value.description),
        ProjectViewObjectDataV3::Requirement(value) => Some(&value.description),
        ProjectViewObjectDataV3::Issue(value) => Some(&value.description),
        ProjectViewObjectDataV3::Work(value) => Some(&value.description),
        ProjectViewObjectDataV3::ProjectProfile(_)
        | ProjectViewObjectDataV3::Goal(_)
        | ProjectViewObjectDataV3::Role(_)
        | ProjectViewObjectDataV3::Resource(_) => None,
    }
}

async fn observe_project_view_source_in_connection(
    connection: &mut PgConnection,
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
    .fetch_optional(&mut *connection)
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
    let source_change_id = digest_from_bytes(row.try_get("source_change_id")?, "source_change_id")?;
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

async fn observe_project_document_source_in_connection(
    connection: &mut PgConnection,
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
    .fetch_optional(&mut *connection)
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

async fn observe_meeting_source_in_connection(
    connection: &mut PgConnection,
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
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| DbError::NotFound("semantic Meeting source".to_string()))?;
    let create_event_id = digest_from_bytes(row.try_get("create_event_id")?, "create_event_id")?;
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

pub(crate) fn semantic_generation_from_row(
    row: &sqlx::postgres::PgRow,
) -> Result<SemanticGenerationRecord> {
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

fn validate_semantic_worker_egress_expectation(
    lease: &SemanticJobLease,
    observation: &CanonicalSemanticSourceObservation,
) -> Result<()> {
    if observation.identity != lease.source
        || !matches!(observation.eligibility, SemanticEligibility::Eligible)
    {
        return Err(DbError::InvalidData(
            "semantic worker egress source expectation is invalid".to_string(),
        ));
    }
    lease
        .model_contract
        .validate()
        .map_err(|error| semantic_contract_error("worker_egress_model_contract", error))?;
    if lease
        .model_contract
        .digest()
        .map_err(|error| semantic_contract_error("worker_egress_model_contract", error))?
        != lease.model_contract_digest
    {
        return Err(DbError::InvalidData(
            "semantic worker egress model contract digest mismatch".to_string(),
        ));
    }
    let verified = CanonicalSemanticSourceObservation::new(
        observation.identity.clone(),
        observation.basis.clone(),
        observation.eligibility,
        observation.filter.clone(),
        observation.title.clone(),
        observation.summary.clone(),
    )
    .map_err(|error| semantic_contract_error("worker_egress_source_observation", error))?;
    if verified.snapshot_digest != observation.snapshot_digest {
        return Err(DbError::InvalidData(
            "semantic worker egress source snapshot digest mismatch".to_string(),
        ));
    }
    Ok(())
}

fn semantic_worker_egress_serialization_failure(error: &DbError) -> bool {
    matches!(
        error,
        DbError::Sqlx(sqlx::Error::Database(database_error))
            if database_error.code().as_deref() == Some("40001")
    )
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

const fn semantic_provider_workload_db(workload: SemanticProviderWorkload) -> &'static str {
    match workload {
        SemanticProviderWorkload::InteractiveQuery => "interactive_query",
        SemanticProviderWorkload::BackgroundIndex => "background_index",
    }
}

pub(crate) fn semantic_source_kind_from_db(
    family: &str,
    subtype: &str,
) -> Result<SemanticSourceKind> {
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
    use buzz_project_view::v3::ProjectViewObjectDataV3;
    use buzz_semantic::{
        extract_overview, CanonicalSemanticSourceObservation, DeterministicFakeEncoder, Digest32,
        EncodedSemanticUnit, IneligibilityReason, ProjectDocumentSourceBasis,
        ProjectViewSemanticType, SemanticEligibility, SemanticEncoder, SemanticEncoderInput,
        SemanticFilterMetadata, SemanticLifecycleClass, SemanticSourceBasis,
        SemanticSourceIdentity, SemanticSourceKind, OVERVIEW_EXTRACTOR_VERSION,
    };
    use uuid::Uuid;

    use super::{
        project_view_preview_data, vector_version_supported, CreateSemanticGeneration,
        SemanticActivationOutcome, SemanticClaimObservationOutcome, SemanticJobLease,
        SemanticPgvectorPreflight, SemanticProviderReservation, SemanticProviderWorkload,
        SemanticRebuildScope, SemanticRebuildState, SemanticScanFamily,
        SemanticWorkerEgressConfirmation,
    };
    use crate::{Db, DbConfig};

    fn assert_concurrent_worker_egress_rejected(
        result: crate::Result<SemanticWorkerEgressConfirmation>,
    ) {
        match result {
            Ok(SemanticWorkerEgressConfirmation::Unavailable) => {}
            Ok(SemanticWorkerEgressConfirmation::Permitted(_)) => {
                panic!("concurrent committed mutation must not receive an egress permit")
            }
            Err(error) => panic!("unexpected final egress fence error: {error}"),
        }
    }

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
    fn project_view_preview_reconstructs_the_canonical_v3_body_shape() {
        let data = project_view_preview_data(
            ProjectViewSemanticType::Work,
            "work",
            serde_json::json!({
                "title": "Client interaction work",
                "description": "Preserve the source-owned description.",
                "status": "in_progress",
                "priority": "normal",
                "summary": "A retrieval summary.",
                "context_references": [],
            }),
        )
        .expect("valid persisted v3 Work body");
        let ProjectViewObjectDataV3::Work(work) = data else {
            panic!("preview body must reconstruct the declared Work variant");
        };
        assert_eq!(work.description, "Preserve the source-owned description.");

        let role = project_view_preview_data(
            ProjectViewSemanticType::Role,
            "role",
            serde_json::json!({
                "name": "Client role",
                "purpose": "Own client behavior.",
                "responsibilities": ["Interaction state"],
                "boundaries": ["No server authorization"],
                "active": true,
                "level": "member",
                "context_references": [],
            }),
        )
        .expect("valid persisted v3 Role body");
        assert!(matches!(role, ProjectViewObjectDataV3::Role(_)));
    }

    #[test]
    fn project_view_preview_rejects_missing_context_references_and_subtype_drift() {
        let missing_references = project_view_preview_data(
            ProjectViewSemanticType::Work,
            "work",
            serde_json::json!({
                "title": "Work",
                "description": "Description",
                "status": "pending",
                "priority": "normal",
            }),
        );
        assert!(missing_references.is_err());

        let subtype_drift = project_view_preview_data(
            ProjectViewSemanticType::Role,
            "work",
            serde_json::json!({"context_references": []}),
        );
        assert!(subtype_drift.is_err());
    }

    fn semantic_test_lease(
        lease: &SemanticJobLease,
        claim_id: Uuid,
        desired_invalidation_epoch: u64,
    ) -> SemanticJobLease {
        SemanticJobLease {
            source: lease.source.clone(),
            generation_id: lease.generation_id,
            desired_invalidation_epoch,
            claim_id,
            lease_until: lease.lease_until,
            attempts: lease.attempts,
            extractor_version: lease.extractor_version.clone(),
            model_contract: lease.model_contract.clone(),
            model_contract_digest: lease.model_contract_digest,
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
        sqlx::query("UPDATE communities SET semantic_graph_query_enabled=TRUE WHERE id=$1")
            .bind(community_id.as_uuid())
            .execute(&db.pool)
            .await
            .expect("seed query gate for atomic Foundation disable");
        db.set_semantic_community_enabled(community_id, false)
            .await
            .expect("Foundation disable closes query in the same transaction");
        assert_eq!(
            db.semantic_community_query_state(community_id)
                .await
                .expect("read both semantic gates"),
            (false, false, None)
        );
        db.set_semantic_community_enabled(community_id, true)
            .await
            .expect("re-enable Foundation for pipeline test");
        let deadline = chrono::Utc::now() + chrono::Duration::seconds(1);
        let first_slot = db
            .try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake",
                SemanticProviderWorkload::BackgroundIndex,
                std::time::Duration::from_millis(100),
                deadline,
            )
            .await
            .expect("first distributed provider slot");
        let physical_before_busy: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT next_request_at FROM semantic_provider_rate_gates \
             WHERE community_id=$1 AND provider='deterministic_fake'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("physical gate before busy reservation");
        assert_eq!(
            db.try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake",
                SemanticProviderWorkload::BackgroundIndex,
                std::time::Duration::from_millis(100),
                deadline,
            )
            .await
            .expect("busy background slot does not fail"),
            SemanticProviderReservation::Busy
        );
        let physical_after_busy: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT next_request_at FROM semantic_provider_rate_gates \
             WHERE community_id=$1 AND provider='deterministic_fake'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("physical gate after busy reservation");
        assert_eq!(physical_before_busy, physical_after_busy);
        let second_slot = db
            .try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake",
                SemanticProviderWorkload::InteractiveQuery,
                std::time::Duration::from_millis(100),
                deadline,
            )
            .await
            .expect("second distributed provider slot");
        let SemanticProviderReservation::Reserved { wait: first_wait } = first_slot else {
            panic!("first background slot must be reserved");
        };
        let SemanticProviderReservation::Reserved { wait: second_wait } = second_slot else {
            panic!("second interactive slot must be reserved");
        };
        let physical_after_second: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT next_request_at FROM semantic_provider_rate_gates \
             WHERE community_id=$1 AND provider='deterministic_fake'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("physical gate after alternate workload reservation");
        assert_eq!(
            physical_after_second - physical_before_busy,
            chrono::Duration::milliseconds(100)
        );
        assert!(second_wait >= first_wait);

        let first_pod = db.clone();
        let second_pod = db.clone();
        let concurrent_deadline = chrono::Utc::now() + chrono::Duration::seconds(1);
        let (background, interactive) = tokio::join!(
            first_pod.try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake_concurrent",
                SemanticProviderWorkload::BackgroundIndex,
                std::time::Duration::from_millis(100),
                concurrent_deadline,
            ),
            second_pod.try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake_concurrent",
                SemanticProviderWorkload::InteractiveQuery,
                std::time::Duration::from_millis(100),
                concurrent_deadline,
            )
        );
        let concurrent_waits = [background, interactive]
            .into_iter()
            .map(
                |outcome| match outcome.expect("concurrent distributed reservation") {
                    SemanticProviderReservation::Reserved { wait } => wait,
                    SemanticProviderReservation::Busy => {
                        panic!("both workload lanes must make bounded first-round progress")
                    }
                },
            )
            .collect::<Vec<_>>();
        assert_eq!(concurrent_waits.len(), 2);
        let concurrent_lanes: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM semantic_query_provider_admission \
             WHERE community_id=$1 AND provider='deterministic_fake_concurrent'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("both distributed workload lanes persisted");
        assert_eq!(concurrent_lanes, 2);

        let deadline_gate: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "INSERT INTO semantic_provider_rate_gates (\
                 community_id,provider,next_request_at) \
             VALUES ($1,'deterministic_fake_deadline',clock_timestamp()+interval '1 second') \
             RETURNING next_request_at",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("seed a provider gate beyond the interactive deadline");
        assert_eq!(
            db.try_reserve_semantic_provider_slot_until(
                community_id,
                "deterministic_fake_deadline",
                SemanticProviderWorkload::InteractiveQuery,
                std::time::Duration::from_millis(100),
                chrono::Utc::now() + chrono::Duration::milliseconds(50),
            )
            .await
            .expect("deadline busy reservation"),
            SemanticProviderReservation::Busy
        );
        let deadline_after: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
            "SELECT next_request_at FROM semantic_provider_rate_gates \
             WHERE community_id=$1 AND provider='deterministic_fake_deadline'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("deadline busy leaves physical gate intact");
        let deadline_lane_rows: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM semantic_query_provider_admission \
             WHERE community_id=$1 AND provider='deterministic_fake_deadline'",
        )
        .bind(community_id.as_uuid())
        .fetch_one(&db.pool)
        .await
        .expect("deadline busy creates no admission state");
        assert_eq!(deadline_after, deadline_gate);
        assert_eq!(deadline_lane_rows, 0);
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
        let mut lease = db
            .claim_due_semantic_job(60)
            .await
            .expect("claim query")
            .expect("claimed job");
        assert_eq!(lease.attempts, 1);
        let stale_claim =
            semantic_test_lease(&lease, Uuid::new_v4(), lease.desired_invalidation_epoch);
        assert!(!db
            .defer_semantic_claim_for_provider_admission(
                &stale_claim,
                std::time::Duration::from_millis(100),
            )
            .await
            .expect("stale claim id does not defer current claim"));
        let stale_epoch =
            semantic_test_lease(&lease, lease.claim_id, lease.desired_invalidation_epoch + 1);
        assert!(!db
            .defer_semantic_claim_for_provider_admission(
                &stale_epoch,
                std::time::Duration::from_millis(100),
            )
            .await
            .expect("stale source epoch does not defer current claim"));
        for iteration in 0..10 {
            assert_eq!(lease.attempts, 1, "lossless admission claim {iteration}");
            assert!(db
                .defer_semantic_claim_for_provider_admission(
                    &lease,
                    std::time::Duration::from_millis(100),
                )
                .await
                .expect("defer provider admission"));
            let (deferred_state, deferred_attempts): (String, i32) = sqlx::query_as(
                "SELECT state,attempts FROM semantic_index_jobs \
                 WHERE community_id=$1 AND generation_id=$2 \
                   AND source_family='project_document' AND source_subtype='document' \
                   AND source_id=$3",
            )
            .bind(community_id.as_uuid())
            .bind(generation_id)
            .bind(observation.identity.source_id)
            .fetch_one(&db.pool)
            .await
            .expect("deferred provider admission state");
            assert_eq!(deferred_state, "retry");
            assert_eq!(deferred_attempts, 0);
            tokio::time::sleep(std::time::Duration::from_millis(110)).await;
            lease = db
                .claim_due_semantic_job(60)
                .await
                .expect("reclaim deferred provider admission")
                .expect("reclaimed deferred provider admission");
        }
        assert_eq!(lease.attempts, 1);
        assert_eq!(lease.source, observation.identity);
        assert_eq!(
            db.prepare_semantic_claim_observation(&lease, &observation)
                .await
                .expect("prepare"),
            SemanticClaimObservationOutcome::Ready
        );
        assert!(matches!(
            db.confirm_semantic_worker_egress(&lease, &observation)
                .await
                .expect("current worker egress fence"),
            SemanticWorkerEgressConfirmation::Permitted(_)
        ));
        {
            let mut disable_tx = db.pool.begin().await.expect("begin concurrent disable");
            sqlx::query(
                "UPDATE communities SET semantic_graph_query_enabled=FALSE, \
                        semantic_index_enabled=FALSE WHERE id=$1",
            )
            .bind(community_id.as_uuid())
            .execute(&mut *disable_tx)
            .await
            .expect("hold Community disable row lock");
            let blocked_confirmation = db.confirm_semantic_worker_egress(&lease, &observation);
            tokio::pin!(blocked_confirmation);
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    blocked_confirmation.as_mut(),
                )
                .await
                .is_err(),
                "final fence must wait behind the in-flight Community writer",
            );
            disable_tx
                .commit()
                .await
                .expect("commit concurrent Foundation disable");
            let disabled_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                blocked_confirmation.as_mut(),
            )
            .await
            .expect("disabled final fence must complete");
            assert_concurrent_worker_egress_rejected(disabled_result);
        }
        db.set_semantic_community_enabled(community_id, true)
            .await
            .expect("re-enable Foundation after egress fence test");
        assert!(matches!(
            db.confirm_semantic_worker_egress(&lease, &observation)
                .await
                .expect("re-enabled worker egress fence"),
            SemanticWorkerEgressConfirmation::Permitted(_)
        ));

        let changed_observation = CanonicalSemanticSourceObservation::new(
            observation.identity.clone(),
            observation.basis.clone(),
            observation.eligibility,
            observation.filter.clone(),
            observation.title.clone(),
            Some("A summary changed during the reserved provider wait".to_string()),
        )
        .expect("changed observation");
        {
            let mut source_writer_tx = db.pool.begin().await.expect("begin source mutation");
            sqlx::query("SELECT semantic_mark_source_changed($1,$2,$3,$4,$5,$6,$7,$8)")
                .bind(community_id.as_uuid())
                .bind("project_document")
                .bind("document")
                .bind(observation.identity.source_id)
                .bind(true)
                .bind("active")
                .bind("active")
                .bind(Option::<&str>::None)
                .execute(&mut *source_writer_tx)
                .await
                .expect("hold canonical-trigger source lock");
            let blocked_confirmation = db.confirm_semantic_worker_egress(&lease, &observation);
            tokio::pin!(blocked_confirmation);
            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    blocked_confirmation.as_mut(),
                )
                .await
                .is_err(),
                "final fence must wait behind the in-flight canonical source trigger",
            );
            source_writer_tx
                .commit()
                .await
                .expect("commit canonical source trigger");
            let stale_result = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                blocked_confirmation.as_mut(),
            )
            .await
            .expect("stale final fence must complete");
            assert_concurrent_worker_egress_rejected(stale_result);
        }
        db.reconcile_semantic_observation(&changed_observation)
            .await
            .expect("canonical summary invalidation");
        assert!(matches!(
            db.confirm_semantic_worker_egress(&lease, &observation)
                .await
                .expect("stale worker egress fence"),
            SemanticWorkerEgressConfirmation::Unavailable
        ));

        // Restore the original fixture as a new epoch and prove only its new
        // exact claim can cross the provider boundary.
        db.reconcile_semantic_observation(&observation)
            .await
            .expect("restore source after stale egress test");
        let lease = db
            .claim_due_semantic_job(60)
            .await
            .expect("replacement claim query")
            .expect("replacement claimed job");
        assert_eq!(
            db.prepare_semantic_claim_observation(&lease, &observation)
                .await
                .expect("prepare replacement claim"),
            SemanticClaimObservationOutcome::Ready
        );
        assert!(matches!(
            db.confirm_semantic_worker_egress(&lease, &observation)
                .await
                .expect("replacement worker egress fence"),
            SemanticWorkerEgressConfirmation::Permitted(_)
        ));
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
