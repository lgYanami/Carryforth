//! Relay-signed Project Document v1 projection and receipt wire types.

use buzz_core::{EventId, PublicKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::validation::{
    deserialize_optional_non_null, validate_document_id, validate_nonnegative_revision,
    validate_positive_revision, validate_snapshot,
};
use crate::{
    CurrentDocument, DocumentCatalog, DocumentError, DocumentOperation, DocumentResult,
    DocumentState, PROJECT_DOCUMENT_SCHEMA_VERSION,
};

/// Exact subtype discriminator carried in every Document projection body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentProjectionType {
    /// Current lightweight metadata for one Document.
    DocumentHead,
    /// One immutable full snapshot or bodyless tombstone.
    DocumentRevision,
    /// Current catalog observation boundary.
    DocumentMeta,
}

/// Current lightweight metadata for one Project Document.
///
/// The enum shape makes it impossible for a deleted head to retain title,
/// summary, or Markdown fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentHeadProjection {
    /// An active head pointing at a complete immutable revision.
    Active {
        /// Must equal one.
        schema_version: u16,
        /// Must equal `document_head`.
        projection_type: DocumentProjectionType,
        /// Host-derived Community/Project identity.
        project_id: Uuid,
        /// Active Relay signer generation.
        projection_generation: u64,
        /// Catalog observation revision committed with this head.
        catalog_revision: u64,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive Document-local revision.
        document_revision: u64,
        /// Current title.
        title: String,
        /// Current optional summary; omitted rather than encoded as `null`.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        summary: Option<String>,
        /// Canonical creation time retained across revisions.
        created_at: DateTime<Utc>,
        /// Verified creator retained across revisions.
        created_by: PublicKey,
        /// Canonical time of the current active revision.
        updated_at: DateTime<Utc>,
        /// Verified actor of the current active revision.
        updated_by: PublicKey,
        /// Canonical coordinate of the immutable revision.
        revision_coordinate: String,
        /// Exact signed immutable revision event.
        revision_event_id: EventId,
        /// Member command that committed this head.
        source_event_id: EventId,
    },
    /// A tombstone head that permanently reserves the Document identity.
    Deleted {
        /// Must equal one.
        schema_version: u16,
        /// Must equal `document_head`.
        projection_type: DocumentProjectionType,
        /// Host-derived Community/Project identity.
        project_id: Uuid,
        /// Active Relay signer generation.
        projection_generation: u64,
        /// Catalog observation revision committed with this head.
        catalog_revision: u64,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive tombstone revision.
        document_revision: u64,
        /// Canonical creation time retained from revision one.
        created_at: DateTime<Utc>,
        /// Verified creator retained from revision one.
        created_by: PublicKey,
        /// Canonical deletion time.
        deleted_at: DateTime<Utc>,
        /// Verified deleting actor.
        deleted_by: PublicKey,
        /// Canonical coordinate of the bodyless tombstone revision.
        revision_coordinate: String,
        /// Exact signed tombstone revision event.
        revision_event_id: EventId,
        /// Member command that committed this tombstone.
        source_event_id: EventId,
    },
}

impl DocumentHeadProjection {
    /// Validate all fields whose authority is contained in the projection.
    pub fn validate(&self) -> DocumentResult<()> {
        let (
            schema_version,
            projection_type,
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
            revision_coordinate,
        ) = match self {
            Self::Active {
                schema_version,
                projection_type,
                project_id,
                projection_generation,
                catalog_revision,
                document_id,
                document_revision,
                title,
                summary,
                created_at,
                updated_at,
                revision_coordinate,
                ..
            } => {
                validate_snapshot(title, summary.as_deref(), "")?;
                if updated_at < created_at {
                    return invalid_projection("updated_at precedes created_at");
                }
                (
                    *schema_version,
                    *projection_type,
                    *project_id,
                    *projection_generation,
                    *catalog_revision,
                    *document_id,
                    *document_revision,
                    revision_coordinate,
                )
            }
            Self::Deleted {
                schema_version,
                projection_type,
                project_id,
                projection_generation,
                catalog_revision,
                document_id,
                document_revision,
                created_at,
                deleted_at,
                revision_coordinate,
                ..
            } => {
                if deleted_at < created_at {
                    return invalid_projection("deleted_at precedes created_at");
                }
                (
                    *schema_version,
                    *projection_type,
                    *project_id,
                    *projection_generation,
                    *catalog_revision,
                    *document_id,
                    *document_revision,
                    revision_coordinate,
                )
            }
        };
        validate_projection_common(
            schema_version,
            projection_type,
            DocumentProjectionType::DocumentHead,
            project_id,
            projection_generation,
            catalog_revision,
        )?;
        validate_positive_revision(catalog_revision, "catalog_revision")?;
        validate_document_id(document_id)?;
        validate_positive_revision(document_revision, "document_revision")?;
        let expected = document_revision_coordinate(project_id, document_id, document_revision);
        if revision_coordinate != &expected {
            return invalid_projection("revision_coordinate does not match projection identity");
        }
        Ok(())
    }

    /// Lifecycle state represented by this head.
    #[must_use]
    pub const fn state(&self) -> DocumentState {
        match self {
            Self::Active { .. } => DocumentState::Active,
            Self::Deleted { .. } => DocumentState::Deleted,
        }
    }
}

/// One Relay-signed immutable Project Document revision.
///
/// Active variants contain the complete business snapshot. Deleted variants
/// are structurally unable to carry title, summary, or Markdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocumentRevisionProjection {
    /// A complete active snapshot.
    Active {
        /// Must equal one.
        schema_version: u16,
        /// Must equal `document_revision`.
        projection_type: DocumentProjectionType,
        /// Host-derived Community/Project identity.
        project_id: Uuid,
        /// Active Relay signer generation.
        projection_generation: u64,
        /// Catalog observation revision committed with this revision.
        catalog_revision: u64,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive Document-local revision.
        document_revision: u64,
        /// Complete title at this revision.
        title: String,
        /// Complete optional summary at this revision.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_optional_non_null"
        )]
        summary: Option<String>,
        /// Exact Markdown snapshot at this revision.
        content_markdown: String,
        /// Canonical creation time retained across revisions.
        created_at: DateTime<Utc>,
        /// Verified creator retained across revisions.
        created_by: PublicKey,
        /// Canonical time of this immutable revision.
        revision_at: DateTime<Utc>,
        /// Verified actor that produced this revision.
        revision_by: PublicKey,
        /// Member command that committed this revision.
        source_event_id: EventId,
    },
    /// A bodyless immutable tombstone revision.
    Deleted {
        /// Must equal one.
        schema_version: u16,
        /// Must equal `document_revision`.
        projection_type: DocumentProjectionType,
        /// Host-derived Community/Project identity.
        project_id: Uuid,
        /// Active Relay signer generation.
        projection_generation: u64,
        /// Catalog observation revision committed with this revision.
        catalog_revision: u64,
        /// Stable Document identity.
        document_id: Uuid,
        /// Positive Document-local revision.
        document_revision: u64,
        /// Canonical creation time retained from revision one.
        created_at: DateTime<Utc>,
        /// Verified creator retained from revision one.
        created_by: PublicKey,
        /// Canonical time of this immutable tombstone revision.
        revision_at: DateTime<Utc>,
        /// Verified actor that deleted the Document.
        revision_by: PublicKey,
        /// Member command that committed this tombstone.
        source_event_id: EventId,
    },
}

impl DocumentRevisionProjection {
    /// Validate all fields whose authority is contained in the projection.
    pub fn validate(&self) -> DocumentResult<()> {
        let (
            schema_version,
            projection_type,
            project_id,
            projection_generation,
            catalog_revision,
            document_id,
            document_revision,
        ) = match self {
            Self::Active {
                schema_version,
                projection_type,
                project_id,
                projection_generation,
                catalog_revision,
                document_id,
                document_revision,
                title,
                summary,
                content_markdown,
                created_at,
                revision_at,
                ..
            } => {
                validate_snapshot(title, summary.as_deref(), content_markdown)?;
                if revision_at < created_at {
                    return invalid_projection("revision_at precedes created_at");
                }
                (
                    *schema_version,
                    *projection_type,
                    *project_id,
                    *projection_generation,
                    *catalog_revision,
                    *document_id,
                    *document_revision,
                )
            }
            Self::Deleted {
                schema_version,
                projection_type,
                project_id,
                projection_generation,
                catalog_revision,
                document_id,
                document_revision,
                created_at,
                revision_at,
                ..
            } => {
                if revision_at < created_at {
                    return invalid_projection("revision_at precedes created_at");
                }
                (
                    *schema_version,
                    *projection_type,
                    *project_id,
                    *projection_generation,
                    *catalog_revision,
                    *document_id,
                    *document_revision,
                )
            }
        };
        validate_projection_common(
            schema_version,
            projection_type,
            DocumentProjectionType::DocumentRevision,
            project_id,
            projection_generation,
            catalog_revision,
        )?;
        validate_positive_revision(catalog_revision, "catalog_revision")?;
        validate_document_id(document_id)?;
        validate_positive_revision(document_revision, "document_revision")
    }

    /// Lifecycle state represented by this immutable revision.
    #[must_use]
    pub const fn state(&self) -> DocumentState {
        match self {
            Self::Active { .. } => DocumentState::Active,
            Self::Deleted { .. } => DocumentState::Deleted,
        }
    }
}

/// One head changed by an incremental catalog metadata projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedDocumentHead {
    /// Canonical current-head coordinate.
    pub head_coordinate: String,
    /// Exact signed current-head event.
    pub head_event_id: EventId,
    /// Stable Document identity.
    pub document_id: Uuid,
    /// Positive Document-local revision.
    pub document_revision: u64,
    /// Exact immutable revision event referenced by the head.
    pub revision_event_id: EventId,
    /// Whether the new head is a tombstone.
    pub deleted: bool,
}

/// Relay-signed Project Document catalog observation boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentMetaProjection {
    /// Must equal one.
    pub schema_version: u16,
    /// Must equal `document_meta`.
    pub projection_type: DocumentProjectionType,
    /// Host-derived Community/Project identity.
    pub project_id: Uuid,
    /// Always true for an emitted v1 metadata projection.
    pub initialized: bool,
    /// Active Relay signer generation.
    pub projection_generation: u64,
    /// Monotonic catalog observation revision; zero only at bootstrap.
    pub catalog_revision: u64,
    /// Number of current active Documents.
    pub active_document_count: u64,
    /// Whether readers must discard the prior generation/catalog cache.
    pub reset: bool,
    /// One changed head for an ordinary command; empty for a reset.
    pub changed_heads: Vec<ChangedDocumentHead>,
    /// Member command for an incremental update; omitted for reset metadata.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub source_event_id: Option<EventId>,
    /// Canonical time of this catalog observation.
    pub updated_at: DateTime<Utc>,
}

impl DocumentMetaProjection {
    /// Validate schema, reset/incremental shape, coordinates, and ranges.
    pub fn validate(&self) -> DocumentResult<()> {
        validate_projection_common(
            self.schema_version,
            self.projection_type,
            DocumentProjectionType::DocumentMeta,
            self.project_id,
            self.projection_generation,
            self.catalog_revision,
        )?;
        validate_nonnegative_revision(self.active_document_count, "active_document_count")?;
        if !self.initialized {
            return invalid_projection("an emitted document_meta must be initialized");
        }
        if self.reset {
            if !self.changed_heads.is_empty() || self.source_event_id.is_some() {
                return invalid_projection(
                    "reset metadata must omit source_event_id and changed_heads",
                );
            }
        } else if self.changed_heads.len() != 1 || self.source_event_id.is_none() {
            return invalid_projection(
                "incremental metadata requires one changed head and source_event_id",
            );
        }
        if self.catalog_revision == 0 && (!self.reset || self.active_document_count != 0) {
            return invalid_projection("catalog revision zero must be an empty reset bootstrap");
        }
        for changed in &self.changed_heads {
            validate_document_id(changed.document_id)?;
            validate_positive_revision(changed.document_revision, "document_revision")?;
            let expected = document_head_coordinate(self.project_id, changed.document_id);
            if changed.head_coordinate != expected {
                return invalid_projection("changed head coordinate does not match its identity");
            }
        }
        Ok(())
    }
}

/// Wire-neutral inputs for one Relay-signed Document projection bundle.
///
/// A bootstrap plan contains only reset metadata. A mutation plan contains one
/// immutable revision, its current head, and one incremental metadata change.
/// Signing and Nostr event construction remain in `buzz-sdk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentProjectionPlan {
    catalog: DocumentCatalog,
    current: Option<CurrentDocument>,
    source_event_id: Option<EventId>,
    reset: bool,
}

impl DocumentProjectionPlan {
    /// Build the only valid empty-catalog bootstrap plan.
    pub fn for_bootstrap(catalog: &DocumentCatalog) -> DocumentResult<Self> {
        catalog.validate()?;
        if catalog.catalog_revision() != 0 || catalog.active_document_count() != 0 {
            return invalid_projection(
                "bootstrap projection requires revision zero and an empty catalog",
            );
        }
        Ok(Self {
            catalog: catalog.clone(),
            current: None,
            source_event_id: None,
            reset: true,
        })
    }

    /// Build a reset observation for a fully staged signer generation.
    ///
    /// Unlike bootstrap, reprojection preserves the positive catalog revision
    /// and active count while carrying no business change or current head.
    pub fn for_reprojection(catalog: &DocumentCatalog) -> DocumentResult<Self> {
        catalog.validate()?;
        Ok(Self {
            catalog: catalog.clone(),
            current: None,
            source_event_id: None,
            reset: true,
        })
    }

    /// Build one deterministic mutation projection plan from canonical output.
    pub fn for_transition(
        catalog: &DocumentCatalog,
        current: &CurrentDocument,
        source_event_id: EventId,
    ) -> DocumentResult<Self> {
        catalog.validate()?;
        current.validate()?;
        if catalog.catalog_revision() == 0 {
            return invalid_projection(
                "a mutation projection requires a positive catalog revision",
            );
        }
        if catalog.updated_at() != current.document().updated().at {
            return invalid_projection(
                "catalog and current Document must share one canonical transition time",
            );
        }
        Ok(Self {
            catalog: catalog.clone(),
            current: Some(current.clone()),
            source_event_id: Some(source_event_id),
            reset: false,
        })
    }

    /// Canonical catalog after the transition.
    #[must_use]
    pub const fn catalog(&self) -> &DocumentCatalog {
        &self.catalog
    }

    /// Changed current Document, absent for bootstrap reset metadata.
    #[must_use]
    pub const fn current(&self) -> Option<&CurrentDocument> {
        self.current.as_ref()
    }

    /// Accepted member command, absent for bootstrap/reset.
    #[must_use]
    pub const fn source_event_id(&self) -> Option<EventId> {
        self.source_event_id
    }

    /// Whether readers must reset their catalog observation cache.
    #[must_use]
    pub const fn reset(&self) -> bool {
        self.reset
    }
}

/// Stable successful Project Document command receipt.
///
/// Projection event identifiers are deliberately absent: signer rotation may
/// replace materialization pointers without changing this business receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDocumentReceipt {
    /// Must equal one.
    pub schema_version: u16,
    /// Stable command event and change identity.
    pub change_id: EventId,
    /// Verified command signer.
    pub actor: PublicKey,
    /// Managed Assignment, omitted for Human commands.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub acting_assignment_id: Option<Uuid>,
    /// Stable accepted operation.
    pub operation: DocumentOperation,
    /// Stable Document identity.
    pub document_id: Uuid,
    /// Revision observed in the accepted command.
    pub expected_document_revision: u64,
    /// Revision committed by the accepted command.
    pub document_revision: u64,
    /// Catalog revision committed by the accepted command.
    pub catalog_revision: u64,
    /// Lifecycle state committed by the accepted command.
    pub state: DocumentState,
    /// Relay-assigned canonical acceptance time.
    pub accepted_at: DateTime<Utc>,
}

impl ProjectDocumentReceipt {
    /// Validate the stable business result independently of projection state.
    pub fn validate(&self) -> DocumentResult<()> {
        if self.schema_version != PROJECT_DOCUMENT_SCHEMA_VERSION {
            return Err(DocumentError::UnsupportedSchemaVersion {
                got: self.schema_version,
                supported: PROJECT_DOCUMENT_SCHEMA_VERSION,
            });
        }
        validate_document_id(self.document_id)?;
        validate_nonnegative_revision(
            self.expected_document_revision,
            "expected_document_revision",
        )?;
        validate_positive_revision(self.document_revision, "document_revision")?;
        validate_positive_revision(self.catalog_revision, "catalog_revision")?;
        if self.acting_assignment_id.is_some_and(|id| id.is_nil()) {
            return invalid_projection("acting_assignment_id cannot be nil");
        }
        let expected_state = match self.operation {
            DocumentOperation::Create | DocumentOperation::Update => DocumentState::Active,
            DocumentOperation::Delete => DocumentState::Deleted,
        };
        if self.state != expected_state {
            return invalid_projection("receipt operation and lifecycle state disagree");
        }
        let Some(committed_revision) = self.expected_document_revision.checked_add(1) else {
            return invalid_projection("receipt revision overflow");
        };
        if committed_revision != self.document_revision {
            return invalid_projection(
                "document_revision must be exactly expected_document_revision + 1",
            );
        }
        if self.operation == DocumentOperation::Create && self.expected_document_revision != 0 {
            return invalid_projection("create receipt must start from revision zero");
        }
        if self.operation != DocumentOperation::Create && self.expected_document_revision == 0 {
            return invalid_projection("update/delete receipt requires a positive base revision");
        }
        Ok(())
    }
}

/// Derive the canonical current-head coordinate.
#[must_use]
pub fn document_head_coordinate(project_id: Uuid, document_id: Uuid) -> String {
    format!("project-document:{project_id}:{document_id}")
}

/// Derive the canonical immutable-revision coordinate.
#[must_use]
pub fn document_revision_coordinate(
    project_id: Uuid,
    document_id: Uuid,
    document_revision: u64,
) -> String {
    format!("project-document:{project_id}:{document_id}:revision:{document_revision}")
}

/// Derive the canonical catalog-metadata coordinate.
#[must_use]
pub fn document_meta_coordinate(project_id: Uuid) -> String {
    format!("project-document:{project_id}:meta")
}

fn validate_projection_common(
    schema_version: u16,
    projection_type: DocumentProjectionType,
    expected_type: DocumentProjectionType,
    project_id: Uuid,
    projection_generation: u64,
    catalog_revision: u64,
) -> DocumentResult<()> {
    if schema_version != PROJECT_DOCUMENT_SCHEMA_VERSION {
        return Err(DocumentError::UnsupportedSchemaVersion {
            got: schema_version,
            supported: PROJECT_DOCUMENT_SCHEMA_VERSION,
        });
    }
    if projection_type != expected_type {
        return invalid_projection("projection_type does not match event kind");
    }
    if project_id.is_nil() {
        return invalid_projection("project_id cannot be nil");
    }
    validate_positive_revision(projection_generation, "projection_generation")?;
    validate_nonnegative_revision(catalog_revision, "catalog_revision")
}

fn invalid_projection<T>(reason: impl Into<String>) -> DocumentResult<T> {
    Err(DocumentError::InvalidProjection {
        reason: reason.into(),
    })
}
