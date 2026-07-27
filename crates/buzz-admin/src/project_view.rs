//! Operator control plane for the centralized Project View feature flag.

use anyhow::{bail, Result};
use buzz_core::tenant::normalize_host;
use buzz_db::project_view::ProjectViewFeatureStatus;
use buzz_db::Db;
use clap::{Args, Subcommand};

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
    }
    Ok(0)
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
    if target.all {
        let changed = db.set_all_project_views_enabled(enabled).await?;
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
        .set_project_view_enabled(status.community_id, enabled)
        .await?
    {
        bail!("Community host '{host}' became unavailable while acquiring its lock");
    }
    println!("{action} Project View for {host} ({})", status.community_id);
    Ok(())
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
