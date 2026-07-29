//! Trusted Runtime supervisor adapter for managed ACP harnesses.
//!
//! The ACP harness is the trusted boundary outside the model-facing Agent
//! process. A separately provisioned supervisor key submits operational
//! evidence; only the resulting runtime ID/epoch fence is inherited by Agent
//! children. The supervisor key and persisted recovery state path are removed
//! from the process environment before Tokio (and therefore any child process)
//! starts.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atomic_write_file::AtomicWriteFile;
use buzz_project_view::v2::{
    AssignmentRuntimeStatus, RuntimeAvailability, RuntimeEvidence, RuntimeEvidenceReceipt,
    RuntimeEvidenceRequest, RuntimeFence, RUNTIME_SUPERVISION_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::{Keys, PublicKey};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::relay::RestClient;

pub(crate) const SUPERVISOR_PRIVATE_KEY_ENV: &str = "BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY";
pub(crate) const SUPERVISION_STATE_PATH_ENV: &str = "BUZZ_RUNTIME_SUPERVISION_STATE_PATH";

const STATE_SCHEMA_VERSION: u16 = 1;
const EVIDENCE_PATH: &str = "/api/project-runtime/evidence";
const LEASE_RETRY_DELAY: Duration = Duration::from_secs(2);
const RECOVERY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);

/// Secret supervisor identity and pair-scoped durable recovery state.
pub(crate) struct RuntimeSupervisorConfig {
    keys: Keys,
    state_path: PathBuf,
}

impl RuntimeSupervisorConfig {
    /// Consume supervisor configuration before the Tokio runtime starts.
    ///
    /// Both variables are removed immediately so neither the secret nor the
    /// state-file capability can be inherited by Agent subprocesses.
    pub(crate) fn take_from_env() -> Result<Option<Self>, String> {
        let key = std::env::var_os(SUPERVISOR_PRIVATE_KEY_ENV);
        let state_path = std::env::var_os(SUPERVISION_STATE_PATH_ENV);
        // This function is called synchronously before Tokio creates worker
        // threads, which is the only safe point for process-env mutation.
        std::env::remove_var(SUPERVISOR_PRIVATE_KEY_ENV);
        std::env::remove_var(SUPERVISION_STATE_PATH_ENV);

        match (key, state_path) {
            (None, None) => Ok(None),
            (Some(_), None) => Err(format!(
                "{SUPERVISION_STATE_PATH_ENV} is required when \
                 {SUPERVISOR_PRIVATE_KEY_ENV} is configured"
            )),
            (None, Some(_)) => Err(format!(
                "{SUPERVISOR_PRIVATE_KEY_ENV} is required when \
                 {SUPERVISION_STATE_PATH_ENV} is configured"
            )),
            (Some(key), Some(state_path)) => {
                let mut key = key
                    .into_string()
                    .map_err(|_| format!("{SUPERVISOR_PRIVATE_KEY_ENV} contains invalid UTF-8"))?;
                let parsed = Keys::parse(key.trim())
                    .map_err(|error| format!("invalid {SUPERVISOR_PRIVATE_KEY_ENV}: {error}"));
                key.zeroize();
                let keys = parsed?;
                let state_path = PathBuf::from(state_path);
                if !state_path.is_absolute() {
                    return Err(format!(
                        "{SUPERVISION_STATE_PATH_ENV} must be an absolute path"
                    ));
                }
                Ok(Some(Self { keys, state_path }))
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct PersistedRuntimeState {
    schema_version: u16,
    assignment_id: Uuid,
    runtime_id: Uuid,
    runtime_epoch: u64,
    member_pubkey: String,
    supervisor_pubkey: String,
    relay_url: String,
}

impl PersistedRuntimeState {
    fn new(
        assignment_id: Uuid,
        receipt: &RuntimeEvidenceReceipt,
        member_pubkey: PublicKey,
        supervisor_pubkey: PublicKey,
        relay_url: &str,
    ) -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            assignment_id,
            runtime_id: receipt.runtime_id,
            runtime_epoch: receipt.runtime_epoch,
            member_pubkey: member_pubkey.to_hex(),
            supervisor_pubkey: supervisor_pubkey.to_hex(),
            relay_url: relay_url.to_owned(),
        }
    }

    fn validate_context(
        &self,
        member_pubkey: PublicKey,
        supervisor_pubkey: PublicKey,
        relay_url: &str,
    ) -> Result<(), String> {
        if self.schema_version != STATE_SCHEMA_VERSION
            || self.runtime_id.is_nil()
            || self.runtime_epoch == 0
        {
            return Err("persisted Runtime supervisor state is malformed".to_owned());
        }
        if self.member_pubkey != member_pubkey.to_hex()
            || self.supervisor_pubkey != supervisor_pubkey.to_hex()
            || self.relay_url != relay_url
        {
            return Err(
                "persisted Runtime supervisor state belongs to another identity or Relay"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeDecision {
    StartNew,
    RefuseUnownedRuntime,
    Recover {
        runtime_epoch: u64,
        availability: RuntimeAvailability,
        recovery_attempt_in_flight: bool,
    },
    RefuseUnavailable,
}

fn resume_decision(
    state: Option<&PersistedRuntimeState>,
    status: &AssignmentRuntimeStatus,
) -> ResumeDecision {
    let Some(state) = state else {
        return if status.runtimes.is_empty() {
            ResumeDecision::StartNew
        } else {
            ResumeDecision::RefuseUnownedRuntime
        };
    };
    let Some(runtime) = status
        .runtimes
        .iter()
        .find(|runtime| runtime.runtime_id == state.runtime_id)
    else {
        return if status.runtimes.is_empty() {
            ResumeDecision::StartNew
        } else {
            ResumeDecision::RefuseUnownedRuntime
        };
    };
    if runtime.availability == RuntimeAvailability::Unavailable {
        ResumeDecision::RefuseUnavailable
    } else {
        ResumeDecision::Recover {
            runtime_epoch: runtime.runtime_epoch,
            availability: runtime.availability,
            recovery_attempt_in_flight: runtime.recovery_attempt_in_flight,
        }
    }
}

/// Active Assignment-scoped runtime fence and its lease-renewal task.
pub(crate) struct RuntimeSupervisor {
    client: RestClient,
    state_path: PathBuf,
    state: PersistedRuntimeState,
    receipt: RuntimeEvidenceReceipt,
    recovery_attempt_in_flight: bool,
    lease_task: Option<tokio::task::JoinHandle<()>>,
}

impl RuntimeSupervisor {
    /// Enable supervision only when the verified current Assignment has an
    /// operator-installed binding. A managed binding without a configured
    /// supervisor fails closed instead of launching an unfenced Agent.
    pub(crate) async fn prepare(
        config: Option<RuntimeSupervisorConfig>,
        agent_client: &RestClient,
        member_pubkey: PublicKey,
        assignment_id: Option<Uuid>,
        relay_url: &str,
    ) -> Result<Option<Self>, String> {
        let Some(assignment_id) = assignment_id else {
            if let Some(config) = config {
                remove_state_if_present(&config.state_path)?;
            }
            return Ok(None);
        };
        let status = read_status(agent_client, assignment_id).await?;
        if status.assignment_id != assignment_id {
            return Err(format!(
                "Runtime status returned Assignment {}, expected {assignment_id}",
                status.assignment_id
            ));
        }
        if !status.managed {
            if let Some(config) = config {
                remove_state_if_present(&config.state_path)?;
            }
            return Ok(None);
        }

        let config = config.ok_or_else(|| {
            format!(
                "Assignment {assignment_id} is supervised, but \
                 {SUPERVISOR_PRIVATE_KEY_ENV} is not configured"
            )
        })?;
        let supervisor_pubkey = config.keys.public_key();
        if supervisor_pubkey == member_pubkey {
            return Err(
                "runtime supervisor identity must be distinct from the managed Agent identity"
                    .to_owned(),
            );
        }
        let mut client = agent_client.clone();
        client.keys = config.keys;
        client.auth_tag_json = None;

        let mut persisted = read_state(&config.state_path)?;
        if let Some(state) = persisted.as_ref() {
            state.validate_context(member_pubkey, supervisor_pubkey, relay_url)?;
            if state.assignment_id != assignment_id {
                // Assignment replacement revokes/fences its old binding in the
                // same Project transaction. Its local runtime coordinate must
                // never be carried into the successor tenure.
                remove_state_if_present(&config.state_path)?;
                persisted = None;
            }
        }

        let (receipt, recovery_attempt_in_flight) =
            match resume_decision(persisted.as_ref(), &status) {
                ResumeDecision::StartNew => {
                    let runtime_id = Uuid::new_v4();
                    let receipt = submit_evidence(
                        &client,
                        assignment_id,
                        runtime_id,
                        None,
                        RuntimeEvidence::Start,
                    )
                    .await?;
                    (receipt, false)
                }
                ResumeDecision::RefuseUnavailable => {
                    return Err(format!(
                        "Assignment {assignment_id} exhausted Runtime recovery; \
                         a new logical Runtime cannot bypass terminal evidence"
                    ));
                }
                ResumeDecision::RefuseUnownedRuntime => {
                    return Err(format!(
                        "Assignment {assignment_id} already has Runtime evidence that is not \
                         owned by this adapter state; refusing to mint a bypass Runtime"
                    ));
                }
                ResumeDecision::Recover {
                    runtime_epoch,
                    availability,
                    recovery_attempt_in_flight,
                } => {
                    let state = persisted.as_ref().ok_or_else(|| {
                        "Runtime recovery decision lost its persisted state".to_owned()
                    })?;
                    let mut receipt = if availability == RuntimeAvailability::Available {
                        submit_evidence(
                            &client,
                            assignment_id,
                            state.runtime_id,
                            Some(runtime_epoch),
                            RuntimeEvidence::AbnormalExit {
                                summary: Some(
                                    "previous managed ACP harness exited without graceful stop"
                                        .to_owned(),
                                ),
                                exit_code: None,
                            },
                        )
                        .await?
                    } else {
                        receipt_from_status(
                            assignment_id,
                            runtime_epoch,
                            &status,
                            state.runtime_id,
                        )?
                    };
                    if recovery_attempt_in_flight {
                        receipt = submit_evidence(
                            &client,
                            assignment_id,
                            state.runtime_id,
                            Some(receipt.runtime_epoch),
                            RuntimeEvidence::RecoveryFailed {
                                summary: Some(
                                    "supervisor restarted during the preceding recovery attempt"
                                        .to_owned(),
                                ),
                            },
                        )
                        .await?;
                    }
                    wait_until_recovery_allowed(
                        &client,
                        assignment_id,
                        state.runtime_id,
                        &mut receipt,
                    )
                    .await?;
                    let receipt = submit_evidence(
                        &client,
                        assignment_id,
                        state.runtime_id,
                        Some(receipt.runtime_epoch),
                        RuntimeEvidence::RecoveryAttempt,
                    )
                    .await?;
                    (receipt, true)
                }
            };

        let state = PersistedRuntimeState::new(
            assignment_id,
            &receipt,
            member_pubkey,
            supervisor_pubkey,
            relay_url,
        );
        if let Err(error) = write_state(&config.state_path, &state) {
            let _ = submit_evidence(
                &client,
                assignment_id,
                receipt.runtime_id,
                Some(receipt.runtime_epoch),
                RuntimeEvidence::GracefulStop,
            )
            .await;
            return Err(error);
        }
        Ok(Some(Self {
            client,
            state_path: config.state_path,
            state,
            receipt,
            recovery_attempt_in_flight,
            lease_task: None,
        }))
    }

    /// Fence inherited by every model-facing Agent process.
    pub(crate) fn fence(&self) -> RuntimeFence {
        RuntimeFence {
            runtime_id: self.state.runtime_id,
            runtime_epoch: self.state.runtime_epoch,
        }
    }

    /// Confirm that the replacement harness is healthy and begin lease renewal.
    pub(crate) async fn mark_healthy(&mut self) -> Result<(), String> {
        if self.recovery_attempt_in_flight {
            self.receipt = submit_evidence(
                &self.client,
                self.state.assignment_id,
                self.state.runtime_id,
                Some(self.state.runtime_epoch),
                RuntimeEvidence::RecoverySucceeded,
            )
            .await?;
            self.recovery_attempt_in_flight = false;
            self.state.runtime_epoch = self.receipt.runtime_epoch;
            write_state(&self.state_path, &self.state)?;
        }
        self.start_lease_task();
        Ok(())
    }

    /// Record a startup failure through the same bounded recovery policy.
    pub(crate) async fn mark_start_failed(&mut self, summary: String) -> Result<(), String> {
        if !self.recovery_attempt_in_flight {
            self.receipt = submit_evidence(
                &self.client,
                self.state.assignment_id,
                self.state.runtime_id,
                Some(self.state.runtime_epoch),
                RuntimeEvidence::AbnormalExit {
                    summary: Some(summary.clone()),
                    exit_code: None,
                },
            )
            .await?;
            wait_until_recovery_allowed(
                &self.client,
                self.state.assignment_id,
                self.state.runtime_id,
                &mut self.receipt,
            )
            .await?;
            self.receipt = submit_evidence(
                &self.client,
                self.state.assignment_id,
                self.state.runtime_id,
                Some(self.receipt.runtime_epoch),
                RuntimeEvidence::RecoveryAttempt,
            )
            .await?;
            self.state.runtime_epoch = self.receipt.runtime_epoch;
            self.recovery_attempt_in_flight = true;
            write_state(&self.state_path, &self.state)?;
        }
        self.receipt = submit_evidence(
            &self.client,
            self.state.assignment_id,
            self.state.runtime_id,
            Some(self.state.runtime_epoch),
            RuntimeEvidence::RecoveryFailed {
                summary: Some(summary),
            },
        )
        .await?;
        self.recovery_attempt_in_flight = false;
        Ok(())
    }

    /// Retire this runtime on a deliberate harness stop. Failure leaves the
    /// state file intact so the next trusted supervisor generation reconciles
    /// the uncertain predecessor instead of silently losing it.
    pub(crate) async fn graceful_stop(&mut self) -> Result<(), String> {
        if let Some(task) = self.lease_task.take() {
            task.abort();
        }
        submit_evidence(
            &self.client,
            self.state.assignment_id,
            self.state.runtime_id,
            Some(self.state.runtime_epoch),
            RuntimeEvidence::GracefulStop,
        )
        .await?;
        remove_state_if_present(&self.state_path)
    }

    /// Record an unexpected harness-level exit while retaining durable state
    /// for the replacement generation. The replacement, not this dying
    /// process, owns the subsequent recovery attempt and new epoch.
    pub(crate) async fn abnormal_stop(&mut self, summary: &str) -> Result<(), String> {
        if let Some(task) = self.lease_task.take() {
            task.abort();
        }
        self.receipt = submit_evidence(
            &self.client,
            self.state.assignment_id,
            self.state.runtime_id,
            Some(self.state.runtime_epoch),
            RuntimeEvidence::AbnormalExit {
                summary: Some(summary.to_owned()),
                exit_code: None,
            },
        )
        .await?;
        self.recovery_attempt_in_flight = false;
        Ok(())
    }

    fn start_lease_task(&mut self) {
        if self.lease_task.is_some() {
            return;
        }
        let client = self.client.clone();
        let assignment_id = self.state.assignment_id;
        let runtime_id = self.state.runtime_id;
        let runtime_epoch = self.state.runtime_epoch;
        let mut receipt = self.receipt.clone();
        self.lease_task = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(lease_renewal_delay(receipt.lease_expires_at)).await;
                match submit_evidence(
                    &client,
                    assignment_id,
                    runtime_id,
                    Some(runtime_epoch),
                    RuntimeEvidence::LeaseRenewed,
                )
                .await
                {
                    Ok(next) => receipt = next,
                    Err(error) => {
                        tracing::warn!(
                            assignment_id = %assignment_id,
                            "Runtime lease renewal failed closed: {error}"
                        );
                        tokio::time::sleep(LEASE_RETRY_DELAY).await;
                    }
                }
            }
        }));
    }
}

async fn read_status(
    client: &RestClient,
    assignment_id: Uuid,
) -> Result<AssignmentRuntimeStatus, String> {
    let value = client
        .get_authed(&format!(
            "/api/project-runtime/status?assignment_id={assignment_id}"
        ))
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_value(value)
        .map_err(|error| format!("invalid Runtime status response: {error}"))
}

async fn submit_evidence(
    client: &RestClient,
    assignment_id: Uuid,
    runtime_id: Uuid,
    runtime_epoch: Option<u64>,
    evidence: RuntimeEvidence,
) -> Result<RuntimeEvidenceReceipt, String> {
    let request = RuntimeEvidenceRequest {
        schema_version: RUNTIME_SUPERVISION_SCHEMA_VERSION,
        assignment_id,
        runtime_id,
        idempotency_key: Uuid::new_v4(),
        runtime_epoch,
        evidence,
    };
    request
        .validate()
        .map_err(|error| format!("invalid Runtime evidence: {error}"))?;
    let body = serde_json::to_value(request)
        .map_err(|error| format!("serialize Runtime evidence: {error}"))?;
    let value = client
        .post_authed_json(EVIDENCE_PATH, &body)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_value(value)
        .map_err(|error| format!("invalid Runtime evidence receipt: {error}"))
}

fn receipt_from_status(
    assignment_id: Uuid,
    runtime_epoch: u64,
    status: &AssignmentRuntimeStatus,
    runtime_id: Uuid,
) -> Result<RuntimeEvidenceReceipt, String> {
    let runtime = status
        .runtimes
        .iter()
        .find(|runtime| runtime.runtime_id == runtime_id)
        .ok_or_else(|| "persisted Runtime disappeared from status".to_owned())?;
    Ok(RuntimeEvidenceReceipt {
        assignment_id,
        runtime_id,
        runtime_epoch,
        availability: runtime.availability,
        lease_expires_at: runtime.lease_expires_at,
        recovery_deadline: runtime.recovery_deadline,
        recovery_attempts: runtime.recovery_attempts,
        recovery_attempt_in_flight: runtime.recovery_attempt_in_flight,
        next_recovery_at: runtime.next_recovery_at,
        // The adapter only needs to preserve server ordering here. The actual
        // maximum is returned by the next accepted evidence receipt.
        max_recovery_attempts: runtime.recovery_attempts.saturating_add(1),
        replayed: false,
    })
}

async fn wait_until_recovery_allowed(
    client: &RestClient,
    assignment_id: Uuid,
    runtime_id: Uuid,
    receipt: &mut RuntimeEvidenceReceipt,
) -> Result<(), String> {
    loop {
        if receipt.availability == RuntimeAvailability::Unavailable {
            return Err(
                "Runtime recovery is unavailable; refusing to bypass exhausted policy".to_owned(),
            );
        }
        let Some(eligible_at) = receipt.next_recovery_at else {
            return Ok(());
        };
        let remaining = eligible_at.signed_duration_since(Utc::now());
        let Ok(remaining) = remaining.to_std() else {
            return Ok(());
        };
        if remaining.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(RECOVERY_HEARTBEAT_INTERVAL)).await;
        if remaining > RECOVERY_HEARTBEAT_INTERVAL {
            *receipt = submit_evidence(
                client,
                assignment_id,
                runtime_id,
                Some(receipt.runtime_epoch),
                RuntimeEvidence::SupervisorHeartbeat,
            )
            .await?;
        }
    }
}

fn lease_renewal_delay(deadline: Option<DateTime<Utc>>) -> Duration {
    let remaining = deadline
        .and_then(|deadline| deadline.signed_duration_since(Utc::now()).to_std().ok())
        .unwrap_or(LEASE_RETRY_DELAY);
    remaining
        .checked_div(2)
        .unwrap_or(LEASE_RETRY_DELAY)
        .max(Duration::from_secs(1))
}

fn read_state(path: &Path) -> Result<Option<PersistedRuntimeState>, String> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|error| {
            format!(
                "invalid Runtime supervisor state {}: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "read Runtime supervisor state {}: {error}",
            path.display()
        )),
    }
}

fn write_state(path: &Path, state: &PersistedRuntimeState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Runtime supervisor state path has no parent".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create Runtime supervisor state directory {}: {error}",
            parent.display()
        )
    })?;
    let payload = serde_json::to_vec(state)
        .map_err(|error| format!("serialize Runtime supervisor state: {error}"))?;
    let mut file = AtomicWriteFile::open(path)
        .map_err(|error| format!("open Runtime supervisor state {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                format!(
                    "set Runtime supervisor state permissions {}: {error}",
                    path.display()
                )
            })?;
    }
    file.write_all(&payload)
        .map_err(|error| format!("write Runtime supervisor state {}: {error}", path.display()))?;
    file.commit().map_err(|error| {
        format!(
            "commit Runtime supervisor state {}: {error}",
            path.display()
        )
    })
}

fn remove_state_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "remove Runtime supervisor state {}: {error}",
            path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(
        runtime: Option<buzz_project_view::v2::RuntimeLeaseStatus>,
    ) -> AssignmentRuntimeStatus {
        AssignmentRuntimeStatus {
            assignment_id: Uuid::new_v4(),
            managed: true,
            availability: runtime.as_ref().map(|runtime| runtime.availability),
            runtimes: runtime.into_iter().collect(),
        }
    }

    fn state(runtime_id: Uuid) -> PersistedRuntimeState {
        PersistedRuntimeState {
            schema_version: STATE_SCHEMA_VERSION,
            assignment_id: Uuid::new_v4(),
            runtime_id,
            runtime_epoch: 1,
            member_pubkey: "11".repeat(32),
            supervisor_pubkey: "22".repeat(32),
            relay_url: "wss://relay.example".to_owned(),
        }
    }

    fn runtime(
        runtime_id: Uuid,
        availability: RuntimeAvailability,
        recovery_attempt_in_flight: bool,
    ) -> buzz_project_view::v2::RuntimeLeaseStatus {
        buzz_project_view::v2::RuntimeLeaseStatus {
            runtime_id,
            runtime_epoch: 7,
            availability,
            lease_expires_at: None,
            recovery_deadline: None,
            recovery_attempts: 1,
            recovery_attempt_in_flight,
            next_recovery_at: None,
            last_evidence_at: Utc::now(),
        }
    }

    #[test]
    fn resume_starts_a_fresh_runtime_when_no_live_coordinate_exists() {
        let runtime_id = Uuid::new_v4();
        assert_eq!(
            resume_decision(Some(&state(runtime_id)), &status(None)),
            ResumeDecision::StartNew
        );
    }

    #[test]
    fn missing_local_state_cannot_bypass_existing_runtime_evidence() {
        let runtime_id = Uuid::new_v4();
        assert_eq!(
            resume_decision(
                None,
                &status(Some(runtime(
                    runtime_id,
                    RuntimeAvailability::Unavailable,
                    false
                )))
            ),
            ResumeDecision::RefuseUnownedRuntime
        );
    }

    #[test]
    fn resume_uses_the_server_epoch_not_a_stale_local_epoch() {
        let runtime_id = Uuid::new_v4();
        assert_eq!(
            resume_decision(
                Some(&state(runtime_id)),
                &status(Some(runtime(
                    runtime_id,
                    RuntimeAvailability::Recovering,
                    true
                )))
            ),
            ResumeDecision::Recover {
                runtime_epoch: 7,
                availability: RuntimeAvailability::Recovering,
                recovery_attempt_in_flight: true,
            }
        );
    }

    #[test]
    fn exhausted_runtime_cannot_be_replaced_with_a_fresh_start() {
        let runtime_id = Uuid::new_v4();
        assert_eq!(
            resume_decision(
                Some(&state(runtime_id)),
                &status(Some(runtime(
                    runtime_id,
                    RuntimeAvailability::Unavailable,
                    false
                )))
            ),
            ResumeDecision::RefuseUnavailable
        );
    }

    #[test]
    fn persisted_state_round_trips_with_owner_only_permissions() {
        let directory = std::env::temp_dir().join(format!(
            "buzz-acp-runtime-supervisor-test-{}",
            Uuid::new_v4()
        ));
        let path = directory.join("state.json");
        let expected = state(Uuid::new_v4());

        write_state(&path, &expected).expect("write supervisor state");
        assert_eq!(
            read_state(&path).expect("read supervisor state"),
            Some(expected)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("read supervisor state metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        remove_state_if_present(&path).expect("remove supervisor state");
        std::fs::remove_dir(&directory).expect("remove supervisor state directory");
    }
}
