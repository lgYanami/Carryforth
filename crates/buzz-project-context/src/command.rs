//! Closed member-signed Project Context Edge v2 commands.

use buzz_core::RuntimeFence;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::coordinate::validate_canonical_coordinates;
use crate::validation::{
    deserialize_optional_non_null, json_depth, validate_document_id, validate_nonnegative,
};
use crate::{
    canonicalize_coordinates, ProjectContextCoordinate, ProjectContextError,
    ProjectContextOperation, ProjectContextResult, MAX_COMMAND_CONTENT_BYTES,
    MAX_COMMAND_JSON_DEPTH, PROJECT_CONTEXT_SCHEMA_VERSION,
};

/// A global-revision-checked Project Context Edge command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextCommand {
    /// Must equal the current Project Context schema version.
    pub schema_version: u16,
    /// Global Context revision observed by the caller.
    pub expected_context_revision: u64,
    /// Optional active Assignment explicitly claimed by a managed Agent.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub acting_assignment_id: Option<Uuid>,
    /// Exact supervised runtime paired with an explicitly claimed Assignment.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub runtime_fence: Option<RuntimeFence>,
    /// One closed attach or detach operation.
    pub request: ProjectContextCommandRequest,
}

impl ProjectContextCommand {
    /// Construct a canonical v2 command without a managed runtime fence.
    pub fn new(
        expected_context_revision: u64,
        operation: ProjectContextOperation,
        coordinates: Vec<ProjectContextCoordinate>,
        context_document_id: Uuid,
    ) -> ProjectContextResult<Self> {
        let coordinates = canonicalize_coordinates(coordinates)?;
        let request = match operation {
            ProjectContextOperation::Attach => ProjectContextCommandRequest::Attach {
                coordinates,
                context_document_id,
            },
            ProjectContextOperation::Detach => ProjectContextCommandRequest::Detach {
                coordinates,
                context_document_id,
            },
        };
        let command = Self {
            schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
            expected_context_revision,
            acting_assignment_id: None,
            runtime_fence: None,
            request,
        };
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Attach one exact Assignment and runtime fence pair.
    #[must_use]
    pub const fn with_runtime_fence(
        mut self,
        acting_assignment_id: Uuid,
        runtime_fence: RuntimeFence,
    ) -> Self {
        self.acting_assignment_id = Some(acting_assignment_id);
        self.runtime_fence = Some(runtime_fence);
        self
    }

    /// Parse JSON while enforcing byte, depth, closed-schema, and canonical-order rules.
    pub fn from_json(content: &str) -> ProjectContextResult<Self> {
        if content.len() > MAX_COMMAND_CONTENT_BYTES {
            return Err(ProjectContextError::ContentTooLarge {
                max: MAX_COMMAND_CONTENT_BYTES,
                actual: content.len(),
            });
        }
        let value: Value =
            serde_json::from_str(content).map_err(|error| ProjectContextError::InvalidJson {
                reason: error.to_string(),
            })?;
        let depth = json_depth(&value);
        if depth > MAX_COMMAND_JSON_DEPTH {
            return Err(ProjectContextError::JsonTooDeep {
                max: MAX_COMMAND_JSON_DEPTH,
                actual: depth,
            });
        }
        let command: Self =
            serde_json::from_value(value).map_err(|error| ProjectContextError::InvalidJson {
                reason: error.to_string(),
            })?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate all fields that do not require the host-derived Project identity.
    pub fn validate_for_submission(&self) -> ProjectContextResult<()> {
        if self.schema_version != PROJECT_CONTEXT_SCHEMA_VERSION {
            return Err(ProjectContextError::UnsupportedSchemaVersion {
                got: self.schema_version,
                supported: PROJECT_CONTEXT_SCHEMA_VERSION,
            });
        }
        validate_nonnegative(self.expected_context_revision, "expected_context_revision")?;
        match (self.acting_assignment_id, self.runtime_fence) {
            (None, None) => {}
            (Some(assignment_id), Some(runtime_fence)) => {
                if assignment_id.is_nil() {
                    return Err(ProjectContextError::InvalidRuntimeFence {
                        reason: "acting_assignment_id cannot be nil".to_owned(),
                    });
                }
                runtime_fence
                    .validate()
                    .map_err(|reason| ProjectContextError::InvalidRuntimeFence { reason })?;
            }
            _ => {
                return Err(ProjectContextError::InvalidRuntimeFence {
                    reason: "acting_assignment_id and runtime_fence must both be omitted or both be present"
                        .to_owned(),
                });
            }
        }
        self.request.validate()
    }

    /// Validate the command against the host-derived Project identity.
    pub fn validate_for_project(&self, project_id: Uuid) -> ProjectContextResult<()> {
        self.validate_for_submission()?;
        for coordinate in self.coordinates() {
            coordinate.validate_for_project(project_id)?;
        }
        Ok(())
    }

    /// Stable operation used by receipts and telemetry.
    #[must_use]
    pub const fn operation(&self) -> ProjectContextOperation {
        self.request.operation()
    }

    /// Canonical coordinate set identifying the edge.
    #[must_use]
    pub fn coordinates(&self) -> &[ProjectContextCoordinate] {
        self.request.coordinates()
    }

    /// Context Document whose binding changes.
    #[must_use]
    pub const fn context_document_id(&self) -> Uuid {
        self.request.context_document_id()
    }
}

/// One of the two operations supported by Project Context Edge v2.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectContextCommandRequest {
    /// Add one Context Document to the exact edge.
    Attach {
        /// Canonically sorted, distinct coordinate set.
        coordinates: Vec<ProjectContextCoordinate>,
        /// Active Document that explains cross-coordinate context.
        context_document_id: Uuid,
    },
    /// Remove one Context Document from the exact edge.
    Detach {
        /// Canonically sorted, distinct coordinate set.
        coordinates: Vec<ProjectContextCoordinate>,
        /// Currently bound Context Document.
        context_document_id: Uuid,
    },
}

impl ProjectContextCommandRequest {
    /// Stable operation name.
    #[must_use]
    pub const fn operation(&self) -> ProjectContextOperation {
        match self {
            Self::Attach { .. } => ProjectContextOperation::Attach,
            Self::Detach { .. } => ProjectContextOperation::Detach,
        }
    }

    /// Canonical coordinate set.
    #[must_use]
    pub fn coordinates(&self) -> &[ProjectContextCoordinate] {
        match self {
            Self::Attach { coordinates, .. } | Self::Detach { coordinates, .. } => coordinates,
        }
    }

    /// Target Context Document.
    #[must_use]
    pub const fn context_document_id(&self) -> Uuid {
        match self {
            Self::Attach {
                context_document_id,
                ..
            }
            | Self::Detach {
                context_document_id,
                ..
            } => *context_document_id,
        }
    }

    fn validate(&self) -> ProjectContextResult<()> {
        validate_canonical_coordinates(self.coordinates())?;
        validate_document_id(self.context_document_id())
    }
}
