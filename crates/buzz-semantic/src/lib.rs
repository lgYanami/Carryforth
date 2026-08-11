#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Pure semantic-index contracts for Carryforth Project Context.
//!
//! The crate has no database, network, Project Context graph, or agent
//! dependency. Canonical source adapters live in `buzz-db`; this crate only
//! validates their typed observations, extracts deterministic overview units,
//! and defines encoder contracts.

mod encoder;
mod extractor;
mod model;

pub use encoder::{
    DeterministicFakeEncoder, EncodedSemanticUnit, SemanticEncoder, SemanticEncoderFuture,
    SemanticEncoderInput,
};
pub use extractor::{extract_overview, visible_markdown_text, OVERVIEW_EXTRACTOR_VERSION};
pub use model::{
    CanonicalSemanticSourceObservation, Digest32, EmbeddingVector, IneligibilityReason,
    MeetingSourceBasis, ProjectDocumentSourceBasis, ProjectViewSemanticType,
    ProjectViewSourceBasis, SemanticCoverage, SemanticDistanceMetric, SemanticEligibility,
    SemanticError, SemanticFilterMetadata, SemanticLifecycleClass, SemanticModelContract,
    SemanticNormalization, SemanticProviderBoundary, SemanticSourceBasis, SemanticSourceIdentity,
    SemanticSourceKind, SemanticUnit, SemanticUnitIdentity, SemanticUnitKind,
    DEFAULT_EMBEDDING_DIMENSIONS, DEFAULT_EMBEDDING_MODEL, DEFAULT_EMBEDDING_PROVIDER,
};
