//! Closed schema-v2 commands for ordinary Project View objects.
//!
//! The object reducer remains shared with v1 because the nine object types and
//! their relation invariants are unchanged. The wire envelope is deliberately
//! distinct: it carries schema version 2 and the optional Assignment fence
//! used by managed actors.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::SchemaVersion;
use crate::{
    DomainError, DomainResult, Mutation, MutationRequest, MAX_MUTATION_CONTENT_BYTES,
    MAX_MUTATION_JSON_DEPTH,
};

/// A revision-checked schema-v2 mutation for an ordinary Project View object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectObjectCommand {
    /// Must be `2`.
    pub schema_version: u16,
    /// Project revision on which the caller based this intent.
    pub expected_project_revision: u64,
    /// Active tenure from which a managed actor performs the write.
    ///
    /// Human Community members may omit this field. The Relay decides whether
    /// a signer is a managed actor and therefore requires the fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_assignment_id: Option<Uuid>,
    /// Current runtime epoch when the acting Assignment is supervised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_fence: Option<super::RuntimeFence>,
    /// Closed ordinary-object operation.
    pub request: MutationRequest,
}

impl ProjectObjectCommand {
    /// Construct a schema-v2 ordinary-object command.
    #[must_use]
    pub const fn new(
        expected_project_revision: u64,
        acting_assignment_id: Option<Uuid>,
        request: MutationRequest,
    ) -> Self {
        Self {
            schema_version: SchemaVersion::V2.as_u16(),
            expected_project_revision,
            acting_assignment_id,
            runtime_fence: None,
            request,
        }
    }

    /// Attach the server-issued fence of a supervised managed runtime.
    #[must_use]
    pub const fn with_runtime_fence(mut self, runtime_fence: super::RuntimeFence) -> Self {
        self.runtime_fence = Some(runtime_fence);
        self
    }

    /// Parse a closed command while enforcing Project View's content limits.
    pub fn from_json(content: &str) -> DomainResult<Self> {
        if content.len() > MAX_MUTATION_CONTENT_BYTES {
            return Err(DomainError::MutationContentTooLarge {
                max: MAX_MUTATION_CONTENT_BYTES,
                actual: content.len(),
            });
        }
        let value: Value =
            serde_json::from_str(content).map_err(|error| DomainError::InvalidMutationJson {
                reason: error.to_string(),
            })?;
        let depth = json_depth(&value);
        if depth > MAX_MUTATION_JSON_DEPTH {
            return Err(DomainError::MutationJsonTooDeep {
                max: MAX_MUTATION_JSON_DEPTH,
                actual: depth,
            });
        }
        let command: Self =
            serde_json::from_value(value).map_err(|error| DomainError::InvalidMutationJson {
                reason: error.to_string(),
            })?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate fields that do not depend on canonical Relay state.
    pub fn validate_for_submission(&self) -> DomainResult<()> {
        if self.schema_version != SchemaVersion::V2.as_u16() {
            return Err(DomainError::UnsupportedSchemaVersion {
                got: u32::from(self.schema_version),
                supported: u32::from(SchemaVersion::V2.as_u16()),
            });
        }
        if self
            .acting_assignment_id
            .is_some_and(|assignment_id| assignment_id.is_nil())
        {
            return Err(DomainError::InvalidField {
                field: "acting_assignment_id",
                reason: "must not be nil".to_owned(),
            });
        }
        if let Some(runtime_fence) = self.runtime_fence {
            runtime_fence
                .validate()
                .map_err(|reason| DomainError::InvalidField {
                    field: "runtime_fence",
                    reason,
                })?;
            if self.acting_assignment_id.is_none() {
                return Err(DomainError::InvalidField {
                    field: "runtime_fence",
                    reason: "requires acting_assignment_id".to_owned(),
                });
            }
        }
        self.as_reducer_mutation().validate_for_submission()
    }

    /// Return the stable operation spelling used by receipts and telemetry.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self.request {
            MutationRequest::Initialize(_) => "initialize",
            MutationRequest::Create(_) => "create",
            MutationRequest::Update(_) => "update",
            MutationRequest::Delete(_) => "delete",
        }
    }

    /// Adapt the v2 envelope to the shared ordinary-object reducer.
    ///
    /// This value is internal canonical input, not a wire payload.
    #[must_use]
    pub fn as_reducer_mutation(&self) -> Mutation {
        Mutation::new(self.expected_project_revision, self.request.clone())
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateMutation, NewProjectViewObject};

    #[test]
    fn command_round_trips_without_changing_the_shared_request() {
        let command = ProjectObjectCommand::new(
            7,
            None,
            MutationRequest::Create(CreateMutation {
                object: NewProjectViewObject::Goal {
                    id: Uuid::new_v4(),
                    title: "Keep editing after cutover".to_owned(),
                    desired_outcome: "v2 retains the Project View object model".to_owned(),
                    directions: Vec::new(),
                },
            }),
        );

        let encoded = serde_json::to_string(&command).expect("serialize command");
        assert_eq!(
            ProjectObjectCommand::from_json(&encoded).expect("parse command"),
            command
        );
        assert_eq!(command.as_reducer_mutation().schema_version, 1);
        assert_eq!(command.operation(), "create");
    }

    #[test]
    fn command_rejects_nil_assignment_fence() {
        let command = ProjectObjectCommand::new(
            7,
            Some(Uuid::nil()),
            MutationRequest::Create(CreateMutation {
                object: NewProjectViewObject::Goal {
                    id: Uuid::new_v4(),
                    title: "Goal".to_owned(),
                    desired_outcome: "Outcome".to_owned(),
                    directions: Vec::new(),
                },
            }),
        );

        assert!(matches!(
            command.validate_for_submission(),
            Err(DomainError::InvalidField {
                field: "acting_assignment_id",
                ..
            })
        ));
    }

    #[test]
    fn v1_wire_envelope_is_not_accepted_as_v2() {
        let content = serde_json::json!({
            "schema_version": 1,
            "expected_project_revision": 7,
            "request": {
                "type": "delete",
                "object_type": "goal",
                "object_id": Uuid::new_v4(),
            },
        })
        .to_string();

        assert!(matches!(
            ProjectObjectCommand::from_json(&content),
            Err(DomainError::UnsupportedSchemaVersion {
                got: 1,
                supported: 2,
            })
        ));
    }
}
