//! Operator control plane for the centralized Project View feature flag.

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use buzz_audit::{AuditAction, AuditService, NewAuditEntry};
use buzz_core::tenant::{normalize_host, TenantContext};
use buzz_db::project_view::{
    PreparedObjectProjection, PreparedProjectViewReprojection, ProjectViewFeatureStatus,
};
use buzz_db::project_view_v2::{ProjectViewV2AdminAssignment, ProjectViewV2CutoverPlan};
use buzz_db::Db;
use buzz_project_view::v2::idempotency_key_hash;
use buzz_project_view::ProjectionPlan;
use buzz_pubsub::{EventTopic, PubSubManager};
use clap::{Args, Subcommand};
use nostr::{Keys, PublicKey};
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
    /// Re-sign every projection after the feature has been disabled.
    Reproject {
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
        ProjectViewCommand::Reproject {
            target,
            relay_key_file,
            expected_pubkey,
        } => {
            reproject(&db, target, relay_key_file.as_deref(), &expected_pubkey).await?;
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
    }
    Ok(0)
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
            .project_view_status_by_host(&host)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Community host '{host}' was not found"))?]
    } else {
        db.list_project_view_statuses().await?
    };

    if statuses.is_empty() {
        println!("(no communities)");
        return Ok(());
    }

    println!(
        "{:<38} {:<32} {:<9} {:<8} {:<12} {:<12} projection_pubkey",
        "community_id", "host", "archived", "enabled", "revision", "generation"
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

async fn reproject(
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
        let generation = reproject_one(db, &pubsub, &keys, &status, &mut publish_failures).await?;
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

async fn reproject_one(
    db: &Db,
    pubsub: &PubSubManager,
    keys: &Keys,
    status: &ProjectViewFeatureStatus,
    publish_failures: &mut Vec<String>,
) -> Result<u64> {
    let mut write = db.begin_project_view_reproject(status.community_id).await?;
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
        .commit_reprojection(PreparedProjectViewReprojection {
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
    println!(
        "{:<38} {:<32} {:<9} {:<8} {:<12} {:<12} {}",
        status.community_id,
        status.host,
        status.archived,
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
}
