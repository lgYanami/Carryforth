#![deny(unsafe_code)]
#![warn(missing_docs)]
//! Pure domain model for Buzz Project View.
//!
//! This crate owns Project View object types, relation validation, typed
//! mutations, the in-memory reference reducer, and deterministic read-model
//! assembly. It deliberately has no database, network, async runtime, or
//! event-signing responsibilities.

mod error;
mod model;
mod mutation;
mod patch;
mod projection;
mod read_model;
mod state;
/// Project View v2 role-continuity primitives.
pub mod v2;
mod validation;

pub use error::{DomainError, DomainResult};
pub use model::{
    Goal, IssueStatus, LocatorType, ObjectRef, PlanStatus, Priority, ProjectIssue, ProjectPlan,
    ProjectProfile, ProjectResource, ProjectRole, ProjectStage, ProjectViewObject,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, ProjectWork, Requirement,
    RequirementStatus, ResourceLocator, ResourceType, StageStatus, WorkStatus,
};
pub use mutation::{
    CreateMutation, DeleteMutation, GoalPatch, InitializeGoal, InitializeMutation, IssuePatch,
    Mutation, MutationOutcome, MutationRequest, NewProjectViewObject, PlanPatch, ProfilePatch,
    RequirementPatch, ResourcePatch, RolePatch, StagePatch, UpdateMutation, WorkPatch,
    MAX_MUTATION_CONTENT_BYTES, MAX_MUTATION_JSON_DEPTH, MUTATION_SCHEMA_VERSION,
};
pub use patch::Patch;
pub use projection::ProjectionPlan;
pub use read_model::{GoalView, IssueView, PlanView, ProjectView, RequirementView, StageView};
pub use state::{ProjectViewEntry, ProjectViewState, ProjectViewTombstone};

/// Largest revision that can be represented exactly by JavaScript.
pub const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;
/// Maximum number of goals accepted by Project View initialization.
pub const MAX_INITIAL_GOALS: usize = 32;

/// Validates one active projection object without requiring the rest of its
/// project snapshot.
///
/// Cross-object relation targets and aggregate invariants are intentionally
/// checked later by [`ProjectViewState::from_snapshot`].
pub fn validate_projected_object(object: &ProjectViewObject) -> DomainResult<()> {
    validation::validate_object(object)
}
