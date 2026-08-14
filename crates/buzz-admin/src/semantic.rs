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
    SemanticGraphQueryEnableRequirement, SemanticGraphQueryFleetPolicy,
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
    QueryReadiness {
        /// Observe the live Relay through its loopback-only `/_status` endpoint.
        #[arg(long)]
        relay_status_url: Option<url::Url>,
    },
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
        SemanticCommand::QueryReadiness { relay_status_url } => {
            query_readiness(relay_status_url.as_ref()).await
        }
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

#[derive(Debug)]
struct QueryHttpRuntimeObservation {
    source: &'static str,
    endpoint: Option<String>,
    live_relay_observed: bool,
    deployment_master: bool,
    semantic_graph_deployment_master: bool,
    coordinate_search_deployment_master: bool,
    one_hop_semantic_search_deployment_master: bool,
    fleet_policy: SemanticGraphQueryFleetPolicy,
    deployment_id: Option<String>,
    instance_id: Option<String>,
    runtime_digest: String,
    parser_ready: Option<bool>,
    handler_ready: Option<bool>,
    relay_reported_fleet_attestation_status: Option<String>,
}

const MAX_RELAY_STATUS_BYTES: usize = 64 * 1024;

fn query_http_runtime_from_environment() -> Result<QueryHttpRuntimeObservation> {
    let semantic_graph_deployment_master = semantic_graph_http_deployment_master()?;
    let coordinate_search_deployment_master = coordinate_search_http_deployment_master()?;
    let one_hop_semantic_search_deployment_master =
        one_hop_semantic_search_http_deployment_master()?;
    Ok(QueryHttpRuntimeObservation {
        source: "admin_process_environment",
        endpoint: None,
        live_relay_observed: false,
        deployment_master: semantic_graph_deployment_master
            || coordinate_search_deployment_master
            || one_hop_semantic_search_deployment_master,
        semantic_graph_deployment_master,
        coordinate_search_deployment_master,
        one_hop_semantic_search_deployment_master,
        fleet_policy: query_http_fleet_policy()?,
        deployment_id: optional_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID")?,
        instance_id: optional_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID")?,
        runtime_digest: semantic_graph_http_runtime_digest()?.to_hex(),
        parser_ready: None,
        handler_ready: None,
        relay_reported_fleet_attestation_status: None,
    })
}

async fn observe_live_query_http_runtime(
    relay_status_url: &url::Url,
) -> Result<QueryHttpRuntimeObservation> {
    validate_relay_status_url(relay_status_url)?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(2))
        .build()
        .context("build isolated Relay status client")?;
    let mut response = client
        .get(relay_status_url.clone())
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .with_context(|| format!("read live Relay status from {relay_status_url}"))?;
    if !response.status().is_success() {
        anyhow::bail!(
            "live Relay status endpoint returned non-success HTTP {}",
            response.status()
        );
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RELAY_STATUS_BYTES as u64)
    {
        anyhow::bail!("live Relay status response exceeds {MAX_RELAY_STATUS_BYTES} bytes");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .context("read live Relay status response body")?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RELAY_STATUS_BYTES {
            anyhow::bail!("live Relay status response exceeds {MAX_RELAY_STATUS_BYTES} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&body).context("decode live Relay status JSON")?;
    parse_live_query_http_runtime(relay_status_url, &value)
}

fn validate_relay_status_url(relay_status_url: &url::Url) -> Result<()> {
    if !matches!(relay_status_url.scheme(), "http" | "https") {
        anyhow::bail!("--relay-status-url must use http or https");
    }
    if !relay_status_url.username().is_empty() || relay_status_url.password().is_some() {
        anyhow::bail!("--relay-status-url must not contain credentials");
    }
    if relay_status_url.query().is_some() || relay_status_url.fragment().is_some() {
        anyhow::bail!("--relay-status-url must not contain a query or fragment");
    }
    if relay_status_url.path() != "/_status" {
        anyhow::bail!("--relay-status-url path must be exactly /_status");
    }
    let loopback = match relay_status_url.host() {
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    };
    if !loopback {
        anyhow::bail!("--relay-status-url host must be a literal loopback IP address");
    }
    Ok(())
}

fn parse_live_query_http_runtime(
    relay_status_url: &url::Url,
    status: &serde_json::Value,
) -> Result<QueryHttpRuntimeObservation> {
    let root = status
        .as_object()
        .context("live Relay status must be a JSON object")?;
    if root.get("service").and_then(serde_json::Value::as_str) != Some("buzz-relay") {
        anyhow::bail!("live Relay status has an unexpected service identity");
    }
    let semantic_graph_runtime = root
        .get("semantic_graph_query_http")
        .and_then(serde_json::Value::as_object)
        .context("live Relay status is missing semantic_graph_query_http")?;
    let coordinate_search_runtime = root
        .get("project_context_coordinate_search_http")
        .and_then(serde_json::Value::as_object);
    let one_hop_semantic_search_runtime = root
        .get("project_context_one_hop_semantic_search_http")
        .and_then(serde_json::Value::as_object);
    let semantic_graph_deployment_master =
        required_status_bool(semantic_graph_runtime, "deployment_master")?;
    let coordinate_search_deployment_master = coordinate_search_runtime
        .map(|runtime| required_status_bool(runtime, "deployment_master"))
        .transpose()?
        .unwrap_or(false);
    let one_hop_semantic_search_deployment_master = one_hop_semantic_search_runtime
        .map(|runtime| required_status_bool(runtime, "deployment_master"))
        .transpose()?
        .unwrap_or(false);
    let deployment_master = semantic_graph_deployment_master
        || coordinate_search_deployment_master
        || one_hop_semantic_search_deployment_master;
    let semantic_graph_parser_ready = required_status_bool(semantic_graph_runtime, "parser_ready")?;
    let semantic_graph_handler_ready =
        required_status_bool(semantic_graph_runtime, "handler_ready")?;
    let coordinate_search_parser_ready = coordinate_search_runtime
        .map(|runtime| required_status_bool(runtime, "parser_ready"))
        .transpose()?
        .unwrap_or(false);
    let coordinate_search_handler_ready = coordinate_search_runtime
        .map(|runtime| required_status_bool(runtime, "handler_ready"))
        .transpose()?
        .unwrap_or(false);
    let one_hop_semantic_search_parser_ready = one_hop_semantic_search_runtime
        .map(|runtime| required_status_bool(runtime, "parser_ready"))
        .transpose()?
        .unwrap_or(false);
    let one_hop_semantic_search_handler_ready = one_hop_semantic_search_runtime
        .map(|runtime| required_status_bool(runtime, "handler_ready"))
        .transpose()?
        .unwrap_or(false);
    let parser_ready = (!semantic_graph_deployment_master || semantic_graph_parser_ready)
        && (!coordinate_search_deployment_master || coordinate_search_parser_ready)
        && (!one_hop_semantic_search_deployment_master || one_hop_semantic_search_parser_ready);
    let handler_ready = (!semantic_graph_deployment_master || semantic_graph_handler_ready)
        && (!coordinate_search_deployment_master || coordinate_search_handler_ready)
        && (!one_hop_semantic_search_deployment_master || one_hop_semantic_search_handler_ready);
    let fleet_policy: SemanticGraphQueryFleetPolicy =
        required_status_string(semantic_graph_runtime, "fleet_policy")?
            .parse()
            .context("live Relay status has an invalid fleet_policy")?;
    let fleet_attestation_required =
        required_status_bool(semantic_graph_runtime, "fleet_attestation_required")?;
    if fleet_attestation_required != (fleet_policy == SemanticGraphQueryFleetPolicy::AttestedFleet)
    {
        anyhow::bail!("live Relay status fleet_attestation_required conflicts with fleet_policy");
    }
    let relay_reported_fleet_attestation_status =
        required_status_string(semantic_graph_runtime, "fleet_attestation_status")?.to_owned();
    let expected_reported_status = match fleet_policy {
        SemanticGraphQueryFleetPolicy::TrustedSingleRelay => "not_required",
        SemanticGraphQueryFleetPolicy::AttestedFleet => "community_scoped_not_evaluated",
    };
    if relay_reported_fleet_attestation_status != expected_reported_status {
        anyhow::bail!("live Relay status has an inconsistent fleet_attestation_status");
    }
    let deployment_id = optional_status_identity(semantic_graph_runtime, "deployment_id")?;
    let instance_id = optional_status_identity(semantic_graph_runtime, "instance_id")?;
    if fleet_policy == SemanticGraphQueryFleetPolicy::AttestedFleet
        && deployment_master
        && (deployment_id.is_none() || instance_id.is_none())
    {
        anyhow::bail!("live attested-fleet Relay status is missing its deployment identity");
    }
    let runtime_digest =
        required_status_string(semantic_graph_runtime, "runtime_digest")?.to_owned();
    if let Some(coordinate_runtime) = coordinate_search_runtime {
        for field in [
            "fleet_policy",
            "fleet_attestation_required",
            "fleet_attestation_status",
            "deployment_id",
            "instance_id",
            "runtime_digest",
        ] {
            if coordinate_runtime.get(field) != semantic_graph_runtime.get(field) {
                anyhow::bail!("live Relay semantic HTTP surfaces disagree on shared field {field}");
            }
        }
    }
    if let Some(one_hop_runtime) = one_hop_semantic_search_runtime {
        for field in [
            "fleet_policy",
            "fleet_attestation_required",
            "fleet_attestation_status",
            "deployment_id",
            "instance_id",
            "runtime_digest",
        ] {
            if one_hop_runtime.get(field) != semantic_graph_runtime.get(field) {
                anyhow::bail!("live Relay semantic HTTP surfaces disagree on shared field {field}");
            }
        }
    }
    let compiled_runtime_digest = semantic_graph_http_runtime_digest()?.to_hex();
    if runtime_digest != compiled_runtime_digest {
        anyhow::bail!(
            "live Relay semantic HTTP runtime digest does not match this buzz-admin binary"
        );
    }
    Ok(QueryHttpRuntimeObservation {
        source: "live_relay_status",
        endpoint: Some(relay_status_url.as_str().to_owned()),
        live_relay_observed: true,
        deployment_master,
        semantic_graph_deployment_master,
        coordinate_search_deployment_master,
        one_hop_semantic_search_deployment_master,
        fleet_policy,
        deployment_id,
        instance_id,
        runtime_digest,
        parser_ready: Some(parser_ready),
        handler_ready: Some(handler_ready),
        relay_reported_fleet_attestation_status: Some(relay_reported_fleet_attestation_status),
    })
}

fn required_status_bool(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<bool> {
    object
        .get(field)
        .and_then(serde_json::Value::as_bool)
        .with_context(|| format!("live Relay status field {field} must be a boolean"))
}

fn required_status_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a str> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .with_context(|| format!("live Relay status field {field} must be a string"))
}

fn optional_status_identity(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<String>> {
    match object.get(field) {
        Some(serde_json::Value::Null) | None => Ok(None),
        Some(serde_json::Value::String(value)) => validate_query_identity(field, value).map(Some),
        Some(_) => anyhow::bail!("live Relay status field {field} must be a string or null"),
    }
}

async fn query_readiness(relay_status_url: Option<&url::Url>) -> Result<i32> {
    let runtime = match relay_status_url {
        Some(url) => observe_live_query_http_runtime(url).await?,
        None => {
            eprintln!(
                "warning: semantic HTTP runtime fields come from the buzz-admin process environment, not a live Relay; pass --relay-status-url http://127.0.0.1:8080/_status to observe one"
            );
            query_http_runtime_from_environment()?
        }
    };
    let (db, tenant) = tenant_db().await?;
    let report = db
        .semantic_graph_query_readiness(tenant.community())
        .await?;
    let fleet = match (runtime.fleet_policy, runtime.deployment_id.as_deref()) {
        (SemanticGraphQueryFleetPolicy::AttestedFleet, Some(deployment_id)) => Some(
            db.semantic_graph_http_fleet_readiness(
                tenant.community(),
                deployment_id,
                runtime.instance_id.as_deref(),
            )
            .await?,
        ),
        _ => None,
    };
    let fleet_ready = fleet.as_ref().is_some_and(|readiness| readiness.ready());
    let fleet_required = runtime.fleet_policy == SemanticGraphQueryFleetPolicy::AttestedFleet;
    let fleet_status = if !fleet_required {
        "not_required"
    } else if fleet_ready {
        "ready"
    } else {
        fleet
            .as_ref()
            .and_then(|readiness| readiness.failure)
            .map_or("deployment_identity_missing", |failure| failure.code())
    };
    let routing_ready = !fleet_required || fleet_ready;
    let http_runtime_ready = runtime.live_relay_observed.then_some(
        runtime.deployment_master
            && runtime.parser_ready == Some(true)
            && runtime.handler_ready == Some(true),
    );
    let admin_process_configuration_ready = (!runtime.live_relay_observed)
        .then_some(report.database_ready() && runtime.deployment_master && routing_ready);
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
            "http_runtime_source": runtime.source,
            "http_runtime_endpoint": runtime.endpoint.as_deref(),
            "live_relay_observed": runtime.live_relay_observed,
            "http_deployment_master": runtime.deployment_master,
            "semantic_graph_http_deployment_master": runtime
                .semantic_graph_deployment_master,
            "coordinate_search_http_deployment_master": runtime
                .coordinate_search_deployment_master,
            "one_hop_semantic_search_http_deployment_master": runtime
                .one_hop_semantic_search_deployment_master,
            "http_deployment_id": runtime.deployment_id.as_deref(),
            "http_instance_id": runtime.instance_id.as_deref(),
            "http_runtime_digest": &runtime.runtime_digest,
            "compiled_http_runtime_digest": semantic_graph_http_runtime_digest()?,
            "http_parser_ready": runtime.parser_ready,
            "http_handler_ready": runtime.handler_ready,
            "http_runtime_ready": http_runtime_ready,
            "relay_reported_fleet_attestation_status": runtime
                .relay_reported_fleet_attestation_status
                .as_deref(),
            "fleet_policy": runtime.fleet_policy,
            "fleet_attestation_required": fleet_required,
            "fleet_attestation_status": fleet_status,
            "fleet_attestation_ready": if fleet_required { Some(fleet_ready) } else { None },
            "fleet_attestation_failure": fleet.as_ref()
                .and_then(|readiness| readiness.failure)
                .map(|failure| failure.code()),
            "fleet_attestation_id": fleet.as_ref()
                .and_then(|readiness| readiness.attestation.as_ref())
                .map(|attestation| attestation.attestation_id),
            "fleet_attestation_expires_at": fleet.as_ref()
                .and_then(|readiness| readiness.attestation.as_ref())
                .map(|attestation| attestation.expires_at),
            "database_and_policy_ready": report.database_ready() && routing_ready,
            "admin_process_configuration_ready": admin_process_configuration_ready,
            "community_binding_verified": false,
            "base_enable_ready_scope": "unbound_diagnostic_components",
            "base_enable_ready": serde_json::Value::Null,
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
            "semantic query-enable requires at least one HTTP surface master: \
             BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE=true or \
             CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE=true or \
             CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE=true"
        );
    }
    let fleet_policy = query_http_fleet_policy()?;
    let (db, tenant) = tenant_db().await?;
    let report = db
        .semantic_graph_query_readiness(tenant.community())
        .await?;
    if !report.database_ready() {
        anyhow::bail!(
            "semantic query-enable database prerequisites are not ready; run semantic query-readiness"
        );
    }
    let (requirement, fleet_attestation_id) = match fleet_policy {
        SemanticGraphQueryFleetPolicy::TrustedSingleRelay => (
            SemanticGraphQueryEnableRequirement::TrustedSingleRelay,
            None::<Uuid>,
        ),
        SemanticGraphQueryFleetPolicy::AttestedFleet => {
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
            let attestation_id = fleet.attestation.as_ref().map(|value| value.attestation_id);
            // The database method repeats the Fleet check while holding the
            // Community and assertion locks. This read is diagnostic only.
            db.enable_semantic_graph_query(
                tenant.community(),
                SemanticGraphQueryEnableRequirement::AttestedFleet {
                    deployment_id: &deployment_id,
                },
            )
            .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "community_id": tenant.community().as_uuid(),
                    "query_enabled": true,
                    "problem_egress_acknowledged": true,
                    "fleet_policy": fleet_policy,
                    "fleet_attestation_required": true,
                    "fleet_attestation_status": "ready",
                    "fleet_attestation_id": attestation_id,
                }))?
            );
            return Ok(0);
        }
    };
    db.enable_semantic_graph_query(tenant.community(), requirement)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "community_id": tenant.community().as_uuid(),
            "query_enabled": true,
            "problem_egress_acknowledged": true,
            "fleet_policy": fleet_policy,
            "fleet_attestation_required": false,
            "fleet_attestation_status": "not_required",
            "fleet_attestation_id": fleet_attestation_id,
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
    require_attested_fleet_policy("fleet-attest")?;
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
    require_attested_fleet_policy("fleet-revoke")?;
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
    let fleet_policy = query_http_fleet_policy()?;
    if fleet_policy == SemanticGraphQueryFleetPolicy::TrustedSingleRelay {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "transport": "http",
                "fleet_policy": fleet_policy,
                "applicable": false,
                "fleet_attestation_required": false,
                "status": "not_required",
            }))?
        );
        return Ok(0);
    }
    let deployment_id = query_http_deployment_id()?;
    let instance_id = query_http_instance_id()?;
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
            "fleet_policy": fleet_policy,
            "applicable": true,
            "fleet_attestation_required": true,
            "status": if readiness.ready() { "ready" } else { "not_ready" },
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

fn require_attested_fleet_policy(command: &str) -> Result<()> {
    if query_http_fleet_policy()? == SemanticGraphQueryFleetPolicy::TrustedSingleRelay {
        anyhow::bail!(
            "semantic {command} is not applicable when BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY=trusted-single-relay; use semantic query-disable as the egress kill switch"
        );
    }
    Ok(())
}

fn query_http_fleet_policy() -> Result<SemanticGraphQueryFleetPolicy> {
    match std::env::var("BUZZ_SEMANTIC_GRAPH_QUERY_FLEET_POLICY") {
        Ok(value) => value.trim().parse().map_err(Into::into),
        Err(std::env::VarError::NotPresent) => Ok(SemanticGraphQueryFleetPolicy::default()),
        Err(error) => Err(error.into()),
    }
}

fn bool_environment_setting(name: &'static str) -> Result<bool> {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "on" => Ok(true),
            "false" | "0" | "off" | "" => Ok(false),
            _ => anyhow::bail!("{name} must be true or false"),
        },
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn semantic_graph_http_deployment_master() -> Result<bool> {
    bool_environment_setting("BUZZ_SEMANTIC_GRAPH_QUERY_HTTP_AVAILABLE")
}

fn coordinate_search_http_deployment_master() -> Result<bool> {
    bool_environment_setting("CARRYFORTH_PROJECT_CONTEXT_COORDINATE_SEARCH_HTTP_AVAILABLE")
}

fn one_hop_semantic_search_http_deployment_master() -> Result<bool> {
    bool_environment_setting("CARRYFORTH_PROJECT_CONTEXT_ONE_HOP_SEMANTIC_SEARCH_HTTP_AVAILABLE")
}

fn query_http_deployment_master() -> Result<bool> {
    Ok(semantic_graph_http_deployment_master()?
        || coordinate_search_http_deployment_master()?
        || one_hop_semantic_search_http_deployment_master()?)
}

fn query_http_deployment_id() -> Result<String> {
    required_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_DEPLOYMENT_ID")
}

fn query_http_instance_id() -> Result<Option<String>> {
    optional_query_identity("BUZZ_SEMANTIC_GRAPH_QUERY_INSTANCE_ID")
}

fn required_query_identity(name: &'static str) -> Result<String> {
    let value = std::env::var(name)
        .with_context(|| format!("{name} is required for semantic HTTP fleet operations"))?;
    validate_query_identity(name, &value)
}

fn optional_query_identity(name: &'static str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => validate_query_identity(name, &value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn validate_query_identity(name: &str, value: &str) -> Result<String> {
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
    use std::time::Duration;

    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
    };

    use super::{
        observe_live_query_http_runtime, parse_live_query_http_runtime, validate_relay_status_url,
        SemanticCommand, MAX_RELAY_STATUS_BYTES,
    };

    async fn read_request_headers(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
                .await
                .expect("status request headers arrive before timeout")
                .expect("read status request headers");
            assert_ne!(read, 0, "status request ended before its headers");
            request.extend_from_slice(&buffer[..read]);
            assert!(
                request.len() <= 16 * 1024,
                "status request headers are bounded"
            );
        }
        assert!(
            request.starts_with(b"GET /_status HTTP/1.1\r\n"),
            "diagnostic client requests only the exact status path"
        );
    }

    async fn spawn_status_response(response: Vec<u8>) -> (url::Url, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind temporary status server");
        let address = listener.local_addr().expect("temporary server address");
        let url = url::Url::parse(&format!("http://{address}/_status"))
            .expect("temporary loopback status URL");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept status request");
            read_request_headers(&mut stream).await;
            stream
                .write_all(&response)
                .await
                .expect("write status response");
            stream.shutdown().await.expect("close status response");
        });
        (url, server)
    }

    fn fixed_length_response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
        let mut response = format!(
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: {}\r\n{headers}\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        response
    }

    fn valid_live_status_body() -> Vec<u8> {
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        serde_json::to_vec(&json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": {
                "runtime_digest": digest,
                "parser_ready": true,
                "handler_ready": true,
                "deployment_master": true,
                "fleet_policy": "trusted-single-relay",
                "fleet_attestation_required": false,
                "fleet_attestation_status": "not_required",
                "deployment_id": null,
                "instance_id": null
            }
        }))
        .expect("encode valid live status")
    }

    fn assert_error_contains<T>(result: anyhow::Result<T>, expected: &str) {
        let error = result.err().expect("operation must fail");
        let message = format!("{error:#}");
        assert!(
            message.contains(expected),
            "expected error containing {expected:?}, got {message:?}"
        );
    }

    #[tokio::test]
    async fn live_relay_observation_accepts_a_valid_bounded_success_response() {
        let body = valid_live_status_body();
        let response = fixed_length_response("200 OK", "", &body);
        let (url, server) = spawn_status_response(response).await;

        let observation = observe_live_query_http_runtime(&url)
            .await
            .expect("observe valid live Relay status");
        server.await.expect("temporary status server completes");

        assert_eq!(observation.source, "live_relay_status");
        assert_eq!(observation.endpoint.as_deref(), Some(url.as_str()));
        assert!(observation.live_relay_observed);
        assert!(observation.deployment_master);
        assert_eq!(observation.fleet_policy.to_string(), "trusted-single-relay");
        assert_eq!(observation.parser_ready, Some(true));
        assert_eq!(observation.handler_ready, Some(true));
    }

    #[tokio::test]
    async fn live_relay_observation_rejects_non_success_status_before_parsing() {
        let response = fixed_length_response("503 Service Unavailable", "", b"not JSON");
        let (url, server) = spawn_status_response(response).await;

        let result = observe_live_query_http_runtime(&url).await;
        server.await.expect("temporary status server completes");

        assert_error_contains(result, "non-success HTTP 503 Service Unavailable");
    }

    #[tokio::test]
    async fn live_relay_observation_does_not_follow_redirects() {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind temporary redirect server");
        let address = listener.local_addr().expect("temporary redirect address");
        let url = url::Url::parse(&format!("http://{address}/_status"))
            .expect("temporary loopback redirect URL");
        let location = url.as_str().to_owned();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept initial request");
            read_request_headers(&mut stream).await;
            let response = format!(
                "HTTP/1.1 302 Found\r\nConnection: close\r\nContent-Length: 0\r\nLocation: {location}\r\n\r\n"
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write redirect response");
            stream.shutdown().await.expect("close redirect response");
            tokio::time::timeout(Duration::from_millis(250), listener.accept())
                .await
                .is_ok()
        });

        let result = observe_live_query_http_runtime(&url).await;
        let followed = server.await.expect("temporary redirect server completes");

        assert_error_contains(result, "non-success HTTP 302 Found");
        assert!(
            !followed,
            "isolated status client must not follow redirects"
        );
    }

    #[tokio::test]
    async fn live_relay_observation_rejects_oversized_declared_content_length() {
        let response = format!(
            "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            MAX_RELAY_STATUS_BYTES + 1
        )
        .into_bytes();
        let (url, server) = spawn_status_response(response).await;

        let result = observe_live_query_http_runtime(&url).await;
        server.await.expect("temporary status server completes");

        assert_error_contains(result, &format!("exceeds {MAX_RELAY_STATUS_BYTES} bytes"));
    }

    #[tokio::test]
    async fn live_relay_observation_rejects_oversized_chunked_stream() {
        let first_chunk_len = MAX_RELAY_STATUS_BYTES / 2;
        let second_chunk_len = MAX_RELAY_STATUS_BYTES - first_chunk_len + 1;
        let mut response =
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for chunk_len in [first_chunk_len, second_chunk_len] {
            response.extend_from_slice(format!("{chunk_len:x}\r\n").as_bytes());
            response.extend(std::iter::repeat_n(b' ', chunk_len));
            response.extend_from_slice(b"\r\n");
        }
        response.extend_from_slice(b"0\r\n\r\n");
        let (url, server) = spawn_status_response(response).await;

        let result = observe_live_query_http_runtime(&url).await;
        server.await.expect("temporary status server completes");

        assert_error_contains(result, &format!("exceeds {MAX_RELAY_STATUS_BYTES} bytes"));
    }

    #[tokio::test]
    async fn live_relay_observation_rejects_malformed_json() {
        let response = fixed_length_response("200 OK", "", b"{ definitely-not-json }");
        let (url, server) = spawn_status_response(response).await;

        let result = observe_live_query_http_runtime(&url).await;
        server.await.expect("temporary status server completes");

        assert_error_contains(result, "decode live Relay status JSON");
    }

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

        let readiness = <crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "semantic",
            "query-readiness",
            "--relay-status-url",
            "http://127.0.0.1:8080/_status",
        ])
        .expect("query-readiness live status CLI");
        assert!(matches!(
            readiness.command,
            crate::Command::Semantic {
                command: SemanticCommand::QueryReadiness {
                    relay_status_url: Some(_)
                }
            }
        ));
    }

    #[test]
    fn relay_status_url_is_limited_to_an_exact_literal_loopback_endpoint() {
        for accepted in [
            "http://127.0.0.1:8080/_status",
            "https://[::1]:8443/_status",
        ] {
            let url = url::Url::parse(accepted).expect("accepted URL parses");
            validate_relay_status_url(&url).expect("accepted loopback status URL");
        }

        for rejected in [
            "http://localhost:8080/_status",
            "http://192.0.2.1:8080/_status",
            "http://127.0.0.1:8080/_status/",
            "http://127.0.0.1:8080/_status?verbose=1",
            "http://operator@127.0.0.1:8080/_status",
            "file:///tmp/_status",
        ] {
            let url = url::Url::parse(rejected).expect("rejected URL still parses");
            assert!(
                validate_relay_status_url(&url).is_err(),
                "URL should be rejected: {rejected}"
            );
        }
    }

    #[test]
    fn live_relay_status_requires_the_closed_policy_and_matching_digest() {
        let url = url::Url::parse("http://127.0.0.1:8080/_status").expect("URL");
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        let local = json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": {
                "runtime_digest": digest,
                "parser_ready": true,
                "handler_ready": true,
                "deployment_master": true,
                "fleet_policy": "trusted-single-relay",
                "fleet_attestation_required": false,
                "fleet_attestation_status": "not_required",
                "deployment_id": null,
                "instance_id": null
            }
        });
        let parsed = parse_live_query_http_runtime(&url, &local).expect("valid local status");
        assert!(parsed.live_relay_observed);
        assert_eq!(parsed.fleet_policy.to_string(), "trusted-single-relay");

        let mut invalid_policy = local.clone();
        invalid_policy["semantic_graph_query_http"]["fleet_policy"] = json!("local");
        assert!(parse_live_query_http_runtime(&url, &invalid_policy).is_err());

        let mut mismatched_digest = local;
        mismatched_digest["semantic_graph_query_http"]["runtime_digest"] = json!("00");
        assert!(parse_live_query_http_runtime(&url, &mismatched_digest).is_err());
    }

    #[test]
    fn live_relay_status_accepts_coordinate_search_as_the_only_enabled_surface() {
        let url = url::Url::parse("http://127.0.0.1:8080/_status").expect("URL");
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        let shared = json!({
            "runtime_digest": digest,
            "parser_ready": true,
            "handler_ready": true,
            "fleet_policy": "trusted-single-relay",
            "fleet_attestation_required": false,
            "fleet_attestation_status": "not_required",
            "deployment_id": null,
            "instance_id": "coordinate-search-canary"
        });
        let mut graph = shared.clone();
        graph["deployment_master"] = json!(false);
        let mut coordinate = shared;
        coordinate["deployment_master"] = json!(true);
        let status = json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": graph,
            "project_context_coordinate_search_http": coordinate
        });

        let parsed = parse_live_query_http_runtime(&url, &status).expect("coordinate-only status");
        assert!(parsed.deployment_master);
        assert!(!parsed.semantic_graph_deployment_master);
        assert!(parsed.coordinate_search_deployment_master);
        assert!(!parsed.one_hop_semantic_search_deployment_master);
        assert_eq!(parsed.parser_ready, Some(true));
        assert_eq!(parsed.handler_ready, Some(true));
    }

    #[test]
    fn live_relay_status_accepts_one_hop_search_as_the_only_enabled_surface() {
        let url = url::Url::parse("http://127.0.0.1:8080/_status").expect("URL");
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        let shared = json!({
            "runtime_digest": digest,
            "parser_ready": true,
            "handler_ready": true,
            "fleet_policy": "trusted-single-relay",
            "fleet_attestation_required": false,
            "fleet_attestation_status": "not_required",
            "deployment_id": null,
            "instance_id": "one-hop-canary"
        });
        let mut graph = shared.clone();
        graph["deployment_master"] = json!(false);
        let mut one_hop = shared;
        one_hop["deployment_master"] = json!(true);
        let status = json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": graph,
            "project_context_one_hop_semantic_search_http": one_hop
        });

        let parsed = parse_live_query_http_runtime(&url, &status).expect("one-hop-only status");
        assert!(parsed.deployment_master);
        assert!(!parsed.semantic_graph_deployment_master);
        assert!(!parsed.coordinate_search_deployment_master);
        assert!(parsed.one_hop_semantic_search_deployment_master);
        assert_eq!(parsed.parser_ready, Some(true));
        assert_eq!(parsed.handler_ready, Some(true));
    }

    #[test]
    fn live_relay_status_rejects_disagreement_between_semantic_http_surfaces() {
        let url = url::Url::parse("http://127.0.0.1:8080/_status").expect("URL");
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        let runtime = json!({
            "runtime_digest": digest,
            "parser_ready": true,
            "handler_ready": true,
            "deployment_master": true,
            "fleet_policy": "trusted-single-relay",
            "fleet_attestation_required": false,
            "fleet_attestation_status": "not_required",
            "deployment_id": null,
            "instance_id": "coordinate-search-canary"
        });
        let mut coordinate = runtime.clone();
        coordinate["runtime_digest"] = json!("00");
        let status = json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": runtime,
            "project_context_coordinate_search_http": coordinate
        });

        assert!(parse_live_query_http_runtime(&url, &status).is_err());
    }

    #[test]
    fn live_strict_status_requires_identity_when_the_master_is_enabled() {
        let url = url::Url::parse("http://127.0.0.1:8080/_status").expect("URL");
        let digest = buzz_semantic_query::semantic_graph_http_runtime_digest()
            .expect("runtime digest")
            .to_hex();
        let missing_identity = json!({
            "service": "buzz-relay",
            "semantic_graph_query_http": {
                "runtime_digest": digest,
                "parser_ready": true,
                "handler_ready": false,
                "deployment_master": true,
                "fleet_policy": "attested-fleet",
                "fleet_attestation_required": true,
                "fleet_attestation_status": "community_scoped_not_evaluated",
                "deployment_id": null,
                "instance_id": null
            }
        });
        assert!(parse_live_query_http_runtime(&url, &missing_identity).is_err());
    }
}
