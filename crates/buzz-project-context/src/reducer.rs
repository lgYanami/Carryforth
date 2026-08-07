//! Pure Project Context Edge attach and detach state transitions.

use buzz_core::{EventId, PublicKey};
use chrono::{DateTime, Utc};

use crate::model::checked_next;
use crate::{
    EdgeKey, ProjectContextBinding, ProjectContextBindingState, ProjectContextCatalog,
    ProjectContextCommand, ProjectContextEdge, ProjectContextError, ProjectContextOperation,
    ProjectContextProjectionPlan, ProjectContextReceipt, ProjectContextResult,
    PROJECT_CONTEXT_SCHEMA_VERSION,
};

/// Trusted, adapter-supplied facts for one pure transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProjectContextChangeContext {
    /// Verified member command signer.
    pub actor: PublicKey,
    /// Exact accepted command event and stable replay identity.
    pub change_id: EventId,
    /// Monotonic PostgreSQL canonical time.
    pub canonical_at: DateTime<Utc>,
    /// Whether every command coordinate is currently active.
    pub all_coordinates_active: bool,
    /// Whether the Context Document is currently active.
    pub context_document_active: bool,
}

impl ProjectContextChangeContext {
    /// Construct transition facts with active coordinate and Document proofs.
    #[must_use]
    pub const fn active(actor: PublicKey, change_id: EventId, canonical_at: DateTime<Utc>) -> Self {
        Self {
            actor,
            change_id,
            canonical_at,
            all_coordinates_active: true,
            context_document_active: true,
        }
    }

    /// Replace the transaction-locked coordinate liveness proof.
    #[must_use]
    pub const fn with_coordinates_active(mut self, active: bool) -> Self {
        self.all_coordinates_active = active;
        self
    }

    /// Replace the transaction-locked Context Document liveness proof.
    #[must_use]
    pub const fn with_context_document_active(mut self, active: bool) -> Self {
        self.context_document_active = active;
        self
    }
}

/// Complete deterministic output of one accepted Context command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextTransition {
    catalog: ProjectContextCatalog,
    edge: Option<ProjectContextEdge>,
    binding: ProjectContextBinding,
    receipt: ProjectContextReceipt,
    projection_plan: ProjectContextProjectionPlan,
}

impl ProjectContextTransition {
    /// Canonical catalog after the accepted command.
    #[must_use]
    pub const fn catalog(&self) -> &ProjectContextCatalog {
        &self.catalog
    }

    /// Updated active edge, or `None` when the last Document was detached.
    #[must_use]
    pub const fn edge(&self) -> Option<&ProjectContextEdge> {
        self.edge.as_ref()
    }

    /// One active or deleted binding transition.
    #[must_use]
    pub const fn binding(&self) -> &ProjectContextBinding {
        &self.binding
    }

    /// Stable business receipt, independent of projection event IDs.
    #[must_use]
    pub const fn receipt(&self) -> &ProjectContextReceipt {
        &self.receipt
    }

    /// Wire-neutral materialization plan for SDK builders.
    #[must_use]
    pub const fn projection_plan(&self) -> &ProjectContextProjectionPlan {
        &self.projection_plan
    }

    /// Revalidate every cross-output invariant.
    pub fn validate(&self) -> ProjectContextResult<()> {
        self.catalog.validate()?;
        self.binding
            .validate(*self.catalog.project_id().as_uuid())?;
        if let Some(edge) = &self.edge {
            edge.validate(*self.catalog.project_id().as_uuid())?;
            if edge.key() != self.binding.edge_key {
                return invalid_state("updated edge and binding use different keys");
            }
        }
        self.receipt.validate()?;
        self.projection_plan.validate()?;
        let edge_document_count = self
            .edge
            .as_ref()
            .map_or(0, |edge| edge.context_document_ids().len() as u64);
        let edge_state = if self.edge.is_some() {
            ProjectContextBindingState::Active
        } else {
            ProjectContextBindingState::Deleted
        };
        if self.receipt.context_revision != self.catalog.context_revision()
            || self.receipt.edge_key != self.binding.edge_key
            || self.receipt.context_document_id != self.binding.context_document_id
            || self.receipt.edge_document_count != edge_document_count
            || self.receipt.edge_state != edge_state
            || self.receipt.accepted_at != self.catalog.updated_at()
            || self.projection_plan.catalog() != &self.catalog
            || self.projection_plan.binding() != Some(&self.binding)
            || self.projection_plan.source_event_id() != Some(self.receipt.change_id)
            || self.projection_plan.reset()
        {
            return invalid_state("transition receipt, state, and projection plan disagree");
        }
        Ok(())
    }
}

/// Reduce one command against its exact edge and the Document's active binding.
///
/// `current_edge` is the active row for the command's derived edge key, if any.
/// `active_document_edge` is the active edge currently owning the target
/// Context Document, if any. The adapter obtains both under the shared Project
/// Context transaction lock. Accepted-command replay is resolved before this
/// reducer is called.
pub fn reduce_project_context(
    catalog: &ProjectContextCatalog,
    current_edge: Option<&ProjectContextEdge>,
    active_document_edge: Option<EdgeKey>,
    command: &ProjectContextCommand,
    context: ProjectContextChangeContext,
) -> ProjectContextResult<ProjectContextTransition> {
    catalog.validate()?;
    command.validate_for_project(*catalog.project_id().as_uuid())?;
    if command.expected_context_revision != catalog.context_revision() {
        return Err(ProjectContextError::RevisionConflict {
            expected: command.expected_context_revision,
            actual: catalog.context_revision(),
        });
    }
    if context.canonical_at <= catalog.updated_at() {
        return invalid_state("canonical transition time must increase past catalog updated_at");
    }

    let edge_key = EdgeKey::derive(*catalog.project_id().as_uuid(), command.coordinates())?;
    if let Some(edge) = current_edge {
        edge.validate(*catalog.project_id().as_uuid())?;
        if edge.key() != edge_key || edge.coordinates() != command.coordinates() {
            return invalid_state(
                "loaded active edge key or canonical coordinates do not match the command",
            );
        }
    }

    let next_revision = checked_next(catalog.context_revision())?;
    let document_id = command.context_document_id();
    let (edge, binding_state, active_edge_count, bound_document_count) = match command.operation() {
        ProjectContextOperation::Attach => {
            if !context.all_coordinates_active {
                return Err(ProjectContextError::InactiveCoordinate);
            }
            if !context.context_document_active {
                return Err(ProjectContextError::InactiveContextDocument { document_id });
            }
            if let Some(existing_key) = active_document_edge {
                if existing_key == edge_key {
                    if current_edge
                        .is_some_and(|edge| edge.context_document_ids().contains(&document_id))
                    {
                        return Err(ProjectContextError::NoChange);
                    }
                    return invalid_state("active Document binding is absent from its active edge");
                }
                return Err(ProjectContextError::DocumentAlreadyBound {
                    document_id,
                    edge_key: existing_key,
                });
            }
            let mut document_ids = current_edge
                .map(|edge| edge.context_document_ids().to_vec())
                .unwrap_or_default();
            document_ids.push(document_id);
            document_ids.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            let edge = ProjectContextEdge::from_snapshot(
                *catalog.project_id().as_uuid(),
                command.coordinates().to_vec(),
                document_ids,
            )?;
            let active_edge_count = if current_edge.is_some() {
                catalog.active_edge_count()
            } else {
                checked_next(catalog.active_edge_count())?
            };
            let bound_document_count = checked_next(catalog.bound_document_count())?;
            (
                Some(edge),
                ProjectContextBindingState::Active,
                active_edge_count,
                bound_document_count,
            )
        }
        ProjectContextOperation::Detach => {
            let Some(existing_key) = active_document_edge else {
                return Err(ProjectContextError::BindingNotFound { document_id });
            };
            if existing_key != edge_key {
                return Err(ProjectContextError::BindingEdgeMismatch {
                    document_id,
                    actual_edge_key: existing_key,
                });
            }
            let Some(current_edge) = current_edge else {
                return invalid_state("active Document binding has no active edge");
            };
            if !current_edge.context_document_ids().contains(&document_id) {
                return invalid_state("active edge does not contain the bound Document");
            }
            let document_ids: Vec<_> = current_edge
                .context_document_ids()
                .iter()
                .copied()
                .filter(|id| *id != document_id)
                .collect();
            let edge = if document_ids.is_empty() {
                None
            } else {
                Some(ProjectContextEdge::from_snapshot(
                    *catalog.project_id().as_uuid(),
                    command.coordinates().to_vec(),
                    document_ids,
                )?)
            };
            let active_edge_count = if edge.is_none() {
                catalog.active_edge_count().checked_sub(1).ok_or_else(|| {
                    ProjectContextError::InvalidCanonicalState {
                        reason: "cannot remove the last binding from an empty catalog".to_owned(),
                    }
                })?
            } else {
                catalog.active_edge_count()
            };
            let bound_document_count =
                catalog
                    .bound_document_count()
                    .checked_sub(1)
                    .ok_or_else(|| ProjectContextError::InvalidCanonicalState {
                        reason: "cannot detach from a catalog with no bound Documents".to_owned(),
                    })?;
            (
                edge,
                ProjectContextBindingState::Deleted,
                active_edge_count,
                bound_document_count,
            )
        }
    };

    let catalog = ProjectContextCatalog::from_snapshot(
        catalog.project_id(),
        next_revision,
        active_edge_count,
        bound_document_count,
        catalog.projection_generation(),
        catalog.initialized_at(),
        context.canonical_at,
    )?;
    let binding = ProjectContextBinding {
        edge_key,
        coordinates: command.coordinates().to_vec(),
        context_document_id: document_id,
        state: binding_state,
        context_revision: next_revision,
        updated_at: context.canonical_at,
    };
    binding.validate(*catalog.project_id().as_uuid())?;
    let edge_document_count = edge
        .as_ref()
        .map_or(0, |edge| edge.context_document_ids().len() as u64);
    let receipt = ProjectContextReceipt {
        schema_version: PROJECT_CONTEXT_SCHEMA_VERSION,
        change_id: context.change_id,
        actor: context.actor,
        acting_assignment_id: command.acting_assignment_id,
        operation: command.operation(),
        expected_context_revision: command.expected_context_revision,
        context_revision: next_revision,
        edge_key,
        edge_state: if edge.is_some() {
            ProjectContextBindingState::Active
        } else {
            ProjectContextBindingState::Deleted
        },
        edge_document_count,
        context_document_id: document_id,
        accepted_at: context.canonical_at,
    };
    let projection_plan =
        ProjectContextProjectionPlan::for_transition(&catalog, &binding, context.change_id)?;
    let transition = ProjectContextTransition {
        catalog,
        edge,
        binding,
        receipt,
        projection_plan,
    };
    transition.validate()?;
    Ok(transition)
}

fn invalid_state<T>(reason: &str) -> ProjectContextResult<T> {
    Err(ProjectContextError::InvalidCanonicalState {
        reason: reason.to_owned(),
    })
}
