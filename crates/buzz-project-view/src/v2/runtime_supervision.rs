//! Closed wire and read-model types for trusted managed-runtime supervision.
//!
//! Runtime supervision is deliberately separate from Project View's canonical
//! revision stream. Lease renewals and recovery attempts are operational facts;
//! only the final, policy-fenced `unrecoverable` transition becomes a Project
//! system change.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::MAX_SAFE_REVISION;

/// Current runtime-supervision wire schema.
pub const RUNTIME_SUPERVISION_SCHEMA_VERSION: u16 = 1;

const MIN_LEASE_SECONDS: u32 = 10;
const MAX_LEASE_SECONDS: u32 = 300;
const MIN_RECOVERY_WINDOW_SECONDS: u32 = 30;
const MAX_RECOVERY_WINDOW_SECONDS: u32 = 86_400;
const MAX_RECOVERY_ATTEMPTS: u32 = 100;
const MIN_RECOVERY_BACKOFF_SECONDS: u32 = 1;
const MAX_RECOVERY_BACKOFF_SECONDS: u32 = 300;
const MIN_MONITOR_TIMEOUT_SECONDS: u32 = 30;
const MAX_MONITOR_TIMEOUT_SECONDS: u32 = 3_600;
const MIN_MONITOR_GRACE_SECONDS: u32 = 30;
const MAX_MONITOR_GRACE_SECONDS: u32 = 86_400;
const MAX_EVIDENCE_SUMMARY_BYTES: usize = 512;

/// Low-frequency availability derived from trusted supervisor evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAvailability {
    /// At least one current, leased runtime can carry the Assignment.
    Available,
    /// The supervisor observed an abnormal exit and is attempting recovery.
    Recovering,
    /// Recovery policy has been exhausted for this runtime.
    Unavailable,
}

/// Signed runtime fence carried by a supervised managed Agent command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFence {
    /// Logical runtime identity.
    pub runtime_id: Uuid,
    /// Current server-allocated epoch for that runtime.
    pub runtime_epoch: u64,
}

impl RuntimeFence {
    /// Validate a non-nil identity and JavaScript-safe positive epoch.
    pub fn validate(self) -> Result<(), String> {
        if self.runtime_id.is_nil() {
            return Err("runtime_id cannot be nil".to_owned());
        }
        if !(1..=MAX_SAFE_REVISION).contains(&self.runtime_epoch) {
            return Err("runtime_epoch must be in the JavaScript-safe positive range".to_owned());
        }
        Ok(())
    }
}

impl RuntimeAvailability {
    /// Stable database and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Recovering => "recovering",
            Self::Unavailable => "unavailable",
        }
    }
}

/// Assignment-scoped recovery policy installed by a deployment operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeRecoveryPolicy {
    /// Duration of one healthy runtime lease.
    pub lease_seconds: u32,
    /// Maximum duration of one recovery episode.
    pub recovery_window_seconds: u32,
    /// Maximum explicit recovery attempts within one episode.
    pub max_recovery_attempts: u32,
    /// Base of the server-enforced exponential delay between failed attempts.
    pub recovery_backoff_seconds: u32,
    /// Maximum age of trusted supervisor health before automation fails closed.
    pub monitor_timeout_seconds: u32,
    /// Extra delay after monitor health returns before automation may act.
    pub monitor_grace_seconds: u32,
    /// Assignment-local opt-in to automatic `unrecoverable`.
    pub automatic_unrecoverable: bool,
}

impl Default for RuntimeRecoveryPolicy {
    fn default() -> Self {
        Self {
            lease_seconds: 60,
            recovery_window_seconds: 900,
            max_recovery_attempts: 5,
            recovery_backoff_seconds: 5,
            monitor_timeout_seconds: 180,
            monitor_grace_seconds: 300,
            automatic_unrecoverable: false,
        }
    }
}

impl RuntimeRecoveryPolicy {
    /// Validate bounded policy values before persistence.
    pub fn validate(self) -> Result<(), String> {
        if !(MIN_LEASE_SECONDS..=MAX_LEASE_SECONDS).contains(&self.lease_seconds) {
            return Err(format!(
                "lease_seconds must be in {MIN_LEASE_SECONDS}..={MAX_LEASE_SECONDS}"
            ));
        }
        if !(MIN_RECOVERY_WINDOW_SECONDS..=MAX_RECOVERY_WINDOW_SECONDS)
            .contains(&self.recovery_window_seconds)
        {
            return Err(format!(
                "recovery_window_seconds must be in \
                 {MIN_RECOVERY_WINDOW_SECONDS}..={MAX_RECOVERY_WINDOW_SECONDS}"
            ));
        }
        if !(1..=MAX_RECOVERY_ATTEMPTS).contains(&self.max_recovery_attempts) {
            return Err(format!(
                "max_recovery_attempts must be in 1..={MAX_RECOVERY_ATTEMPTS}"
            ));
        }
        if !(MIN_RECOVERY_BACKOFF_SECONDS..=MAX_RECOVERY_BACKOFF_SECONDS)
            .contains(&self.recovery_backoff_seconds)
        {
            return Err(format!(
                "recovery_backoff_seconds must be in \
                 {MIN_RECOVERY_BACKOFF_SECONDS}..={MAX_RECOVERY_BACKOFF_SECONDS}"
            ));
        }
        if self.recovery_backoff_seconds >= self.recovery_window_seconds {
            return Err(
                "recovery_backoff_seconds must be shorter than recovery_window_seconds".to_owned(),
            );
        }
        if !(MIN_MONITOR_TIMEOUT_SECONDS..=MAX_MONITOR_TIMEOUT_SECONDS)
            .contains(&self.monitor_timeout_seconds)
        {
            return Err(format!(
                "monitor_timeout_seconds must be in \
                 {MIN_MONITOR_TIMEOUT_SECONDS}..={MAX_MONITOR_TIMEOUT_SECONDS}"
            ));
        }
        if !(MIN_MONITOR_GRACE_SECONDS..=MAX_MONITOR_GRACE_SECONDS)
            .contains(&self.monitor_grace_seconds)
        {
            return Err(format!(
                "monitor_grace_seconds must be in \
                 {MIN_MONITOR_GRACE_SECONDS}..={MAX_MONITOR_GRACE_SECONDS}"
            ));
        }
        Ok(())
    }
}

/// One signed observation submitted by a registered runtime supervisor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum RuntimeEvidence {
    /// Open a new fenced epoch for one logical runtime.
    Start,
    /// Renew a healthy runtime's lease without changing its epoch.
    LeaseRenewed,
    /// Record a trusted abnormal process exit and open a recovery episode.
    AbnormalExit {
        /// Optional bounded diagnostic summary; never secret material.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Optional operating-system exit status.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },
    /// Record the next bounded recovery attempt.
    RecoveryAttempt,
    /// Mark the currently fenced replacement attempt as healthy.
    RecoverySucceeded,
    /// Record one failed recovery result.
    RecoveryFailed {
        /// Optional bounded diagnostic summary; never secret material.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
    },
    /// Prove that the supervisor is healthy while a runtime remains recovering.
    SupervisorHeartbeat,
}

impl RuntimeEvidence {
    /// Stable evidence spelling used by immutable evidence rows.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::LeaseRenewed => "lease_renewed",
            Self::AbnormalExit { .. } => "abnormal_exit",
            Self::RecoveryAttempt => "recovery_attempt",
            Self::RecoverySucceeded => "recovery_succeeded",
            Self::RecoveryFailed { .. } => "recovery_failed",
            Self::SupervisorHeartbeat => "supervisor_heartbeat",
        }
    }
}

/// Closed NIP-98 request body for one trusted runtime observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceRequest {
    /// Must equal [`RUNTIME_SUPERVISION_SCHEMA_VERSION`].
    pub schema_version: u16,
    /// Exact active Assignment registered for this supervisor.
    pub assignment_id: Uuid,
    /// Logical managed runtime identity.
    pub runtime_id: Uuid,
    /// Client idempotency key; retained only as a domain-separated digest.
    pub idempotency_key: Uuid,
    /// Current epoch. Omitted only for `start`; a recovery attempt names the
    /// preceding epoch and receives a new one, then its result names that new
    /// attempt epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_epoch: Option<u64>,
    /// Typed observation.
    pub evidence: RuntimeEvidence,
}

impl RuntimeEvidenceRequest {
    /// Validate closed request shape before consulting canonical state.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != RUNTIME_SUPERVISION_SCHEMA_VERSION {
            return Err(format!(
                "schema_version must be {RUNTIME_SUPERVISION_SCHEMA_VERSION}"
            ));
        }
        for (name, value) in [
            ("assignment_id", self.assignment_id),
            ("runtime_id", self.runtime_id),
            ("idempotency_key", self.idempotency_key),
        ] {
            if value.is_nil() {
                return Err(format!("{name} cannot be nil"));
            }
        }
        match (&self.evidence, self.runtime_epoch) {
            (RuntimeEvidence::Start, None) => {}
            (RuntimeEvidence::Start, Some(_)) => {
                return Err("start must not supply runtime_epoch".to_owned());
            }
            (_, Some(epoch)) if (1..=MAX_SAFE_REVISION).contains(&epoch) => {}
            (_, Some(_)) => {
                return Err(
                    "runtime_epoch must be in the JavaScript-safe positive range".to_owned(),
                );
            }
            (_, None) => return Err("runtime_epoch is required for this evidence".to_owned()),
        }
        let summary = match &self.evidence {
            RuntimeEvidence::AbnormalExit { summary, .. }
            | RuntimeEvidence::RecoveryFailed { summary } => summary.as_deref(),
            _ => None,
        };
        if summary.is_some_and(|value| {
            value.trim().is_empty() || value.len() > MAX_EVIDENCE_SUMMARY_BYTES
        }) {
            return Err(format!(
                "evidence summary must be non-empty and at most \
                 {MAX_EVIDENCE_SUMMARY_BYTES} bytes"
            ));
        }
        Ok(())
    }
}

/// Durable receipt returned after accepting supervisor evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvidenceReceipt {
    /// Assignment carrying this runtime.
    pub assignment_id: Uuid,
    /// Logical runtime identity.
    pub runtime_id: Uuid,
    /// Current server-fenced epoch after applying the observation.
    pub runtime_epoch: u64,
    /// Current availability of this runtime.
    pub availability: RuntimeAvailability,
    /// Healthy lease deadline, when the runtime is available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Recovery deadline, when a recovery episode is active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_deadline: Option<DateTime<Utc>>,
    /// Attempts recorded in the current recovery episode.
    pub recovery_attempts: u32,
    /// Whether the latest recorded attempt has not yet reported a result.
    pub recovery_attempt_in_flight: bool,
    /// Earliest canonical time at which the next attempt may begin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_recovery_at: Option<DateTime<Utc>>,
    /// Assignment-scoped configured maximum.
    pub max_recovery_attempts: u32,
    /// Whether this returned a previously accepted idempotency receipt.
    pub replayed: bool,
}

/// One runtime row in an Assignment availability read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLeaseStatus {
    /// Logical runtime identity.
    pub runtime_id: Uuid,
    /// Current fenced epoch.
    pub runtime_epoch: u64,
    /// Current low-frequency availability.
    pub availability: RuntimeAvailability,
    /// Healthy lease deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<DateTime<Utc>>,
    /// Active recovery deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_deadline: Option<DateTime<Utc>>,
    /// Recovery attempts recorded in this episode.
    pub recovery_attempts: u32,
    /// Whether a recorded recovery attempt is still awaiting its result.
    pub recovery_attempt_in_flight: bool,
    /// Earliest canonical time at which another attempt may begin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_recovery_at: Option<DateTime<Utc>>,
    /// Canonical time of the latest accepted evidence.
    pub last_evidence_at: DateTime<Utc>,
}

/// Read model for one Assignment's trusted runtime supervision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignmentRuntimeStatus {
    /// Assignment queried by the caller.
    pub assignment_id: Uuid,
    /// Whether at least one active supervisor binding exists.
    pub managed: bool,
    /// Aggregate availability. Absent when the Assignment is not supervised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<RuntimeAvailability>,
    /// Current runtime epochs in stable runtime-ID order.
    #[serde(default)]
    pub runtimes: Vec<RuntimeLeaseStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(evidence: RuntimeEvidence, runtime_epoch: Option<u64>) -> RuntimeEvidenceRequest {
        RuntimeEvidenceRequest {
            schema_version: RUNTIME_SUPERVISION_SCHEMA_VERSION,
            assignment_id: Uuid::new_v4(),
            runtime_id: Uuid::new_v4(),
            idempotency_key: Uuid::new_v4(),
            runtime_epoch,
            evidence,
        }
    }

    #[test]
    fn evidence_epoch_shape_is_closed() {
        assert!(request(RuntimeEvidence::Start, None).validate().is_ok());
        assert!(request(RuntimeEvidence::Start, Some(1)).validate().is_err());
        assert!(request(RuntimeEvidence::LeaseRenewed, None)
            .validate()
            .is_err());
        assert!(request(RuntimeEvidence::LeaseRenewed, Some(1))
            .validate()
            .is_ok());
    }

    #[test]
    fn evidence_request_rejects_unknown_fields() {
        let value = json!({
            "schema_version": RUNTIME_SUPERVISION_SCHEMA_VERSION,
            "assignment_id": Uuid::new_v4(),
            "runtime_id": Uuid::new_v4(),
            "idempotency_key": Uuid::new_v4(),
            "evidence": {"type": "start"},
            "pretend_unrecoverable": true
        });
        assert!(serde_json::from_value::<RuntimeEvidenceRequest>(value).is_err());
    }

    #[test]
    fn diagnostics_are_bounded_before_persistence() {
        let empty = request(
            RuntimeEvidence::RecoveryFailed {
                summary: Some(" ".to_owned()),
            },
            Some(1),
        );
        assert!(empty.validate().is_err());
        let oversized = request(
            RuntimeEvidence::AbnormalExit {
                summary: Some("x".repeat(MAX_EVIDENCE_SUMMARY_BYTES + 1)),
                exit_code: None,
            },
            Some(1),
        );
        assert!(oversized.validate().is_err());
    }

    #[test]
    fn recovery_policy_is_bounded_and_defaults_fail_closed() {
        let defaults = RuntimeRecoveryPolicy::default();
        assert!(defaults.validate().is_ok());
        assert!(!defaults.automatic_unrecoverable);
        assert!(RuntimeRecoveryPolicy {
            lease_seconds: MIN_LEASE_SECONDS - 1,
            ..defaults
        }
        .validate()
        .is_err());
        assert!(RuntimeRecoveryPolicy {
            max_recovery_attempts: 0,
            ..defaults
        }
        .validate()
        .is_err());
        assert!(RuntimeRecoveryPolicy {
            recovery_backoff_seconds: defaults.recovery_window_seconds,
            ..defaults
        }
        .validate()
        .is_err());
    }

    #[test]
    fn runtime_fence_requires_non_nil_identity_and_safe_epoch() {
        assert!(RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: 1,
        }
        .validate()
        .is_ok());
        assert!(RuntimeFence {
            runtime_id: Uuid::nil(),
            runtime_epoch: 1,
        }
        .validate()
        .is_err());
        assert!(RuntimeFence {
            runtime_id: Uuid::new_v4(),
            runtime_epoch: MAX_SAFE_REVISION + 1,
        }
        .validate()
        .is_err());
    }
}
