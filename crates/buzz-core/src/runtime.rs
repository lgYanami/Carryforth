//! Wire-neutral runtime fencing shared by supervised command protocols.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Largest integer that supported JavaScript clients can represent exactly.
const MAX_SAFE_RUNTIME_EPOCH: u64 = 9_007_199_254_740_991;

/// Signed runtime fence carried by a supervised managed-Agent command.
///
/// The fence is intentionally owned by `buzz-core`, rather than Project View
/// or Project Document, so every command protocol uses the same runtime
/// identity and epoch wire shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeFence {
    /// Stable logical runtime identity.
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
        if !(1..=MAX_SAFE_RUNTIME_EPOCH).contains(&self.runtime_epoch) {
            return Err("runtime_epoch must be in the JavaScript-safe positive range".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_fence_wire_is_closed_and_validation_is_fail_closed() {
        let runtime_id =
            Uuid::parse_str("74ad5e95-903b-4488-ac19-d95a73fa62d4").expect("fixture UUID");
        let fence: RuntimeFence = serde_json::from_value(serde_json::json!({
            "runtime_id": runtime_id,
            "runtime_epoch": 4
        }))
        .expect("canonical fence parses");
        assert!(fence.validate().is_ok());

        assert!(serde_json::from_value::<RuntimeFence>(serde_json::json!({
            "runtime_id": runtime_id,
            "runtime_epoch": 4,
            "future": true
        }))
        .is_err());
        assert!(RuntimeFence {
            runtime_id: Uuid::nil(),
            runtime_epoch: 1,
        }
        .validate()
        .is_err());
        assert!(RuntimeFence {
            runtime_id,
            runtime_epoch: MAX_SAFE_RUNTIME_EPOCH + 1,
        }
        .validate()
        .is_err());
    }
}
