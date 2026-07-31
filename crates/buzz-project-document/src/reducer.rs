//! Pure Project Document create, update, and delete state transition.

use buzz_core::{EventId, PublicKey};
use chrono::{DateTime, Utc};

use crate::model::DocumentAttribution;
use crate::{
    CurrentDocument, DocumentCatalog, DocumentCommandRequest, DocumentError, DocumentOperation,
    DocumentProjectionPlan, DocumentResult, DocumentRevision, DocumentState, ProjectDocument,
    ProjectDocumentCommand, ProjectDocumentReceipt, MAX_SAFE_REVISION,
    PROJECT_DOCUMENT_SCHEMA_VERSION,
};

/// Trusted, adapter-supplied facts for one pure transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DocumentChangeContext {
    /// Verified member command signer.
    pub actor: PublicKey,
    /// Exact accepted command event and stable change identity.
    pub change_id: EventId,
    /// Monotonic PostgreSQL canonical time.
    pub canonical_at: DateTime<Utc>,
    /// Whether an active Resource Guide or Live Document reference blocks delete.
    pub deletion_blocked: bool,
}

impl DocumentChangeContext {
    /// Construct transition facts for a command with no incoming delete blocker.
    #[must_use]
    pub const fn new(actor: PublicKey, change_id: EventId, canonical_at: DateTime<Utc>) -> Self {
        Self {
            actor,
            change_id,
            canonical_at,
            deletion_blocked: false,
        }
    }

    /// Record the transaction-locked incoming-reference result.
    #[must_use]
    pub const fn with_deletion_blocked(mut self, deletion_blocked: bool) -> Self {
        self.deletion_blocked = deletion_blocked;
        self
    }
}

/// Complete deterministic output of one accepted Document command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTransition {
    catalog: DocumentCatalog,
    current: CurrentDocument,
    receipt: ProjectDocumentReceipt,
    projection_plan: DocumentProjectionPlan,
}

impl DocumentTransition {
    /// Canonical catalog after the accepted command.
    #[must_use]
    pub const fn catalog(&self) -> &DocumentCatalog {
        &self.catalog
    }

    /// Canonical current row and newly appended immutable revision.
    #[must_use]
    pub const fn current(&self) -> &CurrentDocument {
        &self.current
    }

    /// Stable business receipt, independent of projection event IDs.
    #[must_use]
    pub const fn receipt(&self) -> &ProjectDocumentReceipt {
        &self.receipt
    }

    /// Wire-neutral materialization plan for SDK builders.
    #[must_use]
    pub const fn projection_plan(&self) -> &DocumentProjectionPlan {
        &self.projection_plan
    }

    /// Revalidate every cross-output invariant.
    pub fn validate(&self) -> DocumentResult<()> {
        self.catalog.validate()?;
        self.current.validate()?;
        self.receipt.validate()?;
        if self.receipt.document_id != self.current.document().document_id()
            || self.receipt.document_revision != self.current.document().current_revision()
            || self.receipt.catalog_revision != self.catalog.catalog_revision()
            || self.receipt.state != self.current.document().state()
            || self.receipt.accepted_at != self.catalog.updated_at()
            || self.projection_plan.catalog() != &self.catalog
            || self.projection_plan.current() != Some(&self.current)
            || self.projection_plan.source_event_id() != Some(self.receipt.change_id)
            || self.projection_plan.reset()
        {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "transition receipt, state, and projection plan disagree".to_owned(),
            });
        }
        Ok(())
    }
}

/// Reduce one command against its target and catalog without performing I/O.
///
/// `current` is the exact current revision for the command's Document ID, or
/// `None` when that identity has never existed. The caller obtains both it and
/// `deletion_blocked` under the shared Community transaction lock.
pub fn reduce_document(
    catalog: &DocumentCatalog,
    current: Option<&CurrentDocument>,
    command: &ProjectDocumentCommand,
    context: DocumentChangeContext,
) -> DocumentResult<DocumentTransition> {
    catalog.validate()?;
    command.validate_for_submission()?;
    if context.canonical_at <= catalog.updated_at() {
        return Err(DocumentError::InvalidCanonicalState {
            reason: "canonical transition time must increase past catalog updated_at".to_owned(),
        });
    }
    if let Some(current) = current {
        current.validate()?;
        if current.document().document_id() != command.document_id() {
            return Err(DocumentError::InvalidCanonicalState {
                reason: "loaded current Document does not match the command target".to_owned(),
            });
        }
    }

    let operation = command.operation();
    let next_revision = next_revision(command.expected_document_revision)?;
    let (document, revision) = match &command.request {
        DocumentCommandRequest::Create {
            document_id,
            title,
            summary,
            content_markdown,
        } => {
            if current.is_some() {
                return Err(DocumentError::DocumentIdAlreadyExists {
                    document_id: *document_id,
                });
            }
            let attribution = DocumentAttribution {
                at: context.canonical_at,
                by: context.actor,
            };
            let document = ProjectDocument::from_snapshot(
                *document_id,
                next_revision,
                DocumentState::Active,
                attribution,
                attribution,
            )?;
            let revision = DocumentRevision::Active {
                schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
                document_id: *document_id,
                document_revision: next_revision,
                snapshot: crate::DocumentSnapshot {
                    title: title.clone(),
                    summary: summary.clone(),
                    content_markdown: content_markdown.clone(),
                },
                actor: context.actor,
                canonical_at: context.canonical_at,
            };
            (document, revision)
        }
        DocumentCommandRequest::Update {
            document_id,
            title,
            summary,
            content_markdown,
        } => {
            let current = require_active_current(current, command)?;
            let next_snapshot = crate::DocumentSnapshot {
                title: title.clone(),
                summary: summary.clone(),
                content_markdown: content_markdown.clone(),
            };
            if current.revision().snapshot() == Some(&next_snapshot) {
                return Err(DocumentError::NoChange);
            }
            require_increasing_time(current, context.canonical_at)?;
            let document = ProjectDocument::from_snapshot(
                *document_id,
                next_revision,
                DocumentState::Active,
                current.document().created(),
                DocumentAttribution {
                    at: context.canonical_at,
                    by: context.actor,
                },
            )?;
            let revision = DocumentRevision::Active {
                schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
                document_id: *document_id,
                document_revision: next_revision,
                snapshot: next_snapshot,
                actor: context.actor,
                canonical_at: context.canonical_at,
            };
            (document, revision)
        }
        DocumentCommandRequest::Delete { document_id } => {
            let current = require_active_current(current, command)?;
            if context.deletion_blocked {
                return Err(DocumentError::StillReferenced {
                    document_id: *document_id,
                });
            }
            require_increasing_time(current, context.canonical_at)?;
            let document = ProjectDocument::from_snapshot(
                *document_id,
                next_revision,
                DocumentState::Deleted,
                current.document().created(),
                DocumentAttribution {
                    at: context.canonical_at,
                    by: context.actor,
                },
            )?;
            let revision = DocumentRevision::Deleted {
                schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
                document_id: *document_id,
                document_revision: next_revision,
                actor: context.actor,
                canonical_at: context.canonical_at,
            };
            (document, revision)
        }
    };

    let current = CurrentDocument::new(document, revision)?;
    let catalog = next_catalog(catalog, operation, context.canonical_at)?;
    let receipt = ProjectDocumentReceipt {
        schema_version: PROJECT_DOCUMENT_SCHEMA_VERSION,
        change_id: context.change_id,
        actor: context.actor,
        acting_assignment_id: command.acting_assignment_id,
        operation,
        document_id: command.document_id(),
        expected_document_revision: command.expected_document_revision,
        document_revision: next_revision,
        catalog_revision: catalog.catalog_revision(),
        state: current.document().state(),
        accepted_at: context.canonical_at,
    };
    let projection_plan =
        DocumentProjectionPlan::for_transition(&catalog, &current, context.change_id)?;
    let transition = DocumentTransition {
        catalog,
        current,
        receipt,
        projection_plan,
    };
    transition.validate()?;
    Ok(transition)
}

fn require_active_current<'a>(
    current: Option<&'a CurrentDocument>,
    command: &ProjectDocumentCommand,
) -> DocumentResult<&'a CurrentDocument> {
    let Some(current) = current else {
        return Err(DocumentError::DocumentNotFound {
            document_id: command.document_id(),
        });
    };
    let actual = current.document().current_revision();
    if actual != command.expected_document_revision {
        return Err(DocumentError::RevisionConflict {
            expected: command.expected_document_revision,
            actual: Some(actual),
        });
    }
    if current.document().state() == DocumentState::Deleted {
        return Err(DocumentError::DocumentDeleted {
            document_id: command.document_id(),
        });
    }
    Ok(current)
}

fn require_increasing_time(
    current: &CurrentDocument,
    canonical_at: DateTime<Utc>,
) -> DocumentResult<()> {
    if canonical_at <= current.document().updated().at {
        return Err(DocumentError::InvalidCanonicalState {
            reason: "canonical transition time must increase past Document updated_at".to_owned(),
        });
    }
    Ok(())
}

fn next_revision(current: u64) -> DocumentResult<u64> {
    current
        .checked_add(1)
        .filter(|next| *next <= MAX_SAFE_REVISION)
        .ok_or(DocumentError::RevisionExhausted)
}

fn next_catalog(
    catalog: &DocumentCatalog,
    operation: DocumentOperation,
    canonical_at: DateTime<Utc>,
) -> DocumentResult<DocumentCatalog> {
    let catalog_revision = next_revision(catalog.catalog_revision())?;
    let active_document_count =
        match operation {
            DocumentOperation::Create => catalog
                .active_document_count()
                .checked_add(1)
                .filter(|count| *count <= MAX_SAFE_REVISION)
                .ok_or(DocumentError::RevisionExhausted)?,
            DocumentOperation::Update => catalog.active_document_count(),
            DocumentOperation::Delete => catalog
                .active_document_count()
                .checked_sub(1)
                .ok_or_else(|| DocumentError::InvalidCanonicalState {
                    reason: "cannot delete from a catalog with no active Documents".to_owned(),
                })?,
        };
    DocumentCatalog::from_snapshot(
        catalog.project_id(),
        catalog_revision,
        active_document_count,
        catalog.projection_generation(),
        catalog.initialized_at(),
        canonical_at,
    )
}
