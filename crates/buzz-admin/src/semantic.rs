//! Operator controls for the derived Project Context semantic index.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::Subcommand;
use serde_json::json;
use uuid::Uuid;

use buzz_db::semantic::{
    CreateSemanticGeneration, SemanticRebuildScope, SemanticRebuildState, SemanticScanFamily,
};
use buzz_db::semantic_fleet::WriteSemanticGraphHttpFleetAttestation;
use buzz_semantic::{SemanticEncoder, SemanticModelContract, OVERVIEW_EXTRACTOR_VERSION};
use buzz_semantic_query::{
    semantic_graph_http_runtime_digest, SemanticGraphHttpFleetInventory,
    MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES,
};

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
    /// Inspect database/runtime prerequisites for the graph-query gate.
    QueryReadiness,
    /// Invalidate historical zero-vector heads in the active generation and
    /// idempotently schedule their exact current source epochs for rebuild.
    RepairQueryVectors,
    /// Enable graph-query egress for the configured Community.
    QueryEnable {
        /// Explicitly acknowledge that problem and overview text may leave Carryforth.
        #[arg(long, default_value_t = false)]
        acknowledge_problem_egress: bool,
    },
    /// Disable graph-query egress without stopping the indexing worker.
    QueryDisable,
    /// Record a short-lived assertion of the complete HTTP routing inventory.
    FleetAttest {
        /// Closed JSON inventory enumerated from the deployment control plane.
        #[arg(long)]
        inventory: PathBuf,
        /// Unique audit identity; generated when omitted.
        #[arg(long)]
        attestation_id: Option<Uuid>,
        /// Short assertion lifetime in seconds.
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u16).range(30..=900))]
        expires_in_seconds: u16,
        /// Explicitly acknowledge this is the exact current load-balancer inventory.
        #[arg(long, default_value_t = false)]
        acknowledge_current_routing_inventory: bool,
        /// Content-free operator identity retained with the assertion.
        #[arg(long, default_value = "buzz-admin")]
        attested_by: String,
    },
    /// Revoke the current HTTP fleet assertion immediately.
    FleetRevoke {
        /// Content-free operator identity retained with the revocation.
        #[arg(long, default_value = "buzz-admin")]
        revoked_by: String,
    },
    /// Inspect the current HTTP fleet assertion against this binary/deployment.
    FleetCheck,
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
        SemanticCommand::QueryReadiness => query_readiness().await,
        SemanticCommand::RepairQueryVectors => repair_query_vectors().await,
        SemanticCommand::QueryEnable {
            acknowledge_problem_egress,
        } => query_enable(acknowledge_problem_egress).await,
        SemanticCommand::QueryDisable => query_disable().await,
        SemanticCommand::FleetAttest {
            inventory,
            attestation_id,
            expires_in_seconds,
            acknowledge_current_routing_inventory,
            attested_by,
        } => {
            fleet_attest(
                inventory,
                attestation_id,
                expires_in_seconds,
                acknowledge_current_routing_inventory,
                &attested_by,
            )
            .await
        }
        SemanticCommand::FleetRevoke { revoked_by } => fleet_revoke(&revoked_by).await,
        SemanticCommand::FleetCheck => fleet_check().await,
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
    let (enabled, query_enabled, active_generation_id) =
        db.semantic_community_query_state(community).await?;
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
            "query_enabled": query_enabled,
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

async fn query_readiness() -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let report = db
        .semantic_graph_query_readiness(tenant.community())
        .await?;
    let deployment_master = query_http_deployment_master()?;
    let deployment_id = query_http_deployment_id().ok();
    let instance_id = query_http_instance_id();
    let fleet = match deployment_id.as_deref() {
        Some(deployment_id) => Some(
            db.semantic_graph_http_fleet_readiness(
                tenant.community(),
                deployment_id,
                instance_id.as_deref(),
            )
            .await?,
        ),
        None => None,
    };
    let fleet_ready = fleet.as_ref().is_some_and(|readiness| readiness.ready());
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "schema_ready": report.schema_ready,
            "index_enabled": report.index_enabled,
            "query_enabled": report.query_enabled,
            "project_context_enabled": report.project_context_enabled,
            "active_generation_id": report.active_generation_id,
            "active_generation_ready": report.active_generation_ready,
            "non_queryable_current_heads": report.non_queryable_current_heads,
            "persisted_virtual_events": report.persisted_virtual_events,
            "database_ready": report.database_ready(),
            "http_deployment_master": deployment_master,
            "http_deployment_id": deployment_id,
            "compiled_http_runtime_digest": semantic_graph_http_runtime_digest()?,
            "fleet_attestation_ready": fleet_ready,
            "fleet_attestation_failure": fleet.as_ref()
                .and_then(|readiness| readiness.failure)
                .map(|failure| failure.code()),
            "fleet_attestation_id": fleet.as_ref()
                .and_then(|readiness| readiness.attestation.as_ref())
                .map(|attestation| attestation.attestation_id),
            "fleet_attestation_expires_at": fleet.as_ref()
                .and_then(|readiness| readiness.attestation.as_ref())
                .map(|attestation| attestation.expires_at),
            "base_enable_ready": report.database_ready() && deployment_master && fleet_ready,
        }))?
    );
    Ok(0)
}

async fn repair_query_vectors() -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let report = db
        .repair_active_semantic_query_vectors(tenant.community())
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": report.community_id.as_uuid(),
            "active_generation_id": report.active_generation_id,
            "current_heads_scanned": report.current_heads_scanned,
            "queryable_current_heads": report.queryable_current_heads,
            "zero_vector_current_heads": report.zero_vector_current_heads,
            "other_nonqueryable_current_heads": report.other_nonqueryable_current_heads,
            "zero_vector_embeddings": report.zero_vector_embeddings,
            "heads_invalidated": report.heads_invalidated,
            "jobs_scheduled": report.jobs_created + report.jobs_requeued,
            "jobs_created": report.jobs_created,
            "jobs_requeued": report.jobs_requeued,
            "canonical_source_epochs_advanced": 0,
            "other_generations_changed": 0,
        }))?
    );
    Ok(0)
}

async fn query_enable(acknowledge_problem_egress: bool) -> Result<i32> {
    if !acknowledge_problem_egress {
        anyhow::bail!("semantic query-enable requires --acknowledge-problem-egress");
    }
    if !query_http_deployment_master()? {
        anyhow::bail!(
            "semantic query-enable requires BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true"
        );
    }
    let (db, tenant) = tenant_db().await?;
    let deployment_id = query_http_deployment_id()?;
    let fleet = db
        .semantic_graph_http_fleet_readiness(tenant.community(), &deployment_id, None)
        .await?;
    if let Some(failure) = fleet.failure {
        anyhow::bail!(
            "semantic query-enable requires a current homogeneous HTTP fleet attestation: {}",
            failure.code()
        );
    }
    let report = db
        .semantic_graph_query_readiness(tenant.community())
        .await?;
    if !report.database_ready() {
        anyhow::bail!(
            "semantic query-enable database prerequisites are not ready; run semantic query-readiness"
        );
    }
    db.enable_semantic_graph_query_with_http_fleet(tenant.community(), &deployment_id)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "query_enabled": true,
            "problem_egress_acknowledged": true,
            "fleet_attestation_id": fleet.attestation.map(|value| value.attestation_id),
        }))?
    );
    Ok(0)
}

async fn query_disable() -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    db.disable_semantic_graph_query(tenant.community()).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "query_enabled": false,
            "semantic_index_continues": true,
        }))?
    );
    Ok(0)
}

async fn fleet_attest(
    inventory_path: PathBuf,
    attestation_id: Option<Uuid>,
    expires_in_seconds: u16,
    acknowledge_current_routing_inventory: bool,
    attested_by: &str,
) -> Result<i32> {
    if !acknowledge_current_routing_inventory {
        anyhow::bail!("semantic fleet-attest requires --acknowledge-current-routing-inventory");
    }
    let bytes = std::fs::read(&inventory_path)
        .with_context(|| format!("read fleet inventory {}", inventory_path.display()))?;
    if bytes.len() > MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES {
        anyhow::bail!(
            "semantic fleet inventory exceeds {} bytes",
            MAX_SEMANTIC_GRAPH_FLEET_INVENTORY_BYTES
        );
    }
    let inventory = SemanticGraphHttpFleetInventory::parse_json(&bytes)?;
    inventory.validate_for_compiled_runtime()?;
    let deployment_id = query_http_deployment_id()?;
    if inventory.deployment_id != deployment_id {
        anyhow::bail!(
            "fleet inventory deployment_id does not match BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID"
        );
    }
    let (db, tenant) = tenant_db().await?;
    let record = db
        .write_semantic_graph_http_fleet_attestation(WriteSemanticGraphHttpFleetAttestation {
            community_id: tenant.community(),
            attestation_id: attestation_id.unwrap_or_else(Uuid::new_v4),
            inventory: &inventory,
            ttl: Duration::from_secs(u64::from(expires_in_seconds)),
            attested_by,
        })
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "transport": "http",
            "attestation_id": record.attestation_id,
            "deployment_id": record.deployment_id,
            "runtime_digest": record.runtime_digest,
            "inventory_digest": record.inventory_digest,
            "instance_count": record.inventory.instances.len(),
            "routing_inventory_acknowledged": true,
            "attested_at": record.attested_at,
            "expires_at": record.expires_at,
        }))?
    );
    Ok(0)
}

async fn fleet_revoke(revoked_by: &str) -> Result<i32> {
    let (db, tenant) = tenant_db().await?;
    let revoked = db
        .revoke_semantic_graph_http_fleet_attestation(tenant.community(), revoked_by)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "transport": "http",
            "revoked": revoked,
        }))?
    );
    Ok(0)
}

async fn fleet_check() -> Result<i32> {
    let deployment_id = query_http_deployment_id()?;
    let instance_id = query_http_instance_id();
    let (db, tenant) = tenant_db().await?;
    let readiness = db
        .semantic_graph_http_fleet_readiness(
            tenant.community(),
            &deployment_id,
            instance_id.as_deref(),
        )
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "transport": "http",
            "deployment_id": deployment_id,
            "instance_id": instance_id,
            "compiled_runtime_digest": semantic_graph_http_runtime_digest()?,
            "ready": readiness.ready(),
            "failure": readiness.failure.map(|failure| failure.code()),
            "attestation_id": readiness.attestation.as_ref()
                .map(|attestation| attestation.attestation_id),
            "inventory_digest": readiness.attestation.as_ref()
                .map(|attestation| attestation.inventory_digest),
            "instance_count": readiness.attestation.as_ref()
                .map(|attestation| attestation.inventory.instances.len()),
            "expires_at": readiness.attestation.as_ref()
                .map(|attestation| attestation.expires_at),
        }))?
    );
    Ok(if readiness.ready() { 0 } else { 1 })
}

fn query_http_deployment_master() -> Result<bool> {
    match std::env::var("BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE") {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => Ok(true),
            "false" | "0" | "off" | "" => Ok(false),
            _ => anyhow::bail!("BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE must be true or false"),
        },
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn query_http_deployment_id() -> Result<String> {
    required_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID")
}

fn query_http_instance_id() -> Option<String> {
    std::env::var("BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn required_query_identity(name: &'static str) -> Result<String> {
    let value = std::env::var(name)
        .with_context(|| format!("{name} is required for semantic HTTP fleet operations"))?;
    let value = value.trim();
    if value.is_empty()
        || value.len() > 128
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'/' | b'-')
        })
    {
        anyhow::bail!("{name} must be a 1..=128 byte deployment identity");
    }
    Ok(value.to_owned())
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

#[cfg(test)]
mod tests {
    use super::SemanticCommand;

    #[test]
    fn fleet_attestation_commands_have_closed_cli_shapes() {
        let repair = <crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "repair-query-vectors",
        ])
        .expect("repair-query-vectors CLI");
        assert!(matches!(
            repair.command,
            crate::Command::Semantic {
                command: SemanticCommand::RepairQueryVectors
            }
        ));

        let parsed = <crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "fleet-attest",
            "--inventory",
            "/tmp/fleet.json",
            "--expires-in-seconds",
            "300",
            "--acknowledge-current-routing-inventory",
        ])
        .expect("fleet-attest CLI");
        assert!(matches!(
            parsed.command,
            crate::Command::Semantic {
                command: SemanticCommand::FleetAttest {
                    expires_in_seconds: 300,
                    acknowledge_current_routing_inventory: true,
                    ..
                }
            }
        ));

        assert!(<crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "fleet-attest",
            "--inventory",
            "/tmp/fleet.json",
            "--expires-in-seconds",
            "901",
        ])
        .is_err());
        assert!(<crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "fleet-check"
        ])
        .is_ok());
        assert!(<crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "fleet-revoke",
            "--revoked-by",
            "operator-a"
        ])
        .is_ok());
    }
}
