//! Closed errors produced by the pure Project Context Edge contract layer.

use uuid::Uuid;

use crate::EdgeKey;

/// Convenient result type for Project Context protocol operations.
pub type ProjectContextResult<T> = Result<T, ProjectContextError>;

/// Closed failures produced while parsing, validating, or reducing v2 values.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProjectContextError {
    /// Content is not JSON or does not match the closed schema.
    #[error("invalid Project Context JSON: {reason}")]
    InvalidJson {
        /// Safe parser diagnostic.
        reason: String,
    },
    /// Raw command content exceeded the protocol byte limit.
    #[error("Project Context command exceeds {max} UTF-8 bytes (got {actual})")]
    ContentTooLarge {
        /// Maximum accepted byte length.
        max: usize,
        /// Actual byte length.
        actual: usize,
    },
    /// Parsed JSON exceeded the protocol nesting limit.
    #[error("Project Context JSON nesting exceeds {max} (got {actual})")]
    JsonTooDeep {
        /// Maximum accepted depth.
        max: usize,
        /// Actual depth.
        actual: usize,
    },
    /// A command or projection used another major schema.
    #[error("unsupported Project Context schema version {got}; supported version is {supported}")]
    UnsupportedSchemaVersion {
        /// Received schema number.
        got: u16,
        /// Version implemented by this contract.
        supported: u16,
    },
    /// A coordinate identity was malformed or outside the closed v2 union.
    #[error("invalid Project Context coordinate: {reason}")]
    InvalidCoordinate {
        /// Safe coordinate diagnostic.
        reason: String,
    },
    /// A coordinate occurred more than once in one edge identity.
    #[error("duplicate Project Context coordinate")]
    DuplicateCoordinate,
    /// An edge carried fewer than the minimum number of coordinates.
    #[error("Project Context edge requires at least {minimum} coordinates (got {actual})")]
    TooFewCoordinates {
        /// Protocol minimum.
        minimum: usize,
        /// Actual count.
        actual: usize,
    },
    /// Coordinates were not supplied in their canonical order.
    #[error("Project Context coordinates are not in canonical order")]
    NonCanonicalCoordinates,
    /// A context Document identity was not an RFC 4122 UUID v4.
    #[error("context document id {document_id} must be an RFC 4122 UUID v4")]
    InvalidDocumentId {
        /// Rejected identity.
        document_id: Uuid,
    },
    /// An edge key was not its canonical lowercase SHA-256 representation.
    #[error("invalid Project Context edge key: {reason}")]
    InvalidEdgeKey {
        /// Safe key diagnostic.
        reason: String,
    },
    /// A revision, count, or generation was outside its legal range.
    #[error("invalid Project Context revision or count: {reason}")]
    InvalidRevision {
        /// Safe revision diagnostic.
        reason: String,
    },
    /// Assignment and runtime fencing fields did not form one canonical pair.
    #[error("invalid managed runtime fence: {reason}")]
    InvalidRuntimeFence {
        /// Safe fence diagnostic.
        reason: String,
    },
    /// A projection or receipt violated a closed wire invariant.
    #[error("invalid Project Context projection: {reason}")]
    InvalidProjection {
        /// Safe projection diagnostic.
        reason: String,
    },
    /// Serialized derived content exceeded the projection byte limit.
    #[error("Project Context projection exceeds {max} UTF-8 bytes (got {actual})")]
    ProjectionTooLarge {
        /// Maximum accepted byte length.
        max: usize,
        /// Actual byte length.
        actual: usize,
    },
    /// The caller's expected catalog revision differs from canonical state.
    #[error("Project Context revision conflict: expected {expected}, current {actual}")]
    RevisionConflict {
        /// Revision carried by the signed command.
        expected: u64,
        /// Current catalog revision.
        actual: u64,
    },
    /// An attach used a coordinate that is not currently active.
    #[error("Project Context attach contains an inactive coordinate")]
    InactiveCoordinate,
    /// The Document used as context is not currently active.
    #[error("Project Context document {document_id} is not active")]
    InactiveContextDocument {
        /// Rejected Document identity.
        document_id: Uuid,
    },
    /// The context Document is already bound, possibly to another edge.
    #[error("Project Context document {document_id} is already bound to edge {edge_key}")]
    DocumentAlreadyBound {
        /// Already-bound Document identity.
        document_id: Uuid,
        /// Existing active edge.
        edge_key: EdgeKey,
    },
    /// A new event repeated an already-active binding without changing state.
    #[error("Project Context attach does not change the active binding")]
    NoChange,
    /// A detach targeted no active binding.
    #[error("Project Context document {document_id} is not actively bound")]
    BindingNotFound {
        /// Missing active binding.
        document_id: Uuid,
    },
    /// A detach's coordinate set did not identify the Document's active edge.
    #[error("Project Context document {document_id} is bound to another edge {actual_edge_key}")]
    BindingEdgeMismatch {
        /// Target Document identity.
        document_id: Uuid,
        /// Actual active edge key.
        actual_edge_key: EdgeKey,
    },
    /// No next JavaScript-safe revision or count can be allocated.
    #[error("Project Context revision space is exhausted")]
    RevisionExhausted,
    /// Trusted canonical inputs violate pure state invariants.
    #[error("invalid canonical Project Context state: {reason}")]
    InvalidCanonicalState {
        /// Safe state diagnostic.
        reason: String,
    },
}

impl ProjectContextError {
    /// Stable protocol reason appended to an adapter-selected error class.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidJson { .. } | Self::JsonTooDeep { .. } => "invalid_json",
            Self::ContentTooLarge { .. } => "content_too_large",
            Self::UnsupportedSchemaVersion { .. } => "schema",
            Self::InvalidCoordinate { .. }
            | Self::DuplicateCoordinate
            | Self::TooFewCoordinates { .. }
            | Self::NonCanonicalCoordinates => "coordinates",
            Self::InvalidDocumentId { .. } => "invalid_document_id",
            Self::InvalidEdgeKey { .. } => "edge_key",
            Self::InvalidRevision { .. } | Self::RevisionExhausted => "revision",
            Self::InvalidRuntimeFence { .. } => "runtime_fence",
            Self::InvalidProjection { .. } | Self::ProjectionTooLarge { .. } => "projection",
            Self::RevisionConflict { .. } => "revision",
            Self::InactiveCoordinate => "inactive_coordinate",
            Self::InactiveContextDocument { .. } => "inactive_document",
            Self::DocumentAlreadyBound { .. } => "already_bound",
            Self::NoChange => "no_change",
            Self::BindingNotFound { .. } => "binding_not_found",
            Self::BindingEdgeMismatch { .. } => "edge_mismatch",
            Self::InvalidCanonicalState { .. } => "canonical_state",
        }
    }
}
