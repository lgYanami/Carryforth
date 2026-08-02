//! Controlled Project Document v1 bootstrap, verification, and capability gate.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Result};
use buzz_core::tenant::normalize_host;
use buzz_db::project_document::{
    PreparedProjectDocumentBootstrap, PreparedProjectDocumentReprojectEvent,
    ProjectDocumentFeatureStatus, ProjectDocumentHistoryPageRequest,
    ProjectDocumentIntegrityStatus, ProjectDocumentPreflight, ProjectDocumentReprojectContext,
    ProjectDocumentReprojectEventType, ProjectDocumentReprojectStatus,
};
use buzz_db::Db;
use buzz_project_document::{
    DocumentCatalog, DocumentHeadProjection, DocumentProjectionPlan, DocumentProjectionType,
    DocumentRevision, DocumentRevisionProjection, PROJECT_DOCUMENT_SCHEMA_VERSION,
};
use buzz_sdk::project_document::{
    build_document_head_reprojection, build_document_meta_projection,
    build_document_revision_reprojection, document_revision_coordinate,
};
use clap::Subcommand;
use nostr::{Keys, PublicKey};

/// `buzz-admin project-document` controlled operator commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectDocumentCommand {
    /// Show flag, schema, signer, catalog, and immutable revision status.
    Status {
        /// Limit output to one normalized Community host.
        #[arg(long)]
        community: Option<String>,
    },
    /// Verify bootstrap, Project View schema, signer, and pointer parity.
    Preflight {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Expected stable Relay public key (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Create the signed revision-zero empty catalog while the gate is off.
    Bootstrap {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// File containing the Relay private key; must not be group/world readable.
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        /// Expected public key of the supplied stable Relay signer.
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Verify canonical rows and every active projection pointer.
    Verify {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Expected stable Relay public key (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Enable member reads and writes after checked readiness succeeds.
    Enable {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// File containing the Relay private key; must not be group/world readable.
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        /// Expected public key of the supplied stable Relay signer.
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Fail closed immediately without deleting canonical state or history.
    Disable {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
    /// Re-sign every immutable revision into one inactive generation and
    /// atomically activate it while the capability is disabled.
    Reproject {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Required acknowledgement that the complete immutable history is in scope.
        #[arg(long)]
        all_revisions: bool,
        /// File containing the new Relay private key; must not be group/world readable.
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        /// Expected public key of the new stable Relay signer.
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Measure bounded keyset history pagination against a local synthetic
    /// capacity fixture and emit a machine-readable report.
    CapacityProbe {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Stable Relay signer expected by the fixture.
        #[arg(long)]
        expected_pubkey: String,
        /// Authorized Human member public key.
        #[arg(long)]
        reader_pubkey: String,
        /// Hot Document UUID.
        #[arg(long)]
        document_id: uuid::Uuid,
        /// Fixed maximum revision of the synthetic snapshot.
        #[arg(long)]
        max_revision: u64,
        /// Maximum pages to inspect; each page contains at most 50 revisions.
        #[arg(long, default_value_t = 2000)]
        pages: u32,
        /// Local single-page acceptance timeout.
        #[arg(long, default_value_t = 2000)]
        timeout_ms: u64,
    },
}

/// Execute one Project Document operator command.
pub(crate) async fn run(command: ProjectDocumentCommand) -> Result<i32> {
    let db = super::connect_db().await?;
    match command {
        ProjectDocumentCommand::Status { community } => {
            show_status(&db, community.as_deref()).await?
        }
        ProjectDocumentCommand::Preflight {
            community,
            expected_pubkey,
        } => preflight(&db, &community, &expected_pubkey).await?,
        ProjectDocumentCommand::Bootstrap {
            community,
            relay_key_file,
            expected_pubkey,
        } => bootstrap(&db, &community, relay_key_file.as_deref(), &expected_pubkey).await?,
        ProjectDocumentCommand::Verify {
            community,
            expected_pubkey,
        } => verify(&db, &community, &expected_pubkey).await?,
        ProjectDocumentCommand::Enable {
            community,
            relay_key_file,
            expected_pubkey,
        } => enable(&db, &community, relay_key_file.as_deref(), &expected_pubkey).await?,
        ProjectDocumentCommand::Disable { community } => disable(&db, &community).await?,
        ProjectDocumentCommand::Reproject {
            community,
            all_revisions,
            relay_key_file,
            expected_pubkey,
        } => {
            if !all_revisions {
                bail!("Project Document reproject requires --all-revisions");
            }
            reproject(&db, &community, relay_key_file.as_deref(), &expected_pubkey).await?
        }
        ProjectDocumentCommand::CapacityProbe {
            community,
            expected_pubkey,
            reader_pubkey,
            document_id,
            max_revision,
            pages,
            timeout_ms,
        } => {
            capacity_probe(
                &db,
                &community,
                &expected_pubkey,
                &reader_pubkey,
                document_id,
                max_revision,
                pages,
                timeout_ms,
            )
            .await?
        }
    }
    Ok(0)
}

async fn show_status(db: &Db, community: Option<&str>) -> Result<()> {
    let schema_ready = db.project_document_schema_ready().await?;
    if !schema_ready {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_ready": false,
                "enabled": false,
                "reason": "migration_0035_not_applied"
            }))?
        );
        return Ok(());
    }
    let mut statuses = db.list_project_document_statuses().await?;
    if let Some(host) = community {
        let host = normalize_required_host(host)?;
        statuses.retain(|status| status.host == host);
        if statuses.is_empty() {
            bail!("Community host '{host}' was not found");
        }
    }
    let mut values = Vec::with_capacity(statuses.len());
    for status in &statuses {
        let report = match status.projection_pubkey {
            Some(pubkey) => Some(
                db.project_document_preflight(status.community_id, &pubkey)
                    .await?,
            ),
            None => None,
        };
        let reproject = db
            .project_document_reproject_status(status.community_id)
            .await?;
        let integrity = db
            .project_document_integrity_status(status.community_id)
            .await?;
        values.push(status_json(
            status,
            report.as_ref(),
            reproject.as_ref(),
            &integrity,
        ));
    }
    println!("{}", serde_json::to_string_pretty(&values)?);
    Ok(())
}

async fn preflight(db: &Db, community: &str, expected_pubkey: &str) -> Result<()> {
    let host = normalize_required_host(community)?;
    let status = db
        .list_project_document_statuses()
        .await?
        .into_iter()
        .find(|status| status.host == host)
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?;
    let expected_pubkey = PublicKey::parse(expected_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))?;
    let report = db
        .project_document_preflight(status.community_id, &expected_pubkey)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&preflight_json(&status, &report, &expected_pubkey))?
    );
    if !report.ready {
        bail!("Project Document preflight failed for '{host}'");
    }
    Ok(())
}

async fn verify(db: &Db, community: &str, expected_pubkey: &str) -> Result<()> {
    preflight(db, community, expected_pubkey).await
}

async fn bootstrap(
    db: &Db,
    community: &str,
    relay_key_file: Option<&std::path::Path>,
    expected_pubkey: &str,
) -> Result<()> {
    let status = status_for_host(db, community).await?;
    if status.archived || status.enabled || !matches!(status.project_view_schema_version, 2 | 3) {
        bail!(
            "Project Document bootstrap requires an active, disabled Project View v2/v3 Community"
        );
    }
    let keys = checked_relay_keys(relay_key_file, expected_pubkey)?;
    if status.projection_pubkey.is_some() {
        let report = db
            .project_document_preflight(status.community_id, &keys.public_key())
            .await?;
        if report.bootstrapped && report.signer_matches && report.projection_parity {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "community_id": status.community_id.to_string(),
                    "host": status.host,
                    "bootstrapped": true,
                    "replayed": true,
                    "projection_pubkey": keys.public_key().to_hex(),
                }))?
            );
            return Ok(());
        }
        bail!("existing Project Document bootstrap is not safe to replay");
    }
    let canonical_time = db.project_document_canonical_now().await?;
    let catalog = DocumentCatalog::empty(status.community_id, 1, canonical_time)?;
    let plan = DocumentProjectionPlan::for_bootstrap(&catalog)?;
    let meta_projection = build_document_meta_projection(&plan, &[])?
        .sign_with_keys(&keys)
        .map_err(|error| anyhow::anyhow!("sign Project Document bootstrap metadata: {error}"))?;
    db.bootstrap_empty_project_document_catalog(PreparedProjectDocumentBootstrap {
        catalog,
        meta_projection,
    })
    .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "community_id": status.community_id.to_string(),
            "host": status.host,
            "bootstrapped": true,
            "replayed": false,
            "projection_pubkey": keys.public_key().to_hex(),
        }))?
    );
    Ok(())
}

async fn enable(
    db: &Db,
    community: &str,
    relay_key_file: Option<&std::path::Path>,
    expected_pubkey: &str,
) -> Result<()> {
    let status = status_for_host(db, community).await?;
    let keys = checked_relay_keys(relay_key_file, expected_pubkey)?;
    if !db
        .set_project_document_enabled_checked(status.community_id, true, Some(&keys.public_key()))
        .await?
    {
        bail!("Community host '{}' became unavailable", status.host);
    }
    println!(
        "enabled Project Document for {} ({})",
        status.host, status.community_id
    );
    Ok(())
}

async fn disable(db: &Db, community: &str) -> Result<()> {
    let status = status_for_host(db, community).await?;
    if !db
        .set_project_document_enabled_checked(status.community_id, false, None)
        .await?
    {
        bail!("Community host '{}' became unavailable", status.host);
    }
    println!(
        "disabled Project Document for {} ({}) without deleting canonical state",
        status.host, status.community_id
    );
    Ok(())
}

async fn reproject(
    db: &Db,
    community: &str,
    relay_key_file: Option<&std::path::Path>,
    expected_pubkey: &str,
) -> Result<()> {
    let status = status_for_host(db, community).await?;
    if status.archived || status.enabled || !matches!(status.project_view_schema_version, 2 | 3) {
        bail!(
            "Project Document full-history reproject requires an active, disabled Project View v2/v3 Community"
        );
    }
    let keys = checked_relay_keys(relay_key_file, expected_pubkey)?;
    if status.projection_pubkey == Some(keys.public_key()) {
        let latest = db
            .project_document_reproject_status(status.community_id)
            .await?;
        let parity = db
            .project_document_preflight(status.community_id, &keys.public_key())
            .await?;
        if let Some(operation) = latest.filter(|operation| {
            parity.projection_parity
                && operation.state == "activated"
                && Some(operation.target_generation) == status.projection_generation
                && operation.target_pubkey == keys.public_key()
        }) {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "community_id": status.community_id.to_string(),
                    "host": status.host,
                    "operation_id": operation.operation_id.to_string(),
                    "all_revisions": true,
                    "revision_count": operation.revision_count,
                    "document_count": operation.document_count,
                    "projection_generation": operation.target_generation,
                    "projection_pubkey": operation.target_pubkey.to_hex(),
                    "projection_parity": true,
                    "enabled": false,
                    "replayed": true,
                }))?
            );
            return Ok(());
        }
    }
    let context = db
        .begin_project_document_reproject(status.community_id, keys.public_key())
        .await?;
    let reproject_status = db
        .project_document_reproject_status(status.community_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("reproject operation disappeared after begin"))?;
    if reproject_status.state == "staging" {
        stage_reproject(db, &context, &keys).await?;
        db.ready_project_document_reproject(&context).await?;
    } else if reproject_status.state != "ready" {
        bail!(
            "reproject operation {} is not resumable from state {}",
            reproject_status.operation_id,
            reproject_status.state
        );
    }
    db.activate_project_document_reproject(&context).await?;
    let report = db
        .project_document_preflight(status.community_id, &keys.public_key())
        .await?;
    if !report.projection_parity || !report.signer_matches {
        bail!("activated Project Document generation failed final verification");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "community_id": status.community_id.to_string(),
            "host": status.host,
            "operation_id": context.operation_id.to_string(),
            "all_revisions": true,
            "revision_count": context.revision_count,
            "document_count": context.document_count,
            "source_generation": context.source_generation,
            "projection_generation": context.target_generation,
            "projection_pubkey": keys.public_key().to_hex(),
            "projection_parity": true,
            "enabled": false,
        }))?
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn capacity_probe(
    db: &Db,
    community: &str,
    expected_pubkey: &str,
    reader_pubkey: &str,
    document_id: uuid::Uuid,
    max_revision: u64,
    pages: u32,
    timeout_ms: u64,
) -> Result<()> {
    if max_revision == 0 || pages == 0 || timeout_ms == 0 {
        bail!("capacity probe revisions, pages, and timeout must be positive");
    }
    let status = status_for_host(db, community).await?;
    let expected_pubkey = PublicKey::parse(expected_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))?;
    let reader_pubkey = PublicKey::parse(reader_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --reader-pubkey: {error}"))?;
    let generation = status
        .projection_generation
        .ok_or_else(|| anyhow::anyhow!("Project Document catalog is not bootstrapped"))?;
    let first_request = ProjectDocumentHistoryPageRequest {
        community_id: status.community_id,
        expected_pubkey: &expected_pubkey,
        reader_pubkey: reader_pubkey.as_bytes(),
        projection_generation: generation,
        document_id,
        max_document_revision: max_revision,
        before_revision: None,
        limit: 50,
    };
    let plan = db
        .project_document_history_query_plan(first_request)
        .await?;
    let accepted_indexes = [
        "project_document_revisions_pkey",
        "idx_project_document_revisions_history",
    ];
    let used_index = accepted_indexes
        .iter()
        .find(|index| plan_contains_index(&plan, index))
        .copied();
    let uses_expected_index = used_index.is_some();
    let revision_seq_scan =
        plan_contains_relation_node(&plan, "project_document_revisions", "Seq Scan");
    let start_rss_kib = process_rss_kib()?;
    let mut max_rss_kib = start_rss_kib;
    let mut before_revision = None;
    let mut event_count = 0_u64;
    let mut completed_pages = 0_u32;
    let mut max_page_ms = 0_u128;
    let total_started = Instant::now();
    for _ in 0..pages {
        let started = Instant::now();
        let page = db
            .project_document_history_page(ProjectDocumentHistoryPageRequest {
                before_revision,
                ..first_request
            })
            .await?;
        let page_ms = started.elapsed().as_millis();
        max_page_ms = max_page_ms.max(page_ms);
        completed_pages += 1;
        let page_len = u64::try_from(page.events.len()).unwrap_or(u64::MAX);
        event_count = event_count.saturating_add(page_len);
        max_rss_kib = max_rss_kib.max(process_rss_kib()?);
        if page.events.is_empty() || page.events.len() < 50 {
            break;
        }
        before_revision = max_revision
            .checked_sub(event_count)
            .and_then(|value| value.checked_add(1));
        if before_revision.is_none_or(|value| value <= 1) {
            break;
        }
    }
    let end_rss_kib = process_rss_kib()?;
    let total_ms = total_started.elapsed().as_millis();
    let timeout_passed = max_page_ms <= u128::from(timeout_ms);
    let peak_growth_kib = max_rss_kib.saturating_sub(start_rss_kib);
    let retained_growth_kib = end_rss_kib.saturating_sub(start_rss_kib);
    let bounded_memory = peak_growth_kib <= 64 * 1024 && retained_growth_kib <= 16 * 1024;
    let report = serde_json::json!({
        "community_id": status.community_id.to_string(),
        "host": status.host,
        "document_id": document_id.to_string(),
        "projection_generation": generation,
        "max_revision": max_revision,
        "page_limit": 50,
        "pages_requested": pages,
        "pages_completed": completed_pages,
        "events_read": event_count,
        "total_ms": total_ms,
        "max_page_ms": max_page_ms,
        "timeout_ms": timeout_ms,
        "timeout_passed": timeout_passed,
        "rss_start_kib": start_rss_kib,
        "rss_peak_kib": max_rss_kib,
        "rss_end_kib": end_rss_kib,
        "rss_peak_growth_kib": peak_growth_kib,
        "rss_retained_growth_kib": retained_growth_kib,
        "bounded_memory": bounded_memory,
        "accepted_indexes": accepted_indexes,
        "used_index": used_index,
        "uses_expected_index": uses_expected_index,
        "revision_seq_scan": revision_seq_scan,
        "query_plan": plan,
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !timeout_passed || !bounded_memory || !uses_expected_index || revision_seq_scan {
        bail!("Project Document capacity probe failed its local acceptance thresholds");
    }
    Ok(())
}

fn process_rss_kib() -> Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|value| value.split_whitespace().next())
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("/proc/self/status has no parseable VmRSS"))
}

fn plan_contains_index(value: &serde_json::Value, expected: &str) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            fields.get("Index Name").and_then(serde_json::Value::as_str) == Some(expected)
                || fields
                    .values()
                    .any(|child| plan_contains_index(child, expected))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .any(|child| plan_contains_index(child, expected)),
        _ => false,
    }
}

fn plan_contains_relation_node(value: &serde_json::Value, relation: &str, node_type: &str) -> bool {
    match value {
        serde_json::Value::Object(fields) => {
            (fields
                .get("Relation Name")
                .and_then(serde_json::Value::as_str)
                == Some(relation)
                && fields.get("Node Type").and_then(serde_json::Value::as_str) == Some(node_type))
                || fields
                    .values()
                    .any(|child| plan_contains_relation_node(child, relation, node_type))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .any(|child| plan_contains_relation_node(child, relation, node_type)),
        _ => false,
    }
}

async fn stage_reproject(
    db: &Db,
    context: &ProjectDocumentReprojectContext,
    keys: &Keys,
) -> Result<()> {
    let mut after_catalog_revision = 0;
    loop {
        let revisions = db
            .project_document_reproject_revision_page(context, after_catalog_revision, 500)
            .await?;
        if revisions.is_empty() {
            break;
        }
        let mut staged = Vec::with_capacity(revisions.len() * 2);
        for revision in &revisions {
            let revision_projection = reproject_revision_projection(context, revision);
            let revision_event = build_document_revision_reprojection(&revision_projection)?
                .sign_with_keys(keys)
                .map_err(|error| anyhow::anyhow!("sign reprojected Document revision: {error}"))?;
            staged.push(PreparedProjectDocumentReprojectEvent {
                projection_type: ProjectDocumentReprojectEventType::Revision,
                document_id: Some(revision.document_id),
                document_revision: Some(revision.document_revision),
                event: revision_event.clone(),
            });
            if revision.is_current {
                let head_projection =
                    reproject_head_projection(context, revision, revision_event.id);
                let head_event =
                    build_document_head_reprojection(&head_projection, &revision_event)?
                        .sign_with_keys(keys)
                        .map_err(|error| {
                            anyhow::anyhow!("sign reprojected Document head: {error}")
                        })?;
                staged.push(PreparedProjectDocumentReprojectEvent {
                    projection_type: ProjectDocumentReprojectEventType::Head,
                    document_id: Some(revision.document_id),
                    document_revision: Some(revision.document_revision),
                    event: head_event,
                });
            }
            after_catalog_revision = revision.catalog_revision;
        }
        db.stage_project_document_reproject_events(context, &staged)
            .await?;
    }
    let catalog = DocumentCatalog::from_snapshot(
        context.community_id,
        context.catalog_revision,
        context.active_document_count,
        context.target_generation,
        context.initialized_at,
        context.updated_at,
    )?;
    let plan = DocumentProjectionPlan::for_reprojection(&catalog)?;
    let meta_event = build_document_meta_projection(&plan, &[])?
        .sign_with_keys(keys)
        .map_err(|error| anyhow::anyhow!("sign reprojected Document metadata: {error}"))?;
    db.stage_project_document_reproject_events(
        context,
        &[PreparedProjectDocumentReprojectEvent {
            projection_type: ProjectDocumentReprojectEventType::Meta,
            document_id: None,
            document_revision: None,
            event: meta_event,
        }],
    )
    .await?;
    Ok(())
}

fn reproject_revision_projection(
    context: &ProjectDocumentReprojectContext,
    source: &buzz_db::project_document::ProjectDocumentReprojectRevision,
) -> DocumentRevisionProjection {
    match &source.revision {
        DocumentRevision::Active {
            snapshot,
            actor,
            canonical_at,
            ..
        } => DocumentRevisionProjection::Active {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            projection_type: DocumentProjectionType::DocumentRevision,
            project_id: *context.community_id.as_uuid(),
            projection_generation: context.target_generation,
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
            projection_type: DocumentProjectionType::DocumentRevision,
            project_id: *context.community_id.as_uuid(),
            projection_generation: context.target_generation,
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

fn reproject_head_projection(
    context: &ProjectDocumentReprojectContext,
    source: &buzz_db::project_document::ProjectDocumentReprojectRevision,
    revision_event_id: buzz_core::EventId,
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
            projection_type: DocumentProjectionType::DocumentHead,
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
            projection_type: DocumentProjectionType::DocumentHead,
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

async fn status_for_host(db: &Db, community: &str) -> Result<ProjectDocumentFeatureStatus> {
    let host = normalize_required_host(community)?;
    db.list_project_document_statuses()
        .await?
        .into_iter()
        .find(|status| status.host == host)
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))
}

fn checked_relay_keys(
    relay_key_file: Option<&std::path::Path>,
    expected_pubkey: &str,
) -> Result<Keys> {
    let keys = super::project_view::load_relay_keys(relay_key_file)?;
    let expected = PublicKey::parse(expected_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))?;
    if keys.public_key() != expected {
        bail!(
            "relay signer mismatch: expected {}, supplied key resolves to {}",
            expected.to_hex(),
            keys.public_key().to_hex()
        );
    }
    Ok(keys)
}

fn status_json(
    status: &ProjectDocumentFeatureStatus,
    report: Option<&ProjectDocumentPreflight>,
    reproject: Option<&ProjectDocumentReprojectStatus>,
    integrity: &ProjectDocumentIntegrityStatus,
) -> serde_json::Value {
    serde_json::json!({
        "community_id": status.community_id.to_string(),
        "host": status.host,
        "archived": status.archived,
        "enabled": status.enabled,
        "project_view_schema_version": status.project_view_schema_version,
        "catalog_revision": status.catalog_revision,
        "active_document_count": status.active_document_count,
        "revision_count": status.revision_count,
        "projection_generation": status.projection_generation,
        "projection_pubkey": status.projection_pubkey.map(|key| key.to_hex()),
        "meta_parity": report.is_some_and(|value| value.projection_parity),
        "orphan_projection_count": integrity.orphan_projection_count,
        "pointer_mismatch_count": integrity.pointer_mismatch_count,
        "ready": status.enabled && report.is_some_and(|value| value.ready),
        "reproject": reproject.map(|value| serde_json::json!({
            "operation_id": value.operation_id.to_string(),
            "state": value.state,
            "source_generation": value.source_generation,
            "target_generation": value.target_generation,
            "target_pubkey": value.target_pubkey.to_hex(),
            "revision_count": value.revision_count,
            "document_count": value.document_count,
            "staged_revision_count": value.staged_revision_count,
            "staged_head_count": value.staged_head_count,
            "meta_staged": value.meta_staged,
        })),
    })
}

fn preflight_json(
    status: &ProjectDocumentFeatureStatus,
    report: &ProjectDocumentPreflight,
    expected_pubkey: &PublicKey,
) -> serde_json::Value {
    serde_json::json!({
        "community_id": report.community_id.to_string(),
        "host": status.host,
        "enabled": status.enabled,
        "expected_pubkey": expected_pubkey.to_hex(),
        "schema_ready": report.schema_ready,
        "project_view_schema_ready": report.project_view_schema_ready,
        "bootstrapped": report.bootstrapped,
        "signer_matches": report.signer_matches,
        "projection_parity": report.projection_parity,
        "ready": report.ready,
    })
}

fn normalize_required_host(host: &str) -> Result<String> {
    let normalized = normalize_host(host);
    if normalized.is_empty() {
        bail!("Community host cannot be empty");
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: ProjectDocumentCommand,
    }

    #[test]
    fn status_and_control_cli_shapes_are_closed() {
        let status =
            TestCli::try_parse_from(["test", "status", "--community", "Relay.Example.:443"])
                .expect("parse status");
        assert!(matches!(
            status.command,
            ProjectDocumentCommand::Status { community: Some(_) }
        ));

        let preflight = TestCli::try_parse_from([
            "test",
            "preflight",
            "--community",
            "relay.example",
            "--expected-pubkey",
            &"01".repeat(32),
        ])
        .expect("parse preflight");
        assert!(matches!(
            preflight.command,
            ProjectDocumentCommand::Preflight { .. }
        ));

        let disable = TestCli::try_parse_from(["test", "disable", "--community", "relay.example"])
            .expect("parse disable");
        assert!(matches!(
            disable.command,
            ProjectDocumentCommand::Disable { .. }
        ));
    }

    #[test]
    fn host_normalization_matches_tenant_resolution() {
        assert_eq!(
            normalize_required_host(" Relay.Example.:443 ").expect("normalize host"),
            "relay.example"
        );
        assert!(normalize_required_host("   ").is_err());
    }
}
