//! Atomic applied-workspace capture for trusted native request paths.

use nostr::{Keys, PublicKey};

use super::AppState;

/// Signability captured with one applied Desktop workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceSigningEligibility {
    Ready,
    KeyringLocked,
    IdentityLost,
    ResetFailed,
}

/// Immutable request boundary cloned before a trusted workspace-scoped call.
///
/// `keys` deliberately has no `Debug` representation: this value may be held
/// across network awaits but must never disclose key material or the opaque
/// workspace token in diagnostics.
#[derive(Clone)]
pub(crate) struct AppliedWorkspaceCapture {
    pub community_key: String,
    pub applied_workspace_token: String,
    pub relay_http_origin: String,
    pub keys: Keys,
    pub caller: PublicKey,
    signing_eligibility: WorkspaceSigningEligibility,
}

impl std::fmt::Debug for AppliedWorkspaceCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppliedWorkspaceCapture")
            .field("community_key", &"<redacted>")
            .field("applied_workspace_token", &"<redacted>")
            .field("relay_http_origin", &"<redacted>")
            .field("keys", &"<redacted>")
            .field("caller", &self.caller)
            .field("signing_eligibility", &self.signing_eligibility)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AppliedWorkspaceCaptureError {
    /// No workspace has completed the native apply transition.
    NotApplied,
    /// Community key or opaque applied token does not match current state.
    Mismatch,
    /// The persisted signing key is temporarily inaccessible.
    KeyringLocked,
    /// The prior identity is lost and the current key is only ephemeral.
    IdentityLost,
    /// Boot-time reset failed and identity-dependent work is disabled.
    ResetFailed,
    /// The transition lock was poisoned.
    StateUnavailable,
}

/// Mutable owner of the latest complete applied-workspace tuple.
#[derive(Default)]
pub(crate) struct WorkspaceTransitionState {
    applied: Option<AppliedWorkspaceCapture>,
}

/// Build the bounded, fail-closed client for trusted semantic request setup and
/// the one-shot semantic `/query`.
pub(super) fn build_semantic_query_http_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .timeout(std::time::Duration::from_secs(45))
        .pool_idle_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(1)
        .redirect(reqwest::redirect::Policy::none())
        .build()
}

impl AppState {
    fn current_workspace_signing_eligibility(&self) -> WorkspaceSigningEligibility {
        if self.reset_failed.load(std::sync::atomic::Ordering::Acquire) {
            WorkspaceSigningEligibility::ResetFailed
        } else if self
            .identity_lost
            .load(std::sync::atomic::Ordering::Acquire)
        {
            WorkspaceSigningEligibility::IdentityLost
        } else if self
            .keyring_locked
            .load(std::sync::atomic::Ordering::Acquire)
        {
            WorkspaceSigningEligibility::KeyringLocked
        } else {
            WorkspaceSigningEligibility::Ready
        }
    }

    /// Atomically publish the tuple used by trusted workspace-scoped requests.
    ///
    /// Callers that can race an identity mutation must hold `identity_mutation`
    /// before entering this method. The transition lock remains held while the
    /// legacy relay/key fields are updated, so semantic capture can observe
    /// either the complete previous tuple or the complete new tuple, never a
    /// relay/caller hybrid.
    pub(crate) fn apply_workspace_transition(
        &self,
        community_key: String,
        relay_ws_url: String,
        replacement_keys: Option<Keys>,
    ) -> Result<AppliedWorkspaceCapture, String> {
        let mut transition = self
            .workspace_transition
            .lock()
            .map_err(|error| error.to_string())?;
        *self
            .relay_url_override
            .lock()
            .map_err(|error| error.to_string())? = Some(relay_ws_url.clone());
        if let Some(keys) = replacement_keys {
            *self.keys.lock().map_err(|error| error.to_string())? = keys;
        }
        let keys = self.keys.lock().map_err(|error| error.to_string())?.clone();
        let capture = AppliedWorkspaceCapture {
            community_key,
            applied_workspace_token: uuid::Uuid::new_v4().to_string(),
            relay_http_origin: crate::relay::relay_http_base_url(&relay_ws_url),
            caller: keys.public_key(),
            keys,
            signing_eligibility: self.current_workspace_signing_eligibility(),
        };
        transition.applied = Some(capture.clone());
        Ok(capture)
    }

    /// Replace the runtime identity and invalidate any previously applied
    /// workspace token under the same transition lock.
    pub(crate) fn replace_runtime_identity(
        &self,
        keys: Keys,
        identity_lost: bool,
        keyring_locked: bool,
    ) -> Result<(), String> {
        let mut transition = self
            .workspace_transition
            .lock()
            .map_err(|error| error.to_string())?;
        *self.keys.lock().map_err(|error| error.to_string())? = keys.clone();
        self.identity_lost
            .store(identity_lost, std::sync::atomic::Ordering::Release);
        self.keyring_locked
            .store(keyring_locked, std::sync::atomic::Ordering::Release);

        if let Some(applied) = transition.applied.as_mut() {
            applied.applied_workspace_token = uuid::Uuid::new_v4().to_string();
            applied.caller = keys.public_key();
            applied.keys = keys;
            applied.signing_eligibility = self.current_workspace_signing_eligibility();
        }
        Ok(())
    }

    /// Capture one exact applied workspace tuple before any network await.
    pub(crate) fn capture_applied_workspace(
        &self,
        community_key: &str,
        applied_workspace_token: &str,
    ) -> Result<AppliedWorkspaceCapture, AppliedWorkspaceCaptureError> {
        let transition = self
            .workspace_transition
            .lock()
            .map_err(|_| AppliedWorkspaceCaptureError::StateUnavailable)?;
        let applied = transition
            .applied
            .as_ref()
            .ok_or(AppliedWorkspaceCaptureError::NotApplied)?;
        if applied.community_key != community_key
            || applied.applied_workspace_token != applied_workspace_token
        {
            return Err(AppliedWorkspaceCaptureError::Mismatch);
        }
        match applied.signing_eligibility {
            WorkspaceSigningEligibility::Ready => Ok(applied.clone()),
            WorkspaceSigningEligibility::KeyringLocked => {
                Err(AppliedWorkspaceCaptureError::KeyringLocked)
            }
            WorkspaceSigningEligibility::IdentityLost => {
                Err(AppliedWorkspaceCaptureError::IdentityLost)
            }
            WorkspaceSigningEligibility::ResetFailed => {
                Err(AppliedWorkspaceCaptureError::ResetFailed)
            }
        }
    }
}

#[cfg(test)]
#[path = "workspace_transition_tests.rs"]
mod tests;
