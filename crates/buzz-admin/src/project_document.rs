//! Read-only Stage 1 control plane for Project Document readiness.
//!
//! Bootstrap and feature mutation are intentionally absent. This command can
//! inspect migration/default state and verify an expected stable signer, but it
//! cannot expose the capability.

use anyhow::{bail, Result};
use buzz_core::tenant::normalize_host;
use buzz_db::project_document::{ProjectDocumentFeatureStatus, ProjectDocumentPreflight};
use buzz_db::Db;
use clap::Subcommand;
use nostr::PublicKey;

/// `buzz-admin project-document` Stage 1 commands.
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
}

/// Execute one read-only Project Document operator command.
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
                "reason": "migration_0032_not_applied"
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
    let values = statuses.iter().map(status_json).collect::<Vec<_>>();
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

fn status_json(status: &ProjectDocumentFeatureStatus) -> serde_json::Value {
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
    fn status_and_preflight_cli_shapes_are_read_only() {
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
