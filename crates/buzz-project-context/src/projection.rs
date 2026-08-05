//! Relay-signed Project Context Edge v1 projection and receipt wire types.

use buzz_core::{EventId, PublicKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::coordinate::validate_canonical_coordinates;
use crate::validation::{
    deserialize_optional_non_null, validate_document_id, validate_nonnegative, validate_positive,
    validate_projection_size,
};
use crate::{
    EdgeKey, ProjectContextBinding, ProjectContextBindingState, ProjectContextCatalog,
    ProjectContextCoordinate, ProjectContextError, ProjectContextOperation, ProjectContextResult,
    PROJECT_CONTEXT_SCHEMA_VERSION,
};

/// Exact subtype discriminator carried in every Context projection body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectContextProjectionType {
    /// Current one-Document edge binding or its tombstone.
    ContextEdgeBinding,
    /// Current catalog observation boundary.
    ContextMeta,
}

/// Relay-signed current binding projection for one Context Document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextBindingProjection {
    /// Must equal one.
    pub schema_version: u16,
    /// Must equal `context_edge_binding`.
    pub projection_type: ProjectContextProjectionType,
    /// Host-derived Community/Project identity.
    pub project_id: Uuid,
    /// Active Relay signer generation.
    pub projection_generation: u64,
    /// Global Context revision committed with this binding.
    pub context_revision: u64,
    /// Deterministic identity of the exact coordinate set.
    pub edge_key: EdgeKey,
    /// Canonically sorted coordinate set retained by tombstones.
    pub coordinates: Vec<ProjectContextCoordinate>,
    /// One-to-one Context Document identity.
    pub context_document_id: Uuid,
    /// Current binding lifecycle state.
    pub state: ProjectContextBindingState,
    /// Member command that committed this binding.
    pub source_event_id: EventId,
    /// Relay-assigned canonical transition time.
    pub updated_at: DateTime<Utc>,
}

impl ProjectContextBindingProjection {
    /// Validate all body-contained identity, lifecycle, and size invariants.
    pub fn validate(&self) -> ProjectContextResult<()> {
        validate_projection_common(
            self.schema_version,
            self.projection_type,
            ProjectContextProjectionType::ContextEdgeBinding,
            self.project_id,
            self.projection_generation,
            self.context_revision,
        )?;
        validate_positive(self.context_revision, "context_revision")?;
        validate_canonical_coordinates(&self.coordinates)?;
        for coordinate in &self.coordinates {
            coordinate.validate_for_project(self.project_id)?;
        }
        validate_document_id(self.context_document_id)?;
        if self.edge_key != EdgeKey::derive(self.project_id, &self.coordinates)? {
            return invalid_projection("edge_key does not match project and coordinates");
        }
        validate_projection_size(self)
    }

    /// Canonical binding query coordinate.
    #[must_use]
    pub fn binding_coordinate(&self) -> String {
        context_binding_coordinate(self.project_id, self.context_document_id)
    }

    /// Canonical edge query coordinate.
    #[must_use]
    pub fn edge_coordinate(&self) -> String {
        context_edge_coordinate(self.project_id, self.edge_key)
    }
}

/// One binding changed by an incremental Context metadata projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangedContextBinding {
    /// Stable Context Document identity.
    pub context_document_id: Uuid,
    /// Edge identity carried by the changed projection.
    pub edge_key: EdgeKey,
    /// Canonical binding query coordinate.
    pub binding_coordinate: String,
    /// Exact signed binding projection event.
    pub binding_event_id: EventId,
    /// New binding lifecycle state.
    pub state: ProjectContextBindingState,
}

impl ChangedContextBinding {
    /// Validate identity and canonical binding coordinate.
    pub fn validate(&self, project_id: Uuid) -> ProjectContextResult<()> {
        validate_document_id(self.context_document_id)?;
        if self.binding_coordinate
            != context_binding_coordinate(project_id, self.context_document_id)
        {
            return invalid_projection("changed binding coordinate is not canonical");
        }
        Ok(())
    }
}

/// Relay-signed Project Context catalog observation boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextMetaProjection {
    /// Must equal one.
    pub schema_version: u16,
    /// Must equal `context_meta`.
    pub projection_type: ProjectContextProjectionType,
    /// Host-derived Community/Project identity.
    pub project_id: Uuid,
    /// Active Relay signer generation.
    pub projection_generation: u64,
    /// Global Context observation revision; zero only at bootstrap.
    pub context_revision: u64,
    /// Number of active coordinate-set edges.
    pub active_edge_count: u64,
    /// Number of active one-Document bindings.
    pub bound_document_count: u64,
    /// Whether readers must replace prior generation/catalog cache state.
    pub reset: bool,
    /// One changed binding for an ordinary command; empty for a reset.
    pub changed_bindings: Vec<ChangedContextBinding>,
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

impl ProjectContextMetaProjection {
    /// Validate the reset or ordinary-incremental closed shape.
    pub fn validate(&self) -> ProjectContextResult<()> {
        validate_projection_common(
            self.schema_version,
            self.projection_type,
            ProjectContextProjectionType::ContextMeta,
            self.project_id,
            self.projection_generation,
            self.context_revision,
        )?;
        validate_nonnegative(self.context_revision, "context_revision")?;
        validate_nonnegative(self.active_edge_count, "active_edge_count")?;
        validate_nonnegative(self.bound_document_count, "bound_document_count")?;
        if self.active_edge_count > self.bound_document_count {
            return invalid_projection("active_edge_count exceeds bound_document_count");
        }
        if (self.active_edge_count == 0) != (self.bound_document_count == 0) {
            return invalid_projection("edge and binding emptiness must agree");
        }
        if self.context_revision == 0
            && (self.active_edge_count != 0 || self.bound_document_count != 0)
        {
            return invalid_projection("revision zero must describe an empty catalog");
        }
        if self.reset {
            if !self.changed_bindings.is_empty() || self.source_event_id.is_some() {
                return invalid_projection(
                    "reset metadata must omit source_event_id and changed bindings",
                );
            }
        } else {
            validate_positive(self.context_revision, "context_revision")?;
            if self.changed_bindings.len() != 1 || self.source_event_id.is_none() {
                return invalid_projection(
                    "ordinary metadata requires one changed binding and source_event_id",
                );
            }
            self.changed_bindings[0].validate(self.project_id)?;
        }
        validate_projection_size(self)
    }
}

/// Stable business receipt returned for one accepted Context command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectContextReceipt {
    /// Must equal one.
    pub schema_version: u16,
    /// Accepted command event and stable replay identity.
    pub change_id: EventId,
    /// Verified command signer.
    pub actor: PublicKey,
    /// Optional managed Assignment claim retained for audit.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_non_null"
    )]
    pub acting_assignment_id: Option<Uuid>,
    /// Accepted attach or detach operation.
    pub operation: ProjectContextOperation,
    /// Revision expected by the signed command.
    pub expected_context_revision: u64,
    /// Newly committed global Context revision.
    pub context_revision: u64,
    /// Deterministic target edge identity.
    pub edge_key: EdgeKey,
    /// Whether the edge remains active after the command.
    pub edge_state: ProjectContextBindingState,
    /// Number of Context Documents remaining on the edge.
    pub edge_document_count: u64,
    /// Context Document whose binding changed.
    pub context_document_id: Uuid,
    /// Relay-assigned canonical acceptance time.
    pub accepted_at: DateTime<Utc>,
}

impl ProjectContextReceipt {
    /// Validate revision, identity, and edge lifecycle consistency.
    pub fn validate(&self) -> ProjectContextResult<()> {
        if self.schema_version != PROJECT_CONTEXT_SCHEMA_VERSION {
            return Err(ProjectContextError::UnsupportedSchemaVersion {
                got: self.schema_version,
                supported: PROJECT_CONTEXT_SCHEMA_VERSION,
            });
        }
        validate_nonnegative(self.expected_context_revision, "expected_context_revision")?;
        validate_positive(self.context_revision, "context_revision")?;
        if self.expected_context_revision.checked_add(1) != Some(self.context_revision) {
            return invalid_projection(
                "receipt context_revision must immediately follow expected_context_revision",
            );
        }
        validate_document_id(self.context_document_id)?;
        if self
            .acting_assignment_id
            .is_some_and(|assignment_id| assignment_id.is_nil())
        {
            return invalid_projection("receipt acting_assignment_id cannot be nil");
        }
        validate_nonnegative(self.edge_document_count, "edge_document_count")?;
        match self.edge_state {
            ProjectContextBindingState::Active if self.edge_document_count == 0 => {
                return invalid_projection("active receipt edge has no Documents");
            }
            ProjectContextBindingState::Deleted if self.edge_document_count != 0 => {
                return invalid_projection("deleted receipt edge retains Documents");
            }
            _ => {}
        }
        Ok(())
    }
}

/// Wire-neutral materialization plan produced by pure state reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectContextProjectionPlan {
    catalog: ProjectContextCatalog,
    binding: Option<ProjectContextBinding>,
    source_event_id: Option<EventId>,
    reset: bool,
}

impl ProjectContextProjectionPlan {
    /// Build the two-projection plan for one ordinary accepted command.
    pub fn for_transition(
        catalog: &ProjectContextCatalog,
        binding: &ProjectContextBinding,
        source_event_id: EventId,
    ) -> ProjectContextResult<Self> {
        let plan = Self {
            catalog: catalog.clone(),
            binding: Some(binding.clone()),
            source_event_id: Some(source_event_id),
            reset: false,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Build reset metadata for bootstrap or a full reprojection generation.
    pub fn for_reset(catalog: &ProjectContextCatalog) -> ProjectContextResult<Self> {
        let plan = Self {
            catalog: catalog.clone(),
            binding: None,
            source_event_id: None,
            reset: true,
        };
        plan.validate()?;
        Ok(plan)
    }

    /// Validate the closed ordinary/reset shape.
    pub fn validate(&self) -> ProjectContextResult<()> {
        self.catalog.validate()?;
        match (
            self.reset,
            self.binding.as_ref(),
            self.source_event_id.as_ref(),
        ) {
            (true, None, None) => Ok(()),
            (false, Some(binding), Some(_)) => {
                binding.validate(*self.catalog.project_id().as_uuid())?;
                if binding.context_revision != self.catalog.context_revision()
                    || binding.updated_at != self.catalog.updated_at()
                {
                    return invalid_projection("binding and catalog observations disagree");
                }
                Ok(())
            }
            _ => invalid_projection("projection plan has a mixed reset/ordinary shape"),
        }
    }

    /// Canonical catalog observation.
    #[must_use]
    pub const fn catalog(&self) -> &ProjectContextCatalog {
        &self.catalog
    }

    /// Changed binding for an ordinary plan.
    #[must_use]
    pub const fn binding(&self) -> Option<&ProjectContextBinding> {
        self.binding.as_ref()
    }

    /// Accepted command event for an ordinary plan.
    #[must_use]
    pub const fn source_event_id(&self) -> Option<EventId> {
        self.source_event_id
    }

    /// Whether this is reset metadata.
    #[must_use]
    pub const fn reset(&self) -> bool {
        self.reset
    }
}

/// Canonical `d` coordinate for one Document binding head.
#[must_use]
pub fn context_binding_coordinate(project_id: Uuid, context_document_id: Uuid) -> String {
    format!("project-context-edge:{project_id}:binding:{context_document_id}")
}

/// Canonical `g` coordinate for one exact edge.
#[must_use]
pub fn context_edge_coordinate(project_id: Uuid, edge_key: EdgeKey) -> String {
    format!("project-context-edge:{project_id}:{edge_key}")
}

/// Canonical `d` coordinate for the Context catalog metadata head.
#[must_use]
pub fn context_meta_coordinate(project_id: Uuid) -> String {
    format!("project-context-edge:{project_id}:meta")
}

fn validate_projection_common(
    schema_version: u16,
    actual_type: ProjectContextProjectionType,
    expected_type: ProjectContextProjectionType,
    project_id: Uuid,
    projection_generation: u64,
    context_revision: u64,
) -> ProjectContextResult<()> {
    if schema_version != PROJECT_CONTEXT_SCHEMA_VERSION {
        return Err(ProjectContextError::UnsupportedSchemaVersion {
            got: schema_version,
            supported: PROJECT_CONTEXT_SCHEMA_VERSION,
        });
    }
    if actual_type != expected_type {
        return invalid_projection("projection_type does not match event kind");
    }
    crate::validation::validate_uuid_v4(project_id, "project_id")?;
    validate_positive(projection_generation, "projection_generation")?;
    validate_nonnegative(context_revision, "context_revision")
}

fn invalid_projection<T>(reason: &str) -> ProjectContextResult<T> {
    Err(ProjectContextError::InvalidProjection {
        reason: reason.to_owned(),
    })
}
