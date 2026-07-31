#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Pure protocol and domain types for Buzz Project Documents.
//!
//! This crate owns the closed Project Document v1 command and projection
//! contracts. It deliberately performs no SQL, networking, signing,
//! authorization lookup, async work, Markdown execution, or external Resource
//! resolution.

mod command;
mod error;
mod model;
mod projection;
mod reducer;
mod validation;

pub use command::{DocumentCommandRequest, ProjectDocumentCommand};
pub use error::{DocumentError, DocumentResult};
pub use model::{
    CurrentDocument, DocumentAttribution, DocumentCatalog, DocumentOperation, DocumentRevision,
    DocumentSnapshot, DocumentState, ProjectDocument,
};
pub use projection::{
    document_head_coordinate, document_meta_coordinate, document_revision_coordinate,
    ChangedDocumentHead, DocumentHeadProjection, DocumentMetaProjection, DocumentProjectionPlan,
    DocumentProjectionType, DocumentRevisionProjection, ProjectDocumentReceipt,
};
pub use reducer::{reduce_document, DocumentChangeContext, DocumentTransition};
pub use validation::{
    MAX_COMMAND_CONTENT_BYTES, MAX_COMMAND_JSON_DEPTH, MAX_CONTENT_MARKDOWN_BYTES,
    MAX_SAFE_REVISION, MAX_SUMMARY_BYTES, MAX_TITLE_BYTES,
};

/// Project Document wire and canonical schema version.
pub const PROJECT_DOCUMENT_SCHEMA_VERSION: u16 = 1;
/// NIP-11 capability advertised only for a ready Project Document v1 catalog.
pub const PROJECT_DOCUMENT_CAPABILITY: &str = "buzz-project-document-v1";
/// Exact `t` tag value on member-signed Document commands.
pub const PROJECT_DOCUMENT_COMMAND_TAG: &str = "buzz-project-document-command";
/// Common `t` tag value on every relay-signed Document projection.
pub const PROJECT_DOCUMENT_PROJECTION_TAG: &str = "buzz-project-document";
