//! Closed, stable errors returned by the pure Project View domain layer.

use uuid::Uuid;

use crate::model::ProjectViewObjectType;

/// Convenient result type for Project View domain operations.
pub type DomainResult<T> = Result<T, DomainError>;

/// A closed set of failures produced while parsing, validating, or applying a
/// Project View mutation.
///
/// Human-readable messages are diagnostic. Protocol adapters must use
/// [`DomainError::code`] as the stable, low-cardinality machine identifier.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// Mutation content is not valid JSON or does not match the closed schema.
    #[error("invalid Project View mutation JSON: {reason}")]
    InvalidMutationJson {
        /// Safe parser or schema diagnostic.
        reason: String,
    },

    /// Raw mutation content exceeds the Project View-specific byte limit.
    #[error("mutation content exceeds {max} UTF-8 bytes (got {actual})")]
    MutationContentTooLarge {
        /// Maximum accepted content length.
        max: usize,
        /// Actual content length.
        actual: usize,
    },

    /// Parsed mutation JSON exceeds the supported nesting depth.
    #[error("mutation JSON nesting depth exceeds {max} (got {actual})")]
    MutationJsonTooDeep {
        /// Maximum accepted nesting depth.
        max: usize,
        /// Actual nesting depth.
        actual: usize,
    },

    /// The requested wire schema version is not supported.
    #[error("unsupported Project View schema version {got}; supported version is {supported}")]
    UnsupportedSchemaVersion {
        /// Version received from the caller.
        got: u32,
        /// Version implemented by this domain library.
        supported: u32,
    },

    /// The Project View has not yet been initialized.
    #[error("Project View is not initialized")]
    NotInitialized,

    /// The Project View has already been initialized.
    #[error("Project View is already initialized")]
    AlreadyInitialized,

    /// A revision cannot be represented safely by supported clients.
    #[error("revision {revision} exceeds the maximum supported value {max}")]
    RevisionOutOfRange {
        /// Revision rejected by the domain layer.
        revision: u64,
        /// Largest supported revision.
        max: u64,
    },

    /// The caller's expected project revision differs from current state.
    #[error("project revision conflict: expected {expected}, current {actual}")]
    RevisionConflict {
        /// Revision on which the caller based its mutation.
        expected: u64,
        /// Current canonical project revision.
        actual: u64,
    },

    /// No further safe revision can be allocated.
    #[error("Project View revision space is exhausted")]
    RevisionExhausted,

    /// Initialization did not contain the required number of goals.
    #[error("initial goal count {actual} is outside the allowed range {min}..={max}")]
    InvalidInitialGoalCount {
        /// Minimum accepted initial goal count.
        min: usize,
        /// Maximum accepted initial goal count.
        max: usize,
        /// Goal count supplied by the caller.
        actual: usize,
    },

    /// A client-created object ID is not a UUID v4.
    #[error("object id {object_id} must be a UUID v4")]
    InvalidObjectId {
        /// Invalid object identifier.
        object_id: Uuid,
    },

    /// A non-profile object attempted to use the reserved profile ID.
    #[error("object id {object_id} is reserved for the project profile")]
    ReservedProfileId {
        /// Reserved profile identifier.
        object_id: Uuid,
    },

    /// An object ID has already been used, including by a tombstone.
    #[error("object id {object_id} has already been used")]
    ObjectIdAlreadyUsed {
        /// Previously used object identifier.
        object_id: Uuid,
    },

    /// The requested object does not exist in the current project.
    #[error("object {object_id} was not found")]
    ObjectNotFound {
        /// Missing object identifier.
        object_id: Uuid,
    },

    /// The requested object exists only as a tombstone.
    #[error("object {object_id} has been deleted")]
    ObjectDeleted {
        /// Deleted object identifier.
        object_id: Uuid,
    },

    /// An operation declared a type different from the stored object type.
    #[error("object {object_id} has type {actual}, not {expected}")]
    ObjectTypeMismatch {
        /// Object whose type did not match.
        object_id: Uuid,
        /// Type required by the operation.
        expected: ProjectViewObjectType,
        /// Canonical type of the object.
        actual: ProjectViewObjectType,
    },

    /// An object's data variant disagrees with its declared type.
    #[error("object data has type {actual}, not declared type {declared}")]
    DataTypeMismatch {
        /// Type declared in the object envelope.
        declared: ProjectViewObjectType,
        /// Type carried by the strongly typed data variant.
        actual: ProjectViewObjectType,
    },

    /// The profile was addressed through an operation that cannot create it.
    #[error("the project profile can only be created by initialization")]
    ProfileCreateForbidden,

    /// An immutable identity or provenance field was included in an update.
    #[error("field {field} is immutable")]
    ImmutableField {
        /// Immutable field named by the invalid update.
        field: &'static str,
    },

    /// A field value is malformed or violates a field-specific rule.
    #[error("invalid field {field}: {reason}")]
    InvalidField {
        /// Stable field name.
        field: &'static str,
        /// Safe diagnostic explanation.
        reason: String,
    },

    /// A required text or value is absent or empty after trimming.
    #[error("field {field} is required")]
    RequiredField {
        /// Stable field name.
        field: &'static str,
    },

    /// A string field exceeds its UTF-8 byte limit.
    #[error("field {field} exceeds {max} UTF-8 bytes (got {actual})")]
    FieldTooLong {
        /// Stable field name.
        field: &'static str,
        /// Maximum accepted UTF-8 byte length.
        max: usize,
        /// Actual UTF-8 byte length.
        actual: usize,
    },

    /// A bounded list contains too many entries.
    #[error("field {field} has too many items: maximum {max}, got {actual}")]
    TooManyItems {
        /// Stable field name.
        field: &'static str,
        /// Maximum accepted item count.
        max: usize,
        /// Actual item count.
        actual: usize,
    },

    /// A resource locator is syntactically or semantically invalid.
    #[error("invalid resource locator: {reason}")]
    InvalidLocator {
        /// Safe diagnostic explanation.
        reason: String,
    },

    /// A relation violates a relation-specific invariant.
    #[error("invalid relation {relation}: {reason}")]
    InvalidRelation {
        /// Stable relation name.
        relation: &'static str,
        /// Safe diagnostic explanation.
        reason: String,
    },

    /// A relation required by the source object type is absent.
    #[error("required relation {relation} is missing")]
    MissingRequiredRelation {
        /// Stable relation name.
        relation: &'static str,
    },

    /// A source object type attempted to use an unsupported relation slot.
    #[error("relation {relation} is not allowed on {object_type}")]
    RelationNotAllowed {
        /// Stable relation name.
        relation: &'static str,
        /// Source object type.
        object_type: ProjectViewObjectType,
    },

    /// A relation points to an object that does not exist.
    #[error("relation {relation} target {target_id} was not found")]
    RelationTargetNotFound {
        /// Stable relation name.
        relation: &'static str,
        /// Missing target identifier.
        target_id: Uuid,
    },

    /// A relation points to a tombstoned object.
    #[error("relation {relation} target {target_id} has been deleted")]
    RelationTargetDeleted {
        /// Stable relation name.
        relation: &'static str,
        /// Deleted target identifier.
        target_id: Uuid,
    },

    /// A relation target's canonical type differs from its declared type.
    #[error(
        "relation {relation} target {target_id} has type {actual}, not declared type {declared}"
    )]
    RelationTargetTypeMismatch {
        /// Stable relation name.
        relation: &'static str,
        /// Referenced target identifier.
        target_id: Uuid,
        /// Type declared in the reference.
        declared: ProjectViewObjectType,
        /// Canonical type of the referenced object.
        actual: ProjectViewObjectType,
    },

    /// An Issue attempted to make itself its own `about` target.
    #[error("relation {relation} cannot reference its source object {object_id}")]
    SelfReference {
        /// Stable relation name.
        relation: &'static str,
        /// Self-referenced object identifier.
        object_id: Uuid,
    },

    /// A Work relation targets something other than a Requirement or Issue.
    #[error("work handles target must be a requirement or issue, got {actual}")]
    InvalidWorkTarget {
        /// Disallowed target type.
        actual: ProjectViewObjectType,
    },

    /// An object cannot be deleted while active objects still reference it.
    #[error("object {object_id} is still referenced through {relation}")]
    ObjectStillReferenced {
        /// Object whose deletion was rejected.
        object_id: Uuid,
        /// Stable incoming relation name.
        relation: &'static str,
    },

    /// The project's profile cannot be deleted.
    #[error("the project profile cannot be deleted")]
    ProfileDeletionForbidden,

    /// The last active goal cannot be deleted.
    #[error("the last active goal cannot be deleted")]
    LastGoalDeletionForbidden,

    /// An update contained no effective change.
    #[error("mutation would not change Project View state")]
    NoChanges,

    /// The resulting full state violates a cross-object invariant.
    #[error("invalid final Project View state: {reason}")]
    InvalidFinalState {
        /// Safe diagnostic explanation.
        reason: String,
    },
}

impl DomainError {
    /// Return the stable low-cardinality protocol code for this error.
    ///
    /// Callers prepend the appropriate protocol class, for example
    /// `invalid:project_view:` or `conflict:project_view:`.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::InvalidMutationJson { .. } => "invalid_json",
            Self::MutationContentTooLarge { .. } => "content_too_large",
            Self::MutationJsonTooDeep { .. } => "json_too_deep",
            Self::UnsupportedSchemaVersion { .. } => "schema_version",
            Self::NotInitialized => "not_initialized",
            Self::AlreadyInitialized => "already_initialized",
            Self::RevisionOutOfRange { .. } => "revision_out_of_range",
            Self::RevisionConflict { .. } => "revision",
            Self::RevisionExhausted => "revision_exhausted",
            Self::InvalidInitialGoalCount { .. } => "initial_goal_count",
            Self::InvalidObjectId { .. } => "invalid_object_id",
            Self::ReservedProfileId { .. } => "reserved_profile_id",
            Self::ObjectIdAlreadyUsed { .. } => "object_id_used",
            Self::ObjectNotFound { .. } => "object_not_found",
            Self::ObjectDeleted { .. } => "object_deleted",
            Self::ObjectTypeMismatch { .. } => "object_type",
            Self::DataTypeMismatch { .. } => "data_type",
            Self::ProfileCreateForbidden => "profile_create_forbidden",
            Self::ImmutableField { .. } => "immutable_field",
            Self::InvalidField { .. } => "invalid_field",
            Self::RequiredField { .. } => "required_field",
            Self::FieldTooLong { .. } => "field_too_long",
            Self::TooManyItems { .. } => "too_many_items",
            Self::InvalidLocator { .. } => "invalid_locator",
            Self::InvalidRelation { .. } => "invalid_relation",
            Self::MissingRequiredRelation { .. } => "missing_relation",
            Self::RelationNotAllowed { .. } => "relation_not_allowed",
            Self::RelationTargetNotFound { .. } => "relation_target_not_found",
            Self::RelationTargetDeleted { .. } => "relation_target_deleted",
            Self::RelationTargetTypeMismatch { .. } => "relation_target_type",
            Self::SelfReference { .. } => "self_reference",
            Self::InvalidWorkTarget { .. } => "invalid_work_target",
            Self::ObjectStillReferenced { .. } => "object_referenced",
            Self::ProfileDeletionForbidden => "profile_delete_forbidden",
            Self::LastGoalDeletionForbidden => "last_goal_delete_forbidden",
            Self::NoChanges => "no_changes",
            Self::InvalidFinalState { .. } => "invalid_final_state",
        }
    }
}
