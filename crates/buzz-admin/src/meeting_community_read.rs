//! Audited operator cutover for Community-wide Meeting history reads.

use anyhow::{bail, Context, Result};
use buzz_core::{tenant::normalize_host, CommunityId};
use buzz_db::meeting_community_read::{
    MeetingCommunityReadStatus, MeetingSourceRiskCounts, MeetingVisibilityAudit,
};
use buzz_db::Db;
use clap::Subcommand;
use serde_json::{json, Value};

/// `buzz-admin meeting-community-read` operator commands.
#[derive(Debug, Subcommand)]
pub(crate) enum MeetingCommunityReadCommand {
    /// Show durable publication, audit, and Meeting Create state.
    Status {
        /// Limit output to one normalized Community host.
        #[arg(long)]
        community: Option<String>,
    },
    /// Pause new Meeting creation before a legacy corpus audit.
    PauseCreate {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
    /// Resume Meeting creation before publication and invalidate stale evidence.
    ResumeCreate {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
    /// Compute the complete legacy visibility digest while Create is paused.
    Audit {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
    /// Approve the exact watermark and digest returned by `audit`.
    Approve {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
        /// Exact legacy Meeting security-order watermark.
        #[arg(long)]
        watermark: u64,
        /// Exact 32-byte SHA-256 digest returned by `audit`.
        #[arg(long)]
        audit_digest: String,
        /// Durable operator identity, ticket, or approval reference.
        #[arg(long)]
        approved_by: String,
    },
    /// Atomically re-audit, publish Community reads, and resume Meeting Create.
    Enable {
        /// Exact normalized Community host.
        #[arg(long)]
        community: String,
    },
}

/// Execute one Meeting Community-read operator command.
pub(crate) async fn run(command: MeetingCommunityReadCommand) -> Result<i32> {
    let db = super::connect_db().await?;
    if !db.meeting_community_read_schema_ready().await? {
        bail!("Meeting Community-read schema is not ready; run buzz-admin migrate first");
    }
    match command {
        MeetingCommunityReadCommand::Status { community } => {
            show_status(&db, community.as_deref()).await?
        }
        MeetingCommunityReadCommand::PauseCreate { community } => {
            set_create_paused(&db, &community, true).await?
        }
        MeetingCommunityReadCommand::ResumeCreate { community } => {
            set_create_paused(&db, &community, false).await?
        }
        MeetingCommunityReadCommand::Audit { community } => audit(&db, &community).await?,
        MeetingCommunityReadCommand::Approve {
            community,
            watermark,
            audit_digest,
            approved_by,
        } => approve(&db, &community, watermark, &audit_digest, &approved_by).await?,
        MeetingCommunityReadCommand::Enable { community } => enable(&db, &community).await?,
    }
    Ok(0)
}

async fn show_status(db: &Db, community: Option<&str>) -> Result<()> {
    let mut statuses = db.list_meeting_community_read_statuses().await?;
    if let Some(community) = community {
        let (_, host) = resolve_community(db, community).await?;
        statuses.retain(|status| status.host == host);
    }
    let values: Vec<_> = statuses.iter().map(status_json).collect();
    println!("{}", serde_json::to_string_pretty(&values)?);
    Ok(())
}

async fn set_create_paused(db: &Db, community: &str, paused: bool) -> Result<()> {
    let (community_id, _) = resolve_community(db, community).await?;
    let status = db
        .set_meeting_community_read_create_paused(community_id, paused)
        .await?;
    println!("{}", serde_json::to_string_pretty(&status_json(&status))?);
    Ok(())
}

async fn audit(db: &Db, community: &str) -> Result<()> {
    let (community_id, host) = resolve_community(db, community).await?;
    let audit = db
        .audit_legacy_meeting_visibility(community_id)
        .await
        .context("audit legacy Meeting visibility")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&audit_json(&host, &audit))?
    );
    Ok(())
}

async fn approve(
    db: &Db,
    community: &str,
    watermark: u64,
    audit_digest: &str,
    approved_by: &str,
) -> Result<()> {
    let (community_id, host) = resolve_community(db, community).await?;
    let digest = parse_digest(audit_digest)?;
    let audit = db
        .approve_legacy_meeting_visibility(community_id, watermark, &digest, approved_by)
        .await
        .context("approve legacy Meeting visibility")?;
    let status = db
        .meeting_community_read_status(community_id)
        .await?
        .with_context(|| format!("Community host {host:?} disappeared after approval"))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "audit": audit_json(&host, &audit),
            "status": status_json(&status),
        }))?
    );
    Ok(())
}

async fn enable(db: &Db, community: &str) -> Result<()> {
    let (community_id, _) = resolve_community(db, community).await?;
    let status = db
        .enable_meeting_community_read(community_id)
        .await
        .context("publish Meeting Community-read contract")?;
    println!("{}", serde_json::to_string_pretty(&status_json(&status))?);
    Ok(())
}

async fn resolve_community(db: &Db, candidate: &str) -> Result<(CommunityId, String)> {
    let host = normalize_host(candidate);
    if host.is_empty() {
        bail!("--community must contain a valid Community host");
    }
    let record = db
        .lookup_community_by_host_for_management(&host)
        .await?
        .with_context(|| format!("Community host {host:?} is not registered"))?;
    Ok((record.id, record.host))
}

fn parse_digest(candidate: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(candidate.trim()).context("--audit-digest must be hexadecimal")?;
    bytes.try_into().map_err(|bytes: Vec<u8>| {
        anyhow::anyhow!(
            "--audit-digest must encode exactly 32 bytes, got {}",
            bytes.len()
        )
    })
}

fn status_json(status: &MeetingCommunityReadStatus) -> Value {
    json!({
        "community_id": status.community_id.to_string(),
        "host": status.host,
        "archived": status.archived,
        "enabled": status.enabled,
        "create_paused": status.create_paused,
        "watermark": status.watermark,
        "audit_digest": status.audit_digest.map(hex::encode),
        "meeting_count": status.meeting_count,
        "source_risks": status.source_risks.map(source_risks_json),
        "audited_at": status.audited_at,
        "approved_at": status.approved_at,
        "approved_by": status.approved_by,
        "enabled_at": status.enabled_at,
    })
}

fn audit_json(host: &str, audit: &MeetingVisibilityAudit) -> Value {
    json!({
        "community_id": audit.community_id.to_string(),
        "host": host,
        "watermark": audit.watermark,
        "audit_digest": hex::encode(audit.digest),
        "meeting_count": audit.meeting_count,
        "source_risks": source_risks_json(audit.source_risks),
    })
}

fn source_risks_json(risks: MeetingSourceRiskCounts) -> Value {
    json!({
        "community_wide": risks.community_wide,
        "private": risks.private,
        "missing": risks.missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(subcommand)]
        command: MeetingCommunityReadCommand,
    }

    #[test]
    fn operator_cli_shapes_are_closed() {
        assert!(matches!(
            TestCli::try_parse_from(["test", "status", "--community", "localhost:3000"])
                .expect("parse status")
                .command,
            MeetingCommunityReadCommand::Status { community: Some(_) }
        ));
        assert!(matches!(
            TestCli::try_parse_from(["test", "pause-create", "--community", "localhost:3000"])
                .expect("parse pause")
                .command,
            MeetingCommunityReadCommand::PauseCreate { .. }
        ));
        assert!(matches!(
            TestCli::try_parse_from([
                "test",
                "approve",
                "--community",
                "localhost:3000",
                "--watermark",
                "7",
                "--audit-digest",
                &"ab".repeat(32),
                "--approved-by",
                "operator:change-42",
            ])
            .expect("parse approval")
            .command,
            MeetingCommunityReadCommand::Approve { watermark: 7, .. }
        ));
        assert!(
            TestCli::try_parse_from(["test", "disable", "--community", "localhost:3000"]).is_err()
        );
    }

    #[test]
    fn digest_parser_is_exact_and_closed() {
        assert_eq!(parse_digest(&"ab".repeat(32)).expect("digest"), [0xab; 32]);
        assert!(parse_digest(&"ab".repeat(31)).is_err());
        assert!(parse_digest(&"ab".repeat(33)).is_err());
        assert!(parse_digest("not-hex").is_err());
    }
}
