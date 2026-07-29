//! `buzz runtime` — trusted supervisor evidence and availability reads.

use buzz_project_view::v2::{
    RuntimeEvidence, RuntimeEvidenceRequest, RUNTIME_SUPERVISION_SCHEMA_VERSION,
};
use uuid::Uuid;

use crate::client::BuzzClient;
use crate::error::CliError;
use crate::{RuntimeCmd, RuntimeEvidenceArg};

/// Dispatch trusted runtime-supervision operations.
pub async fn dispatch(command: RuntimeCmd, client: &BuzzClient) -> Result<(), CliError> {
    match command {
        RuntimeCmd::Evidence {
            evidence,
            assignment,
            runtime,
            epoch,
            idempotency_key,
            summary,
            exit_code,
        } => {
            let evidence = build_evidence(evidence, summary, exit_code)?;
            let request = RuntimeEvidenceRequest {
                schema_version: RUNTIME_SUPERVISION_SCHEMA_VERSION,
                assignment_id: assignment,
                runtime_id: runtime,
                idempotency_key: idempotency_key.unwrap_or_else(Uuid::new_v4),
                runtime_epoch: epoch,
                evidence,
            };
            request
                .validate()
                .map_err(|error| CliError::Usage(format!("invalid runtime evidence: {error}")))?;
            let body = serde_json::to_value(request)
                .map_err(|error| CliError::Other(format!("serialize runtime evidence: {error}")))?;
            let response = client
                .post_authed_json("/api/project-runtime/evidence", &body)
                .await?;
            println!("{response}");
            Ok(())
        }
        RuntimeCmd::Status { assignment } => {
            let response = client
                .get_authed(&format!(
                    "/api/project-runtime/status?assignment_id={assignment}"
                ))
                .await?;
            println!("{response}");
            Ok(())
        }
    }
}

fn build_evidence(
    evidence: RuntimeEvidenceArg,
    summary: Option<String>,
    exit_code: Option<i32>,
) -> Result<RuntimeEvidence, CliError> {
    match evidence {
        RuntimeEvidenceArg::Start => reject_diagnostics(summary, exit_code, RuntimeEvidence::Start),
        RuntimeEvidenceArg::LeaseRenewed => {
            reject_diagnostics(summary, exit_code, RuntimeEvidence::LeaseRenewed)
        }
        RuntimeEvidenceArg::GracefulStop => {
            reject_diagnostics(summary, exit_code, RuntimeEvidence::GracefulStop)
        }
        RuntimeEvidenceArg::AbnormalExit => {
            Ok(RuntimeEvidence::AbnormalExit { summary, exit_code })
        }
        RuntimeEvidenceArg::RecoveryAttempt => {
            reject_diagnostics(summary, exit_code, RuntimeEvidence::RecoveryAttempt)
        }
        RuntimeEvidenceArg::RecoverySucceeded => {
            reject_diagnostics(summary, exit_code, RuntimeEvidence::RecoverySucceeded)
        }
        RuntimeEvidenceArg::RecoveryFailed => {
            if exit_code.is_some() {
                return Err(CliError::Usage(
                    "--exit-code is only valid for abnormal_exit".to_owned(),
                ));
            }
            Ok(RuntimeEvidence::RecoveryFailed { summary })
        }
        RuntimeEvidenceArg::SupervisorHeartbeat => {
            reject_diagnostics(summary, exit_code, RuntimeEvidence::SupervisorHeartbeat)
        }
    }
}

fn reject_diagnostics(
    summary: Option<String>,
    exit_code: Option<i32>,
    evidence: RuntimeEvidence,
) -> Result<RuntimeEvidence, CliError> {
    if summary.is_some() || exit_code.is_some() {
        return Err(CliError::Usage(
            "--summary/--exit-code are not valid for this evidence type".to_owned(),
        ));
    }
    Ok(evidence)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_closed_by_evidence_type() {
        assert!(build_evidence(
            RuntimeEvidenceArg::LeaseRenewed,
            Some("no".to_owned()),
            None
        )
        .is_err());
        assert!(matches!(
            build_evidence(
                RuntimeEvidenceArg::RecoveryFailed,
                Some("still down".to_owned()),
                None
            ),
            Ok(RuntimeEvidence::RecoveryFailed { .. })
        ));
        assert!(matches!(
            build_evidence(RuntimeEvidenceArg::GracefulStop, None, None),
            Ok(RuntimeEvidence::GracefulStop)
        ));
        assert!(build_evidence(
            RuntimeEvidenceArg::GracefulStop,
            Some("not a failure".to_owned()),
            None
        )
        .is_err());
    }
}
