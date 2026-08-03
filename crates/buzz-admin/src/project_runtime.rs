//! Operator control plane for Assignment-scoped Runtime supervision.

use anyhow::{bail, Context as _, Result};
use buzz_core::tenant::normalize_host;
use buzz_project_view::v2::RuntimeRecoveryPolicy;
use clap::{Args, Subcommand};
use nostr::PublicKey;
use serde_json::json;
use uuid::Uuid;

/// `buzz-admin project-runtime` subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum ProjectRuntimeCommand {
    /// Show the canonical binding, policy, lease, and recovery state.
    Status(ProjectRuntimeTarget),
    /// Idempotently register one supervisor public key for an Assignment.
    Bind {
        #[command(flatten)]
        target: ProjectRuntimeTarget,
        /// Trusted supervisor public key (hex or npub). Never pass a private key.
        #[arg(long)]
        supervisor_pubkey: String,
        #[command(flatten)]
        policy: RuntimePolicyArgs,
        /// Audit actor public key. Defaults to the public key derived from
        /// `BUZZ_PRIVATE_KEY`.
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
    /// Revoke the active binding and end all of its current leases.
    Revoke {
        #[command(flatten)]
        target: ProjectRuntimeTarget,
        /// Audit actor public key. Defaults to the public key derived from
        /// `BUZZ_PRIVATE_KEY`.
        #[arg(long)]
        operator_pubkey: Option<String>,
    },
}

/// Exact Community and Assignment coordinate.
#[derive(Debug, Clone, Args)]
pub(crate) struct ProjectRuntimeTarget {
    /// Community host, for example `localhost:3000`.
    #[arg(long)]
    host: String,
    /// Active managed-Agent Assignment UUID.
    #[arg(long)]
    assignment: Uuid,
}

/// Bounded recovery policy. Defaults match the Relay protocol defaults.
#[derive(Debug, Clone, Args)]
pub(crate) struct RuntimePolicyArgs {
    #[arg(long, default_value_t = 60)]
    lease_seconds: u32,
    #[arg(long, default_value_t = 900)]
    recovery_window_seconds: u32,
    #[arg(long, default_value_t = 5)]
    max_recovery_attempts: u32,
    #[arg(long, default_value_t = 5)]
    recovery_backoff_seconds: u32,
    #[arg(long, default_value_t = 180)]
    monitor_timeout_seconds: u32,
    #[arg(long, default_value_t = 300)]
    monitor_grace_seconds: u32,
    #[arg(long, default_value_t = false)]
    automatic_unrecoverable: bool,
}

impl From<RuntimePolicyArgs> for RuntimeRecoveryPolicy {
    fn from(value: RuntimePolicyArgs) -> Self {
        Self {
            lease_seconds: value.lease_seconds,
            recovery_window_seconds: value.recovery_window_seconds,
            max_recovery_attempts: value.max_recovery_attempts,
            recovery_backoff_seconds: value.recovery_backoff_seconds,
            monitor_timeout_seconds: value.monitor_timeout_seconds,
            monitor_grace_seconds: value.monitor_grace_seconds,
            automatic_unrecoverable: value.automatic_unrecoverable,
        }
    }
}

pub(crate) async fn run(command: ProjectRuntimeCommand) -> Result<i32> {
    let db = super::connect_db().await?;
    match command {
        ProjectRuntimeCommand::Status(target) => {
            let (community, host) = resolve_community(&db, &target.host).await?;
            let status = db
                .assignment_runtime_status(community, target.assignment)
                .await
                .context("read Runtime supervision status")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "community_id": community.as_uuid().to_string(),
                    "host": host,
                    "status": status,
                }))?
            );
            Ok(0)
        }
        ProjectRuntimeCommand::Bind {
            target,
            supervisor_pubkey,
            policy,
            operator_pubkey,
        } => {
            let (community, host) = resolve_community(&db, &target.host).await?;
            let supervisor = parse_public_key(&supervisor_pubkey, "supervisor_pubkey")?;
            let operator = resolve_operator(operator_pubkey.as_deref())?;
            if supervisor == operator {
                bail!(
                    "supervisor identity must be distinct from the operator identity; prepare a dedicated key"
                );
            }
            let binding = db
                .register_runtime_supervisor(
                    community,
                    target.assignment,
                    supervisor,
                    operator,
                    policy.into(),
                )
                .await
                .context("register Runtime supervisor binding")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "community_id": community.as_uuid().to_string(),
                    "host": host,
                    "binding_id": binding.binding_id,
                    "assignment_id": binding.assignment_id,
                    "supervisor_pubkey": binding.supervisor_pubkey.to_hex(),
                    "policy": binding.policy,
                    "registered_at": binding.registered_at,
                }))?
            );
            Ok(0)
        }
        ProjectRuntimeCommand::Revoke {
            target,
            operator_pubkey,
        } => {
            let (community, host) = resolve_community(&db, &target.host).await?;
            let operator = resolve_operator(operator_pubkey.as_deref())?;
            let revoked = db
                .revoke_runtime_supervisor(community, target.assignment, operator)
                .await
                .context("revoke Runtime supervisor binding")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "community_id": community.as_uuid().to_string(),
                    "host": host,
                    "assignment_id": target.assignment,
                    "revoked": revoked,
                }))?
            );
            Ok(0)
        }
    }
}

async fn resolve_community(
    db: &buzz_db::Db,
    candidate: &str,
) -> Result<(buzz_core::CommunityId, String)> {
    let host = normalize_host(candidate);
    if host.is_empty() {
        bail!("--host must contain a valid Community host");
    }
    let record = db
        .lookup_community_by_host_for_management(&host)
        .await?
        .with_context(|| format!("Community host {host:?} is not registered"))?;
    Ok((record.id, record.host))
}

fn resolve_operator(candidate: Option<&str>) -> Result<PublicKey> {
    if let Some(candidate) = candidate {
        return parse_public_key(candidate, "operator_pubkey");
    }
    let private_key = std::env::var("BUZZ_PRIVATE_KEY")
        .context("--operator-pubkey or BUZZ_PRIVATE_KEY is required for Runtime mutations")?;
    nostr::Keys::parse(private_key.trim())
        .map(|keys| keys.public_key())
        .context("BUZZ_PRIVATE_KEY is invalid")
}

fn parse_public_key(candidate: &str, label: &str) -> Result<PublicKey> {
    PublicKey::parse(candidate.trim()).with_context(|| format!("invalid {label}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_matches_protocol_default() {
        let args = RuntimePolicyArgs {
            lease_seconds: 60,
            recovery_window_seconds: 900,
            max_recovery_attempts: 5,
            recovery_backoff_seconds: 5,
            monitor_timeout_seconds: 180,
            monitor_grace_seconds: 300,
            automatic_unrecoverable: false,
        };
        assert_eq!(
            RuntimeRecoveryPolicy::from(args),
            RuntimeRecoveryPolicy::default()
        );
    }
}
