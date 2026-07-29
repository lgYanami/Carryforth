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
use chrono::{DateTime, Utc};
use nostr::{Keys, PublicKey};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;
use zeroize::Zeroize;

use crate::relay::RestClient;

pub(crate) const SUPERVISOR_PRIVATE_KEY_ENV: &str = "BUZZ_RUNTIME_SUPERVISOR_PRIVATE_KEY";
pub(crate) const SUPERVISION_STATE_PATH_ENV: &str = "BUZZ_RUNTIME_SUPERVISION_STATE_PATH";
pub(crate) const RUNTIME_FENCE_PATH_ENV: &str = "BUZZ_RUNTIME_FENCE_PATH";

const STATE_SCHEMA_VERSION: u16 = 1;
const EVIDENCE_PATH: &str = "/api/project-runtime/evidence";
const LEASE_RETRY_DELAY: Duration = Duration::from_secs(2);
const RECOVERY_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(20);
const SUPERVISOR_COMMAND_CAPACITY: usize = 16;

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
    member_pubkey: PublicKey,
    relay_url: String,
    current_assignment_id: Option<Uuid>,
    active: Option<RuntimeSupervisor>,
}

impl RuntimeSupervisorCoordinator {
    pub(crate) fn new(
        config: Option<RuntimeSupervisorConfig>,
        agent_client: RestClient,
        member_pubkey: PublicKey,
        relay_url: String,
    ) -> Self {
        Self {
            config,
            agent_client,
            member_pubkey,
            relay_url,
            current_assignment_id: None,
            active: None,
        }
    }

    /// Agent-readable path derived from the private pair-scoped state path.
    pub(crate) fn fence_path(&self) -> Option<PathBuf> {
        self.config
            .as_ref()
            .map(RuntimeSupervisorConfig::fence_path)
    }

    /// Reconcile startup before any model-facing child can receive work.
    pub(crate) async fn prepare_startup(
        &mut self,
        assignment_id: Option<Uuid>,
    ) -> Result<(), String> {
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

    fn suspend(&self) -> Result<(), String> {
        self.clear_fence()
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
        let client = RuntimeSupervisorClient { sender };
        let task = tokio::spawn(async move {
            while let Some(command) = receiver.recv().await {
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
                    RuntimeSupervisorCommand::Stop { exit, response } => {
                        let result = self.stop(exit).await;
                        let _ = response.send(result);
                        break;
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
    Stop {
        exit: RuntimeSupervisorExit,
        response: oneshot::Sender<Result<(), String>>,
    },
}

/// Cloneable per-turn gate into the trusted Runtime coordinator.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeSupervisorClient {
    sender: mpsc::Sender<RuntimeSupervisorCommand>,
}

impl RuntimeSupervisorClient {
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
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    #[derive(Default)]
    struct MockRuntimeApi {
        assignment_id: Uuid,
        managed: bool,
        runtime: Option<buzz_project_view::v2::RuntimeLeaseStatus>,
        evidence: Vec<String>,
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
                    let response = if request_line.starts_with("GET ") {
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
