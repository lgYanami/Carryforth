#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Pure protocol and domain types for Buzz Project Context Edges.
//!
//! This crate owns the closed v2 coordinate, edge, command, receipt, reducer,
//! and projection contracts. It deliberately performs no SQL, networking,
//! signing, authorization lookup, or async work.

mod command;
mod coordinate;
mod error;
mod model;
mod projection;
mod reducer;
mod validation;

pub use command::{ProjectContextCommand, ProjectContextCommandRequest};
pub use coordinate::{canonicalize_coordinates, ProjectContextCoordinate};
pub use error::{ProjectContextError, ProjectContextResult};
pub use model::{
    EdgeKey, ProjectContextBinding, ProjectContextBindingState, ProjectContextCatalog,
    ProjectContextEdge, ProjectContextOperation,
};
pub use projection::{
    context_binding_coordinate, context_edge_coordinate, context_meta_coordinate,
    ChangedContextBinding, ProjectContextBindingProjection, ProjectContextMetaProjection,
    ProjectContextProjectionPlan, ProjectContextProjectionType, ProjectContextReceipt,
};
pub use reducer::{reduce_project_context, ProjectContextChangeContext, ProjectContextTransition};
pub use validation::{
    MAX_COMMAND_CONTENT_BYTES, MAX_COMMAND_JSON_DEPTH, MAX_PROJECTION_CONTENT_BYTES,
    MAX_SAFE_REVISION, MIN_EDGE_COORDINATES,
};

/// Project Context Edge wire and canonical schema version.
pub const PROJECT_CONTEXT_SCHEMA_VERSION: u16 = 2;
/// Future NIP-11 capability name for a ready Project Context implementation.
pub const PROJECT_CONTEXT_CAPABILITY: &str = "buzz-project-context-edge-v2";
/// Exact `t` tag value on member-signed Project Context commands.
pub const PROJECT_CONTEXT_COMMAND_TAG: &str = "buzz-project-context-edge-command";
/// Common `t` tag value on every relay-signed Project Context projection.
pub const PROJECT_CONTEXT_PROJECTION_TAG: &str = "buzz-project-context-edge";
