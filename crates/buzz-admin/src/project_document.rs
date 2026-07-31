//! Controlled Project Document v1 bootstrap, verification, and capability gate.

use std::path::PathBuf;

use anyhow::{bail, Result};
use buzz_core::tenant::normalize_host;
use buzz_db::project_document::{
    PreparedProjectDocumentBootstrap, ProjectDocumentFeatureStatus, ProjectDocumentPreflight,
};
use buzz_db::Db;
use buzz_project_document::{DocumentCatalog, DocumentProjectionPlan};
use buzz_sdk::project_document::build_document_meta_projection;
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
    let mut values = Vec::with_capacity(statuses.len());
    for status in &statuses {
        let report = match status.projection_pubkey {
            Some(pubkey) => Some(
                db.project_document_preflight(status.community_id, &pubkey)
                    .await?,
            ),
            None => None,
        };
        values.push(status_json(status, report.as_ref()));
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
        "ready": status.enabled && report.is_some_and(|value| value.ready),
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
