//! Operator controls for the derived Project Context semantic index.

use anyhow::Result;
use clap::Subcommand;
use serde_json::json;
use uuid::Uuid;

use buzz_db::semantic::{
    CreateSemanticGeneration, SemanticRebuildScope, SemanticRebuildState, SemanticScanFamily,
};
use buzz_semantic::{SemanticEncoder, SemanticModelContract, OVERVIEW_EXTRACTOR_VERSION};

use crate::{connect_db, resolve_admin_tenant};

/// Semantic-index operator commands.
#[derive(Debug, Subcommand)]
pub enum SemanticCommand {
    /// Verify PostgreSQL 17 and pgvector 0.8.5+ without changing the database.
    Preflight,
    /// Show the current Community gate, generations, and exact coverage.
    Status,
    /// Create a frozen model generation.
    GenerationCreate {
        /// Stable generation UUID; generated when omitted.
        #[arg(long)]
        generation_id: Option<Uuid>,
        /// Use an offline deterministic encoder contract of this dimension.
        #[arg(long, value_parser = clap::value_parser!(usize), conflicts_with = "volcengine")]
        fake_dimensions: Option<usize>,
        /// Use the approved 2048-dimensional Volcengine overview contract.
        #[arg(long, default_value_t = false)]
        volcengine: bool,
    },
    /// Enable worker/provider eligibility for the configured Community.
    Enable,
    /// Disable worker/provider/query eligibility while currentness capture continues.
    Disable,
    /// Scan canonical sources and idempotently coalesce current jobs.
    Rebuild {
        /// Building generation whose full-source rebuild fence is updated.
        #[arg(long)]
        generation_id: Uuid,
        /// Resume a prior durable operation; generated when omitted.
        #[arg(long)]
        operation_id: Option<Uuid>,
        /// Canonical source family, or all families when omitted.
        #[arg(long, value_parser = ["project-view", "document", "meeting"])]
        family: Option<String>,
        /// Canonical keyset page size.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=500))]
        page_size: u16,
    },
    /// Cancel a running durable rebuild cursor.
    RebuildCancel {
        /// Rebuild operation UUID.
        #[arg(long)]
        operation_id: Uuid,
    },
    /// Verify a generation's complete-current coverage.
    Verify {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Mark a fully covered building generation ready.
    GenerationReady {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Atomically activate a ready or rollback-ready generation.
    Activate {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Requeue poison jobs for one generation.
    RetryFailed {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Retire a ready or rollback-ready generation.
    Retire {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Purge one retired/failed derived generation.
    Purge {
        /// Generation UUID.
        #[arg(long)]
        generation_id: Uuid,
    },
    /// Delete unreferenced staging/retired unit sets past retention.
    Gc {
        /// Minimum age in hours.
        #[arg(long, default_value_t = 168, value_parser = clap::value_parser!(u32).range(1..))]
        older_than_hours: u32,
        /// Maximum sets to remove.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=1000))]
        limit: u16,
    },
}

/// Execute a semantic-index operator command.
pub async fn run(command: SemanticCommand) -> Result<i32> {
    match command {
        SemanticCommand::Preflight => preflight().await,
        SemanticCommand::Status => status().await,
        SemanticCommand::GenerationCreate {
            generation_id,
            fake_dimensions,
            volcengine,
        } => generation_create(generation_id, fake_dimensions, volcengine).await,
        SemanticCommand::Enable => set_enabled(true).await,
        SemanticCommand::Disable => set_enabled(false).await,
        SemanticCommand::Rebuild {
            generation_id,
            operation_id,
            family,
            page_size,
        } => rebuild(generation_id, operation_id, family, page_size).await,
        SemanticCommand::RebuildCancel { operation_id } => rebuild_cancel(operation_id).await,
        SemanticCommand::Verify { generation_id } => verify(generation_id).await,
        SemanticCommand::GenerationReady { generation_id } => ready(generation_id).await,
        SemanticCommand::Activate { generation_id } => activate(generation_id).await,
        SemanticCommand::RetryFailed { generation_id } => retry_failed(generation_id).await,
        SemanticCommand::Retire { generation_id } => retire(generation_id).await,
        SemanticCommand::Purge { generation_id } => purge(generation_id).await,
        SemanticCommand::Gc {
            older_than_hours,
            limit,
        } => gc(older_than_hours, limit).await,
    }
}

async fn tenant_db() -> Result<(buzz_db::Db, buzz_core::TenantContext)> {
    let db = connect_db().await?;
    let tenant = resolve_admin_tenant(&db).await?;
    Ok((db, tenant))
}

async fn status() -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let community = tenant.community();
    let (enabled, active_generation_id) = db.semantic_community_state(community).await?;
    let mut generations = Vec::new();
    for generation in db.list_semantic_generations(community).await? {
        let coverage = db
            .semantic_generation_coverage(community, generation.generation_id)
            .await?;
        generations.push(json!({
            "generation_id": generation.generation_id,
            "lifecycle": generation.lifecycle,
            "extractor_version": generation.extractor_version,
            "model_contract": generation.model_contract,
            "model_contract_digest": generation.model_contract_digest,
            "rebuild_completed_at": generation.rebuild_completed_at,
            "created_at": generation.created_at,
            "coverage": coverage_json(&coverage),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": community.as_uuid(),
            "schema_ready": db.semantic_schema_ready().await?,
            "enabled": enabled,
            "active_generation_id": active_generation_id,
            "generations": generations,
        }))?
    );
    Ok(0)
}

async fn generation_create(
    generation_id: Option<Uuid>,
    fake_dimensions: Option<usize>,
    volcengine: bool,
) -> Result<i32> {
    if !volcengine && fake_dimensions.is_none() {
        anyhow::bail!("choose --volcengine or --fake-dimensions");
    }
    let (db, tenant) = tenant_db().await?;
    let contract = if volcengine {
        SemanticModelContract::volcengine_overview_v1()
    } else {
        buzz_semantic::DeterministicFakeEncoder::new(fake_dimensions.unwrap_or(32))?
            .contract()
            .clone()
    };
    let generation = db
        .create_semantic_generation(CreateSemanticGeneration {
            community_id: tenant.community(),
            generation_id: generation_id.unwrap_or_else(Uuid::new_v4),
            extractor_version: OVERVIEW_EXTRACTOR_VERSION,
            model_contract: &contract,
            created_by: "buzz-admin",
        })
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": generation.community_id.as_uuid(),
            "generation_id": generation.generation_id,
            "lifecycle": generation.lifecycle,
            "model_contract": generation.model_contract,
            "model_contract_digest": generation.model_contract_digest,
        }))?
    );
    Ok(0)
}

async fn set_enabled(enabled: bool) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    if enabled {
        if !db.semantic_schema_ready().await? || !db.semantic_pgvector_preflight().await?.ready() {
            anyhow::bail!(
                "semantic enable requires a ready PostgreSQL/pgvector schema; run semantic preflight"
            );
        }
        if db
            .list_semantic_generations(tenant.community())
            .await?
            .is_empty()
        {
            anyhow::bail!("semantic enable requires at least one frozen generation");
        }
    }
    db.set_semantic_community_enabled(tenant.community(), enabled)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "enabled": enabled,
            "currentness_capture_continues": true,
        }))?
    );
    Ok(0)
}

async fn rebuild(
    generation_id: Uuid,
    operation_id: Option<Uuid>,
    family: Option<String>,
    page_size: u16,
) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let scope = match family.as_deref() {
        Some("project-view") => SemanticRebuildScope::Family(SemanticScanFamily::ProjectView),
        Some("document") => SemanticRebuildScope::Family(SemanticScanFamily::ProjectDocument),
        Some("meeting") => SemanticRebuildScope::Family(SemanticScanFamily::Meeting),
        None => SemanticRebuildScope::All,
        Some(_) => unreachable!("clap validates the family"),
    };
    let operation_id = operation_id.unwrap_or_else(Uuid::new_v4);
    let mut operation = db
        .start_semantic_rebuild(tenant.community(), generation_id, operation_id, scope)
        .await?;
    if operation.state == SemanticRebuildState::Cancelled {
        anyhow::bail!("semantic rebuild operation is cancelled");
    }
    let mut observed = 0_u64;
    let mut eligible = 0_u64;
    while operation.state == SemanticRebuildState::Running {
        let page = db
            .scan_current_semantic_sources(
                tenant.community(),
                operation.current_family,
                operation.cursor.as_ref(),
                page_size,
            )
            .await?;
        eprintln!(
            "semantic rebuild operation_id={} generation_id={}",
            operation.operation_id, operation.generation_id
        );
        let page_len = page.observations.len();
        for observation in &page.observations {
            db.reconcile_semantic_observation(observation).await?;
            observed += 1;
            eligible += u64::from(matches!(
                observation.eligibility,
                buzz_semantic::SemanticEligibility::Eligible
            ));
        }
        let family_complete = page_len < usize::from(page_size);
        operation = db
            .checkpoint_semantic_rebuild(&operation, page.next_cursor.as_ref(), family_complete)
            .await?;
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "generation_id": generation_id,
            "operation_id": operation.operation_id,
            "scope": match operation.scope {
                SemanticRebuildScope::All => "all",
                SemanticRebuildScope::Family(SemanticScanFamily::ProjectView) => "project_view",
                SemanticRebuildScope::Family(SemanticScanFamily::ProjectDocument) => "project_document",
                SemanticRebuildScope::Family(SemanticScanFamily::Meeting) => "meeting",
            },
            "state": "completed",
            "observed": observed,
            "eligible": eligible,
            "jobs_coalesced": true,
        }))?
    );
    Ok(0)
}

async fn rebuild_cancel(operation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let operation = db
        .cancel_semantic_rebuild(tenant.community(), operation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "operation_id": operation.operation_id,
            "state": "cancelled",
        }))?
    );
    Ok(0)
}

async fn verify(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let coverage = db
        .semantic_generation_coverage(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "generation_id": generation_id,
            "complete": coverage.complete(),
            "coverage": coverage_json(&coverage),
        }))?
    );
    Ok(if coverage.complete() { 0 } else { 5 })
}

async fn ready(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let coverage = db
        .mark_semantic_generation_ready(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&coverage_json(&coverage))?
    );
    Ok(0)
}

async fn activate(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let coverage = db
        .activate_semantic_generation(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&coverage_json(&coverage))?
    );
    Ok(0)
}

async fn retry_failed(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let jobs = db
        .retry_poison_semantic_jobs(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"requeued": jobs}))?
    );
    Ok(0)
}

async fn retire(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    db.retire_semantic_generation(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"retired": generation_id}))?
    );
    Ok(0)
}

async fn purge(generation_id: Uuid) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let embeddings = db
        .purge_semantic_generation(tenant.community(), generation_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "purged_generation": generation_id,
            "purged_embeddings": embeddings,
        }))?
    );
    Ok(0)
}

async fn gc(older_than_hours: u32, limit: u16) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(i64::from(older_than_hours));
    let deleted = db
        .gc_semantic_derived_sets(tenant.community(), cutoff, limit)
        .await?;
    let deleted_jobs = db
        .gc_semantic_jobs(tenant.community(), cutoff, limit)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "deleted_sets": deleted,
            "deleted_jobs": deleted_jobs,
        }))?
    );
    Ok(0)
}

fn coverage_json(coverage: &buzz_db::semantic::SemanticGenerationCoverage) -> serde_json::Value {
    json!({
        "rebuild_complete": coverage.rebuild_complete,
        "eligible_sources": coverage.eligible_sources,
        "current_heads": coverage.current_heads,
        "queued_jobs": coverage.queued_jobs,
        "claimed_jobs": coverage.claimed_jobs,
        "poison_jobs": coverage.poison_jobs,
    })
}

async fn preflight() -> Result<i32> {
    let db = connect_db().await?;
    let report = db.semantic_pgvector_preflight().await?;
    let ready = report.ready();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "ready": ready,
            "postgres": {
                "version_num": report.server_version_num,
                "version": report.server_version,
                "supported": (170_000..180_000).contains(&report.server_version_num),
            },
            "pgvector": {
                "available_version": report.available_vector_version,
                "installed_version": report.installed_vector_version,
                "vector_type_available": report.vector_type_available,
                "halfvec_type_available": report.halfvec_type_available,
                "vector_roundtrip_ok": report.vector_roundtrip_ok,
                "cosine_distance_ok": report.cosine_distance_ok,
                "halfvec_cast_ok": report.halfvec_cast_ok,
                "sqlx_2048_roundtrip_ok": report.sqlx_2048_roundtrip_ok,
            },
            "failure_codes": report.failure_codes(),
        }))?
    );
    Ok(if ready { 0 } else { 5 })
}
