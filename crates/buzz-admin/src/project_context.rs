//! Controlled Project Context Edge bootstrap, verification, and capability gate.

use std::path::PathBuf;

use anyhow::{bail, Result};
use buzz_core::tenant::normalize_host;
use buzz_db::project_context::{
    PreparedProjectContextBootstrap, ProjectContextFeatureStatus, ProjectContextIntegrityStatus,
    ProjectContextPreflight,
};
use buzz_db::Db;
use buzz_project_context::{
    ProjectContextCatalog, ProjectContextProjectionPlan, PROJECT_CONTEXT_CAPABILITY,
};
use buzz_sdk::project_context::build_project_context_meta_projection;
use clap::Subcommand;
use nostr::{Keys, PublicKey};

/// `buzz-admin project-context` controlled operator commands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectContextCommand {
    /// Show capability, prerequisites, signer, catalog, and row status.
    Status {
        /// Limit output to one normalized Community host.
        #[arg(long)]
        community: Option<String>,
    },
    /// Verify prerequisites, signer, and current projection parity.
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
    /// Verify canonical rows, hashes, counts, and every current pointer.
    Verify {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Expected stable Relay public key (hex or npub).
        #[arg(long)]
        expected_pubkey: String,
    },
    /// Enable advertisement and attach after checked readiness succeeds.
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
    /// Stop advertisement and attach without deleting state; reads/detach remain available.
    Disable {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
}

/// Execute one Project Context Edge operator command.
pub(crate) async fn run(command: ProjectContextCommand) -> Result<i32> {
    let db = super::connect_db().await?;
    match command {
        ProjectContextCommand::Status { community } => {
            show_status(&db, community.as_deref()).await?
        }
        ProjectContextCommand::Preflight {
            community,
            expected_pubkey,
        } => preflight(&db, &community, &expected_pubkey).await?,
        ProjectContextCommand::Bootstrap {
            community,
            relay_key_file,
            expected_pubkey,
        } => bootstrap(&db, &community, relay_key_file.as_deref(), &expected_pubkey).await?,
        ProjectContextCommand::Verify {
            community,
            expected_pubkey,
        } => verify(&db, &community, &expected_pubkey).await?,
        ProjectContextCommand::Enable {
            community,
            relay_key_file,
            expected_pubkey,
        } => enable(&db, &community, relay_key_file.as_deref(), &expected_pubkey).await?,
        ProjectContextCommand::Disable { community } => disable(&db, &community).await?,
    }
    Ok(0)
}

async fn show_status(db: &Db, community: Option<&str>) -> Result<()> {
    if !db.project_context_schema_ready().await? {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "schema_ready": false,
                "enabled": false,
                "reason": "migration_0047_not_applied"
            }))?
        );
        return Ok(());
    }
    let mut statuses = db.list_project_context_statuses().await?;
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
                db.project_context_preflight(status.community_id, &pubkey)
                    .await?,
            ),
            None => None,
        };
        let integrity = db
            .project_context_integrity_status(status.community_id)
            .await?;
        values.push(status_json(status, report.as_ref(), integrity.as_ref()));
    }
    println!("{}", serde_json::to_string_pretty(&values)?);
    Ok(())
}

async fn preflight(db: &Db, community: &str, expected_pubkey: &str) -> Result<()> {
    let status = status_for_host(db, community).await?;
    let expected_pubkey = parse_pubkey(expected_pubkey)?;
    let report = db
        .project_context_preflight(status.community_id, &expected_pubkey)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&preflight_json(&status, &report, &expected_pubkey))?
    );
    if !report.structural_read_ready {
        bail!(
            "Project Context Edge preflight failed for '{}'",
            status.host
        );
    }
    Ok(())
}

async fn verify(db: &Db, community: &str, expected_pubkey: &str) -> Result<()> {
    let status = status_for_host(db, community).await?;
    let expected_pubkey = parse_pubkey(expected_pubkey)?;
    let integrity = db
        .verify_project_context_storage(status.community_id, &expected_pubkey)
        .await?;
    let report = db
        .project_context_preflight(status.community_id, &expected_pubkey)
        .await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "community_id": status.community_id.to_string(),
            "host": status.host,
            "verified": true,
            "structural_read_ready": report.structural_read_ready,
            "orphan_projection_count": integrity.orphan_projection_count,
            "pointer_mismatch_count": integrity.pointer_mismatch_count,
        }))?
    );
    Ok(())
}

async fn bootstrap(
    db: &Db,
    community: &str,
    relay_key_file: Option<&std::path::Path>,
    expected_pubkey: &str,
) -> Result<()> {
    let status = status_for_host(db, community).await?;
    if status.archived
        || status.enabled
        || status.project_view_schema_version != 3
        || !status.project_view_enabled
        || !status.project_document_enabled
        || status.maintenance_state != "normal"
    {
        bail!(
            "Project Context Edge bootstrap requires an active, disabled, normal Project View v3 and Project Document Community"
        );
    }
    let keys = checked_relay_keys(relay_key_file, expected_pubkey)?;
    if status.projection_pubkey.is_some() {
        let report = db
            .project_context_preflight(status.community_id, &keys.public_key())
            .await?;
        if report.initialized
            && report.signer_matches
            && report.projection_parity
            && status.context_revision == Some(0)
            && status.active_edge_count == Some(0)
            && status.bound_document_count == Some(0)
            && status.edge_row_count == 0
            && status.binding_row_count == 0
            && status.change_count == 0
        {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "community_id": status.community_id.to_string(),
                    "host": status.host,
                    "bootstrapped": true,
                    "replayed": true,
                    "projection_generation": status.projection_generation,
                    "projection_pubkey": keys.public_key().to_hex(),
                }))?
            );
            return Ok(());
        }
        bail!("existing Project Context Edge bootstrap is not safe to replay");
    }
    let canonical_time = db.project_context_canonical_now().await?;
    let catalog = ProjectContextCatalog::empty(status.community_id, 1, canonical_time)?;
    let plan = ProjectContextProjectionPlan::for_reset(&catalog)?;
    let meta_projection = build_project_context_meta_projection(&plan, &[])?
        .sign_with_keys(&keys)
        .map_err(|error| anyhow::anyhow!("sign Project Context bootstrap metadata: {error}"))?;
    let outcome = db
        .bootstrap_empty_project_context_catalog(PreparedProjectContextBootstrap {
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
            "replayed": outcome.replayed,
            "projection_generation": 1,
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
        .set_project_context_edge_enabled_checked(
            status.community_id,
            true,
            Some(&keys.public_key()),
        )
        .await?
    {
        bail!("Community host '{}' became unavailable", status.host);
    }
    println!(
        "enabled {} for {} ({})",
        PROJECT_CONTEXT_CAPABILITY, status.host, status.community_id
    );
    Ok(())
}

async fn disable(db: &Db, community: &str) -> Result<()> {
    let status = status_for_host(db, community).await?;
    if !db
        .set_project_context_edge_enabled_checked(status.community_id, false, None)
        .await?
    {
        bail!("Community host '{}' became unavailable", status.host);
    }
    println!(
        "disabled {} for {} ({}) without deleting canonical state",
        PROJECT_CONTEXT_CAPABILITY, status.host, status.community_id
    );
    Ok(())
}

async fn status_for_host(db: &Db, community: &str) -> Result<ProjectContextFeatureStatus> {
    let host = normalize_required_host(community)?;
    db.list_project_context_statuses()
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
    let expected = parse_pubkey(expected_pubkey)?;
    if keys.public_key() != expected {
        bail!(
            "relay signer mismatch: expected {}, supplied key resolves to {}",
            expected.to_hex(),
            keys.public_key().to_hex()
        );
    }
    Ok(keys)
}

fn parse_pubkey(value: &str) -> Result<PublicKey> {
    PublicKey::parse(value).map_err(|error| anyhow::anyhow!("invalid --expected-pubkey: {error}"))
}

fn status_json(
    status: &ProjectContextFeatureStatus,
    report: Option<&ProjectContextPreflight>,
    integrity: Option<&ProjectContextIntegrityStatus>,
) -> serde_json::Value {
    serde_json::json!({
        "community_id": status.community_id.to_string(),
        "host": status.host,
        "archived": status.archived,
        "enabled": status.enabled,
        "project_view_schema_version": status.project_view_schema_version,
        "project_view_enabled": status.project_view_enabled,
        "project_document_enabled": status.project_document_enabled,
        "maintenance_state": status.maintenance_state,
        "context_revision": status.context_revision,
        "active_edge_count": status.active_edge_count,
        "bound_document_count": status.bound_document_count,
        "projection_generation": status.projection_generation,
        "projection_pubkey": status.projection_pubkey.map(|key| key.to_hex()),
        "edge_row_count": status.edge_row_count,
        "binding_row_count": status.binding_row_count,
        "change_count": status.change_count,
        "projection_parity": report.is_some_and(|value| value.projection_parity),
        "structural_read_ready": report.is_some_and(|value| value.structural_read_ready),
        "advertised_ready": report.is_some_and(|value| value.advertised_ready),
        "orphan_projection_count": integrity.map(|value| value.orphan_projection_count),
        "pointer_mismatch_count": integrity.map(|value| value.pointer_mismatch_count),
    })
}

fn preflight_json(
    status: &ProjectContextFeatureStatus,
    report: &ProjectContextPreflight,
    expected_pubkey: &PublicKey,
) -> serde_json::Value {
    serde_json::json!({
        "community_id": report.community_id.to_string(),
        "host": status.host,
        "enabled": report.enabled,
        "expected_pubkey": expected_pubkey.to_hex(),
        "schema_ready": report.schema_ready,
        "project_view_ready": report.project_view_ready,
        "project_document_ready": report.project_document_ready,
        "initialized": report.initialized,
        "signer_matches": report.signer_matches,
        "projection_parity": report.projection_parity,
        "structural_read_ready": report.structural_read_ready,
        "advertised_ready": report.advertised_ready,
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
        command: ProjectContextCommand,
    }

    #[test]
    fn status_and_control_cli_shapes_are_closed() {
        let status =
            TestCli::try_parse_from(["test", "status", "--community", "Relay.Example.:443"])
                .expect("parse status");
        assert!(matches!(
            status.command,
            ProjectContextCommand::Status { community: Some(_) }
        ));

        let bootstrap = TestCli::try_parse_from([
            "test",
            "bootstrap",
            "--community",
            "relay.example",
            "--expected-pubkey",
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
        ])
        .expect("parse bootstrap");
        assert!(matches!(
            bootstrap.command,
            ProjectContextCommand::Bootstrap { .. }
        ));

        let disable = TestCli::try_parse_from(["test", "disable", "--community", "relay.example"])
            .expect("parse disable");
        assert!(matches!(
            disable.command,
            ProjectContextCommand::Disable { .. }
        ));
    }
}
