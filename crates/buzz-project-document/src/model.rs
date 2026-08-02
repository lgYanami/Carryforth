//! Closed Project Document lifecycle and full-snapshot types.

use buzz_core::{CommunityId, PublicKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::validation::{
    deserialize_optional_non_null, validate_document_id, validate_nonnegative_revision,
    validate_positive_revision, validate_snapshot,
};
use crate::{DocumentError, DocumentResult, PROJECT_DOCUMENT_SCHEMA_VERSION};

/// Current or historical lifecycle state of a Project Document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentState {
    /// The revision carries a complete Markdown snapshot.
    Active,
    /// The revision is a bodyless tombstone.
    Deleted,
}

impl DocumentState {
    /// Stable wire and database spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }
}

/// Stable operation names used by commands, receipts, audit, and telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentOperation {
    /// Allocate a new Document identity at revision one.
    Create,
    /// Replace the complete active snapshot.
    Update,
    /// Append a bodyless tombstone revision.
    Delete,
}

impl DocumentOperation {
    /// Stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// Complete business snapshot carried by every active Document revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSnapshot {
    /// Canonical non-empty title.
    pub title: String,
    /// Optional short description. `None` is encoded by omitting the field.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
    /// Exact Markdown bytes, including whitespace and line endings.
    pub content_markdown: String,
}

impl DocumentSnapshot {
    /// Validate all v1 byte, canonical-text, and NUL constraints.
    pub fn validate(&self) -> DocumentResult<()> {
        validate_snapshot(&self.title, self.summary.as_deref(), &self.content_markdown)
    }
}

/// One immutable canonical Project Document revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentRevision {
    /// A complete active snapshot.
    Active {
        /// Must equal one for this contract.
        schema_version: u16,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive Document-local revision.
        document_revision: u64,
        /// Complete business snapshot.
        snapshot: DocumentSnapshot,
        /// Verified actor that produced this revision.
        actor: PublicKey,
        /// Relay-assigned canonical acceptance time.
        canonical_at: DateTime<Utc>,
    },
    /// A bodyless tombstone. Deleted business fields cannot be represented.
    Deleted {
        /// Must equal one for this contract.
        schema_version: u16,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive Document-local revision.
        document_revision: u64,
        /// Verified deleting actor.
        actor: PublicKey,
        /// Relay-assigned canonical deletion time.
        canonical_at: DateTime<Utc>,
    },
}

impl DocumentRevision {
    /// Validate the schema, identity, revision, and lifecycle-specific body.
    pub fn validate(&self) -> DocumentResult<()> {
        let (schema_version, document_id, document_revision) = match self {
            Self::Active {
                schema_version,
                document_id,
                document_revision,
                snapshot,
                ..
            } => {
                snapshot.validate()?;
                (*schema_version, *document_id, *document_revision)
            }
            Self::Deleted {
                schema_version,
                document_id,
                document_revision,
                ..
            } => (*schema_version, *document_id, *document_revision),
        };
        if schema_version != PROJECT_DOCUMENT_SCHEMA_VERSION {
            return Err(crate::DocumentError::UnsupportedSchemaVersion {
                got: schema_version,
                supported: PROJECT_DOCUMENT_SCHEMA_VERSION,
            });
        }
        validate_document_id(document_id)?;
        validate_positive_revision(document_revision, "document_revision")
    }

    /// Stable Document identity carried by this revision.
    #[must_use]
    pub const fn document_id(&self) -> Uuid {
        match self {
            Self::Active { document_id, .. } | Self::Deleted { document_id, .. } => *document_id,
        }
    }

    /// Positive Document-local revision number.
    #[must_use]
    pub const fn document_revision(&self) -> u64 {
        match self {
            Self::Active {
                document_revision, ..
            }
            | Self::Deleted {
                document_revision, ..
            } => *document_revision,
        }
    }

    /// Lifecycle state represented by this immutable revision.
    #[must_use]
    pub const fn state(&self) -> DocumentState {
        match self {
            Self::Active { .. } => DocumentState::Active,
            Self::Deleted { .. } => DocumentState::Deleted,
        }
    }

    /// Complete active snapshot, absent for a tombstone revision.
    #[must_use]
    pub const fn snapshot(&self) -> Option<&DocumentSnapshot> {
        match self {
            Self::Active { snapshot, .. } => Some(snapshot),
            Self::Deleted { .. } => None,
        }
    }

    /// Verified actor that authored this revision.
    #[must_use]
    pub const fn actor(&self) -> PublicKey {
        match self {
            Self::Active { actor, .. } | Self::Deleted { actor, .. } => *actor,
        }
    }

    /// Relay-assigned canonical time for this revision.
    #[must_use]
    pub const fn canonical_at(&self) -> DateTime<Utc> {
        match self {
            Self::Active { canonical_at, .. } | Self::Deleted { canonical_at, .. } => *canonical_at,
        }
    }
}

/// One actor/time pair retained in canonical current state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentAttribution {
    /// Canonical PostgreSQL time.
    pub at: DateTime<Utc>,
    /// Verified command signer.
    pub by: PublicKey,
}

/// Lightweight canonical current row for one permanently allocated Document ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectDocument {
    document_id: Uuid,
    current_revision: u64,
    state: DocumentState,
    created: DocumentAttribution,
    updated: DocumentAttribution,
}

impl ProjectDocument {
    /// Reconstruct and validate one trusted canonical current row.
    pub fn from_snapshot(
        document_id: Uuid,
        current_revision: u64,
        state: DocumentState,
        created: DocumentAttribution,
        updated: DocumentAttribution,
    ) -> DocumentResult<Self> {
        let document = Self {
            document_id,
            current_revision,
            state,
            created,
            updated,
        };
        document.validate()?;
        Ok(document)
    }

    /// Validate identity, revision, and monotonic provenance.
    pub fn validate(&self) -> DocumentResult<()> {
        validate_document_id(self.document_id)?;
        validate_positive_revision(self.current_revision, "current_revision")?;
        if self.updated.at < self.created.at {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "Document updated_at precedes created_at".to_owned(),
            });
        }
        Ok(())
    }

    /// Stable Document identity.
    #[must_use]
    pub const fn document_id(&self) -> Uuid {
        self.document_id
    }

    /// Current positive Document-local revision.
    #[must_use]
    pub const fn current_revision(&self) -> u64 {
        self.current_revision
    }

    /// Current active or deleted lifecycle state.
    #[must_use]
    pub const fn state(&self) -> DocumentState {
        self.state
    }

    /// Immutable creation attribution.
    #[must_use]
    pub const fn created(&self) -> DocumentAttribution {
        self.created
    }

    /// Attribution of the current revision.
    #[must_use]
    pub const fn updated(&self) -> DocumentAttribution {
        self.updated
    }
}

/// Canonical current row paired with its exact immutable revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurrentDocument {
    document: ProjectDocument,
    revision: DocumentRevision,
}

impl CurrentDocument {
    /// Pair and validate a current row with the revision it names.
    pub fn new(document: ProjectDocument, revision: DocumentRevision) -> DocumentResult<Self> {
        document.validate()?;
        revision.validate()?;
        if document.document_id != revision.document_id()
            || document.current_revision != revision.document_revision()
            || document.state != revision.state()
            || document.updated.by != revision.actor()
            || document.updated.at != revision.canonical_at()
        {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "current Document row does not match its immutable revision".to_owned(),
            });
        }
        if document.current_revision == 1
            && (document.created.by != revision.actor()
                || document.created.at != revision.canonical_at())
        {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "revision one does not match Document creation provenance".to_owned(),
            });
        }
        Ok(Self { document, revision })
    }

    /// Lightweight current row.
    #[must_use]
    pub const fn document(&self) -> &ProjectDocument {
        &self.document
    }

    /// Exact immutable current revision.
    #[must_use]
    pub const fn revision(&self) -> &DocumentRevision {
        &self.revision
    }

    /// Validate current/revision parity again.
    pub fn validate(&self) -> DocumentResult<()> {
        Self::new(self.document.clone(), self.revision.clone()).map(|_| ())
    }
}

/// Canonical metadata for one initialized Community Document catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentCatalog {
    project_id: CommunityId,
    catalog_revision: u64,
    active_document_count: u64,
    projection_generation: u64,
    initialized_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl DocumentCatalog {
    /// Construct the revision-zero initialized empty catalog.
    pub fn empty(
        project_id: CommunityId,
        projection_generation: u64,
        initialized_at: DateTime<Utc>,
    ) -> DocumentResult<Self> {
        Self::from_snapshot(
            project_id,
            0,
            0,
            projection_generation,
            initialized_at,
            initialized_at,
        )
    }

    /// Reconstruct and validate trusted catalog metadata.
    pub fn from_snapshot(
        project_id: CommunityId,
        catalog_revision: u64,
        active_document_count: u64,
        projection_generation: u64,
        initialized_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> DocumentResult<Self> {
        let catalog = Self {
            project_id,
            catalog_revision,
            active_document_count,
            projection_generation,
            initialized_at,
            updated_at,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Validate catalog identity, counters, generation, and canonical time.
    pub fn validate(&self) -> DocumentResult<()> {
        if self.project_id.as_uuid().is_nil() {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "project_id cannot be nil".to_owned(),
            });
        }
        validate_nonnegative_revision(self.catalog_revision, "catalog_revision")?;
        validate_nonnegative_revision(self.active_document_count, "active_document_count")?;
        validate_positive_revision(self.projection_generation, "projection_generation")?;
        if self.catalog_revision == 0 && self.active_document_count != 0 {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "catalog revision zero must have no active Documents".to_owned(),
            });
        }
        if self.updated_at < self.initialized_at {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "catalog updated_at precedes initialized_at".to_owned(),
            });
        }
        Ok(())
    }

    /// Host-derived Community/Project identity.
    #[must_use]
    pub const fn project_id(&self) -> CommunityId {
        self.project_id
    }

    /// Monotonic catalog observation revision.
    #[must_use]
    pub const fn catalog_revision(&self) -> u64 {
        self.catalog_revision
    }

    /// Number of current active Documents.
    #[must_use]
    pub const fn active_document_count(&self) -> u64 {
        self.active_document_count
    }

    /// Active Relay projection signer generation.
    #[must_use]
    pub const fn projection_generation(&self) -> u64 {
        self.projection_generation
    }

    /// Canonical catalog initialization time.
    #[must_use]
    pub const fn initialized_at(&self) -> DateTime<Utc> {
        self.initialized_at
    }

    /// Canonical time of the latest catalog transition.
    #[must_use]
    pub const fn updated_at(&self) -> DateTime<Utc> {
        self.updated_at
    }
}
