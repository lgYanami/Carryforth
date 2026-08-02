//! Schema-v3 Role continuity command envelope.
//!
//! Role definition CRUD is intentionally absent. It belongs exclusively to
//! [`super::ProjectObjectCommandV3`], leaving this command with the existing
//! Proposal, Assignment, Work continuity, Checkpoint, and Handoff operations.

use buzz_core::PublicKey;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::PROJECT_VIEW_V3_SCHEMA_VERSION;
use crate::v2::{
    GeneratedRoleContinuityIds, RoleCommand, RoleCommandRequest, RoleContinuityError,
    RoleContinuityOutcome, RoleContinuityState, RuntimeFence, SchemaVersion,
};
use crate::MAX_SAFE_REVISION;

/// Closed schema-v3 continuity-only Role command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleCommandV3 {
    /// Must equal three.
    pub schema_version: u16,
    /// Exact canonical Project revision observed by the caller.
    pub expected_project_revision: u64,
    /// Active Assignment from which the operation is performed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_assignment_id: Option<Uuid>,
    /// Current supervised runtime epoch. DB coordination makes this mandatory
    /// for every managed Agent in schema 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_fence: Option<RuntimeFence>,
    /// Closed continuity operation inherited byte-for-byte from v2.
    pub request: RoleCommandRequest,
}

impl RoleCommandV3 {
    /// Construct a schema-v3 Role continuity command.
    #[must_use]
    pub const fn new(
        expected_project_revision: u64,
        acting_assignment_id: Option<Uuid>,
        request: RoleCommandRequest,
    ) -> Self {
        Self {
            schema_version: PROJECT_VIEW_V3_SCHEMA_VERSION,
            expected_project_revision,
            acting_assignment_id,
            runtime_fence: None,
            request,
        }
    }

    /// Attach the exact current managed-runtime fence.
    #[must_use]
    pub const fn with_runtime_fence(mut self, runtime_fence: RuntimeFence) -> Self {
        self.runtime_fence = Some(runtime_fence);
        self
    }

    /// Parse and validate the closed schema-v3 command.
    pub fn from_json(json: &str) -> Result<Self, RoleContinuityError> {
        let command: Self = serde_json::from_str(json)
            .map_err(|error| RoleContinuityError::InvalidCommand(error.to_string()))?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate local shape while preserving the mature continuity operation
    /// validation implemented by the v2 reducer.
    pub fn validate_for_submission(&self) -> Result<(), RoleContinuityError> {
        if self.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION {
            return Err(RoleContinuityError::UnsupportedSchema);
        }
        if self.expected_project_revision == 0 || self.expected_project_revision > MAX_SAFE_REVISION
        {
            return Err(RoleContinuityError::InvalidCommand(
                "expected_project_revision must be in the JavaScript-safe positive range"
                    .to_owned(),
            ));
        }
        self.as_v2_reducer_command().validate_for_submission()
    }

    /// Stable operation spelling.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        self.request.operation()
    }

    /// Adapt only the internal reducer input to the mature continuity kernel.
    /// The returned schema-2 envelope must never be serialized or projected as
    /// a v3 wire value.
    #[must_use]
    pub fn as_v2_reducer_command(&self) -> RoleCommand {
        RoleCommand {
            schema_version: SchemaVersion::V2.as_u16(),
            expected_project_revision: self.expected_project_revision,
            acting_assignment_id: self.acting_assignment_id,
            runtime_fence: self.runtime_fence,
            request: self.request.clone(),
        }
    }
}

/// Reduce one schema-v3 continuity command with the existing proven continuity
/// semantics. Runtime supervision strictness remains a DB coordinator fence,
/// not a pure state concern.
pub fn reduce_role_command_v3(
    state: &RoleContinuityState,
    command: &RoleCommandV3,
    actor: PublicKey,
    canonical_time: DateTime<Utc>,
    generated_ids: &GeneratedRoleContinuityIds,
) -> Result<(RoleContinuityState, RoleContinuityOutcome), RoleContinuityError> {
    command.validate_for_submission()?;
    state.reduce(
        &command.as_v2_reducer_command(),
        actor,
        canonical_time,
        generated_ids,
    )
}

/// Re-run current membership and Assignment eligibility before consulting a
/// schema-v3 idempotency receipt.
pub fn validate_role_actor_for_v3_replay(
    state: &RoleContinuityState,
    command: &RoleCommandV3,
    actor: PublicKey,
) -> Result<(), RoleContinuityError> {
    command.validate_for_submission()?;
    state.validate_actor_for_replay(&command.as_v2_reducer_command(), actor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_and_v3_envelopes_are_fail_closed() {
        let command = RoleCommandV3::new(
            4,
            None,
            RoleCommandRequest::ExpireProposal {
                proposal_id: Uuid::new_v4(),
            },
        );
        let json = serde_json::to_string(&command).expect("serialize v3 command");
        assert_eq!(RoleCommandV3::from_json(&json).expect("parse v3"), command);
        assert!(RoleCommand::from_json(&json).is_err());

        let mut wrong = command;
        wrong.schema_version = SchemaVersion::V2.as_u16();
        assert_eq!(
            wrong.validate_for_submission(),
            Err(RoleContinuityError::UnsupportedSchema)
        );
    }
}
