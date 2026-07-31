//! Closed Project Document lifecycle and full-snapshot types.

use buzz_core::PublicKey;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::validation::{deserialize_optional_non_null, validate_document_id, validate_snapshot};
use crate::{DocumentResult, PROJECT_DOCUMENT_SCHEMA_VERSION};

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
        crate::validation::validate_positive_revision(document_revision, "document_revision")
    }
}
