//! Operator control plane for the centralized Project View feature flag.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use buzz_audit::{AuditAction, AuditService, NewAuditEntry};
use buzz_core::tenant::{normalize_host, TenantContext};
use buzz_db::project_view::{
    LegacyV1PreparedProjectViewReprojection, PreparedObjectProjection, ProjectViewFeatureStatus,
};
use buzz_db::project_view_v2::{ProjectViewV2AdminAssignment, ProjectViewV2CutoverPlan};
use buzz_db::Db;
use buzz_project_view::v2::idempotency_key_hash;
use buzz_project_view::v3::{
    CanonicalMaintenanceRepairPlanEnvelopeV1, CanonicalMaintenanceRepairPlanV1,
    ResourceMappingManifestEnvelopeV1, MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES,
    MAX_MANIFEST_JSON_BYTES,
};
use buzz_project_view::ProjectionPlan;
use buzz_pubsub::{EventTopic, PubSubManager};
use clap::{Args, Subcommand};
use nostr::{EventId, Keys, PublicKey};
use tracing::warn;
use uuid::Uuid;

/// `buzz-admin project-view` subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectViewCommand {
    /// Show feature, initialization, signer, and revision readiness.
    Status {
        /// Limit output to one normalized Community host.
        #[arg(long)]
        community: Option<String>,
    },
    /// Enable mutation writes and future capability advertisement.
    Enable {
        #[command(flatten)]
        target: ProjectViewTarget,
    },
    /// Disable new mutation writes without deleting canonical state.
    Disable {
        #[command(flatten)]
        target: ProjectViewTarget,
    },
    /// Explicit legacy schema-v1 projection recovery before migration.
    LegacyV1Reproject {
        #[command(flatten)]
        target: ProjectViewTarget,
        /// File containing the relay private key; must not be group/world accessible.
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        /// Expected public key of the supplied relay signer (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Explicitly migrate one disabled initialized Community from v1 to v2.
    CutoverV2 {
        /// Normalized Community host.
        #[arg(long)]
        community: String,
        /// Existing admin mapping in `<pubkey>=<role-uuid>` form; repeat as needed.
        #[arg(long = "admin-assignment")]
        admin_assignments: Vec<String>,
        /// Existing admin to explicitly downgrade to member; repeat as needed.
        #[arg(long = "downgrade-admin")]
        downgraded_admins: Vec<String>,
        /// Caller-stable idempotency key. It is hashed before storage/audit.
        #[arg(long)]
        idempotency_key: String,
        /// File containing the relay private key; must not be group/world accessible.
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        /// Expected public key of the supplied relay signer (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Prepare an empty, disabled Community for owner-signed schema-v3 initialization.
    PrepareV3 {
        /// Normalized Community host.
        #[arg(long)]
        community: String,
        /// Caller-stable idempotency key.
        #[arg(long)]
        idempotency_key: String,
        /// Current Human owner/admin public key; defaults to BUZZ_PRIVATE_KEY.
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
    /// Control the durable Project View v3 maintenance state machine.
    Maintenance {
        #[command(subcommand)]
        command: ProjectViewMaintenanceCommand,
    },
    /// Inspect or transition the staged Project Context sub-capability.
    Context {
        #[command(subcommand)]
        command: ProjectViewContextCommand,
    },
    /// Operate the capability-off Project View schema-v3 migration surface.
    V3 {
        #[command(subcommand)]
        command: ProjectViewV3Command,
    },
}

/// Project Context capability control commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectViewContextCommand {
    /// Show flags, structural parity, Document readiness, and reference counts.
    Status {
        #[arg(long)]
        community: String,
    },
    /// Enable only after all server, client, closure, and storage gates pass.
    Enable {
        #[arg(long)]
        community: String,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
    /// Fail closed without deleting canonical Context coordinates.
    Disable {
        #[arg(long)]
        community: String,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
}

/// Project View v3 operator commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectViewV3Command {
    /// Export or validate reviewed legacy Resource mappings.
    Resources {
        #[command(subcommand)]
        command: ProjectViewV3ResourcesCommand,
    },
    /// Execute one replay-first frozen schema-v2-to-v3 cutover.
    Cutover {
        #[arg(long)]
        community: String,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        maintenance_epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        #[arg(long)]
        expected_pubkey: String,
    },
}

/// Reviewed legacy Resource migration commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectViewV3ResourcesCommand {
    /// Export a deterministic local review draft from the exact schema-v2 base.
    Export {
        #[arg(long)]
        community: String,
        /// Owner-only directory that receives `resource-mapping-draft.json`.
        #[arg(long)]
        out: PathBuf,
        /// Current Human owner/admin public key; defaults to BUZZ_PRIVATE_KEY.
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
    /// Recompute and persist all canonical mapping/signature evidence.
    Validate {
        #[arg(long)]
        community: String,
        #[arg(long)]
        manifest: PathBuf,
    },
}

/// Durable Project View maintenance operations.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectViewMaintenanceCommand {
    /// Disable ordinary writes and capture the exact Assignment/Runtime baseline.
    Begin {
        #[arg(long)]
        community: String,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        /// Minimum ordered ACP maintenance protocol version accepted by freeze.
        #[arg(long)]
        required_client_protocol_version: u64,
        /// Expected stable Relay projection signer (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Inspect current or historical durable maintenance state.
    Status {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: Option<u64>,
    },
    /// Inspect ordered ACP fleet compatibility and exact drain blockers.
    Readiness {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        /// A non-ACKed supervisor poll older than this is reported stale.
        #[arg(long, default_value_t = 30)]
        max_poll_age_seconds: u64,
    },
    /// Exit successfully only when the exact epoch is safe to freeze.
    AckProbe {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long, default_value_t = 30)]
        max_poll_age_seconds: u64,
    },
    /// Freeze an exact fully acknowledged drain epoch.
    Freeze {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
    /// Abort an exact pre-cutover epoch without reviving old Runtime fences.
    Abort {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Record exact structural verification while remaining frozen.
    Verify {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Apply one bounded typed forward repair while remaining frozen.
    Repair {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        /// Closed repair-plan JSON envelope.
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        /// Validate the plan and print its canonical digest without writing.
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Re-sign every canonical v3 head at the next generation while frozen.
    Reproject {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Restore the exact v3 membership snapshot after a semantically equal
    /// local bootstrap replacement retired the referenced canonical event.
    RestoreMembershipSnapshot {
        #[arg(long)]
        community: String,
        #[arg(long)]
        expected_project_revision: u64,
        #[arg(long)]
        expected_projection_generation: u64,
        #[arg(long)]
        expected_old_membership_event_id: String,
        #[arg(long)]
        candidate_current_membership_event_id: String,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        relay_key_file: Option<PathBuf>,
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Resume a verified committed v3 epoch and re-enable eligible Communities.
    Resume {
        #[arg(long)]
        community: String,
        #[arg(long)]
        epoch: u64,
        #[arg(long)]
        idempotency_key: String,
        #[arg(long)]
        operator_pubkey: Option<String>,
        #[arg(long)]
        expected_pubkey: String,
    },
}

/// Exactly one Community target for a feature-flag mutation.
#[derive(Debug, Args)]
#[group(id = "project_view_target", required = true, multiple = false)]
pub(crate) struct ProjectViewTarget {
    /// One normalized Community host.
    #[arg(long)]
    community: Option<String>,
    /// Every non-archived Community, locked in stable UUID order.
    #[arg(long)]
    all: bool,
}

/// Execute one Project View operator command.
pub(crate) async fn run(command: ProjectViewCommand) -> Result<i32> {
    let db = super::connect_db().await?;
    match command {
        ProjectViewCommand::Status { community } => {
            show_status(&db, community.as_deref()).await?;
        }
        ProjectViewCommand::Enable { target } => {
            set_enabled(&db, target, true).await?;
        }
        ProjectViewCommand::Disable { target } => {
            set_enabled(&db, target, false).await?;
        }
        ProjectViewCommand::LegacyV1Reproject {
            target,
            relay_key_file,
            expected_pubkey,
        } => {
            legacy_v1_reproject(&db, target, relay_key_file.as_deref(), &expected_pubkey).await?;
        }
        ProjectViewCommand::CutoverV2 {
            community,
            admin_assignments,
            downgraded_admins,
            idempotency_key,
            relay_key_file,
            expected_pubkey,
        } => {
            cutover_v2(
                &db,
                &community,
                &admin_assignments,
                &downgraded_admins,
                &idempotency_key,
                relay_key_file.as_deref(),
                &expected_pubkey,
            )
            .await?;
        }
        ProjectViewCommand::PrepareV3 {
            community,
            idempotency_key,
            operator_pubkey,
        } => {
            prepare_v3(
                &db,
                &community,
                &idempotency_key,
                operator_pubkey.as_deref(),
            )
            .await?;
        }
        ProjectViewCommand::Maintenance { command } => {
            run_maintenance(&db, command).await?;
        }
        ProjectViewCommand::Context { command } => {
            run_context(&db, command).await?;
        }
        ProjectViewCommand::V3 { command } => {
            run_v3(&db, command).await?;
        }
    }
    Ok(0)
}

async fn run_context(db: &Db, command: ProjectViewContextCommand) -> Result<()> {
    match command {
        ProjectViewContextCommand::Status { community } => {
            let status = required_status(db, &community).await?;
            let context = db
                .project_context_feature_status(status.community_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Community '{}' was not found", status.host))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "community_id": context.community_id.to_string(),
                    "host": context.host,
                    "archived": context.archived,
                    "project_view_schema_version": context.project_view_schema_version,
                    "project_view_enabled": context.project_view_enabled,
                    "context_enabled": context.context_enabled,
                    "document_enabled": context.document_enabled,
                    "maintenance_state": context.maintenance_state,
                    "project_revision": context.project_revision,
                    "projection_generation": context.projection_generation,
                    "projection_pubkey": context.projection_pubkey.map(|key| key.to_hex()),
                    "document_catalog_revision": context.document_catalog_revision,
                    "resource_reference_count": context.resource_reference_count,
                    "document_reference_count": context.document_reference_count,
                    "project_view_ready": context.project_view_ready,
                    "document_ready": context.document_ready,
                    "advertised_ready": context.advertised_ready,
                }))?
            );
        }
        ProjectViewContextCommand::Enable {
            community,
            idempotency_key,
            operator_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let receipt = db
                .set_project_context_enabled_checked(
                    status.community_id,
                    true,
                    operator,
                    &idempotency_key,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewContextCommand::Disable {
            community,
            idempotency_key,
            operator_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let receipt = db
                .set_project_context_enabled_checked(
                    status.community_id,
                    false,
                    operator,
                    &idempotency_key,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
    }
    Ok(())
}

async fn run_v3(db: &Db, command: ProjectViewV3Command) -> Result<()> {
    match command {
        ProjectViewV3Command::Resources { command } => match command {
            ProjectViewV3ResourcesCommand::Export {
                community,
                out,
                operator_pubkey,
            } => {
                let status = required_status(db, &community).await?;
                let relay = required_projection_pubkey(&status)?;
                let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
                let draft = db
                    .export_project_view_v3_resource_draft(status.community_id, operator, &relay)
                    .await?;
                let bytes = serde_json::to_vec_pretty(&draft)?;
                let path = write_owner_only_json_dir(&out, "resource-mapping-draft.json", &bytes)?;
                println!("{}", path.display());
            }
            ProjectViewV3ResourcesCommand::Validate {
                community,
                manifest,
            } => {
                let status = required_status(db, &community).await?;
                let relay = required_projection_pubkey(&status)?;
                let manifest = read_resource_manifest(&manifest)?;
                if Uuid::from_bytes(manifest.community_id) != *status.community_id.as_uuid() {
                    bail!("manifest Community does not match --community");
                }
                let receipt = db
                    .validate_project_view_v3_resource_manifest(
                        status.community_id,
                        &manifest,
                        &relay,
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            }
        },
        ProjectViewV3Command::Cutover {
            community,
            manifest,
            maintenance_epoch,
            idempotency_key,
            operator_pubkey,
            relay_key_file,
            expected_pubkey,
        } => {
            if idempotency_key.trim().is_empty() {
                bail!("--idempotency-key cannot be empty");
            }
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let keys = load_relay_keys(relay_key_file.as_deref())?;
            let expected = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            if keys.public_key() != expected {
                bail!(
                    "relay signer mismatch: expected {}, supplied key resolves to {}",
                    expected.to_hex(),
                    keys.public_key().to_hex()
                );
            }
            let manifest = read_resource_manifest(&manifest)?;
            if Uuid::from_bytes(manifest.community_id) != *status.community_id.as_uuid() {
                bail!("manifest Community does not match --community");
            }

            // Catch an absent Redis before committing. A later fan-out fault is
            // reported but leaves the durable Community frozen for repair.
            let pubsub = super::connect_pubsub().await?;
            let outcome = db
                .cutover_project_view_v3(
                    status.community_id,
                    maintenance_epoch,
                    operator,
                    &idempotency_key,
                    &manifest,
                    &keys,
                )
                .await?;
            let tenant = TenantContext::resolved(status.community_id, status.host);
            let mut publish_failures = Vec::new();
            for event in &outcome.events {
                if let Err(error) = pubsub
                    .publish_event(&tenant, EventTopic::Global, event)
                    .await
                {
                    warn!(
                        community_id = %status.community_id,
                        event_id = %event.id,
                        "Project View v3 cutover Redis fan-out failed: {error}"
                    );
                    publish_failures.push(event.id.to_hex());
                }
            }
            println!("{}", serde_json::to_string_pretty(&outcome.result)?);
            if !publish_failures.is_empty() {
                bail!(
                    "cutover committed and remains frozen, but Redis fan-out failed for: {}",
                    publish_failures.join(",")
                );
            }
        }
    }
    Ok(())
}

fn required_projection_pubkey(status: &ProjectViewFeatureStatus) -> Result<PublicKey> {
    status.projection_pubkey.ok_or_else(|| {
        anyhow::anyhow!(
            "Project View for '{}' has no stable projection signer",
            status.host
        )
    })
}

fn read_resource_manifest(path: &Path) -> Result<buzz_project_view::v3::ResourceMappingManifestV1> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("read manifest metadata: {error}"))?;
    if !metadata.is_file() {
        bail!("--manifest must name a regular file");
    }
    if metadata.len() > MAX_MANIFEST_JSON_BYTES as u64 {
        bail!("manifest exceeds {MAX_MANIFEST_JSON_BYTES} bytes");
    }
    let bytes = std::fs::read(path).map_err(|error| anyhow::anyhow!("read --manifest: {error}"))?;
    ResourceMappingManifestEnvelopeV1::parse_json(&bytes).map_err(Into::into)
}

fn read_repair_plan(path: &Path) -> Result<CanonicalMaintenanceRepairPlanV1> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| anyhow::anyhow!("read repair-plan metadata: {error}"))?;
    if !metadata.is_file() {
        bail!("--plan must name a regular file");
    }
    if metadata.len() > MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES as u64 {
        bail!("repair plan exceeds {MAX_MAINTENANCE_REPAIR_PLAN_JSON_BYTES} bytes");
    }
    let bytes = std::fs::read(path).map_err(|error| anyhow::anyhow!("read --plan: {error}"))?;
    CanonicalMaintenanceRepairPlanEnvelopeV1::parse_json(&bytes).map_err(Into::into)
}

async fn publish_recovery_events(
    pubsub: &PubSubManager,
    status: &ProjectViewFeatureStatus,
    events: &[nostr::Event],
    operation: &str,
) -> Result<()> {
    let tenant = TenantContext::resolved(status.community_id, status.host.clone());
    let mut failures = Vec::new();
    for event in events {
        if let Err(error) = pubsub
            .publish_event(&tenant, EventTopic::Global, event)
            .await
        {
            warn!(
                community_id = %status.community_id,
                event_id = %event.id,
                operation,
                "Project View v3 recovery Redis fan-out failed: {error}"
            );
            failures.push(event.id.to_hex());
        }
    }
    if !failures.is_empty() {
        bail!(
            "{operation} committed and remains frozen, but Redis fan-out failed for: {}",
            failures.join(",")
        );
    }
    Ok(())
}

fn write_owner_only_json_dir(directory: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    if directory.exists() {
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|error| anyhow::anyhow!("read --out metadata: {error}"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            bail!("--out must name a real directory, not a file or symlink");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                bail!("--out permissions are too broad; remove all group/world access");
            }
        }
    } else {
        std::fs::create_dir(directory)
            .map_err(|error| anyhow::anyhow!("create --out directory: {error}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        }
    }

    let path = directory.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|error| anyhow::anyhow!("create {}: {error}", path.display()))?;
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(path)
}

async fn prepare_v3(
    db: &Db,
    community: &str,
    idempotency_key: &str,
    operator_pubkey: Option<&str>,
) -> Result<()> {
    let status = required_status(db, community).await?;
    let operator = resolve_operator_pubkey(operator_pubkey)?;
    let receipt = db
        .prepare_project_view_v3(status.community_id, operator, idempotency_key)
        .await?;
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    Ok(())
}

async fn run_maintenance(db: &Db, command: ProjectViewMaintenanceCommand) -> Result<()> {
    match command {
        ProjectViewMaintenanceCommand::Status { community, epoch } => {
            let status = required_status(db, &community).await?;
            let result = db
                .project_view_maintenance_status(status.community_id, epoch)
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ProjectViewMaintenanceCommand::Readiness {
            community,
            epoch,
            max_poll_age_seconds,
        } => {
            let status = required_status(db, &community).await?;
            let result = db
                .project_view_maintenance_readiness(
                    status.community_id,
                    epoch,
                    max_poll_age_seconds,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        ProjectViewMaintenanceCommand::AckProbe {
            community,
            epoch,
            max_poll_age_seconds,
        } => {
            let status = required_status(db, &community).await?;
            let result = db
                .project_view_maintenance_readiness(
                    status.community_id,
                    epoch,
                    max_poll_age_seconds,
                )
                .await?;
            let ready = result
                .get("ready_to_freeze")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| anyhow::anyhow!("readiness response omitted ready_to_freeze"))?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            if !ready {
                bail!("maintenance epoch {epoch} is not ready to freeze");
            }
        }
        ProjectViewMaintenanceCommand::Begin {
            community,
            idempotency_key,
            operator_pubkey,
            required_client_protocol_version,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let relay = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            let receipt = db
                .begin_project_view_v3_maintenance(
                    status.community_id,
                    operator,
                    required_client_protocol_version,
                    &idempotency_key,
                    &relay,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewMaintenanceCommand::Freeze {
            community,
            epoch,
            idempotency_key,
            operator_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let receipt = db
                .freeze_project_view_v3_maintenance(
                    status.community_id,
                    epoch,
                    operator,
                    &idempotency_key,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewMaintenanceCommand::Abort {
            community,
            epoch,
            idempotency_key,
            operator_pubkey,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let relay = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            let receipt = db
                .abort_project_view_v3_maintenance(
                    status.community_id,
                    epoch,
                    operator,
                    &idempotency_key,
                    &relay,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewMaintenanceCommand::Verify {
            community,
            epoch,
            idempotency_key,
            operator_pubkey,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let relay = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            let receipt = db
                .verify_project_view_v3_maintenance(
                    status.community_id,
                    epoch,
                    operator,
                    &idempotency_key,
                    &relay,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewMaintenanceCommand::Repair {
            community,
            epoch,
            plan,
            idempotency_key,
            operator_pubkey,
            dry_run,
            relay_key_file,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let relay = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            let plan = read_repair_plan(&plan)?;
            if plan.maintenance_epoch != epoch {
                bail!("repair plan maintenance_epoch differs from --epoch");
            }
            if Uuid::from_bytes(plan.community_id) != *status.community_id.as_uuid() {
                bail!("repair plan Community differs from --community");
            }
            if dry_run {
                let receipt = db
                    .validate_project_view_v3_repair_plan(
                        status.community_id,
                        operator,
                        &plan,
                        &relay,
                    )
                    .await?;
                println!("{}", serde_json::to_string_pretty(&receipt)?);
            } else {
                let keys = load_relay_keys(relay_key_file.as_deref())?;
                if keys.public_key() != relay {
                    bail!(
                        "relay signer mismatch: expected {}, supplied key resolves to {}",
                        relay.to_hex(),
                        keys.public_key().to_hex()
                    );
                }
                let pubsub = super::connect_pubsub().await?;
                let outcome = db
                    .repair_project_view_v3(
                        status.community_id,
                        epoch,
                        operator,
                        &idempotency_key,
                        &plan,
                        &keys,
                    )
                    .await?;
                publish_recovery_events(&pubsub, &status, &outcome.events, "repair").await?;
                println!("{}", serde_json::to_string_pretty(&outcome.receipt)?);
            }
        }
        ProjectViewMaintenanceCommand::Reproject {
            community,
            epoch,
            idempotency_key,
            operator_pubkey,
            relay_key_file,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let keys = load_relay_keys(relay_key_file.as_deref())?;
            let expected = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            if keys.public_key() != expected {
                bail!(
                    "relay signer mismatch: expected {}, supplied key resolves to {}",
                    expected.to_hex(),
                    keys.public_key().to_hex()
                );
            }
            let pubsub = super::connect_pubsub().await?;
            let outcome = db
                .reproject_project_view_v3(
                    status.community_id,
                    epoch,
                    operator,
                    &idempotency_key,
                    &keys,
                )
                .await?;
            publish_recovery_events(&pubsub, &status, &outcome.events, "reproject").await?;
            println!("{}", serde_json::to_string_pretty(&outcome.receipt)?);
        }
        ProjectViewMaintenanceCommand::RestoreMembershipSnapshot {
            community,
            expected_project_revision,
            expected_projection_generation,
            expected_old_membership_event_id,
            candidate_current_membership_event_id,
            idempotency_key,
            operator_pubkey,
            relay_key_file,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let keys = load_relay_keys(relay_key_file.as_deref())?;
            let expected = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            if keys.public_key() != expected {
                bail!(
                    "relay signer mismatch: expected {}, supplied key resolves to {}",
                    expected.to_hex(),
                    keys.public_key().to_hex()
                );
            }
            let old_event_id = parse_event_id_argument(
                &expected_old_membership_event_id,
                "--expected-old-membership-event-id",
            )?;
            let candidate_event_id = parse_event_id_argument(
                &candidate_current_membership_event_id,
                "--candidate-current-membership-event-id",
            )?;
            let receipt = db
                .restore_project_view_v3_membership_snapshot(
                    status.community_id,
                    operator,
                    &idempotency_key,
                    expected_project_revision,
                    expected_projection_generation,
                    old_event_id,
                    candidate_event_id,
                    &keys,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
        ProjectViewMaintenanceCommand::Resume {
            community,
            epoch,
            idempotency_key,
            operator_pubkey,
            expected_pubkey,
        } => {
            let status = required_status(db, &community).await?;
            let operator = resolve_operator_pubkey(operator_pubkey.as_deref())?;
            let relay = parse_pubkey_argument(&expected_pubkey, "--expected-pubkey")?;
            let receipt = db
                .resume_project_view_v3_maintenance(
                    status.community_id,
                    epoch,
                    operator,
                    &idempotency_key,
                    &relay,
                )
                .await?;
            println!("{}", serde_json::to_string_pretty(&receipt)?);
        }
    }
    Ok(())
}

async fn required_status(db: &Db, community: &str) -> Result<ProjectViewFeatureStatus> {
    let host = normalize_required_host(community)?;
    db.project_view_status_by_host(&host)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))
}

fn resolve_operator_pubkey(value: Option<&str>) -> Result<PublicKey> {
    if let Some(value) = value {
        return parse_pubkey_argument(value, "--operator-pubkey");
    }
    let private_key = std::env::var("BUZZ_PRIVATE_KEY")
        .map_err(|_| anyhow::anyhow!("--operator-pubkey or BUZZ_PRIVATE_KEY is required"))?;
    Keys::parse(private_key.trim())
        .map(|keys| keys.public_key())
        .map_err(|error| anyhow::anyhow!("invalid BUZZ_PRIVATE_KEY: {error}"))
}

fn parse_pubkey_argument(value: &str, argument: &str) -> Result<PublicKey> {
    PublicKey::parse(value).map_err(|error| anyhow::anyhow!("invalid {argument}: {error}"))
}

fn parse_event_id_argument(value: &str, argument: &str) -> Result<EventId> {
    EventId::from_hex(value).map_err(|error| anyhow::anyhow!("invalid {argument}: {error}"))
}

#[allow(clippy::too_many_arguments)]
async fn cutover_v2(
    db: &Db,
    community: &str,
    admin_assignments: &[String],
    downgraded_admins: &[String],
    idempotency_key: &str,
    relay_key_file: Option<&Path>,
    expected_pubkey: &str,
) -> Result<()> {
    if idempotency_key.trim().is_empty() {
        bail!("--idempotency-key cannot be empty");
    }
    let host = normalize_required_host(community)?;
    let status = db
        .project_view_status_by_host(&host)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?;
    if status.archived {
        bail!("Community host '{host}' is archived");
    }
    if status.enabled {
        bail!("Project View for '{host}' must be disabled before cutover");
    }
    let keys = load_relay_keys(relay_key_file)?;
    let expected_pubkey = PublicKey::parse(expected_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))?;
    if keys.public_key() != expected_pubkey {
        bail!(
            "relay signer mismatch: expected {}, supplied key resolves to {}",
            expected_pubkey.to_hex(),
            keys.public_key().to_hex()
        );
    }

    let mappings = admin_assignments
        .iter()
        .map(|value| parse_admin_assignment(value))
        .collect::<Result<Vec<_>>>()?;
    let downgraded = downgraded_admins
        .iter()
        .map(|value| {
            PublicKey::parse(value)
                .map_err(|error| anyhow::anyhow!("invalid --downgrade-admin '{value}': {error}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let key_hash = idempotency_key_hash(idempotency_key.as_bytes());
    if let Some(receipt) = db
        .project_view_v2_operator_receipt(status.community_id, &key_hash)
        .await?
    {
        println!("{}", serde_json::to_string_pretty(&receipt)?);
        return Ok(());
    }

    let audit = AuditService::new(db.writer().clone());
    let audit_entry = audit
        .log(NewAuditEntry {
            community_id: status.community_id,
            action: AuditAction::ProjectViewCutover,
            actor_pubkey: None,
            object_id: Some(status.community_id.to_string()),
            detail: serde_json::json!({
                "from_schema_version": 1,
                "to_schema_version": 2,
                "admin_assignment_count": mappings.len(),
                "downgraded_admin_count": downgraded.len(),
                "idempotency_key_hash": hex::encode(key_hash),
            }),
        })
        .await?;
    let outcome = db
        .cutover_project_view_v2(
            status.community_id,
            &ProjectViewV2CutoverPlan {
                admin_assignments: mappings,
                downgraded_admins: downgraded,
                audit_seq: audit_entry.seq,
                idempotency_key_hash: key_hash,
            },
            &keys,
        )
        .await?;
    if !outcome.replayed {
        let pubsub = super::connect_pubsub().await?;
        let tenant = TenantContext::resolved(status.community_id, status.host.clone());
        for event in &outcome.events {
            if let Err(error) = pubsub
                .publish_event(&tenant, EventTopic::Global, event)
                .await
            {
                warn!(
                    community_id = %status.community_id,
                    event_id = %event.id,
                    "Project View v2 cutover Redis fan-out failed: {error}"
                );
            }
        }
    }
    println!("{}", serde_json::to_string_pretty(&outcome.result)?);
    Ok(())
}

fn parse_admin_assignment(value: &str) -> Result<ProjectViewV2AdminAssignment> {
    let (pubkey, role_id) = value.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("invalid --admin-assignment '{value}'; expected <pubkey>=<role-uuid>")
    })?;
    let member_pubkey = PublicKey::parse(pubkey)
        .map_err(|error| anyhow::anyhow!("invalid admin pubkey '{pubkey}': {error}"))?;
    let role_id = Uuid::parse_str(role_id)
        .map_err(|error| anyhow::anyhow!("invalid Leader Role UUID '{role_id}': {error}"))?;
    if role_id.is_nil() {
        bail!("Leader Role UUID cannot be nil");
    }
    Ok(ProjectViewV2AdminAssignment {
        member_pubkey,
        role_id,
    })
}

async fn show_status(db: &Db, community: Option<&str>) -> Result<()> {
    let statuses = if let Some(host) = community {
        let host = normalize_required_host(host)?;
        vec![db
            .project_view_status_by_host_with_strict_readiness(&host)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?]
    } else {
        db.list_project_view_statuses_with_strict_readiness()
            .await?
    };

    if statuses.is_empty() {
        println!("(no communities)");
        return Ok(());
    }

    println!(
        "{:<38} {:<32} {:<9} {:<6} {:<9} {:<11} {:<12} {:<8} {:<12} {:<12} projection_pubkey",
        "community_id",
        "host",
        "archived",
        "schema",
        "prepared",
        "initialized",
        "strict-ready",
        "enabled",
        "revision",
        "generation"
    );
    println!("{}", "-".repeat(190));
    for status in statuses {
        print_status(&status);
    }
    Ok(())
}

async fn set_enabled(db: &Db, target: ProjectViewTarget, enabled: bool) -> Result<()> {
    let action = if enabled { "enabled" } else { "disabled" };
    let relay_keys = enabled.then(load_relay_keys_from_env).transpose()?;
    let relay_pubkey = relay_keys.as_ref().map(Keys::public_key);
    if target.all {
        let changed = db
            .set_all_project_views_enabled_checked(enabled, relay_pubkey.as_ref())
            .await?;
        println!("{action} Project View for {changed} active communities");
        return Ok(());
    }

    let host = normalize_required_host(
        target
            .community
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--community or --all is required"))?,
    )?;
    let status = db
        .project_view_status_by_host(&host)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?;
    if status.archived {
        bail!("Community host '{host}' is archived");
    }
    if !db
        .set_project_view_enabled_checked(status.community_id, enabled, relay_pubkey.as_ref())
        .await?
    {
        bail!("Community host '{host}' became unavailable while acquiring its lock");
    }
    println!("{action} Project View for {host} ({})", status.community_id);
    Ok(())
}

async fn legacy_v1_reproject(
    db: &Db,
    target: ProjectViewTarget,
    relay_key_file: Option<&Path>,
    expected_pubkey: &str,
) -> Result<()> {
    let keys = load_relay_keys(relay_key_file)?;
    let expected_pubkey = PublicKey::parse(expected_pubkey)
        .map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))?;
    if keys.public_key() != expected_pubkey {
        bail!(
            "relay signer mismatch: expected {}, supplied key resolves to {}",
            expected_pubkey.to_hex(),
            keys.public_key().to_hex()
        );
    }

    let targets = resolve_targets(db, target).await?;
    for status in &targets {
        if status.enabled {
            bail!(
                "Project View for {} ({}) is enabled; disable every target before reprojecting",
                status.host,
                status.community_id
            );
        }
        if status.project_revision.is_some()
            && (status.projection_generation.is_none() || status.projection_pubkey.is_none())
        {
            bail!(
                "Project View for {} ({}) has incomplete projection metadata",
                status.host,
                status.community_id
            );
        }
    }
    let initialized: Vec<ProjectViewFeatureStatus> = targets
        .into_iter()
        .filter(|status| status.project_revision.is_some())
        .collect();
    if initialized.is_empty() {
        println!("no initialized Project View projections require re-signing");
        return Ok(());
    }

    // Establish Redis before changing any durable generation. A later publish
    // can still fail, but this catches absent/misconfigured Redis up front.
    let pubsub = super::connect_pubsub().await?;
    let mut publish_failures = Vec::new();
    for status in initialized {
        let generation =
            legacy_v1_reproject_one(db, &pubsub, &keys, &status, &mut publish_failures).await?;
        println!(
            "reprojected {} ({}) at generation {generation} with signer {}",
            status.host,
            status.community_id,
            keys.public_key().to_hex()
        );
    }

    if !publish_failures.is_empty() {
        bail!(
            "reprojection committed, but {} Redis fan-out(s) failed: {}",
            publish_failures.len(),
            publish_failures.join("; ")
        );
    }
    Ok(())
}

async fn legacy_v1_reproject_one(
    db: &Db,
    pubsub: &PubSubManager,
    keys: &Keys,
    status: &ProjectViewFeatureStatus,
    publish_failures: &mut Vec<String>,
) -> Result<u64> {
    let mut write = db
        .begin_legacy_v1_project_view_reproject(status.community_id)
        .await?;
    let context = write.load_current().await?;
    let projection_generation = context
        .metadata
        .projection_generation
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("projection generation overflow"))?;
    let plan = ProjectionPlan::for_reprojection(&context.state, projection_generation)?;

    let mut object_projections = Vec::with_capacity(plan.entries().len());
    for entry in plan.entries() {
        let event = buzz_sdk::project_view::build_object_projection(&plan, entry)?
            .sign_with_keys(keys)
            .map_err(|error| anyhow::anyhow!("sign object projection: {error}"))?;
        object_projections.push(PreparedObjectProjection::new(entry.id(), event));
    }
    let meta_projection = buzz_sdk::project_view::build_meta_projection(&plan, &[])?
        .sign_with_keys(keys)
        .map_err(|error| anyhow::anyhow!("sign metadata projection: {error}"))?;
    let outcome = write
        .commit_reprojection(LegacyV1PreparedProjectViewReprojection {
            state: context.state,
            object_projections,
            meta_projection,
            projection_generation,
        })
        .await?;

    let tenant = TenantContext::resolved(status.community_id, status.host.clone());
    for event in &outcome.events {
        if let Err(error) = pubsub
            .publish_event(&tenant, EventTopic::Global, event)
            .await
        {
            warn!(
                community_id = %status.community_id,
                event_id = %event.id,
                "Project View reproject Redis fan-out failed: {error}"
            );
            publish_failures.push(format!("{}:{}", status.host, event.id));
        }
    }
    Ok(outcome.projection_generation)
}

async fn resolve_targets(
    db: &Db,
    target: ProjectViewTarget,
) -> Result<Vec<ProjectViewFeatureStatus>> {
    if target.all {
        return Ok(db
            .list_project_view_statuses()
            .await?
            .into_iter()
            .filter(|status| !status.archived)
            .collect());
    }

    let host = normalize_required_host(
        target
            .community
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("--community or --all is required"))?,
    )?;
    let status = db
        .project_view_status_by_host(&host)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?;
    if status.archived {
        bail!("Community host '{host}' is archived");
    }
    Ok(vec![status])
}

fn load_relay_keys_from_env() -> Result<Keys> {
    load_relay_keys(None)
}

pub(crate) fn load_relay_keys(relay_key_file: Option<&Path>) -> Result<Keys> {
    let secret = if let Some(path) = relay_key_file {
        let metadata = std::fs::metadata(path)
            .map_err(|error| anyhow::anyhow!("read relay key metadata: {error}"))?;
        if !metadata.is_file() {
            bail!("--relay-key-file must name a regular file");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o077 != 0 {
                bail!("--relay-key-file permissions are too broad; remove all group/world access");
            }
        }
        std::fs::read_to_string(path)
            .map_err(|error| anyhow::anyhow!("read --relay-key-file: {error}"))?
    } else {
        std::env::var("BUZZ_RELAY_PRIVATE_KEY").map_err(|_| {
            anyhow::anyhow!(
                "BUZZ_RELAY_PRIVATE_KEY is required when --relay-key-file is not supplied"
            )
        })?
    };
    Keys::parse(secret.trim())
        .map_err(|error| anyhow::anyhow!("invalid relay private key: {error}"))
}

fn normalize_required_host(host: &str) -> Result<String> {
    let normalized = normalize_host(host);
    if normalized.is_empty() {
        bail!("Community host cannot be empty");
    }
    Ok(normalized)
}

fn print_status(status: &ProjectViewFeatureStatus) {
    let revision = status
        .project_revision
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    let generation = status
        .projection_generation
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    let projection_pubkey = status
        .projection_pubkey
        .map_or_else(|| "-".to_owned(), |value| value.to_hex());
    let strict_ready = status
        .strict_ready
        .map_or_else(|| "-".to_owned(), |value| value.to_string());
    println!(
        "{:<38} {:<32} {:<9} {:<6} {:<9} {:<11} {:<12} {:<8} {:<12} {:<12} {}",
        status.community_id,
        status.host,
        status.archived,
        status.schema_version,
        status.prepared,
        status.initialized,
        strict_ready,
        status.enabled,
        revision,
        generation,
        projection_pubkey
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_normalization_matches_tenant_resolution() {
        assert_eq!(
            normalize_required_host(" Relay.Example.:443 ").expect("normalize host"),
            "relay.example"
        );
        assert!(normalize_required_host("   ").is_err());
    }

    #[test]
    fn schema_v1_reproject_is_explicitly_legacy_named() {
        let legacy = <crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "project-view",
            "legacy-v1-reproject",
            "--community",
            "relay.example",
            "--expected-pubkey",
            "00",
        ])
        .expect("parse explicitly legacy v1 reproject command");
        assert!(matches!(
            legacy.command,
            crate::Command::ProjectView {
                command: ProjectViewCommand::LegacyV1Reproject { .. }
            }
        ));
        assert!(<crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "project-view",
            "reproject",
            "--community",
            "relay.example",
            "--expected-pubkey",
            "00",
        ])
        .is_err());
    }

    #[test]
    fn context_control_cli_has_closed_status_enable_and_disable_shapes() {
        let status = <crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "project-view",
            "context",
            "status",
            "--community",
            "relay.example",
        ])
        .expect("parse context status");
        assert!(matches!(
            status.command,
            crate::Command::ProjectView {
                command: ProjectViewCommand::Context {
                    command: ProjectViewContextCommand::Status { community }
                }
            } if community == "relay.example"
        ));

        for operation in ["enable", "disable"] {
            let parsed = <crate::Cli as clap::Parser>::try_parse_from([
                "buzz-admin",
                "project-view",
                "context",
                operation,
                "--community",
                "relay.example",
                "--idempotency-key",
                "stage6-canary",
                "--operator-pubkey",
                "00",
            ])
            .unwrap_or_else(|error| panic!("parse context {operation}: {error}"));
            assert!(matches!(
                parsed.command,
                crate::Command::ProjectView {
                    command: ProjectViewCommand::Context { .. }
                }
            ));

            let missing_key = <crate::Cli as clap::Parser>::try_parse_from([
                "buzz-admin",
                "project-view",
                "context",
                operation,
                "--community",
                "relay.example",
            ]);
            assert!(missing_key.is_err(), "{operation} must require idempotency");
        }

        assert!(<crate::Cli as clap::Parser>::try_parse_from([
            "buzz-admin",
            "project-view",
            "context",
            "enable",
            "--community",
            "relay.example",
            "--idempotency-key",
            "stage6-canary",
            "--force",
        ])
        .is_err());
    }
}
