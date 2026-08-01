//! Trusted Runtime supervisor adapter for managed ACP harnesses.
//!
//! The ACP harness is the trusted boundary outside the model-facing Agent
//! process. A separately provisioned supervisor key submits operational
//! evidence; Agent children inherit only a non-secret path to the current
//! runtime ID/epoch fence. The supervisor key and persisted recovery state path
//! are removed from the process environment before Tokio (and therefore any
//! child process) starts.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use atomic_write_file::AtomicWriteFile;
use buzz_project_view::v2::{
    AssignmentRuntimeStatus, RuntimeAvailability, RuntimeEvidence, RuntimeEvidenceReceipt,
    RuntimeEvidenceRequest, RuntimeFence, RUNTIME_SUPERVISION_SCHEMA_VERSION,
};
use buzz_project_view::v3::{
    MaintenanceAckCommand, MaintenanceAckRequest, MaintenanceRuntimeAckStatus,
    PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION,
};
use chrono::{DateTime, Utc};
use nostr::{Keys, PublicKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::{mpsc, oneshot, watch};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::relay::RestClient;

pub(crate) const SUPERVISOR_PRIVATE_KEY_ENV: &str = "BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY";
pub(crate) const SUPERVISION_STATE_PATH_ENV: &str = "BUZZ_RUNTIME_SUPERVISION_STATE_PATH";
pub(crate) const RUNTIME_FENCE_PATH_ENV: &str = "BUZZ_RUNTIME_FENCE_PATH";

const STATE_SCHEMA_VERSION: u16 = 1;
const EVIDENCE_PATH: &str = "/api/project-runtime/evidence";
const MAINTENANCE_PATH: &str = "/api/project-runtime/maintenance";
const MAINTENANCE_ACK_PATH: &str = "/api/project-runtime/maintenance/ack";
const MAINTENANCE_CLIENT_PROTOCOL_VERSION: u64 = 1;
const MAINTENANCE_CLIENT_BUILD: &str = concat!("buzz-acp/", env!("CARGO_PKG_VERSION"));
const MAINTENANCE_POLL_MIN: u64 = 1;
const MAINTENANCE_POLL_MAX: u64 = 60;
const MAINTENANCE_ACK_ID_DOMAIN: &[u8] = b"buzz-acp-maintenance-ack-id-v1\0";
const LEASE_RETRY_DELAY: Duration = Duration::from_secs(2);
const RECOVERY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const SUPERVISOR_COMMAND_CAPACITY: usize = 16;

/// Long-lived maintenance observation shared with the main-loop admission
/// gate. `ResumeRequired` remains closed until the old pool and durable child
/// registry have been discarded and a fresh Runtime generation is prepared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MaintenanceWatchState {
    Normal,
    Holding {
        state: String,
        maintenance_epoch: u64,
        poll_after_seconds: u64,
    },
    ResumeRequired {
        completed_epoch: Option<u64>,
        poll_after_seconds: u64,
    },
    Unavailable {
        detail: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceAckView {
    status: String,
    acked_at: Option<DateTime<Utc>>,
    ack_request_id: Option<Uuid>,
    canonical_request_hash: Option<String>,
    receipt: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceAssignmentBaseline {
    assignment_id: Uuid,
    member_pubkey: String,
    binding_id: Uuid,
    state_at_begin: String,
    last_polled_at: Option<DateTime<Utc>>,
    client_protocol_version: Option<u64>,
    client_build: Option<String>,
    ack: Option<MaintenanceAckView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceRuntimeBaseline {
    binding_id: Uuid,
    assignment_id: Uuid,
    runtime_id: Uuid,
    runtime_epoch: u64,
    availability_at_begin: String,
    ack: Option<MaintenanceAckView>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceEpochView {
    maintenance_epoch: u64,
    required_client_protocol_version: u64,
    outcome: String,
    requested_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    assignments: Vec<MaintenanceAssignmentBaseline>,
    runtimes: Vec<MaintenanceRuntimeBaseline>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceStatus {
    community_id: Uuid,
    host: String,
    state: String,
    current_epoch: Option<u64>,
    latest_epoch: Option<u64>,
    project_view_schema_version: u16,
    project_view_enabled: bool,
    archived: bool,
    poll_after_seconds: u64,
    epoch: Option<MaintenanceEpochView>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MaintenanceAckReceiptView {
    community_id: Uuid,
    maintenance_epoch: u64,
    ack_type: String,
    ack_request_id: Uuid,
    replayed: bool,
    result: Value,
}

/// Secret supervisor identity and pair-scoped durable recovery state.
pub(crate) struct RuntimeSupervisorConfig {
    keys: Keys,
    state_path: PathBuf,
    fence_path: PathBuf,
}

impl RuntimeSupervisorConfig {
    /// Consume supervisor configuration before the Tokio runtime starts.
    ///
    /// Both privileged variables and any ambient derived-fence path are
    /// removed immediately so Agent subprocesses receive only harness-issued
    /// values.
    pub(crate) fn take_from_env() -> Result<Option<Self>, String> {
        let key = std::env::var_os(SUPERVISOR_PRIVATE_KEY_ENV);
        let state_path = std::env::var_os(SUPERVISION_STATE_PATH_ENV);
        // This function is called synchronously before Tokio creates worker
        // threads, which is the only safe point for process-env mutation.
        std::env::remove_var(SUPERVISOR_PRIVATE_KEY_ENV);
        std::env::remove_var(SUPERVISION_STATE_PATH_ENV);
        std::env::remove_var(RUNTIME_FENCE_PATH_ENV);

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
                if state_path.to_str().is_none() {
                    return Err(format!(
                        "{SUPERVISION_STATE_PATH_ENV} must contain valid UTF-8"
                    ));
                }
                let fence_path = fresh_runtime_fence_path()?;
                Ok(Some(Self {
                    keys,
                    state_path,
                    fence_path,
                }))
            }
        }
    }

    /// Non-secret, Agent-readable file carrying the currently accepted fence.
    ///
    /// The generation-random temporary path cannot be transformed back into
    /// the durable recovery path held by the trusted harness. Agent processes
    /// may read or even corrupt this derived file, but Relay validation means
    /// doing so can only make their own writes fail; it cannot grant a
    /// different Assignment or epoch.
    pub(crate) fn fence_path(&self) -> PathBuf {
        self.fence_path.clone()
    }

    /// Pair-scoped durable registry used to prove that no model-facing child
    /// from an earlier harness generation survives maintenance.
    pub(crate) fn child_registry_path(&self) -> PathBuf {
        let mut path = self.state_path.as_os_str().to_os_string();
        path.push(".children.json");
        PathBuf::from(path)
    }
}

fn fresh_runtime_fence_path() -> Result<PathBuf, String> {
    let path = std::env::temp_dir()
        .join("buzz-runtime-fences")
        .join(format!("{}.json", Uuid::new_v4()));
    if !path.is_absolute() || path.to_str().is_none() {
        return Err("could not derive an absolute UTF-8 Runtime fence path".to_owned());
    }
    Ok(path)
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

fn runtime_is_current(
    state: &PersistedRuntimeState,
    status: &AssignmentRuntimeStatus,
    now: DateTime<Utc>,
) -> bool {
    status.assignment_id == state.assignment_id
        && status.managed
        && status.runtimes.iter().any(|runtime| {
            runtime.runtime_id == state.runtime_id
                && runtime.runtime_epoch == state.runtime_epoch
                && runtime.availability == RuntimeAvailability::Available
                && runtime
                    .lease_expires_at
                    .is_some_and(|deadline| deadline > now)
        })
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
        config: Option<&RuntimeSupervisorConfig>,
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
        client.keys = config.keys.clone();
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
            state_path: config.state_path.clone(),
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

    fn is_current(&self, status: &AssignmentRuntimeStatus) -> bool {
        runtime_is_current(&self.state, status, Utc::now())
    }

    fn pause(&mut self) {
        if let Some(task) = self.lease_task.take() {
            task.abort();
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
        self.pause();
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
        self.pause();
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

impl Drop for RuntimeSupervisor {
    fn drop(&mut self) {
        self.pause();
    }
}

/// Serializes dynamic Assignment/binding reconciliation for one ACP harness.
///
/// Recovery backoff can be much longer than a prompt-context timeout, so this
/// coordinator runs in its own task. Role Brief resolution awaits an
/// acknowledgement while the Relay main loop remains responsive.
pub(crate) struct RuntimeSupervisorCoordinator {
    config: Option<RuntimeSupervisorConfig>,
    agent_client: RestClient,
    maintenance_client: Option<RestClient>,
    member_pubkey: PublicKey,
    relay_url: String,
    current_assignment_id: Option<Uuid>,
    active: Option<RuntimeSupervisor>,
    lifecycle_gate: crate::role_brief::TurnLifecycleGate,
    maintenance_tx: watch::Sender<MaintenanceWatchState>,
    maintenance_latch: Option<u64>,
    maintenance_blocked: bool,
    poll_after: Duration,
}

impl RuntimeSupervisorCoordinator {
    pub(crate) fn new(
        config: Option<RuntimeSupervisorConfig>,
        agent_client: RestClient,
        member_pubkey: PublicKey,
        relay_url: String,
        lifecycle_gate: crate::role_brief::TurnLifecycleGate,
    ) -> Self {
        let maintenance_client = config.as_ref().map(|config| {
            let mut client = agent_client.clone();
            client.keys = config.keys.clone();
            client.auth_tag_json = None;
            client
        });
        let initial_state = if maintenance_client.is_some() {
            MaintenanceWatchState::Unavailable {
                detail: "maintenance status has not been checked".to_owned(),
            }
        } else {
            MaintenanceWatchState::Normal
        };
        let (maintenance_tx, _) = watch::channel(initial_state);
        Self {
            config,
            agent_client,
            maintenance_client,
            member_pubkey,
            relay_url,
            current_assignment_id: None,
            active: None,
            lifecycle_gate,
            maintenance_tx,
            maintenance_latch: None,
            maintenance_blocked: false,
            poll_after: Duration::from_secs(MAINTENANCE_POLL_MIN),
        }
    }

    /// Agent-readable path derived from the private pair-scoped state path.
    pub(crate) fn fence_path(&self) -> Option<PathBuf> {
        self.config
            .as_ref()
            .map(RuntimeSupervisorConfig::fence_path)
    }

    /// Perform the synchronous maintenance observation required before Role
    /// Brief resolution, Runtime Start/Recovery, or AgentPool creation. A
    /// restarted harness stays in maintenance-only mode and can finish its
    /// exact baseline acknowledgements without relying on the hidden Project
    /// View capability.
    pub(crate) async fn wait_for_maintenance_first_startup(&mut self) -> Result<(), String> {
        if self.maintenance_client.is_none() {
            let _ = self.maintenance_tx.send(MaintenanceWatchState::Normal);
            return Ok(());
        }
        loop {
            match self.poll_maintenance().await {
                Ok(MaintenanceWatchState::Normal) => return Ok(()),
                Ok(MaintenanceWatchState::ResumeRequired { .. }) => {
                    self.finish_maintenance_generation()?;
                    return Ok(());
                }
                Ok(MaintenanceWatchState::Holding {
                    state,
                    maintenance_epoch,
                    poll_after_seconds,
                }) => {
                    tracing::info!(
                        state,
                        maintenance_epoch,
                        "managed ACP startup is maintenance-only"
                    );
                    if state == "draining" {
                        if let Err(error) = self.acknowledge_latched_maintenance().await {
                            tracing::warn!(
                                maintenance_epoch,
                                "maintenance acknowledgement remains pending: {error}"
                            );
                        }
                    } else {
                        self.discard_retired_runtime_state()?;
                    }
                    tokio::time::sleep(Duration::from_secs(poll_after_seconds)).await;
                }
                Ok(MaintenanceWatchState::Unavailable { detail }) => {
                    tracing::warn!("maintenance-first startup poll failed closed: {detail}");
                    tokio::time::sleep(self.poll_after).await;
                }
                Err(error) => {
                    self.fail_maintenance_poll(&error);
                    tracing::warn!("maintenance-first startup poll failed closed: {error}");
                    tokio::time::sleep(self.poll_after).await;
                }
            }
        }
    }

    async fn poll_maintenance(&mut self) -> Result<MaintenanceWatchState, String> {
        let Some(client) = self.maintenance_client.as_ref() else {
            return Ok(MaintenanceWatchState::Normal);
        };
        let status = read_maintenance_status(client).await?;
        self.poll_after = Duration::from_secs(status.poll_after_seconds);
        match status.state.as_str() {
            "normal" => {
                if status.current_epoch.is_some() {
                    return Err("normal maintenance state carried a current epoch".to_owned());
                }
                let state = if self.maintenance_blocked {
                    MaintenanceWatchState::ResumeRequired {
                        completed_epoch: self.maintenance_latch.or(status.latest_epoch),
                        poll_after_seconds: status.poll_after_seconds,
                    }
                } else {
                    MaintenanceWatchState::Normal
                };
                let _ = self.maintenance_tx.send(state.clone());
                Ok(state)
            }
            "draining" | "frozen" => {
                let epoch = status.current_epoch.ok_or_else(|| {
                    format!("{} maintenance state has no current epoch", status.state)
                })?;
                let epoch_view = status.epoch.as_ref().ok_or_else(|| {
                    format!("{} maintenance state has no epoch body", status.state)
                })?;
                if epoch_view.maintenance_epoch != epoch {
                    return Err("maintenance pointer and epoch body disagree".to_owned());
                }
                if let Some(latched) = self.maintenance_latch {
                    if latched != epoch {
                        return Err(format!(
                            "maintenance epoch changed from latched {latched} to {epoch}"
                        ));
                    }
                } else {
                    self.maintenance_latch = Some(epoch);
                }
                self.maintenance_blocked = true;
                self.lifecycle_gate.cancel();
                self.pause_active();
                self.clear_fence()?;
                let state = MaintenanceWatchState::Holding {
                    state: status.state,
                    maintenance_epoch: epoch,
                    poll_after_seconds: status.poll_after_seconds,
                };
                let _ = self.maintenance_tx.send(state.clone());
                Ok(state)
            }
            state => Err(format!("unsupported maintenance state {state:?}")),
        }
    }

    fn fail_maintenance_poll(&mut self, detail: &str) {
        self.maintenance_blocked = true;
        self.lifecycle_gate.cancel();
        self.pause_active();
        let _ = self.clear_fence();
        let _ = self
            .maintenance_tx
            .send(MaintenanceWatchState::Unavailable {
                detail: detail.to_owned(),
            });
    }

    async fn acknowledge_latched_maintenance(&mut self) -> Result<(), String> {
        if !crate::child_registry::is_empty()? {
            return Err(
                "owned Agent child registry is not empty; refusing maintenance ACK".to_owned(),
            );
        }
        self.pause_active();
        self.clear_fence()?;
        let client = self
            .maintenance_client
            .as_ref()
            .ok_or_else(|| "maintenance supervisor identity is unavailable".to_owned())?;
        let status = read_maintenance_status(client).await?;
        let community_id = status.community_id;
        let epoch = self
            .maintenance_latch
            .ok_or_else(|| "maintenance epoch was not latched".to_owned())?;
        if status.state == "frozen" {
            verify_owned_acks(&status, self.member_pubkey, epoch)?;
            return self.discard_retired_runtime_state();
        }
        if status.state != "draining" || status.current_epoch != Some(epoch) {
            return Err("maintenance ACK no longer names the active draining epoch".to_owned());
        }
        let epoch_view = status
            .epoch
            .as_ref()
            .ok_or_else(|| "draining maintenance status omitted its epoch".to_owned())?;
        if epoch_view.required_client_protocol_version > MAINTENANCE_CLIENT_PROTOCOL_VERSION {
            return Err(format!(
                "maintenance protocol {} is required, but this ACP implements {}",
                epoch_view.required_client_protocol_version, MAINTENANCE_CLIENT_PROTOCOL_VERSION
            ));
        }
        let assignments = owned_assignments(epoch_view, self.member_pubkey)?;
        for assignment in &assignments {
            for runtime in epoch_view
                .runtimes
                .iter()
                .filter(|runtime| runtime.assignment_id == assignment.assignment_id)
            {
                if runtime.ack.is_some() {
                    continue;
                }
                let status = match runtime.availability_at_begin.as_str() {
                    "available" | "recovering" => MaintenanceRuntimeAckStatus::Suspended,
                    "unavailable" => MaintenanceRuntimeAckStatus::Terminal,
                    other => {
                        return Err(format!(
                            "unsupported baseline Runtime availability {other:?}"
                        ));
                    }
                };
                let command = MaintenanceAckCommand {
                    schema_version: PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION,
                    request: MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                        maintenance_epoch: epoch,
                        binding_id: runtime.binding_id,
                        assignment_id: runtime.assignment_id,
                        runtime_id: runtime.runtime_id,
                        runtime_epoch: runtime.runtime_epoch,
                        status,
                        idempotency_key: maintenance_runtime_ack_id(epoch, runtime, status),
                    },
                };
                submit_maintenance_ack(client, community_id, &command).await?;
            }
        }

        // Re-read exact durable Runtime receipts before claiming the Assignment
        // watcher is quiesced. This also resolves a lost POST response without
        // inventing a second idempotency coordinate.
        let status = read_maintenance_status(client).await?;
        if status.community_id != community_id {
            return Err("maintenance Community changed during Runtime ACK read-back".to_owned());
        }
        let epoch_view = status
            .epoch
            .as_ref()
            .ok_or_else(|| "maintenance status omitted its epoch after Runtime ACK".to_owned())?;
        let assignments = owned_assignments(epoch_view, self.member_pubkey)?;
        for assignment in &assignments {
            let runtime_pending = epoch_view.runtimes.iter().any(|runtime| {
                runtime.assignment_id == assignment.assignment_id && runtime.ack.is_none()
            });
            if runtime_pending {
                return Err(format!(
                    "Runtime acknowledgements remain pending for Assignment {}",
                    assignment.assignment_id
                ));
            }
            if assignment.ack.is_none() {
                let command = MaintenanceAckCommand {
                    schema_version: PROJECT_VIEW_MAINTENANCE_ACK_SCHEMA_VERSION,
                    request: MaintenanceAckRequest::AssignmentQuiesced {
                        maintenance_epoch: epoch,
                        binding_id: assignment.binding_id,
                        assignment_id: assignment.assignment_id,
                        client_protocol_version: MAINTENANCE_CLIENT_PROTOCOL_VERSION,
                        client_build: MAINTENANCE_CLIENT_BUILD.to_owned(),
                        idempotency_key: maintenance_assignment_ack_id(epoch, assignment),
                    },
                };
                submit_maintenance_ack(client, community_id, &command).await?;
            }
        }

        let verified = read_maintenance_status(client).await?;
        if verified.community_id != community_id {
            return Err("maintenance Community changed during Assignment ACK read-back".to_owned());
        }
        verify_owned_acks(&verified, self.member_pubkey, epoch)?;
        self.discard_retired_runtime_state()
    }

    fn discard_retired_runtime_state(&mut self) -> Result<(), String> {
        self.pause_active();
        self.clear_fence()?;
        if let Some(config) = &self.config {
            remove_state_if_present(&config.state_path)?;
        }
        Ok(())
    }

    fn finish_maintenance_generation(&mut self) -> Result<(), String> {
        if !crate::child_registry::is_empty()? {
            return Err("cannot resume while an owned Agent child remains registered".to_owned());
        }
        self.discard_retired_runtime_state()?;
        self.maintenance_latch = None;
        self.maintenance_blocked = false;
        self.lifecycle_gate.rotate_after_reap()?;
        let _ = self.maintenance_tx.send(MaintenanceWatchState::Normal);
        Ok(())
    }

    /// Reconcile startup before any model-facing child can receive work.
    pub(crate) async fn prepare_startup(
        &mut self,
        assignment_id: Option<Uuid>,
    ) -> Result<(), String> {
        if self.maintenance_blocked {
            return Err("Runtime admission is blocked by Project View maintenance".to_owned());
        }
        self.current_assignment_id = assignment_id;
        self.active = RuntimeSupervisor::prepare(
            self.config.as_ref(),
            &self.agent_client,
            self.member_pubkey,
            assignment_id,
            &self.relay_url,
        )
        .await?;
        self.publish_current_fence()
    }

    /// Mark the initialized harness healthy and publish its final fence.
    pub(crate) async fn mark_healthy(&mut self) -> Result<(), String> {
        if let Some(active) = self.active.as_mut() {
            active.mark_healthy().await?;
        }
        self.publish_current_fence()
    }

    /// Record eager pool startup failure and immediately withdraw local writes.
    pub(crate) async fn mark_start_failed(&mut self, summary: String) -> Result<(), String> {
        let fence_result = self.clear_fence();
        let evidence_result = match self.active.as_mut() {
            Some(active) => active.mark_start_failed(summary).await,
            None => Ok(()),
        };
        fence_result?;
        evidence_result
    }

    async fn reconcile(&mut self, assignment_id: Option<Uuid>) -> Result<(), String> {
        if self.maintenance_blocked {
            self.pause_active();
            self.clear_fence()?;
            return Err("Runtime admission is blocked by Project View maintenance".to_owned());
        }
        if assignment_id != self.current_assignment_id {
            self.pause_active();
            self.clear_fence()?;
            self.current_assignment_id = assignment_id;
        }

        let Some(assignment_id) = assignment_id else {
            self.pause_active();
            if let Some(config) = &self.config {
                remove_state_if_present(&config.state_path)?;
            }
            return self.clear_fence();
        };

        let status = match read_status(&self.agent_client, assignment_id).await {
            Ok(status) => status,
            Err(error) => {
                self.clear_fence()?;
                return Err(error);
            }
        };
        if status.assignment_id != assignment_id {
            self.clear_fence()?;
            return Err(format!(
                "Runtime status returned Assignment {}, expected {assignment_id}",
                status.assignment_id
            ));
        }
        if !status.managed {
            self.pause_active();
            if let Some(config) = &self.config {
                remove_state_if_present(&config.state_path)?;
            }
            return self.clear_fence();
        }
        if self.config.is_none() {
            self.pause_active();
            self.clear_fence()?;
            return Err(format!(
                "Assignment {assignment_id} is supervised, but \
                 {SUPERVISOR_PRIVATE_KEY_ENV} is not configured"
            ));
        }
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.is_current(&status))
        {
            return self.publish_current_fence();
        }

        self.pause_active();
        self.clear_fence()?;
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| "Runtime supervisor configuration disappeared".to_owned())?;
        let mut active = RuntimeSupervisor::prepare(
            Some(config),
            &self.agent_client,
            self.member_pubkey,
            Some(assignment_id),
            &self.relay_url,
        )
        .await?
        .ok_or_else(|| {
            format!("Assignment {assignment_id} changed supervision state during reconciliation")
        })?;
        active.mark_healthy().await?;
        self.active = Some(active);
        self.publish_current_fence()
    }

    fn suspend(&mut self) -> Result<(), String> {
        self.pause_active();
        self.clear_fence()
    }

    async fn acknowledge_after_quiesce(&mut self) -> Result<(), String> {
        self.acknowledge_latched_maintenance().await
    }

    async fn resume_after_maintenance(
        &mut self,
        assignment_id: Option<Uuid>,
    ) -> Result<(), String> {
        let client = self
            .maintenance_client
            .as_ref()
            .ok_or_else(|| "maintenance supervisor identity is unavailable".to_owned())?;
        let status = read_maintenance_status(client).await?;
        if status.state != "normal" || status.current_epoch.is_some() {
            return Err("maintenance has not been explicitly resumed or aborted".to_owned());
        }
        self.finish_maintenance_generation()?;
        if let Err(error) = self.prepare_startup(assignment_id).await {
            self.fail_maintenance_poll(&format!(
                "fresh Runtime preparation after maintenance failed: {error}"
            ));
            return Err(error);
        }
        Ok(())
    }

    async fn stop(&mut self, exit: RuntimeSupervisorExit) -> Result<(), String> {
        let fence_result = self.clear_fence();
        let evidence_result = match self.active.as_mut() {
            Some(active) => match exit {
                RuntimeSupervisorExit::Graceful => active.graceful_stop().await,
                RuntimeSupervisorExit::Abnormal(summary) => active.abnormal_stop(&summary).await,
            },
            None => Ok(()),
        };
        fence_result?;
        evidence_result
    }

    fn pause_active(&mut self) {
        if let Some(mut active) = self.active.take() {
            active.pause();
        }
    }

    fn publish_current_fence(&self) -> Result<(), String> {
        let Some(path) = self.fence_path() else {
            return Ok(());
        };
        match self.active.as_ref() {
            Some(active) => write_fence(&path, active.fence()),
            None => remove_file_if_present(&path, "Runtime fence"),
        }
    }

    fn clear_fence(&self) -> Result<(), String> {
        let Some(path) = self.fence_path() else {
            return Ok(());
        };
        remove_file_if_present(&path, "Runtime fence")
    }

    /// Move reconciliation into a dedicated worker and return its client.
    pub(crate) fn spawn(mut self) -> (RuntimeSupervisorClient, tokio::task::JoinHandle<()>) {
        let (sender, mut receiver) =
            mpsc::channel::<RuntimeSupervisorCommand>(SUPERVISOR_COMMAND_CAPACITY);
        let client = RuntimeSupervisorClient {
            sender,
            maintenance_rx: self.maintenance_tx.subscribe(),
        };
        let task = tokio::spawn(async move {
            let mut next_maintenance_poll = tokio::time::Instant::now() + self.poll_after;
            'worker: loop {
                tokio::select! {
                    biased;
                    _ = tokio::time::sleep_until(next_maintenance_poll), if self.maintenance_client.is_some() => {
                        if let Err(error) = self.poll_maintenance().await {
                            tracing::warn!("Runtime maintenance watcher failed closed: {error}");
                            self.fail_maintenance_poll(&error);
                        }
                        next_maintenance_poll = tokio::time::Instant::now() + self.poll_after;
                    }
                    command = receiver.recv() => {
                        let Some(command) = command else {
                            break;
                        };
                        match command {
                    RuntimeSupervisorCommand::Reconcile {
                        assignment_id,
                        response,
                    } => {
                        if response.is_closed() {
                            continue;
                        }
                        let result = self.reconcile(assignment_id).await;
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::Suspend { response } => {
                        let result = self.suspend();
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::AcknowledgeMaintenance { response } => {
                        let result = self.acknowledge_after_quiesce().await;
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::ResumeAfterMaintenance {
                        assignment_id,
                        response,
                    } => {
                        let result = self.resume_after_maintenance(assignment_id).await;
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::MarkHealthy { response } => {
                        let result = self.mark_healthy().await;
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::MarkStartFailed { summary, response } => {
                        let result = self.mark_start_failed(summary).await;
                        let _ = response.send(result);
                    }
                    RuntimeSupervisorCommand::Stop { exit, response } => {
                        let result = self.stop(exit).await;
                        let _ = response.send(result);
                        break 'worker;
                    }
                        }
                    }
                }
            }
            let _ = self.clear_fence();
            self.pause_active();
        });
        (client, task)
    }
}

impl Drop for RuntimeSupervisorCoordinator {
    fn drop(&mut self) {
        let _ = self.clear_fence();
        self.pause_active();
    }
}

#[derive(Debug)]
enum RuntimeSupervisorExit {
    Graceful,
    Abnormal(String),
}

#[derive(Debug)]
enum RuntimeSupervisorCommand {
    Reconcile {
        assignment_id: Option<Uuid>,
        response: oneshot::Sender<Result<(), String>>,
    },
    Suspend {
        response: oneshot::Sender<Result<(), String>>,
    },
    AcknowledgeMaintenance {
        response: oneshot::Sender<Result<(), String>>,
    },
    ResumeAfterMaintenance {
        assignment_id: Option<Uuid>,
        response: oneshot::Sender<Result<(), String>>,
    },
    MarkHealthy {
        response: oneshot::Sender<Result<(), String>>,
    },
    MarkStartFailed {
        summary: String,
        response: oneshot::Sender<Result<(), String>>,
    },
    Stop {
        exit: RuntimeSupervisorExit,
        response: oneshot::Sender<Result<(), String>>,
    },
}

/// Cloneable per-turn gate into the trusted Runtime coordinator.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeSupervisorClient {
    sender: mpsc::Sender<RuntimeSupervisorCommand>,
    maintenance_rx: watch::Receiver<MaintenanceWatchState>,
}

impl RuntimeSupervisorClient {
    pub(crate) fn maintenance_receiver(&self) -> watch::Receiver<MaintenanceWatchState> {
        self.maintenance_rx.clone()
    }

    pub(crate) async fn reconcile(&self, assignment_id: Option<Uuid>) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::Reconcile {
            assignment_id,
            response,
        })
        .await
    }

    pub(crate) async fn suspend(&self) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::Suspend { response })
            .await
    }

    pub(crate) async fn acknowledge_maintenance(&self) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::AcknowledgeMaintenance { response })
            .await
    }

    pub(crate) async fn resume_after_maintenance(
        &self,
        assignment_id: Option<Uuid>,
    ) -> Result<(), String> {
        self.request(
            |response| RuntimeSupervisorCommand::ResumeAfterMaintenance {
                assignment_id,
                response,
            },
        )
        .await
    }

    pub(crate) async fn mark_healthy(&self) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::MarkHealthy { response })
            .await
    }

    pub(crate) async fn mark_start_failed(&self, summary: String) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::MarkStartFailed { summary, response })
            .await
    }

    pub(crate) async fn graceful_stop(&self) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::Stop {
            exit: RuntimeSupervisorExit::Graceful,
            response,
        })
        .await
    }

    pub(crate) async fn abnormal_stop(&self, summary: &str) -> Result<(), String> {
        self.request(|response| RuntimeSupervisorCommand::Stop {
            exit: RuntimeSupervisorExit::Abnormal(summary.to_owned()),
            response,
        })
        .await
    }

    async fn request(
        &self,
        make_command: impl FnOnce(oneshot::Sender<Result<(), String>>) -> RuntimeSupervisorCommand,
    ) -> Result<(), String> {
        let (response, receiver) = oneshot::channel();
        self.sender
            .send(make_command(response))
            .await
            .map_err(|_| "Runtime supervisor worker is unavailable".to_owned())?;
        receiver
            .await
            .map_err(|_| "Runtime supervisor worker stopped before acknowledging".to_owned())?
    }
}

async fn read_maintenance_status(client: &RestClient) -> Result<MaintenanceStatus, String> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair(
            "client_protocol_version",
            &MAINTENANCE_CLIENT_PROTOCOL_VERSION.to_string(),
        )
        .append_pair("client_build", MAINTENANCE_CLIENT_BUILD)
        .finish();
    let value = client
        .get_authed(&format!("{MAINTENANCE_PATH}?{query}"))
        .await
        .map_err(|error| error.to_string())?;
    let status: MaintenanceStatus = serde_json::from_value(value)
        .map_err(|error| format!("invalid maintenance status response: {error}"))?;
    validate_maintenance_status(&status)?;
    Ok(status)
}

fn validate_maintenance_status(status: &MaintenanceStatus) -> Result<(), String> {
    if status.community_id.is_nil()
        || status.host.is_empty()
        || status.host.contains('\0')
        || !matches!(status.project_view_schema_version, 2 | 3)
        || !(MAINTENANCE_POLL_MIN..=MAINTENANCE_POLL_MAX).contains(&status.poll_after_seconds)
    {
        return Err("maintenance status header is malformed".to_owned());
    }
    if status.archived && status.project_view_enabled {
        return Err("archived Community advertised Project View enabled".to_owned());
    }
    match status.state.as_str() {
        "normal" if status.current_epoch.is_none() => {}
        "draining" | "frozen" if status.current_epoch.is_some() => {}
        "normal" | "draining" | "frozen" => {
            return Err("maintenance state and current_epoch disagree".to_owned());
        }
        _ => return Err("maintenance status contains an unknown state".to_owned()),
    }
    if let Some(epoch) = &status.epoch {
        if epoch.maintenance_epoch == 0
            || epoch.required_client_protocol_version == 0
            || !matches!(
                epoch.outcome.as_str(),
                "active" | "aborted" | "cutover_committed" | "resumed"
            )
        {
            return Err("maintenance epoch body is malformed".to_owned());
        }
        if status
            .current_epoch
            .is_some_and(|current| current != epoch.maintenance_epoch)
        {
            return Err("maintenance epoch body does not match current_epoch".to_owned());
        }
        if status
            .latest_epoch
            .is_some_and(|latest| latest < epoch.maintenance_epoch)
        {
            return Err("maintenance latest_epoch precedes the returned epoch".to_owned());
        }
        let _ = (&epoch.requested_at, &epoch.completed_at);
        for assignment in &epoch.assignments {
            if assignment.assignment_id.is_nil()
                || assignment.binding_id.is_nil()
                || !matches!(assignment.state_at_begin.as_str(), "idle" | "has_runtime")
                || assignment
                    .client_protocol_version
                    .is_some_and(|version| version == 0)
                || assignment.client_build.as_ref().is_some_and(|build| {
                    build.is_empty() || build.len() > 256 || build.contains('\0')
                })
                || PublicKey::parse(&assignment.member_pubkey).is_err()
            {
                return Err("maintenance Assignment baseline is malformed".to_owned());
            }
            let _ = &assignment.last_polled_at;
            if let Some(ack) = &assignment.ack {
                validate_ack_view(ack, &["quiesced"])?;
                validate_assignment_ack_receipt(
                    ack,
                    epoch.maintenance_epoch,
                    assignment,
                    epoch.required_client_protocol_version,
                )?;
            }
        }
        for runtime in &epoch.runtimes {
            if runtime.binding_id.is_nil()
                || runtime.assignment_id.is_nil()
                || runtime.runtime_id.is_nil()
                || runtime.runtime_epoch == 0
                || !matches!(
                    runtime.availability_at_begin.as_str(),
                    "available" | "recovering" | "unavailable"
                )
            {
                return Err("maintenance Runtime baseline is malformed".to_owned());
            }
            if let Some(ack) = &runtime.ack {
                validate_ack_view(ack, &["suspended", "terminal"])?;
                validate_runtime_ack_receipt(ack, epoch.maintenance_epoch, runtime)?;
            }
        }
    } else if status.current_epoch.is_some() {
        return Err("current maintenance epoch has no body".to_owned());
    }
    Ok(())
}

fn validate_ack_view(ack: &MaintenanceAckView, allowed: &[&str]) -> Result<(), String> {
    if !allowed.contains(&ack.status.as_str())
        || ack.acked_at.is_none()
        || ack.ack_request_id.is_none_or(|id| id.is_nil())
        || ack.canonical_request_hash.as_ref().is_none_or(|hash| {
            hash.len() != 64
                || !hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        || ack.receipt.is_none()
    {
        return Err("maintenance acknowledgement view is malformed".to_owned());
    }
    Ok(())
}

fn validate_assignment_ack_receipt(
    ack: &MaintenanceAckView,
    maintenance_epoch: u64,
    assignment: &MaintenanceAssignmentBaseline,
    required_client_protocol_version: u64,
) -> Result<(), String> {
    let result = ack
        .receipt
        .as_ref()
        .ok_or_else(|| "maintenance Assignment acknowledgement omitted its receipt".to_owned())?;
    let object = result.as_object().ok_or_else(|| {
        "maintenance Assignment acknowledgement receipt is not an object".to_owned()
    })?;
    let protocol = result
        .get("client_protocol_version")
        .and_then(Value::as_u64);
    let build = result.get("client_build").and_then(Value::as_str);
    if object.len() != 7
        || result.get("maintenance_epoch").and_then(Value::as_u64) != Some(maintenance_epoch)
        || result.get("type").and_then(Value::as_str) != Some("assignment_quiesced")
        || result_uuid(result, "binding_id") != Some(assignment.binding_id)
        || result_uuid(result, "assignment_id") != Some(assignment.assignment_id)
        || result.get("status").and_then(Value::as_str) != Some("quiesced")
        || protocol.is_none_or(|version| version < required_client_protocol_version)
        || build.is_none_or(|value| value.is_empty() || value.len() > 256 || value.contains('\0'))
    {
        return Err("maintenance Assignment acknowledgement receipt is malformed".to_owned());
    }
    Ok(())
}

fn validate_runtime_ack_receipt(
    ack: &MaintenanceAckView,
    maintenance_epoch: u64,
    runtime: &MaintenanceRuntimeBaseline,
) -> Result<(), String> {
    let result = ack
        .receipt
        .as_ref()
        .ok_or_else(|| "maintenance Runtime acknowledgement omitted its receipt".to_owned())?;
    let object = result
        .as_object()
        .ok_or_else(|| "maintenance Runtime acknowledgement receipt is not an object".to_owned())?;
    let expected_status = match runtime.availability_at_begin.as_str() {
        "available" | "recovering" => "suspended",
        "unavailable" => "terminal",
        _ => return Err("maintenance Runtime baseline is malformed".to_owned()),
    };
    if object.len() != 7
        || result.get("maintenance_epoch").and_then(Value::as_u64) != Some(maintenance_epoch)
        || result.get("type").and_then(Value::as_str) != Some("runtime_suspended_or_terminal")
        || result_uuid(result, "binding_id") != Some(runtime.binding_id)
        || result_uuid(result, "assignment_id") != Some(runtime.assignment_id)
        || result_uuid(result, "runtime_id") != Some(runtime.runtime_id)
        || result.get("runtime_epoch").and_then(Value::as_u64) != Some(runtime.runtime_epoch)
        || result.get("status").and_then(Value::as_str) != Some(expected_status)
        || ack.status != expected_status
    {
        return Err("maintenance Runtime acknowledgement receipt is malformed".to_owned());
    }
    Ok(())
}

fn result_uuid(result: &Value, field: &str) -> Option<Uuid> {
    result
        .get(field)
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
}

fn owned_assignments(
    epoch: &MaintenanceEpochView,
    member_pubkey: PublicKey,
) -> Result<Vec<&MaintenanceAssignmentBaseline>, String> {
    let mut assignments = Vec::new();
    for assignment in &epoch.assignments {
        let member = PublicKey::parse(&assignment.member_pubkey)
            .map_err(|error| format!("invalid baseline member pubkey: {error}"))?;
        if member == member_pubkey {
            assignments.push(assignment);
        }
    }
    if assignments.len() > 1 {
        return Err("member has more than one active maintenance Assignment baseline".to_owned());
    }
    Ok(assignments)
}

fn verify_owned_acks(
    status: &MaintenanceStatus,
    member_pubkey: PublicKey,
    expected_epoch: u64,
) -> Result<(), String> {
    let epoch = status
        .epoch
        .as_ref()
        .ok_or_else(|| "maintenance status omitted the exact epoch".to_owned())?;
    if epoch.maintenance_epoch != expected_epoch {
        return Err("maintenance status returned another epoch".to_owned());
    }
    for assignment in owned_assignments(epoch, member_pubkey)? {
        if assignment.ack.as_ref().map(|ack| ack.status.as_str()) != Some("quiesced") {
            return Err(format!(
                "Assignment {} has no durable quiesced acknowledgement",
                assignment.assignment_id
            ));
        }
        if epoch.runtimes.iter().any(|runtime| {
            runtime.assignment_id == assignment.assignment_id && runtime.ack.is_none()
        }) {
            return Err(format!(
                "Assignment {} has an unacknowledged Runtime baseline",
                assignment.assignment_id
            ));
        }
    }
    Ok(())
}

async fn submit_maintenance_ack(
    client: &RestClient,
    expected_community_id: Uuid,
    command: &MaintenanceAckCommand,
) -> Result<(), String> {
    command
        .validate()
        .map_err(|error| format!("invalid maintenance acknowledgement: {error}"))?;
    let body = serde_json::to_value(command)
        .map_err(|error| format!("serialize maintenance acknowledgement: {error}"))?;
    let value = client
        .post_authed_json(MAINTENANCE_ACK_PATH, &body)
        .await
        .map_err(|error| error.to_string())?;
    let receipt: MaintenanceAckReceiptView = serde_json::from_value(value)
        .map_err(|error| format!("invalid maintenance acknowledgement receipt: {error}"))?;
    if receipt.community_id != expected_community_id
        || receipt.maintenance_epoch != command.maintenance_epoch()
        || receipt.ack_type != command.ack_type()
        || receipt.ack_request_id.is_nil()
        || !maintenance_ack_result_matches(&receipt.result, command)
    {
        return Err("maintenance acknowledgement receipt does not match the request".to_owned());
    }
    let _ = receipt.replayed;
    Ok(())
}

fn maintenance_ack_result_matches(result: &Value, command: &MaintenanceAckCommand) -> bool {
    if result.get("maintenance_epoch").and_then(Value::as_u64) != Some(command.maintenance_epoch())
    {
        return false;
    }
    match &command.request {
        MaintenanceAckRequest::AssignmentQuiesced {
            binding_id,
            assignment_id,
            client_protocol_version,
            client_build,
            ..
        } => {
            result.as_object().is_some_and(|object| object.len() == 7)
                && result.get("type").and_then(Value::as_str) == Some("assignment_quiesced")
                && result_uuid(result, "binding_id") == Some(*binding_id)
                && result_uuid(result, "assignment_id") == Some(*assignment_id)
                && result.get("status").and_then(Value::as_str) == Some("quiesced")
                && result
                    .get("client_protocol_version")
                    .and_then(Value::as_u64)
                    == Some(*client_protocol_version)
                && result.get("client_build").and_then(Value::as_str) == Some(client_build.as_str())
        }
        MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
            binding_id,
            assignment_id,
            runtime_id,
            runtime_epoch,
            status,
            ..
        } => {
            result.as_object().is_some_and(|object| object.len() == 7)
                && result.get("type").and_then(Value::as_str)
                    == Some("runtime_suspended_or_terminal")
                && result_uuid(result, "binding_id") == Some(*binding_id)
                && result_uuid(result, "assignment_id") == Some(*assignment_id)
                && result_uuid(result, "runtime_id") == Some(*runtime_id)
                && result.get("runtime_epoch").and_then(Value::as_u64) == Some(*runtime_epoch)
                && result.get("status").and_then(Value::as_str) == Some(status.as_str())
        }
    }
}

fn maintenance_assignment_ack_id(
    maintenance_epoch: u64,
    assignment: &MaintenanceAssignmentBaseline,
) -> Uuid {
    deterministic_ack_id(&[
        b"assignment",
        &maintenance_epoch.to_be_bytes(),
        assignment.binding_id.as_bytes(),
        assignment.assignment_id.as_bytes(),
    ])
}

fn maintenance_runtime_ack_id(
    maintenance_epoch: u64,
    runtime: &MaintenanceRuntimeBaseline,
    status: MaintenanceRuntimeAckStatus,
) -> Uuid {
    deterministic_ack_id(&[
        b"runtime",
        &maintenance_epoch.to_be_bytes(),
        runtime.binding_id.as_bytes(),
        runtime.assignment_id.as_bytes(),
        runtime.runtime_id.as_bytes(),
        &runtime.runtime_epoch.to_be_bytes(),
        status.as_str().as_bytes(),
    ])
}

fn deterministic_ack_id(parts: &[&[u8]]) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(MAINTENANCE_ACK_ID_DOMAIN);
    for part in parts {
        digest.update((part.len() as u64).to_be_bytes());
        digest.update(part);
    }
    let hash = digest.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
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
    remove_file_if_present(path, "Runtime supervisor state")
}

fn write_fence(path: &Path, fence: RuntimeFence) -> Result<(), String> {
    fence
        .validate()
        .map_err(|error| format!("invalid Runtime fence: {error}"))?;
    let parent = path
        .parent()
        .ok_or_else(|| "Runtime fence path has no parent".to_owned())?;
    std::fs::create_dir_all(parent).map_err(|error| {
        format!(
            "create Runtime fence directory {}: {error}",
            parent.display()
        )
    })?;
    let payload =
        serde_json::to_vec(&fence).map_err(|error| format!("serialize Runtime fence: {error}"))?;
    let mut file = AtomicWriteFile::open(path)
        .map_err(|error| format!("open Runtime fence {}: {error}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| {
                format!("set Runtime fence permissions {}: {error}", path.display())
            })?;
    }
    file.write_all(&payload)
        .map_err(|error| format!("write Runtime fence {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("commit Runtime fence {}: {error}", path.display()))
}

fn remove_file_if_present(path: &Path, label: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("remove {label} {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[derive(Default)]
    struct MockRuntimeApi {
        assignment_id: Uuid,
        managed: bool,
        runtime: Option<buzz_project_view::v2::RuntimeLeaseStatus>,
        evidence: Vec<String>,
        maintenance: Option<MockMaintenanceApi>,
    }

    struct MockMaintenanceApi {
        community_id: Uuid,
        member_pubkey: String,
        binding_id: Uuid,
        assignment_id: Uuid,
        runtime_id: Uuid,
        runtime_epoch: u64,
        runtime_acked: bool,
        assignment_acked: bool,
        ack_order: Vec<String>,
    }

    fn mock_ack_view(status: &str, request_id: Uuid, receipt: Value) -> Value {
        json!({
            "status": status,
            "acked_at": Utc::now(),
            "ack_request_id": request_id,
            "canonical_request_hash": "ab".repeat(32),
            "receipt": receipt,
        })
    }

    fn mock_maintenance_status(state: &MockRuntimeApi) -> Value {
        let Some(maintenance) = state.maintenance.as_ref() else {
            return json!({
                "community_id": Uuid::from_u128(1),
                "host": "buzz.example",
                "state": "normal",
                "current_epoch": null,
                "latest_epoch": null,
                "project_view_schema_version": 2,
                "project_view_enabled": true,
                "archived": false,
                "poll_after_seconds": 5,
                "epoch": null,
            });
        };
        json!({
            "community_id": maintenance.community_id,
            "host": "buzz.example",
            "state": "draining",
            "current_epoch": 7,
            "latest_epoch": 7,
            "project_view_schema_version": 2,
            "project_view_enabled": false,
            "archived": false,
            "poll_after_seconds": 5,
            "epoch": {
                "maintenance_epoch": 7,
                "required_client_protocol_version": MAINTENANCE_CLIENT_PROTOCOL_VERSION,
                "outcome": "active",
                "requested_at": Utc::now(),
                "completed_at": null,
                "assignments": [{
                    "assignment_id": maintenance.assignment_id,
                    "member_pubkey": maintenance.member_pubkey,
                    "binding_id": maintenance.binding_id,
                    "state_at_begin": "has_runtime",
                    "last_polled_at": Utc::now(),
                    "client_protocol_version": MAINTENANCE_CLIENT_PROTOCOL_VERSION,
                    "client_build": MAINTENANCE_CLIENT_BUILD,
                    "ack": maintenance.assignment_acked.then(|| {
                        mock_ack_view("quiesced", Uuid::from_u128(12), json!({
                            "maintenance_epoch": 7,
                            "type": "assignment_quiesced",
                            "binding_id": maintenance.binding_id,
                            "assignment_id": maintenance.assignment_id,
                            "status": "quiesced",
                            "client_protocol_version": MAINTENANCE_CLIENT_PROTOCOL_VERSION,
                            "client_build": MAINTENANCE_CLIENT_BUILD,
                        }))
                    }),
                }],
                "runtimes": [{
                    "binding_id": maintenance.binding_id,
                    "assignment_id": maintenance.assignment_id,
                    "runtime_id": maintenance.runtime_id,
                    "runtime_epoch": maintenance.runtime_epoch,
                    "availability_at_begin": "available",
                    "ack": maintenance.runtime_acked.then(|| {
                        mock_ack_view("suspended", Uuid::from_u128(11), json!({
                            "maintenance_epoch": 7,
                            "type": "runtime_suspended_or_terminal",
                            "binding_id": maintenance.binding_id,
                            "assignment_id": maintenance.assignment_id,
                            "runtime_id": maintenance.runtime_id,
                            "runtime_epoch": maintenance.runtime_epoch,
                            "status": "suspended",
                        }))
                    }),
                }],
            },
        })
    }

    fn apply_mock_maintenance_ack(
        state: &mut MockRuntimeApi,
        command: &MaintenanceAckCommand,
    ) -> Value {
        let maintenance = state
            .maintenance
            .as_mut()
            .expect("maintenance ACK requires a fixture");
        let (request_id, result) = match &command.request {
            MaintenanceAckRequest::RuntimeSuspendedOrTerminal {
                maintenance_epoch,
                binding_id,
                assignment_id,
                runtime_id,
                runtime_epoch,
                status,
                ..
            } => {
                assert_eq!(*maintenance_epoch, 7);
                assert_eq!(*binding_id, maintenance.binding_id);
                assert_eq!(*assignment_id, maintenance.assignment_id);
                assert_eq!(*runtime_id, maintenance.runtime_id);
                assert_eq!(*runtime_epoch, maintenance.runtime_epoch);
                assert!(!maintenance.assignment_acked);
                maintenance.runtime_acked = true;
                maintenance.ack_order.push("runtime".to_owned());
                (
                    Uuid::from_u128(11),
                    json!({
                        "maintenance_epoch": maintenance_epoch,
                        "type": "runtime_suspended_or_terminal",
                        "binding_id": binding_id,
                        "assignment_id": assignment_id,
                        "runtime_id": runtime_id,
                        "runtime_epoch": runtime_epoch,
                        "status": status.as_str(),
                    }),
                )
            }
            MaintenanceAckRequest::AssignmentQuiesced {
                maintenance_epoch,
                binding_id,
                assignment_id,
                client_protocol_version,
                client_build,
                ..
            } => {
                assert_eq!(*maintenance_epoch, 7);
                assert_eq!(*binding_id, maintenance.binding_id);
                assert_eq!(*assignment_id, maintenance.assignment_id);
                assert!(maintenance.runtime_acked);
                maintenance.assignment_acked = true;
                maintenance.ack_order.push("assignment".to_owned());
                (
                    Uuid::from_u128(12),
                    json!({
                        "maintenance_epoch": maintenance_epoch,
                        "type": "assignment_quiesced",
                        "binding_id": binding_id,
                        "assignment_id": assignment_id,
                        "status": "quiesced",
                        "client_protocol_version": client_protocol_version,
                        "client_build": client_build,
                    }),
                )
            }
        };
        json!({
            "community_id": maintenance.community_id,
            "maintenance_epoch": command.maintenance_epoch(),
            "ack_type": command.ack_type(),
            "ack_request_id": request_id,
            "replayed": false,
            "result": result,
        })
    }

    async fn mock_runtime_client(
        state: Arc<Mutex<MockRuntimeApi>>,
        member_keys: Keys,
    ) -> (RestClient, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock Runtime API");
        let base_url = format!(
            "http://{}",
            listener.local_addr().expect("mock API address")
        );
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    let mut request = Vec::new();
                    let mut buffer = [0_u8; 4096];
                    let (header_end, content_length) = loop {
                        let read = socket.read(&mut buffer).await.expect("read mock request");
                        if read == 0 {
                            return;
                        }
                        request.extend_from_slice(&buffer[..read]);
                        let Some(header_end) =
                            request.windows(4).position(|window| window == b"\r\n\r\n")
                        else {
                            continue;
                        };
                        let header_end = header_end + 4;
                        let headers = String::from_utf8_lossy(&request[..header_end]);
                        let content_length = headers
                            .lines()
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        if request.len() >= header_end + content_length {
                            break (header_end, content_length);
                        }
                    };
                    let request_line = String::from_utf8_lossy(&request)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .to_owned();
                    let body = &request[header_end..header_end + content_length];
                    let response = if request_line
                        .starts_with("GET /api/project-runtime/maintenance?")
                    {
                        let state = state.lock().expect("lock mock Runtime API");
                        mock_maintenance_status(&state)
                    } else if request_line.starts_with("GET ") {
                        let state = state.lock().expect("lock mock Runtime API");
                        serde_json::to_value(AssignmentRuntimeStatus {
                            assignment_id: state.assignment_id,
                            managed: state.managed,
                            availability: if state.managed {
                                Some(
                                    state
                                        .runtime
                                        .as_ref()
                                        .map_or(RuntimeAvailability::Unavailable, |runtime| {
                                            runtime.availability
                                        }),
                                )
                            } else {
                                None
                            },
                            runtimes: state.runtime.iter().cloned().collect(),
                        })
                        .expect("serialize Runtime status")
                    } else if request_line.starts_with("POST /api/project-runtime/maintenance/ack ")
                    {
                        let command: MaintenanceAckCommand =
                            serde_json::from_slice(body).expect("parse maintenance ACK");
                        let mut state = state.lock().expect("lock mock Runtime API");
                        apply_mock_maintenance_ack(&mut state, &command)
                    } else {
                        let request: RuntimeEvidenceRequest =
                            serde_json::from_slice(body).expect("parse Runtime evidence");
                        let evidence_type = request.evidence.as_str().to_owned();
                        let runtime_epoch = request.runtime_epoch.unwrap_or(1);
                        let lease_expires_at = Utc::now() + chrono::Duration::minutes(30);
                        let mut state = state.lock().expect("lock mock Runtime API");
                        state.evidence.push(evidence_type);
                        match request.evidence {
                            RuntimeEvidence::Start
                            | RuntimeEvidence::LeaseRenewed
                            | RuntimeEvidence::RecoverySucceeded => {
                                state.runtime = Some(buzz_project_view::v2::RuntimeLeaseStatus {
                                    runtime_id: request.runtime_id,
                                    runtime_epoch,
                                    availability: RuntimeAvailability::Available,
                                    lease_expires_at: Some(lease_expires_at),
                                    recovery_deadline: None,
                                    recovery_attempts: 0,
                                    recovery_attempt_in_flight: false,
                                    next_recovery_at: None,
                                    last_evidence_at: Utc::now(),
                                });
                            }
                            RuntimeEvidence::GracefulStop => state.runtime = None,
                            _ => panic!("unexpected evidence in dynamic reconciliation test"),
                        }
                        serde_json::to_value(RuntimeEvidenceReceipt {
                            assignment_id: request.assignment_id,
                            runtime_id: request.runtime_id,
                            runtime_epoch,
                            availability: RuntimeAvailability::Available,
                            lease_expires_at: Some(lease_expires_at),
                            recovery_deadline: None,
                            recovery_attempts: 0,
                            recovery_attempt_in_flight: false,
                            next_recovery_at: None,
                            max_recovery_attempts: 3,
                            replayed: false,
                        })
                        .expect("serialize Runtime receipt")
                    };
                    let body = response.to_string();
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    socket
                        .write_all(response.as_bytes())
                        .await
                        .expect("write mock response");
                });
            }
        });
        (
            RestClient {
                http: reqwest::Client::new(),
                base_url,
                keys: member_keys,
                auth_tag_json: None,
            },
            task,
        )
    }

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

    #[test]
    fn only_a_live_matching_server_epoch_is_current() {
        let runtime_id = Uuid::new_v4();
        let state = state(runtime_id);
        let mut status = status(Some(runtime(
            runtime_id,
            RuntimeAvailability::Available,
            false,
        )));
        status.assignment_id = state.assignment_id;
        status.runtimes[0].runtime_epoch = state.runtime_epoch;
        status.runtimes[0].lease_expires_at = Some(Utc::now() + chrono::Duration::minutes(1));
        assert!(runtime_is_current(&state, &status, Utc::now()));

        status.runtimes[0].runtime_epoch += 1;
        assert!(!runtime_is_current(&state, &status, Utc::now()));
        status.runtimes[0].runtime_epoch = state.runtime_epoch;
        status.runtimes[0].lease_expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(!runtime_is_current(&state, &status, Utc::now()));
    }

    #[test]
    fn dynamic_fence_round_trips_with_owner_only_permissions() {
        let directory =
            std::env::temp_dir().join(format!("buzz-acp-runtime-fence-test-{}", Uuid::new_v4()));
        let path = directory.join("runtime.fence.json");
        let expected = RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 9,
        };

        write_fence(&path, expected).expect("write Runtime fence");
        let actual: RuntimeFence =
            serde_json::from_slice(&std::fs::read(&path).expect("read Runtime fence"))
                .expect("parse Runtime fence");
        assert_eq!(actual, expected);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path)
                .expect("read Runtime fence metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        remove_file_if_present(&path, "Runtime fence").expect("remove Runtime fence");
        std::fs::remove_dir(&directory).expect("remove Runtime fence directory");
    }

    #[test]
    fn project_view_maintenance_status_is_strict_and_ack_ids_are_coordinate_stable() {
        let member = Keys::generate().public_key();
        let assignment = MaintenanceAssignmentBaseline {
            assignment_id: Uuid::new_v4(),
            member_pubkey: member.to_hex(),
            binding_id: Uuid::new_v4(),
            state_at_begin: "has_runtime".to_owned(),
            last_polled_at: Some(Utc::now()),
            client_protocol_version: Some(MAINTENANCE_CLIENT_PROTOCOL_VERSION),
            client_build: Some(MAINTENANCE_CLIENT_BUILD.to_owned()),
            ack: None,
        };
        let runtime = MaintenanceRuntimeBaseline {
            binding_id: assignment.binding_id,
            assignment_id: assignment.assignment_id,
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 9,
            availability_at_begin: "available".to_owned(),
            ack: None,
        };
        let status = MaintenanceStatus {
            community_id: Uuid::new_v4(),
            host: "buzz.example".to_owned(),
            state: "draining".to_owned(),
            current_epoch: Some(7),
            latest_epoch: Some(7),
            project_view_schema_version: 2,
            project_view_enabled: false,
            archived: false,
            poll_after_seconds: 5,
            epoch: Some(MaintenanceEpochView {
                maintenance_epoch: 7,
                required_client_protocol_version: 1,
                outcome: "active".to_owned(),
                requested_at: Utc::now(),
                completed_at: None,
                assignments: vec![assignment.clone()],
                runtimes: vec![runtime.clone()],
            }),
        };
        validate_maintenance_status(&status).expect("valid maintenance status");
        assert_eq!(
            maintenance_assignment_ack_id(7, &assignment),
            maintenance_assignment_ack_id(7, &assignment)
        );
        assert_ne!(
            maintenance_assignment_ack_id(7, &assignment),
            maintenance_runtime_ack_id(7, &runtime, MaintenanceRuntimeAckStatus::Suspended,)
        );
        assert_ne!(
            maintenance_runtime_ack_id(7, &runtime, MaintenanceRuntimeAckStatus::Suspended,),
            maintenance_runtime_ack_id(7, &runtime, MaintenanceRuntimeAckStatus::Terminal,)
        );

        let mut acknowledged = status.clone();
        let acknowledged_epoch = acknowledged.epoch.as_mut().expect("maintenance epoch");
        acknowledged_epoch.assignments[0].ack = Some(MaintenanceAckView {
            status: "quiesced".to_owned(),
            acked_at: Some(Utc::now()),
            ack_request_id: Some(Uuid::new_v4()),
            canonical_request_hash: Some("ab".repeat(32)),
            receipt: Some(json!({
                "maintenance_epoch": 7,
                "type": "assignment_quiesced",
                "binding_id": assignment.binding_id,
                "assignment_id": assignment.assignment_id,
                "status": "quiesced",
                "client_protocol_version": MAINTENANCE_CLIENT_PROTOCOL_VERSION,
                "client_build": MAINTENANCE_CLIENT_BUILD,
            })),
        });
        acknowledged_epoch.runtimes[0].ack = Some(MaintenanceAckView {
            status: "suspended".to_owned(),
            acked_at: Some(Utc::now()),
            ack_request_id: Some(Uuid::new_v4()),
            canonical_request_hash: Some("cd".repeat(32)),
            receipt: Some(json!({
                "maintenance_epoch": 7,
                "type": "runtime_suspended_or_terminal",
                "binding_id": runtime.binding_id,
                "assignment_id": runtime.assignment_id,
                "runtime_id": runtime.runtime_id,
                "runtime_epoch": runtime.runtime_epoch,
                "status": "suspended",
            })),
        });
        validate_maintenance_status(&acknowledged).expect("exact ACK receipts");

        let mut tampered_ack = acknowledged;
        tampered_ack
            .epoch
            .as_mut()
            .expect("maintenance epoch")
            .runtimes[0]
            .ack
            .as_mut()
            .expect("Runtime ACK")
            .receipt = Some(json!({
            "maintenance_epoch": 7,
            "type": "runtime_suspended_or_terminal",
            "binding_id": runtime.binding_id,
            "assignment_id": runtime.assignment_id,
            "runtime_id": Uuid::new_v4(),
            "runtime_epoch": runtime.runtime_epoch,
            "status": "suspended",
        }));
        assert!(validate_maintenance_status(&tampered_ack).is_err());

        let mut malformed = status;
        malformed.poll_after_seconds = 0;
        assert!(validate_maintenance_status(&malformed).is_err());
    }

    #[tokio::test]
    async fn project_view_maintenance_acknowledges_runtime_before_assignment_and_reads_back() {
        let member_keys = Keys::generate();
        let assignment_id = Uuid::new_v4();
        let api_state = Arc::new(Mutex::new(MockRuntimeApi {
            assignment_id,
            managed: true,
            maintenance: Some(MockMaintenanceApi {
                community_id: Uuid::new_v4(),
                member_pubkey: member_keys.public_key().to_hex(),
                binding_id: Uuid::new_v4(),
                assignment_id,
                runtime_id: Uuid::new_v4(),
                runtime_epoch: 9,
                runtime_acked: false,
                assignment_acked: false,
                ack_order: Vec::new(),
            }),
            ..MockRuntimeApi::default()
        }));
        let (client, server) =
            mock_runtime_client(Arc::clone(&api_state), member_keys.clone()).await;
        let directory = std::env::temp_dir().join(format!(
            "buzz-acp-project-view-maintenance-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir(&directory).expect("create maintenance test directory");
        let lifecycle_gate = crate::role_brief::TurnLifecycleGate::new();
        let mut coordinator = RuntimeSupervisorCoordinator::new(
            Some(RuntimeSupervisorConfig {
                keys: Keys::generate(),
                state_path: directory.join("state.json"),
                fence_path: directory.join("runtime.fence.json"),
            }),
            client,
            member_keys.public_key(),
            "ws://relay.example".to_owned(),
            lifecycle_gate.clone(),
        );

        assert!(matches!(
            coordinator
                .poll_maintenance()
                .await
                .expect("latch maintenance"),
            MaintenanceWatchState::Holding {
                state,
                maintenance_epoch: 7,
                ..
            } if state == "draining"
        ));
        assert!(!lifecycle_gate.admission_open());
        coordinator
            .acknowledge_latched_maintenance()
            .await
            .expect("acknowledge exact maintenance baseline");

        let state = api_state.lock().expect("lock mock Runtime API");
        let maintenance = state.maintenance.as_ref().expect("maintenance fixture");
        assert!(maintenance.runtime_acked);
        assert!(maintenance.assignment_acked);
        assert_eq!(maintenance.ack_order, ["runtime", "assignment"]);
        drop(state);

        server.abort();
        std::fs::remove_dir(&directory).expect("remove maintenance test directory");
    }

    #[tokio::test]
    async fn running_harness_converges_across_binding_and_assignment_changes() {
        let assignment_id = Uuid::new_v4();
        let member_keys = Keys::generate();
        let supervisor_keys = Keys::generate();
        let api_state = Arc::new(Mutex::new(MockRuntimeApi {
            assignment_id,
            managed: true,
            ..MockRuntimeApi::default()
        }));
        let (client, server) =
            mock_runtime_client(Arc::clone(&api_state), member_keys.clone()).await;
        let directory =
            std::env::temp_dir().join(format!("buzz-acp-dynamic-runtime-test-{}", Uuid::new_v4()));
        let state_path = directory.join("state.json");
        let config = RuntimeSupervisorConfig {
            keys: supervisor_keys,
            state_path: state_path.clone(),
            fence_path: directory.join("runtime.fence.json"),
        };
        let fence_path = config.fence_path();
        let mut coordinator = RuntimeSupervisorCoordinator::new(
            Some(config),
            client,
            member_keys.public_key(),
            "ws://relay.example".to_owned(),
            crate::role_brief::TurnLifecycleGate::new(),
        );
        coordinator
            .prepare_startup(Some(assignment_id))
            .await
            .expect("prepare supervised Runtime");
        coordinator
            .mark_healthy()
            .await
            .expect("mark supervised Runtime healthy");
        let first: RuntimeFence =
            serde_json::from_slice(&std::fs::read(&fence_path).expect("read first fence"))
                .expect("parse first fence");
        let (client, worker) = coordinator.spawn();

        {
            let mut state = api_state.lock().expect("lock mock Runtime API");
            state.managed = false;
            state.runtime = None;
        }
        client
            .reconcile(Some(assignment_id))
            .await
            .expect("reconcile revoked binding");
        assert!(!fence_path.exists());
        assert!(!state_path.exists());

        api_state.lock().expect("lock mock Runtime API").managed = true;
        client
            .reconcile(Some(assignment_id))
            .await
            .expect("reconcile restored binding");
        let second: RuntimeFence =
            serde_json::from_slice(&std::fs::read(&fence_path).expect("read replacement fence"))
                .expect("parse replacement fence");
        assert_ne!(second.runtime_id, first.runtime_id);

        let replacement_assignment_id = Uuid::new_v4();
        {
            let mut state = api_state.lock().expect("lock mock Runtime API");
            state.assignment_id = replacement_assignment_id;
            state.runtime = None;
        }
        client
            .reconcile(Some(replacement_assignment_id))
            .await
            .expect("reconcile replacement Assignment");
        let third: RuntimeFence =
            serde_json::from_slice(&std::fs::read(&fence_path).expect("read successor fence"))
                .expect("parse successor fence");
        assert_ne!(third.runtime_id, second.runtime_id);

        client
            .graceful_stop()
            .await
            .expect("stop dynamic Runtime worker");
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("Runtime worker stop timeout")
            .expect("join Runtime worker");
        assert!(!fence_path.exists());
        assert!(!state_path.exists());
        assert_eq!(
            api_state.lock().expect("lock mock Runtime API").evidence,
            ["start", "start", "start", "graceful_stop"]
        );

        server.abort();
        std::fs::remove_dir(&directory).expect("remove dynamic Runtime test directory");
    }
}
