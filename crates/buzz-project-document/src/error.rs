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
    /// The caller's expected revision differs from current canonical state.
    #[error("Project Document revision conflict: expected {expected}, current {actual:?}")]
    RevisionConflict {
        /// Revision carried by the signed command.
        expected: u64,
        /// Current revision, or `None` when the identity has never existed.
        actual: Option<u64>,
    },
    /// A create attempted to reuse an active or tombstoned identity.
    #[error("Project Document id {document_id} has already been used")]
    DocumentIdAlreadyExists {
        /// Permanently occupied Document identity.
        document_id: Uuid,
    },
    /// An update or delete targeted an identity that has never existed.
    #[error("Project Document {document_id} was not found")]
    DocumentNotFound {
        /// Missing Document identity.
        document_id: Uuid,
    },
    /// An operation targeted a tombstoned identity.
    #[error("Project Document {document_id} has been deleted")]
    DocumentDeleted {
        /// Permanently tombstoned Document identity.
        document_id: Uuid,
    },
    /// An update supplied the exact current full snapshot.
    #[error("Project Document update does not change the current snapshot")]
    NoChange,
    /// A delete was blocked by an active cross-domain reference.
    #[error("Project Document {document_id} is still referenced")]
    StillReferenced {
        /// Referenced Document identity.
        document_id: Uuid,
    },
    /// No next JavaScript-safe Document or catalog revision can be allocated.
    #[error("Project Document revision space is exhausted")]
    RevisionExhausted,
    /// Trusted canonical inputs violate the pure state invariants.
    #[error("invalid canonical Project Document state: {reason}")]
    InvalidCanonicalState {
        /// Safe state diagnostic that never includes body content.
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
            Self::RevisionConflict { .. } => "revision",
            Self::DocumentIdAlreadyExists { .. } => "id_exists",
            Self::DocumentNotFound { .. } | Self::DocumentDeleted { .. } => "revision_target",
            Self::NoChange => "no_change",
            Self::StillReferenced { .. } => "still_referenced",
            Self::RevisionExhausted => "revision_target",
            Self::InvalidCanonicalState { .. } => "invalid_snapshot",
        }
    }
}
