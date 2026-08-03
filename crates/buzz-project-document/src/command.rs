//! Closed member-signed Project Document v1 commands.

use buzz_core::RuntimeFence;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::validation::{
    deserialize_optional_non_null, json_depth, validate_document_id, validate_nonnegative_revision,
    validate_positive_revision, validate_snapshot,
};
use crate::{
    DocumentError, DocumentOperation, DocumentResult, DocumentSnapshot, MAX_COMMAND_CONTENT_BYTES,
    MAX_COMMAND_JSON_DEPTH, PROJECT_DOCUMENT_SCHEMA_VERSION,
};

/// A revision-checked Project Document command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDocumentCommand {
    /// Must equal one.
    pub schema_version: u16,
    /// Revision observed by the caller: zero for create, positive for update or delete.
    pub expected_document_revision: u64,
    /// Optional active Assignment explicitly claimed by a managed Agent.
    /// Human and ordinary Community-authority commands omit it.
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
    /// One closed create, update, or delete operation.
    pub request: DocumentCommandRequest,
}

impl ProjectDocumentCommand {
    /// Construct a v1 command without a managed runtime fence.
    #[must_use]
    pub const fn new(expected_document_revision: u64, request: DocumentCommandRequest) -> Self {
        Self {
            schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
            expected_document_revision,
            acting_assignment_id: None,
            runtime_fence: None,
            request,
        }
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

    /// Parse JSON while enforcing the Document-specific byte, depth, schema,
    /// and canonical-value constraints.
    pub fn from_json(content: &str) -> DocumentResult<Self> {
        if content.len() > MAX_COMMAND_CONTENT_BYTES {
            return Err(DocumentError::ContentTooLarge {
                max: MAX_COMMAND_CONTENT_BYTES,
                actual: content.len(),
            });
        }
        let value: Value =
            serde_json::from_str(content).map_err(|error| DocumentError::InvalidJson {
                reason: error.to_string(),
            })?;
        let depth = json_depth(&value);
        if depth > MAX_COMMAND_JSON_DEPTH {
            return Err(DocumentError::JsonTooDeep {
                max: MAX_COMMAND_JSON_DEPTH,
                actual: depth,
            });
        }
        let command: Self =
            serde_json::from_value(value).map_err(|error| DocumentError::InvalidJson {
                reason: error.to_string(),
            })?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate all fields that do not require canonical Relay state.
    pub fn validate_for_submission(&self) -> DocumentResult<()> {
        if self.schema_version != PROJECT_DOCUMENT_SCHEMA_VERSION {
            return Err(DocumentError::UnsupportedSchemaVersion {
                got: self.schema_version,
                supported: PROJECT_DOCUMENT_SCHEMA_VERSION,
            });
        }
        validate_nonnegative_revision(
            self.expected_document_revision,
            "expected_document_revision",
        )?;
        match self.request.operation() {
            DocumentOperation::Create if self.expected_document_revision != 0 => {
                return Err(DocumentError::InvalidRevisionTarget {
                    reason: "create requires expected_document_revision = 0".to_owned(),
                });
            }
            DocumentOperation::Update | DocumentOperation::Delete => {
                validate_positive_revision(
                    self.expected_document_revision,
                    "expected_document_revision",
                )?;
            }
            DocumentOperation::Create => {}
        }

        match (self.acting_assignment_id, self.runtime_fence) {
            (None, None) => {}
            (Some(assignment_id), Some(runtime_fence)) => {
                if assignment_id.is_nil() {
                    return Err(DocumentError::InvalidRuntimeFence {
                        reason: "acting_assignment_id cannot be nil".to_owned(),
                    });
                }
                runtime_fence
                    .validate()
                    .map_err(|reason| DocumentError::InvalidRuntimeFence { reason })?;
            }
            _ => {
                return Err(DocumentError::InvalidRuntimeFence {
                    reason: "acting_assignment_id and runtime_fence must both be omitted or both be present"
                        .to_owned(),
                });
            }
        }

        self.request.validate()
    }

    /// Stable operation used by receipts and telemetry.
    #[must_use]
    pub const fn operation(&self) -> DocumentOperation {
        self.request.operation()
    }

    /// Stable target Document identity.
    #[must_use]
    pub const fn document_id(&self) -> Uuid {
        self.request.document_id()
    }
}

/// One of the three operations supported by Project Document v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentCommandRequest {
    /// Allocate a new identity and complete revision-one snapshot.
    Create {
        /// Client-generated RFC 4122 UUID v4.
        document_id: Uuid,
        /// Canonical title.
        title: String,
        /// Optional summary; omit rather than encode `null` or an empty string.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        summary: Option<String>,
        /// Exact Markdown snapshot.
        content_markdown: String,
    },
    /// Replace the complete current active snapshot.
    Update {
        /// Existing stable Document identity.
        document_id: Uuid,
        /// Complete next title.
        title: String,
        /// Complete next summary.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        summary: Option<String>,
        /// Complete next Markdown snapshot.
        content_markdown: String,
    },
    /// Append a bodyless tombstone revision.
    Delete {
        /// Existing stable Document identity.
        document_id: Uuid,
    },
}

impl DocumentCommandRequest {
    /// Stable operation name.
    #[must_use]
    pub const fn operation(&self) -> DocumentOperation {
        match self {
            Self::Create { .. } => DocumentOperation::Create,
            Self::Update { .. } => DocumentOperation::Update,
            Self::Delete { .. } => DocumentOperation::Delete,
        }
    }

    /// Target Document identity.
    #[must_use]
    pub const fn document_id(&self) -> Uuid {
        match self {
            Self::Create { document_id, .. }
            | Self::Update { document_id, .. }
            | Self::Delete { document_id } => *document_id,
        }
    }

    /// Complete active snapshot for create/update, absent for delete.
    #[must_use]
    pub fn snapshot(&self) -> Option<DocumentSnapshot> {
        match self {
            Self::Create {
                title,
                summary,
                content_markdown,
                ..
            }
            | Self::Update {
                title,
                summary,
                content_markdown,
                ..
            } => Some(DocumentSnapshot {
                title: title.clone(),
                summary: summary.clone(),
                content_markdown: content_markdown.clone(),
            }),
            Self::Delete { .. } => None,
        }
    }

    fn validate(&self) -> DocumentResult<()> {
        validate_document_id(self.document_id())?;
        match self {
            Self::Create {
                title,
                summary,
                content_markdown,
                ..
            }
            | Self::Update {
                title,
                summary,
                content_markdown,
                ..
            } => validate_snapshot(title, summary.as_deref(), content_markdown),
            Self::Delete { .. } => Ok(()),
        }
    }
}
