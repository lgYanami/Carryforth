use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::ManagedAgentProcess;

/// Canonical identity of one managed-agent harness on one relay.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeKey {
    pub pubkey: String,
    pub relay_url: String,
}

impl ManagedAgentRuntimeKey {
    pub fn new(pubkey: impl Into<String>, relay_url: &str) -> Result<Self, String> {
        let pubkey = pubkey.into();
        if pubkey.len() != 64 || !pubkey.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("managed-agent pubkey must be 64 hexadecimal characters".into());
        }
        Ok(Self {
            pubkey: pubkey.to_ascii_lowercase(),
            relay_url: buzz_core_pkg::relay::normalize_relay_url(relay_url)
                .map_err(|error| error.to_string())?,
        })
    }

    /// Stable opaque identifier/path suffix derived only from canonical fields.
    pub fn runtime_id(&self) -> String {
        let relay_hash = hex::encode(Sha256::digest(self.relay_url.as_bytes()));
        format!("{}__{relay_hash}", self.pubkey)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentRuntimeLifecycle {
    Starting,
    Listening,
    Waking,
    Ready,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ManagedAgentRuntimeSupervisionState {
    NotApplicable,
    Disabled,
    AwaitingBinding,
    Starting,
    Active,
    Recovering,
    DegradedMissingKey,
    DegradedMismatch,
    Expired,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeSupervisionStatus {
    pub state: ManagedAgentRuntimeSupervisionState,
    /// Exact relay URL used by the live harness. This stays separate from the
    /// canonical process key so host-scoped operator commands do not silently
    /// rewrite `localhost` to `127.0.0.1`.
    pub connection_relay_url: Option<String>,
    pub assignment_id: Option<uuid::Uuid>,
    pub binding_id: Option<uuid::Uuid>,
    pub supervisor_pubkey: Option<String>,
    pub local_supervisor_pubkey: Option<String>,
    pub identity_availability: Option<super::RuntimeSupervisorIdentityAvailability>,
    pub identity_source: Option<super::RuntimeSupervisorIdentitySource>,
    pub identity_detail_code: Option<String>,
    pub runtime_id: Option<uuid::Uuid>,
    pub runtime_epoch: Option<u64>,
    pub lease_expires_at: Option<String>,
    pub detail_code: Option<String>,
    pub observed_at: String,
    pub stale: bool,
}

impl ManagedAgentRuntimeSupervisionStatus {
    pub fn awaiting_observer(identity: Option<&super::RuntimeSupervisorIdentityStatus>) -> Self {
        Self {
            state: ManagedAgentRuntimeSupervisionState::Unknown,
            connection_relay_url: None,
            assignment_id: None,
            binding_id: None,
            supervisor_pubkey: None,
            local_supervisor_pubkey: identity.and_then(|status| status.public_key.clone()),
            identity_availability: identity.map(|status| status.availability),
            identity_source: identity.and_then(|status| status.source),
            identity_detail_code: identity.and_then(|status| status.detail_code.clone()),
            runtime_id: None,
            runtime_epoch: None,
            lease_expires_at: None,
            detail_code: Some("awaiting_observer".to_owned()),
            observed_at: crate::util::now_iso(),
            stale: false,
        }
    }
}

#[derive(Debug)]
pub struct ManagedAgentPairRuntime {
    pub process: ManagedAgentProcess,
    pub lifecycle: ManagedAgentRuntimeLifecycle,
    pub error: Option<String>,
    /// Unpredictable identity for this exact harness generation. Lifecycle
    /// frames from prior processes are rejected even when the pair is live.
    pub start_nonce: String,
}

impl std::ops::Deref for ManagedAgentPairRuntime {
    type Target = ManagedAgentProcess;

    fn deref(&self) -> &Self::Target {
        &self.process
    }
}

impl std::ops::DerefMut for ManagedAgentPairRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.process
    }
}

impl ManagedAgentPairRuntime {
    pub fn starting(process: ManagedAgentProcess) -> Self {
        let start_nonce = process.start_nonce.clone();
        Self {
            process,
            lifecycle: ManagedAgentRuntimeLifecycle::Starting,
            error: None,
            start_nonce,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeStatus {
    pub pubkey: String,
    pub relay_url: String,
    /// Exact descriptor URL echoed only by reconcile result rows so callers can
    /// correlate a canonical response without normalizing on the frontend.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_relay_url: Option<String>,
    pub local_setup: bool,
    pub lifecycle: ManagedAgentRuntimeLifecycle,
    pub pid: Option<u32>,
    pub error: Option<String>,
    pub log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supervision: Option<ManagedAgentRuntimeSupervisionStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeLifecycleObserverPayload {
    pub pubkey: String,
    pub relay_url: String,
    pub start_nonce: String,
    pub lifecycle: ManagedAgentRuntimeLifecycle,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeSupervisionObserverPayload {
    pub pubkey: String,
    pub relay_url: String,
    pub start_nonce: String,
    pub state: ManagedAgentRuntimeSupervisionState,
    pub assignment_id: Option<uuid::Uuid>,
    pub binding_id: Option<uuid::Uuid>,
    pub supervisor_pubkey: Option<String>,
    pub runtime_id: Option<uuid::Uuid>,
    pub runtime_epoch: Option<u64>,
    pub lease_expires_at: Option<String>,
    pub detail_code: Option<String>,
    pub observed_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentCommunityTarget {
    pub relay_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAgentRuntimeReceipt {
    pub key: ManagedAgentRuntimeKey,
    pub pid: u32,
    pub desktop_instance_id: String,
    pub started_at: String,
}
