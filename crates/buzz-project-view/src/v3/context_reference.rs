//! Pure Context Reference gates and bounded Document target proofs.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    canonicalize_context_references, DocumentReferenceMode, ProjectContextReference,
    V3ContractError,
};

/// One exact Document coordinate whose existence may need to be proved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentCoordinate {
    /// Stable Project Document identity.
    pub document_id: Uuid,
    /// Current-head or historical-revision lookup mode.
    pub mode: DocumentReferenceMode,
    /// Exact revision in pinned mode; omitted in live mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub document_revision: Option<u64>,
}

impl DocumentCoordinate {
    /// Coordinate for a Resource's mandatory current Guide.
    #[must_use]
    pub const fn live(document_id: Uuid) -> Self {
        Self {
            document_id,
            mode: DocumentReferenceMode::Live,
            document_revision: None,
        }
    }

    /// Convert a Document Context Reference into its proof coordinate.
    #[must_use]
    pub const fn from_context(reference: &ProjectContextReference) -> Option<Self> {
        match reference {
            ProjectContextReference::Resource { .. } => None,
            ProjectContextReference::Document {
                document_id,
                mode,
                document_revision,
            } => Some(Self {
                document_id: *document_id,
                mode: *mode,
                document_revision: *document_revision,
            }),
        }
    }

    /// Validate the live/pinned coordinate shape.
    pub fn validate(self) -> Result<(), V3ReferenceError> {
        let reference = ProjectContextReference::Document {
            document_id: self.document_id,
            mode: self.mode,
            document_revision: self.document_revision,
        };
        reference.validate().map_err(V3ReferenceError::Contract)
    }
}

/// Canonical lifecycle fact returned by one bounded Document point lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentTargetState {
    /// The Document current head is active at this positive revision.
    CurrentActive {
        /// Current Document-local revision.
        current_revision: u64,
    },
    /// The Document identity exists but its current head is a tombstone.
    CurrentTombstone,
    /// The exact pinned revision contains active Document content.
    ActiveContentRevision,
    /// The exact pinned revision is a tombstone.
    TombstoneRevision,
}

/// Sparse proof for only the Document targets newly introduced by a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReferenceTargetProof {
    documents: BTreeMap<DocumentCoordinate, DocumentTargetState>,
}

impl ReferenceTargetProof {
    /// Construct an empty fail-closed proof.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            documents: BTreeMap::new(),
        }
    }

    /// Construct a proof from exact point-query results.
    pub fn from_documents(
        documents: impl IntoIterator<Item = (DocumentCoordinate, DocumentTargetState)>,
    ) -> Result<Self, V3ReferenceError> {
        let mut proof = Self::new();
        for (coordinate, state) in documents {
            coordinate.validate()?;
            if proof.documents.insert(coordinate, state).is_some() {
                return Err(V3ReferenceError::DuplicateProof { coordinate });
            }
        }
        Ok(proof)
    }

    /// Borrow the exact proof rows in canonical coordinate order.
    #[must_use]
    pub const fn documents(&self) -> &BTreeMap<DocumentCoordinate, DocumentTargetState> {
        &self.documents
    }

    /// Verify that every requested coordinate has an exact active target fact.
    pub fn validate_required(
        &self,
        required: &BTreeSet<DocumentCoordinate>,
    ) -> Result<(), V3ReferenceError> {
        for coordinate in required {
            coordinate.validate()?;
            let state = self.documents.get(coordinate).copied().ok_or(
                V3ReferenceError::MissingDocumentProof {
                    coordinate: *coordinate,
                },
            )?;
            let valid = matches!(
                (coordinate.mode, state),
                (
                    DocumentReferenceMode::Live,
                    DocumentTargetState::CurrentActive {
                        current_revision: 1..
                    }
                ) | (
                    DocumentReferenceMode::Pinned,
                    DocumentTargetState::ActiveContentRevision
                )
            );
            if !valid {
                return Err(V3ReferenceError::InactiveDocumentTarget {
                    coordinate: *coordinate,
                    state,
                });
            }
        }
        Ok(())
    }
}

/// Document coordinates introduced by the candidate next object state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocumentTargetDelta {
    /// Newly introduced Context Document coordinates.
    pub context_documents: BTreeSet<DocumentCoordinate>,
    /// Newly selected Resource Guide, when the Guide changed or the Resource
    /// is new.
    pub guide_document: Option<DocumentCoordinate>,
}

impl DocumentTargetDelta {
    /// Return all required point-query coordinates as one bounded set.
    #[must_use]
    pub fn required_coordinates(&self) -> BTreeSet<DocumentCoordinate> {
        let mut coordinates = self.context_documents.clone();
        if let Some(guide) = self.guide_document {
            coordinates.insert(guide);
        }
        coordinates
    }

    /// Return whether this command introduces any new Document authority.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.context_documents.is_empty() && self.guide_document.is_none()
    }
}

/// Stable failures for Context gates and sparse target validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum V3ReferenceError {
    /// Context wire or canonical-set validation failed.
    #[error(transparent)]
    Contract(#[from] V3ContractError),
    /// Context is disabled and a command attempted to add or retarget a
    /// coordinate.
    #[error("unavailable:project_view:context_capability")]
    ContextCapabilityUnavailable,
    /// Document canonical/projection capability cannot prove a new target.
    #[error("unavailable:project_view:document_capability")]
    DocumentCapabilityUnavailable,
    /// A required point lookup was absent from the sparse proof.
    #[error("missing Document target proof for {coordinate:?}")]
    MissingDocumentProof {
        /// Missing coordinate.
        coordinate: DocumentCoordinate,
    },
    /// A point lookup proves that the target is not valid for its mode.
    #[error("Document target {coordinate:?} is not active ({state:?})")]
    InactiveDocumentTarget {
        /// Rejected coordinate.
        coordinate: DocumentCoordinate,
        /// Canonical target lifecycle fact.
        state: DocumentTargetState,
    },
    /// A coordinator supplied two facts for one coordinate.
    #[error("duplicate Document target proof for {coordinate:?}")]
    DuplicateProof {
        /// Duplicated coordinate.
        coordinate: DocumentCoordinate,
    },
}

/// Canonicalize a candidate Context set and enforce the independent Context
/// capability gate against the current canonical set.
pub fn validate_context_replacement(
    current: &[ProjectContextReference],
    replacement: Vec<ProjectContextReference>,
    project_context_enabled: bool,
) -> Result<Vec<ProjectContextReference>, V3ReferenceError> {
    let replacement = canonicalize_context_references(replacement)?;
    if project_context_enabled {
        return Ok(replacement);
    }
    let current = canonicalize_context_references(current.to_vec())?;
    let current = current.into_iter().collect::<HashSet<_>>();
    if replacement
        .iter()
        .all(|reference| current.contains(reference))
    {
        Ok(replacement)
    } else {
        Err(V3ReferenceError::ContextCapabilityUnavailable)
    }
}

/// Compute only newly introduced Document coordinates. Stable coordinates do
/// not need to be re-proved when Document projection availability is degraded.
#[must_use]
pub fn introduced_document_targets(
    current_context: &[ProjectContextReference],
    next_context: &[ProjectContextReference],
    current_guide_document_id: Option<Uuid>,
    next_guide_document_id: Option<Uuid>,
) -> DocumentTargetDelta {
    let current_documents = current_context
        .iter()
        .filter_map(DocumentCoordinate::from_context)
        .collect::<BTreeSet<_>>();
    let context_documents = next_context
        .iter()
        .filter_map(DocumentCoordinate::from_context)
        .filter(|coordinate| !current_documents.contains(coordinate))
        .collect();
    let guide_document = next_guide_document_id
        .filter(|next| Some(*next) != current_guide_document_id)
        .map(DocumentCoordinate::live);
    DocumentTargetDelta {
        context_documents,
        guide_document,
    }
}

/// Enforce Document availability and validate the sparse proof for a delta.
pub fn validate_document_target_delta(
    delta: &DocumentTargetDelta,
    document_capability_available: bool,
    proof: &ReferenceTargetProof,
) -> Result<(), V3ReferenceError> {
    if delta.is_empty() {
        return Ok(());
    }
    if !document_capability_available {
        return Err(V3ReferenceError::DocumentCapabilityUnavailable);
    }
    proof.validate_required(&delta.required_coordinates())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_context_allows_only_subsets() {
        let resource_id = Uuid::new_v4();
        let current = vec![ProjectContextReference::Resource { resource_id }];
        assert_eq!(
            validate_context_replacement(&current, Vec::new(), false),
            Ok(Vec::new())
        );
        let added = ProjectContextReference::Document {
            document_id: Uuid::new_v4(),
            mode: DocumentReferenceMode::Live,
            document_revision: None,
        };
        assert_eq!(
            validate_context_replacement(&current, vec![added], false),
            Err(V3ReferenceError::ContextCapabilityUnavailable)
        );
    }

    #[test]
    fn sparse_proof_fails_closed_and_distinguishes_live_from_pinned() {
        let document_id = Uuid::new_v4();
        let live = DocumentCoordinate::live(document_id);
        let pinned = DocumentCoordinate {
            document_id,
            mode: DocumentReferenceMode::Pinned,
            document_revision: Some(7),
        };
        let required = [live, pinned].into_iter().collect();
        let incomplete = ReferenceTargetProof::from_documents([(
            live,
            DocumentTargetState::CurrentActive {
                current_revision: 9,
            },
        )])
        .expect("valid proof");
        assert!(matches!(
            incomplete.validate_required(&required),
            Err(V3ReferenceError::MissingDocumentProof { coordinate }) if coordinate == pinned
        ));

        let complete = ReferenceTargetProof::from_documents([
            (
                live,
                DocumentTargetState::CurrentActive {
                    current_revision: 9,
                },
            ),
            (pinned, DocumentTargetState::ActiveContentRevision),
        ])
        .expect("valid proof");
        assert_eq!(complete.validate_required(&required), Ok(()));
    }
}
