//! Closed errors produced by the pure Project Document contract layer.

use uuid::Uuid;

/// Convenient result type for Project Document protocol operations.
pub type DocumentResult<T> = Result<T, DocumentError>;

/// Closed failures produced while parsing or validating Document v1 values.
///
/// Adapters use [`DocumentError::code`] as the stable low-cardinality reason
/// after choosing a server class such as `invalid:project_document:`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DocumentError {
    /// Content is not JSON or does not match the closed schema.
    #[error("invalid Project Document JSON: {reason}")]
    InvalidJson {
        /// Safe parser diagnostic; never contains Document Markdown from a
        /// successfully parsed command.
        reason: String,
    },
    /// Raw command content exceeded the protocol byte limit.
    #[error("Project Document command exceeds {max} UTF-8 bytes (got {actual})")]
    ContentTooLarge {
        /// Maximum accepted byte length.
        max: usize,
        /// Actual byte length.
        actual: usize,
    },
    /// Parsed JSON exceeded the protocol nesting limit.
    #[error("Project Document JSON nesting exceeds {max} (got {actual})")]
    JsonTooDeep {
        /// Maximum accepted depth.
        max: usize,
        /// Actual depth.
        actual: usize,
    },
    /// A command or projection used another major schema.
    #[error("unsupported Project Document schema version {got}; supported version is {supported}")]
    UnsupportedSchemaVersion {
        /// Received schema number.
        got: u16,
        /// Version implemented by this contract.
        supported: u16,
    },
    /// A client-owned Document identity was not a canonical UUID v4.
    #[error("document id {document_id} must be an RFC 4122 UUID v4")]
    InvalidDocumentId {
        /// Rejected identifier.
        document_id: Uuid,
    },
    /// A revision or generation was outside its legal range or operation.
    #[error("invalid revision target: {reason}")]
    InvalidRevisionTarget {
        /// Safe revision diagnostic.
        reason: String,
    },
    /// Assignment and runtime fencing fields did not form one canonical pair.
    #[error("invalid managed runtime fence: {reason}")]
    InvalidRuntimeFence {
        /// Safe fence diagnostic.
        reason: String,
    },
    /// A title, summary, Markdown body, lifecycle shape, or coordinate was invalid.
    #[error("invalid Project Document snapshot: {reason}")]
    InvalidSnapshot {
        /// Safe field/shape diagnostic.
        reason: String,
    },
    /// A projection or receipt violated a pointer or lifecycle invariant.
    #[error("invalid Project Document projection: {reason}")]
    InvalidProjection {
        /// Safe projection diagnostic.
        reason: String,
    },
}

impl DocumentError {
    /// Stable protocol reason appended to an adapter-selected error class.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson { .. } => "invalid_json",
            Self::ContentTooLarge { .. } => "content_too_large",
            Self::JsonTooDeep { .. } => "invalid_json",
            Self::UnsupportedSchemaVersion { .. } => "schema",
            Self::InvalidDocumentId { .. } => "invalid_document_id",
            Self::InvalidRevisionTarget { .. } => "revision_target",
            Self::InvalidRuntimeFence { .. } => "runtime_fence",
            Self::InvalidSnapshot { .. } => "invalid_snapshot",
            Self::InvalidProjection { .. } => "invalid_snapshot",
        }
    }
}
