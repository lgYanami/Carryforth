//! Closed schema-v3 ordinary-object command and pure reducer.

use std::collections::BTreeMap;

use buzz_core::{CommunityId, PublicKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::{
    canonicalize_context_references, introduced_document_targets, validate_context_replacement,
    validate_document_target_delta, DocumentTargetDelta, ProjectContextReference,
    ProjectResourceV3, ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3,
    ProjectViewTombstoneV3, ReferenceTargetProof, V3ContractError, V3ReferenceError,
    PROJECT_VIEW_V3_SCHEMA_VERSION,
};
use crate::v2::{RoleLevel, RuntimeFence};
use crate::{
    DomainError, Goal, IssueStatus, ObjectRef, Patch, PlanStatus, Priority, ProjectIssue,
    ProjectPlan, ProjectRole, ProjectStage, ProjectViewObjectType, ProjectViewRelations,
    ProjectWork, Requirement, RequirementStatus, StageStatus, WorkStatus,
    MAX_MUTATION_CONTENT_BYTES, MAX_MUTATION_JSON_DEPTH, MAX_SAFE_REVISION,
};

/// Availability facts consulted by the pure v3 reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct V3ReducerCapabilities {
    /// Whether non-empty Context writes may add coordinates.
    pub project_context_enabled: bool,
    /// Whether new Guide and Document coordinates can be proved now.
    pub document_capability_available: bool,
}

impl V3ReducerCapabilities {
    /// Stage-4 flag-off behavior: Context stays empty, while the independently
    /// enabled Document domain may still prove Resource Guides.
    #[must_use]
    pub const fn stage4(document_capability_available: bool) -> Self {
        Self {
            project_context_enabled: false,
            document_capability_available,
        }
    }
}

/// Stable schema-v3 ordinary-object domain failures.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum V3ProjectObjectError {
    /// A legacy object invariant failed.
    #[error(transparent)]
    Object(#[from] DomainError),
    /// A v3 Resource, Context, or wire invariant failed.
    #[error(transparent)]
    Contract(#[from] V3ContractError),
    /// A Context capability or sparse Document proof failed.
    #[error(transparent)]
    Reference(#[from] V3ReferenceError),
    /// A Resource source attempted to point at another Resource.
    #[error("Resource objects cannot carry Resource Context References")]
    ResourceSourceReferenceForbidden,
    /// A Context Resource target does not exist as an active Resource.
    #[error("Context Resource target {resource_id} is not an active Resource")]
    InvalidResourceTarget {
        /// Missing, deleted, or incorrectly typed target.
        resource_id: Uuid,
    },
    /// A Resource still has an incoming normalized Context reference.
    #[error("Resource {resource_id} is still referenced by object {source_object_id}")]
    ResourceStillContextReferenced {
        /// Resource being deleted.
        resource_id: Uuid,
        /// First canonical source object.
        source_object_id: Uuid,
    },
    /// Snapshot Role-level metadata did not match Role object identities.
    #[error("invalid v3 Role level state: {0}")]
    InvalidRoleLevels(String),
}

/// Closed schema-v3 member command for ordinary Project View objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectObjectCommandV3 {
    /// Must equal three.
    pub schema_version: u16,
    /// Exact canonical Project revision observed by the caller.
    pub expected_project_revision: u64,
    /// Active Assignment used by a role-bearing or managed actor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_assignment_id: Option<Uuid>,
    /// Current supervised runtime epoch. A v3 managed actor is required by the
    /// DB coordinator to supply both Assignment and fence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_fence: Option<RuntimeFence>,
    /// Closed ordinary-object operation.
    pub request: ProjectObjectRequestV3,
}

impl ProjectObjectCommandV3 {
    /// Construct a schema-v3 ordinary-object command.
    #[must_use]
    pub const fn new(
        expected_project_revision: u64,
        acting_assignment_id: Option<Uuid>,
        request: ProjectObjectRequestV3,
    ) -> Self {
        Self {
            schema_version: PROJECT_VIEW_V3_SCHEMA_VERSION,
            expected_project_revision,
            acting_assignment_id,
            runtime_fence: None,
            request,
        }
    }

    /// Attach the exact current managed-runtime fence.
    #[must_use]
    pub const fn with_runtime_fence(mut self, runtime_fence: RuntimeFence) -> Self {
        self.runtime_fence = Some(runtime_fence);
        self
    }

    /// Parse a closed command with the existing Project View size/depth
    /// limits. A v2 command never falls back into this parser.
    pub fn from_json(content: &str) -> Result<Self, V3ProjectObjectError> {
        if content.len() > MAX_MUTATION_CONTENT_BYTES {
            return Err(DomainError::MutationContentTooLarge {
                max: MAX_MUTATION_CONTENT_BYTES,
                actual: content.len(),
            }
            .into());
        }
        let value: Value =
            serde_json::from_str(content).map_err(|error| DomainError::InvalidMutationJson {
                reason: error.to_string(),
            })?;
        let depth = json_depth(&value);
        if depth > MAX_MUTATION_JSON_DEPTH {
            return Err(DomainError::MutationJsonTooDeep {
                max: MAX_MUTATION_JSON_DEPTH,
                actual: depth,
            }
            .into());
        }
        let command: Self =
            serde_json::from_value(value).map_err(|error| DomainError::InvalidMutationJson {
                reason: error.to_string(),
            })?;
        command.validate_for_submission()?;
        Ok(command)
    }

    /// Validate fields that do not require canonical Project or Document
    /// state.
    pub fn validate_for_submission(&self) -> Result<(), V3ProjectObjectError> {
        if self.schema_version != PROJECT_VIEW_V3_SCHEMA_VERSION {
            return Err(DomainError::UnsupportedSchemaVersion {
                got: u32::from(self.schema_version),
                supported: u32::from(PROJECT_VIEW_V3_SCHEMA_VERSION),
            }
            .into());
        }
        require_revision(self.expected_project_revision)?;
        if self.expected_project_revision == 0 {
            return Err(DomainError::InvalidField {
                field: "expected_project_revision",
                reason: "ordinary v3 commands require an initialized positive revision".to_owned(),
            }
            .into());
        }
        if self
            .acting_assignment_id
            .is_some_and(|assignment_id| assignment_id.is_nil())
        {
            return Err(DomainError::InvalidField {
                field: "acting_assignment_id",
                reason: "must not be nil".to_owned(),
            }
            .into());
        }
        if let Some(fence) = self.runtime_fence {
            fence
                .validate()
                .map_err(|reason| DomainError::InvalidField {
                    field: "runtime_fence",
                    reason,
                })?;
            if self.acting_assignment_id.is_none() {
                return Err(DomainError::InvalidField {
                    field: "runtime_fence",
                    reason: "requires acting_assignment_id".to_owned(),
                }
                .into());
            }
        }
        self.request.validate_for_submission()
    }

    /// Stable operation spelling used by receipts and metrics.
    #[must_use]
    pub const fn operation(&self) -> &'static str {
        match self.request {
            ProjectObjectRequestV3::Create(_) => "create",
            ProjectObjectRequestV3::Update(_) => "update",
            ProjectObjectRequestV3::Delete(_) => "delete",
        }
    }
}

/// Closed v3 ordinary-object operation set. Initialization uses the separate
/// prepared bootstrap command and is intentionally absent here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProjectObjectRequestV3 {
    /// Create one non-profile object.
    Create(CreateProjectObjectV3),
    /// Patch one active object and optionally replace its complete Context set.
    Update(UpdateProjectObjectV3),
    /// Tombstone one active object.
    Delete(DeleteProjectObjectV3),
}

impl ProjectObjectRequestV3 {
    fn validate_for_submission(&self) -> Result<(), V3ProjectObjectError> {
        match self {
            Self::Create(create) => create.object.validate_for_submission(),
            Self::Update(update) => update.validate_for_submission(),
            Self::Delete(delete) => {
                if delete.object_type == ProjectViewObjectType::ProjectProfile {
                    return Err(DomainError::ProfileDeletionForbidden.into());
                }
                require_uuid_v4(delete.object_id)?;
                Ok(())
            }
        }
    }
}

/// Create payload for one v3 Project View object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateProjectObjectV3 {
    /// Complete typed object creation payload.
    pub object: NewProjectViewObjectV3,
}

/// Typed v3 creation payloads for every object except Project Profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "object_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NewProjectViewObjectV3 {
    /// Goal.
    Goal {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Goal title.
        title: String,
        /// Observable outcome.
        desired_outcome: String,
        /// Strategic directions.
        directions: Vec<String>,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Project Role definition. New ordinary Roles begin at member level.
    Role {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Role name.
        name: String,
        /// Role purpose.
        purpose: String,
        /// Responsibilities.
        responsibilities: Vec<String>,
        /// Explicit boundaries.
        boundaries: Vec<String>,
        /// Whether this Role may receive Assignments.
        active: bool,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Plan.
    Plan {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Plan title.
        title: String,
        /// Plan description.
        description: String,
        /// Plan status.
        status: PlanStatus,
        /// Optional owning Goal.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        under_goal_id: Option<Uuid>,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Stage.
    Stage {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Stage title.
        title: String,
        /// Stage description.
        description: String,
        /// Stage status.
        status: StageStatus,
        /// Required parent Plan.
        under_plan_id: Uuid,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Requirement.
    Requirement {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Requirement title.
        title: String,
        /// Requirement description.
        description: String,
        /// Requirement status.
        status: RequirementStatus,
        /// Requirement priority.
        priority: Priority,
        /// Optional planning Stage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        planned_in_stage_id: Option<Uuid>,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Issue.
    Issue {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Issue title.
        title: String,
        /// Issue description.
        description: String,
        /// Issue status.
        status: IssueStatus,
        /// Issue priority.
        priority: Priority,
        /// Optional planning Stage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        planned_in_stage_id: Option<Uuid>,
        /// Optional subject.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        about: Option<ObjectRef>,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Work item.
    Work {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Work title.
        title: String,
        /// Work description.
        description: String,
        /// Work status.
        status: WorkStatus,
        /// Work priority.
        priority: Priority,
        /// Requirement or Issue handled by this Work.
        handles: ObjectRef,
        /// Initial canonical Context set.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
    /// Locator-free Resource with a mandatory Guide Document.
    Resource {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Human-readable Resource name.
        name: String,
        /// Open canonical kind token.
        resource_kind: String,
        /// Optional summary.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        summary: Option<String>,
        /// Required active Guide Document.
        guide_document_id: Uuid,
        /// Initial canonical Context set; Resource targets are forbidden.
        #[serde(default)]
        context_references: Vec<ProjectContextReference>,
    },
}

impl NewProjectViewObjectV3 {
    /// Client-generated object ID.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Goal { id, .. }
            | Self::Role { id, .. }
            | Self::Plan { id, .. }
            | Self::Stage { id, .. }
            | Self::Requirement { id, .. }
            | Self::Issue { id, .. }
            | Self::Work { id, .. }
            | Self::Resource { id, .. } => *id,
        }
    }

    /// Canonical object type.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::Goal { .. } => ProjectViewObjectType::Goal,
            Self::Role { .. } => ProjectViewObjectType::Role,
            Self::Plan { .. } => ProjectViewObjectType::Plan,
            Self::Stage { .. } => ProjectViewObjectType::Stage,
            Self::Requirement { .. } => ProjectViewObjectType::Requirement,
            Self::Issue { .. } => ProjectViewObjectType::Issue,
            Self::Work { .. } => ProjectViewObjectType::Work,
            Self::Resource { .. } => ProjectViewObjectType::Resource,
        }
    }

    fn validate_for_submission(&self) -> Result<(), V3ProjectObjectError> {
        require_uuid_v4(self.id())?;
        let (_, data, relations, context) = self.clone().into_parts();
        super::validation::validate_object_data(&data)?;
        super::validation::validate_relation_shape(data.object_type(), &relations)?;
        let canonical = canonicalize_context_references(context.clone())?;
        if canonical != context {
            return Err(V3ContractError::InvalidContext(
                "Context References are not in canonical order".to_owned(),
            )
            .into());
        }
        validate_context_source(data.object_type(), &context)?;
        if let ProjectViewObjectDataV3::Issue(_) = data {
            if relations
                .about
                .is_some_and(|reference| reference.object_id == self.id())
            {
                return Err(DomainError::SelfReference {
                    relation: "about",
                    object_id: self.id(),
                }
                .into());
            }
        }
        Ok(())
    }

    /// Split the create payload into canonical identity, data, structural
    /// relations, and Context set.
    #[allow(clippy::too_many_lines)]
    pub fn into_parts(
        self,
    ) -> (
        Uuid,
        ProjectViewObjectDataV3,
        ProjectViewRelations,
        Vec<ProjectContextReference>,
    ) {
        match self {
            Self::Goal {
                id,
                title,
                desired_outcome,
                directions,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Goal(Goal {
                    title,
                    desired_outcome,
                    directions,
                }),
                ProjectViewRelations::default(),
                context_references,
            ),
            Self::Role {
                id,
                name,
                purpose,
                responsibilities,
                boundaries,
                active,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Role(ProjectRole {
                    name,
                    purpose,
                    responsibilities,
                    boundaries,
                    active,
                }),
                ProjectViewRelations::default(),
                context_references,
            ),
            Self::Plan {
                id,
                title,
                description,
                status,
                under_goal_id,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Plan(ProjectPlan {
                    title,
                    description,
                    status,
                }),
                ProjectViewRelations {
                    under_goal_id,
                    ..ProjectViewRelations::default()
                },
                context_references,
            ),
            Self::Stage {
                id,
                title,
                description,
                status,
                under_plan_id,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Stage(ProjectStage {
                    title,
                    description,
                    status,
                }),
                ProjectViewRelations {
                    under_plan_id: Some(under_plan_id),
                    ..ProjectViewRelations::default()
                },
                context_references,
            ),
            Self::Requirement {
                id,
                title,
                description,
                status,
                priority,
                planned_in_stage_id,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Requirement(Requirement {
                    title,
                    description,
                    status,
                    priority,
                }),
                ProjectViewRelations {
                    planned_in_stage_id,
                    ..ProjectViewRelations::default()
                },
                context_references,
            ),
            Self::Issue {
                id,
                title,
                description,
                status,
                priority,
                planned_in_stage_id,
                about,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Issue(ProjectIssue {
                    title,
                    description,
                    status,
                    priority,
                }),
                ProjectViewRelations {
                    planned_in_stage_id,
                    about,
                    ..ProjectViewRelations::default()
                },
                context_references,
            ),
            Self::Work {
                id,
                title,
                description,
                status,
                priority,
                handles,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Work(ProjectWork {
                    title,
                    description,
                    status,
                    priority,
                }),
                ProjectViewRelations {
                    handles: Some(handles),
                    ..ProjectViewRelations::default()
                },
                context_references,
            ),
            Self::Resource {
                id,
                name,
                resource_kind,
                summary,
                guide_document_id,
                context_references,
            } => (
                id,
                ProjectViewObjectDataV3::Resource(ProjectResourceV3 {
                    name,
                    resource_kind,
                    summary,
                    guide_document_id,
                }),
                ProjectViewRelations::default(),
                context_references,
            ),
        }
    }
}

macro_rules! v3_patch {
    (
        $(#[$meta:meta])*
        $name:ident {
            $(
                $(#[$field_meta:meta])*
                $field:ident: $field_type:ty
            ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                $(#[$field_meta])*
                #[serde(default, skip_serializing_if = "Patch::is_unchanged")]
                pub $field: Patch<$field_type>,
            )*
            /// Complete replacement Context set. Absence preserves the current
            /// canonical set; explicit JSON `null` is rejected.
            #[serde(
                default,
                skip_serializing_if = "Option::is_none",
                deserialize_with = "deserialize_optional_non_null"
            )]
            pub context_references: Option<Vec<ProjectContextReference>>,
        }
    };
}

v3_patch! {
    /// Project Profile v3 patch.
    ProfilePatchV3 {
        /// Profile name.
        name: String,
        /// Product positioning.
        positioning: String,
        /// Project purpose.
        purpose: String,
        /// Problem being solved.
        problem: String,
        /// Project scope.
        scope: String,
    }
}

v3_patch! {
    /// Goal v3 patch.
    GoalPatchV3 {
        /// Goal title.
        title: String,
        /// Observable desired outcome.
        desired_outcome: String,
        /// Strategic directions.
        directions: Vec<String>,
    }
}

v3_patch! {
    /// Role definition v3 patch.
    RolePatchV3 {
        /// Role name.
        name: String,
        /// Role purpose.
        purpose: String,
        /// Responsibilities.
        responsibilities: Vec<String>,
        /// Boundaries.
        boundaries: Vec<String>,
        /// Whether the Role may receive Assignments.
        active: bool,
    }
}

v3_patch! {
    /// Plan v3 patch.
    PlanPatchV3 {
        /// Plan title.
        title: String,
        /// Plan description.
        description: String,
        /// Plan status.
        status: PlanStatus,
        /// Optional owning Goal; clear unbinds it.
        under_goal_id: Uuid,
    }
}

v3_patch! {
    /// Stage v3 patch.
    StagePatchV3 {
        /// Stage title.
        title: String,
        /// Stage description.
        description: String,
        /// Stage status.
        status: StageStatus,
        /// Required parent Plan.
        under_plan_id: Uuid,
    }
}

v3_patch! {
    /// Requirement v3 patch.
    RequirementPatchV3 {
        /// Requirement title.
        title: String,
        /// Requirement description.
        description: String,
        /// Requirement status.
        status: RequirementStatus,
        /// Requirement priority.
        priority: Priority,
        /// Optional planning Stage.
        planned_in_stage_id: Uuid,
    }
}

v3_patch! {
    /// Issue v3 patch.
    IssuePatchV3 {
        /// Issue title.
        title: String,
        /// Issue description.
        description: String,
        /// Issue status.
        status: IssueStatus,
        /// Issue priority.
        priority: Priority,
        /// Optional planning Stage.
        planned_in_stage_id: Uuid,
        /// Optional subject.
        about: ObjectRef,
    }
}

v3_patch! {
    /// Work v3 patch.
    WorkPatchV3 {
        /// Work title.
        title: String,
        /// Work description.
        description: String,
        /// Work status.
        status: WorkStatus,
        /// Work priority.
        priority: Priority,
        /// Required Requirement or Issue target.
        handles: ObjectRef,
    }
}

v3_patch! {
    /// Locator-free Resource v3 patch.
    ResourcePatchV3 {
        /// Resource name.
        name: String,
        /// Open kind token.
        resource_kind: String,
        /// Optional summary; clear removes it.
        summary: String,
        /// Required Guide Document identity.
        guide_document_id: Uuid,
    }
}

/// Typed v3 update of one active object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "object_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateProjectObjectV3 {
    /// Patch the unique Project Profile.
    ProjectProfile {
        /// Profile object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: ProfilePatchV3,
    },
    /// Patch a Goal.
    Goal {
        /// Goal object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: GoalPatchV3,
    },
    /// Patch a Role definition.
    Role {
        /// Role object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: RolePatchV3,
    },
    /// Patch a Plan.
    Plan {
        /// Plan object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: PlanPatchV3,
    },
    /// Patch a Stage.
    Stage {
        /// Stage object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: StagePatchV3,
    },
    /// Patch a Requirement.
    Requirement {
        /// Requirement object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: RequirementPatchV3,
    },
    /// Patch an Issue.
    Issue {
        /// Issue object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: IssuePatchV3,
    },
    /// Patch a Work item.
    Work {
        /// Work object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: WorkPatchV3,
    },
    /// Patch a Resource and/or replace its Document-only Context set.
    Resource {
        /// Resource object ID.
        object_id: Uuid,
        /// Typed v3 patch.
        patch: ResourcePatchV3,
    },
}

impl UpdateProjectObjectV3 {
    /// Target object ID.
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        match self {
            Self::ProjectProfile { object_id, .. }
            | Self::Goal { object_id, .. }
            | Self::Role { object_id, .. }
            | Self::Plan { object_id, .. }
            | Self::Stage { object_id, .. }
            | Self::Requirement { object_id, .. }
            | Self::Issue { object_id, .. }
            | Self::Work { object_id, .. }
            | Self::Resource { object_id, .. } => *object_id,
        }
    }

    /// Expected immutable object type.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::ProjectProfile { .. } => ProjectViewObjectType::ProjectProfile,
            Self::Goal { .. } => ProjectViewObjectType::Goal,
            Self::Role { .. } => ProjectViewObjectType::Role,
            Self::Plan { .. } => ProjectViewObjectType::Plan,
            Self::Stage { .. } => ProjectViewObjectType::Stage,
            Self::Requirement { .. } => ProjectViewObjectType::Requirement,
            Self::Issue { .. } => ProjectViewObjectType::Issue,
            Self::Work { .. } => ProjectViewObjectType::Work,
            Self::Resource { .. } => ProjectViewObjectType::Resource,
        }
    }

    /// Optional complete Context replacement.
    #[must_use]
    pub fn context_references(&self) -> Option<&[ProjectContextReference]> {
        match self {
            Self::ProjectProfile { patch, .. } => patch.context_references.as_deref(),
            Self::Goal { patch, .. } => patch.context_references.as_deref(),
            Self::Role { patch, .. } => patch.context_references.as_deref(),
            Self::Plan { patch, .. } => patch.context_references.as_deref(),
            Self::Stage { patch, .. } => patch.context_references.as_deref(),
            Self::Requirement { patch, .. } => patch.context_references.as_deref(),
            Self::Issue { patch, .. } => patch.context_references.as_deref(),
            Self::Work { patch, .. } => patch.context_references.as_deref(),
            Self::Resource { patch, .. } => patch.context_references.as_deref(),
        }
    }

    fn validate_for_submission(&self) -> Result<(), V3ProjectObjectError> {
        if self.object_type() != ProjectViewObjectType::ProjectProfile {
            require_uuid_v4(self.object_id())?;
        }
        if let Some(references) = self.context_references() {
            let canonical = canonicalize_context_references(references.to_vec())?;
            if canonical != references {
                return Err(V3ContractError::InvalidContext(
                    "Context References are not in canonical order".to_owned(),
                )
                .into());
            }
            validate_context_source(self.object_type(), references)?;
        }
        super::validation::validate_update(self)
    }
}

/// Delete payload for one active v3 object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteProjectObjectV3 {
    /// Type expected by the caller.
    pub object_type: ProjectViewObjectType,
    /// Object to tombstone.
    pub object_id: Uuid,
}

/// Result of one successful pure v3 object reduction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectObjectOutcomeV3 {
    /// New global Project revision.
    pub project_revision: u64,
    /// Canonical entries changed by this command.
    pub changed_entries: Vec<ProjectViewEntryV3>,
    /// Document coordinates proved for this exact change.
    pub document_target_delta: DocumentTargetDelta,
}

/// Canonical Project View v3 object state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewStateV3 {
    project_id: CommunityId,
    project_revision: u64,
    initialized_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    entries: BTreeMap<Uuid, ProjectViewEntryV3>,
    role_levels: BTreeMap<Uuid, RoleLevel>,
}

impl ProjectViewStateV3 {
    /// Construct an empty uninitialized v3 state.
    #[must_use]
    pub const fn new(project_id: CommunityId) -> Self {
        Self {
            project_id,
            project_revision: 0,
            initialized_at: None,
            updated_at: None,
            entries: BTreeMap::new(),
            role_levels: BTreeMap::new(),
        }
    }

    /// Reconstruct and validate one canonical snapshot.
    pub fn from_snapshot(
        project_id: CommunityId,
        project_revision: u64,
        initialized_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
        entries: impl IntoIterator<Item = ProjectViewEntryV3>,
        role_levels: impl IntoIterator<Item = (Uuid, RoleLevel)>,
    ) -> Result<Self, V3ProjectObjectError> {
        let mut by_id = BTreeMap::new();
        for entry in entries {
            let object_id = entry.id();
            if by_id.insert(object_id, entry).is_some() {
                return Err(DomainError::ObjectIdAlreadyUsed { object_id }.into());
            }
        }
        let mut levels = BTreeMap::new();
        for (role_id, level) in role_levels {
            if levels.insert(role_id, level).is_some() {
                return Err(V3ProjectObjectError::InvalidRoleLevels(format!(
                    "duplicate Role level for {role_id}"
                )));
            }
        }
        let state = Self {
            project_id,
            project_revision,
            initialized_at,
            updated_at,
            entries: by_id,
            role_levels: levels,
        };
        state.validate()?;
        Ok(state)
    }

    /// Server-resolved Community/Project identity.
    #[must_use]
    pub const fn project_id(&self) -> CommunityId {
        self.project_id
    }

    /// Current global Project revision.
    #[must_use]
    pub const fn project_revision(&self) -> u64 {
        self.project_revision
    }

    /// Whether the v3 state has been initialized.
    #[must_use]
    pub const fn is_initialized(&self) -> bool {
        self.initialized_at.is_some()
    }

    /// Canonical initialization time.
    #[must_use]
    pub const fn initialized_at(&self) -> Option<DateTime<Utc>> {
        self.initialized_at
    }

    /// Canonical aggregate update time.
    #[must_use]
    pub const fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.updated_at
    }

    /// Every occupied identity, including tombstones.
    #[must_use]
    pub const fn entries(&self) -> &BTreeMap<Uuid, ProjectViewEntryV3> {
        &self.entries
    }

    /// Lookup one active object or tombstone.
    #[must_use]
    pub fn entry(&self, object_id: Uuid) -> Option<&ProjectViewEntryV3> {
        self.entries.get(&object_id)
    }

    /// Iterate active objects in stable UUID order.
    pub fn active_objects(&self) -> impl Iterator<Item = &ProjectViewObjectV3> {
        self.entries.values().filter_map(|entry| match entry {
            ProjectViewEntryV3::Active(object) => Some(object.as_ref()),
            ProjectViewEntryV3::Tombstone(_) => None,
        })
    }

    /// Governance level associated with a Role identity, including a retained
    /// Role tombstone.
    #[must_use]
    pub fn role_level(&self, role_id: Uuid) -> Option<RoleLevel> {
        self.role_levels.get(&role_id).copied()
    }

    pub(super) const fn role_levels(&self) -> &BTreeMap<Uuid, RoleLevel> {
        &self.role_levels
    }

    /// Validate the complete canonical v3 snapshot.
    pub fn validate(&self) -> Result<(), V3ProjectObjectError> {
        super::validation::validate_state(self)
    }

    /// Compute the bounded Document proof request for one command without
    /// mutating state.
    pub fn document_target_delta(
        &self,
        command: &ProjectObjectCommandV3,
        capabilities: V3ReducerCapabilities,
    ) -> Result<DocumentTargetDelta, V3ProjectObjectError> {
        let (_, delta) = self.prepare_next_entry(command, capabilities)?;
        Ok(delta)
    }

    /// Apply one command atomically.
    pub fn apply(
        &mut self,
        command: &ProjectObjectCommandV3,
        actor: PublicKey,
        now: DateTime<Utc>,
        capabilities: V3ReducerCapabilities,
        proof: &ReferenceTargetProof,
    ) -> Result<ProjectObjectOutcomeV3, V3ProjectObjectError> {
        let (next, outcome) = self.reduce(command, actor, now, capabilities, proof)?;
        *self = next;
        Ok(outcome)
    }

    /// Compute one atomic v3 transition without changing current state.
    pub fn reduce(
        &self,
        command: &ProjectObjectCommandV3,
        actor: PublicKey,
        now: DateTime<Utc>,
        capabilities: V3ReducerCapabilities,
        proof: &ReferenceTargetProof,
    ) -> Result<(Self, ProjectObjectOutcomeV3), V3ProjectObjectError> {
        self.validate()?;
        command.validate_for_submission()?;
        if !self.is_initialized() {
            return Err(DomainError::NotInitialized.into());
        }
        if command.expected_project_revision != self.project_revision {
            return Err(DomainError::RevisionConflict {
                expected: command.expected_project_revision,
                actual: self.project_revision,
            }
            .into());
        }
        let (prepared, delta) = self.prepare_next_entry(command, capabilities)?;
        validate_document_target_delta(&delta, capabilities.document_capability_available, proof)?;

        let project_revision = next_revision(self.project_revision)?;
        let mut next = self.clone();
        let changed = match prepared {
            PreparedEntryV3::Create {
                object_id,
                data,
                relations,
                context_references,
            } => {
                let object = ProjectViewObjectV3 {
                    id: object_id,
                    object_type: data.object_type(),
                    object_revision: 1,
                    project_revision,
                    created_at: now,
                    updated_at: now,
                    created_by: actor,
                    updated_by: actor,
                    data,
                    relations,
                    context_references,
                };
                if object.object_type == ProjectViewObjectType::Role {
                    next.role_levels.insert(object.id, RoleLevel::Member);
                }
                ProjectViewEntryV3::Active(Box::new(object))
            }
            PreparedEntryV3::Update(mut object) => {
                object.object_revision = next_revision(object.object_revision)?;
                object.project_revision = project_revision;
                object.updated_at = now;
                object.updated_by = actor;
                ProjectViewEntryV3::Active(Box::new(object))
            }
            PreparedEntryV3::Delete(current) => {
                ProjectViewEntryV3::Tombstone(ProjectViewTombstoneV3 {
                    id: current.id,
                    object_type: current.object_type,
                    object_revision: next_revision(current.object_revision)?,
                    project_revision,
                    created_at: current.created_at,
                    deleted_at: now,
                    created_by: current.created_by,
                    deleted_by: actor,
                })
            }
        };
        next.entries.insert(changed.id(), changed.clone());
        next.project_revision = project_revision;
        next.updated_at = Some(now);
        next.validate()?;
        Ok((
            next,
            ProjectObjectOutcomeV3 {
                project_revision,
                changed_entries: vec![changed],
                document_target_delta: delta,
            },
        ))
    }

    fn prepare_next_entry(
        &self,
        command: &ProjectObjectCommandV3,
        capabilities: V3ReducerCapabilities,
    ) -> Result<(PreparedEntryV3, DocumentTargetDelta), V3ProjectObjectError> {
        match &command.request {
            ProjectObjectRequestV3::Create(create) => {
                let (object_id, data, relations, context_references) =
                    create.object.clone().into_parts();
                if object_id == *self.project_id.as_uuid() {
                    return Err(DomainError::ReservedProfileId { object_id }.into());
                }
                if self.entries.contains_key(&object_id) {
                    return Err(DomainError::ObjectIdAlreadyUsed { object_id }.into());
                }
                let context_references = validate_context_replacement(
                    &[],
                    context_references,
                    capabilities.project_context_enabled,
                )?;
                validate_context_source(data.object_type(), &context_references)?;
                let next_guide = guide_document_id(&data);
                let delta = introduced_document_targets(&[], &context_references, None, next_guide);
                let prepared = PreparedEntryV3::Create {
                    object_id,
                    data,
                    relations,
                    context_references,
                };
                self.validate_candidate(&prepared)?;
                Ok((prepared, delta))
            }
            ProjectObjectRequestV3::Update(update) => {
                let object_id = update.object_id();
                let current = self.active_object(object_id)?.clone();
                if current.object_type != update.object_type() {
                    return Err(DomainError::ObjectTypeMismatch {
                        object_id,
                        expected: update.object_type(),
                        actual: current.object_type,
                    }
                    .into());
                }
                let mut updated = current.clone();
                apply_update(&mut updated, update)?;
                if let Some(replacement) = update.context_references() {
                    updated.context_references = validate_context_replacement(
                        &current.context_references,
                        replacement.to_vec(),
                        capabilities.project_context_enabled,
                    )?;
                }
                validate_context_source(updated.object_type, &updated.context_references)?;
                if updated.data == current.data
                    && updated.relations == current.relations
                    && updated.context_references == current.context_references
                {
                    return Err(DomainError::NoChanges.into());
                }
                let delta = introduced_document_targets(
                    &current.context_references,
                    &updated.context_references,
                    guide_document_id(&current.data),
                    guide_document_id(&updated.data),
                );
                let prepared = PreparedEntryV3::Update(updated);
                self.validate_candidate(&prepared)?;
                Ok((prepared, delta))
            }
            ProjectObjectRequestV3::Delete(delete) => {
                let current = self.active_object(delete.object_id)?.clone();
                if current.object_type != delete.object_type {
                    return Err(DomainError::ObjectTypeMismatch {
                        object_id: delete.object_id,
                        expected: delete.object_type,
                        actual: current.object_type,
                    }
                    .into());
                }
                if current.object_type == ProjectViewObjectType::ProjectProfile {
                    return Err(DomainError::ProfileDeletionForbidden.into());
                }
                if current.object_type == ProjectViewObjectType::Goal
                    && self
                        .active_objects()
                        .filter(|object| object.object_type == ProjectViewObjectType::Goal)
                        .count()
                        == 1
                {
                    return Err(DomainError::LastGoalDeletionForbidden.into());
                }
                if let Some(relation) = self.first_incoming_structural_relation(current.id) {
                    return Err(DomainError::ObjectStillReferenced {
                        object_id: current.id,
                        relation,
                    }
                    .into());
                }
                if current.object_type == ProjectViewObjectType::Resource {
                    if let Some(source_object_id) = self.first_incoming_resource_context(current.id)
                    {
                        return Err(V3ProjectObjectError::ResourceStillContextReferenced {
                            resource_id: current.id,
                            source_object_id,
                        });
                    }
                }
                Ok((
                    PreparedEntryV3::Delete(current),
                    DocumentTargetDelta::default(),
                ))
            }
        }
    }

    fn validate_candidate(&self, prepared: &PreparedEntryV3) -> Result<(), V3ProjectObjectError> {
        let mut candidate = self.clone();
        let exemplar =
            self.active_objects()
                .next()
                .ok_or_else(|| DomainError::InvalidFinalState {
                    reason: "initialized v3 state has no active Profile".to_owned(),
                })?;
        let entry = match prepared {
            PreparedEntryV3::Create {
                object_id,
                data,
                relations,
                context_references,
            } => ProjectViewEntryV3::Active(Box::new(ProjectViewObjectV3 {
                id: *object_id,
                object_type: data.object_type(),
                object_revision: 1,
                project_revision: self.project_revision.saturating_add(1),
                created_at: exemplar.created_at,
                updated_at: exemplar.updated_at,
                created_by: exemplar.created_by,
                updated_by: exemplar.updated_by,
                data: data.clone(),
                relations: *relations,
                context_references: context_references.clone(),
            })),
            PreparedEntryV3::Update(object) => ProjectViewEntryV3::Active(Box::new(object.clone())),
            PreparedEntryV3::Delete(_) => return Ok(()),
        };
        if entry.object_type() == ProjectViewObjectType::Role {
            candidate
                .role_levels
                .entry(entry.id())
                .or_insert(RoleLevel::Member);
        }
        candidate.entries.insert(entry.id(), entry);
        candidate.validate_relation_targets_and_context()
    }

    fn active_object(&self, object_id: Uuid) -> Result<&ProjectViewObjectV3, DomainError> {
        match self.entries.get(&object_id) {
            Some(ProjectViewEntryV3::Active(object)) => Ok(object),
            Some(ProjectViewEntryV3::Tombstone(_)) => Err(DomainError::ObjectDeleted { object_id }),
            None => Err(DomainError::ObjectNotFound { object_id }),
        }
    }

    fn first_incoming_structural_relation(&self, target_id: Uuid) -> Option<&'static str> {
        for source in self.active_objects() {
            if source.relations.under_goal_id == Some(target_id) {
                return Some("under_goal_id");
            }
            if source.relations.under_plan_id == Some(target_id) {
                return Some("under_plan_id");
            }
            if source.relations.planned_in_stage_id == Some(target_id) {
                return Some("planned_in_stage_id");
            }
            if source
                .relations
                .about
                .is_some_and(|reference| reference.object_id == target_id)
            {
                return Some("about");
            }
            if source
                .relations
                .handles
                .is_some_and(|reference| reference.object_id == target_id)
            {
                return Some("handles");
            }
        }
        None
    }

    fn first_incoming_resource_context(&self, resource_id: Uuid) -> Option<Uuid> {
        self.active_objects().find_map(|source| {
            source.context_references.iter().find_map(|reference| {
                matches!(
                    reference,
                    ProjectContextReference::Resource {
                        resource_id: target
                    } if *target == resource_id
                )
                .then_some(source.id)
            })
        })
    }

    pub(super) fn validate_relation_targets_and_context(&self) -> Result<(), V3ProjectObjectError> {
        for object in self.active_objects() {
            super::validation::validate_relation_targets(self, object)?;
            validate_context_source(object.object_type, &object.context_references)?;
            for reference in &object.context_references {
                if let ProjectContextReference::Resource { resource_id } = reference {
                    match self.entries.get(resource_id) {
                        Some(ProjectViewEntryV3::Active(target))
                            if target.object_type == ProjectViewObjectType::Resource => {}
                        _ => {
                            return Err(V3ProjectObjectError::InvalidResourceTarget {
                                resource_id: *resource_id,
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

enum PreparedEntryV3 {
    Create {
        object_id: Uuid,
        data: ProjectViewObjectDataV3,
        relations: ProjectViewRelations,
        context_references: Vec<ProjectContextReference>,
    },
    Update(ProjectViewObjectV3),
    Delete(ProjectViewObjectV3),
}

fn validate_context_source(
    object_type: ProjectViewObjectType,
    references: &[ProjectContextReference],
) -> Result<(), V3ProjectObjectError> {
    if object_type == ProjectViewObjectType::Resource
        && references
            .iter()
            .any(|reference| matches!(reference, ProjectContextReference::Resource { .. }))
    {
        return Err(V3ProjectObjectError::ResourceSourceReferenceForbidden);
    }
    Ok(())
}

fn guide_document_id(data: &ProjectViewObjectDataV3) -> Option<Uuid> {
    match data {
        ProjectViewObjectDataV3::Resource(resource) => Some(resource.guide_document_id),
        _ => None,
    }
}

fn apply_update(
    object: &mut ProjectViewObjectV3,
    update: &UpdateProjectObjectV3,
) -> Result<(), V3ProjectObjectError> {
    match (update, &mut object.data) {
        (
            UpdateProjectObjectV3::ProjectProfile { patch, .. },
            ProjectViewObjectDataV3::ProjectProfile(profile),
        ) => {
            apply_required(&mut profile.name, &patch.name, "name")?;
            apply_required(&mut profile.positioning, &patch.positioning, "positioning")?;
            apply_required(&mut profile.purpose, &patch.purpose, "purpose")?;
            apply_required(&mut profile.problem, &patch.problem, "problem")?;
            apply_required(&mut profile.scope, &patch.scope, "scope")?;
        }
        (UpdateProjectObjectV3::Goal { patch, .. }, ProjectViewObjectDataV3::Goal(goal)) => {
            apply_required(&mut goal.title, &patch.title, "title")?;
            apply_required(
                &mut goal.desired_outcome,
                &patch.desired_outcome,
                "desired_outcome",
            )?;
            apply_required(&mut goal.directions, &patch.directions, "directions")?;
        }
        (UpdateProjectObjectV3::Role { patch, .. }, ProjectViewObjectDataV3::Role(role)) => {
            apply_required(&mut role.name, &patch.name, "name")?;
            apply_required(&mut role.purpose, &patch.purpose, "purpose")?;
            apply_required(
                &mut role.responsibilities,
                &patch.responsibilities,
                "responsibilities",
            )?;
            apply_required(&mut role.boundaries, &patch.boundaries, "boundaries")?;
            apply_required(&mut role.active, &patch.active, "active")?;
        }
        (UpdateProjectObjectV3::Plan { patch, .. }, ProjectViewObjectDataV3::Plan(plan)) => {
            apply_required(&mut plan.title, &patch.title, "title")?;
            apply_required(&mut plan.description, &patch.description, "description")?;
            apply_required(&mut plan.status, &patch.status, "status")?;
            apply_optional(&mut object.relations.under_goal_id, &patch.under_goal_id);
        }
        (UpdateProjectObjectV3::Stage { patch, .. }, ProjectViewObjectDataV3::Stage(stage)) => {
            apply_required(&mut stage.title, &patch.title, "title")?;
            apply_required(&mut stage.description, &patch.description, "description")?;
            apply_required(&mut stage.status, &patch.status, "status")?;
            apply_required_relation(
                &mut object.relations.under_plan_id,
                &patch.under_plan_id,
                "under_plan_id",
            )?;
        }
        (
            UpdateProjectObjectV3::Requirement { patch, .. },
            ProjectViewObjectDataV3::Requirement(requirement),
        ) => {
            apply_required(&mut requirement.title, &patch.title, "title")?;
            apply_required(
                &mut requirement.description,
                &patch.description,
                "description",
            )?;
            apply_required(&mut requirement.status, &patch.status, "status")?;
            apply_required(&mut requirement.priority, &patch.priority, "priority")?;
            apply_optional(
                &mut object.relations.planned_in_stage_id,
                &patch.planned_in_stage_id,
            );
        }
        (UpdateProjectObjectV3::Issue { patch, .. }, ProjectViewObjectDataV3::Issue(issue)) => {
            apply_required(&mut issue.title, &patch.title, "title")?;
            apply_required(&mut issue.description, &patch.description, "description")?;
            apply_required(&mut issue.status, &patch.status, "status")?;
            apply_required(&mut issue.priority, &patch.priority, "priority")?;
            apply_optional(
                &mut object.relations.planned_in_stage_id,
                &patch.planned_in_stage_id,
            );
            apply_optional(&mut object.relations.about, &patch.about);
        }
        (UpdateProjectObjectV3::Work { patch, .. }, ProjectViewObjectDataV3::Work(work)) => {
            apply_required(&mut work.title, &patch.title, "title")?;
            apply_required(&mut work.description, &patch.description, "description")?;
            apply_required(&mut work.status, &patch.status, "status")?;
            apply_required(&mut work.priority, &patch.priority, "priority")?;
            apply_required_relation(&mut object.relations.handles, &patch.handles, "handles")?;
        }
        (
            UpdateProjectObjectV3::Resource { patch, .. },
            ProjectViewObjectDataV3::Resource(resource),
        ) => {
            apply_required(&mut resource.name, &patch.name, "name")?;
            apply_required(
                &mut resource.resource_kind,
                &patch.resource_kind,
                "resource_kind",
            )?;
            apply_optional(&mut resource.summary, &patch.summary);
            apply_required(
                &mut resource.guide_document_id,
                &patch.guide_document_id,
                "guide_document_id",
            )?;
        }
        _ => {
            return Err(DomainError::DataTypeMismatch {
                declared: object.object_type,
                actual: object.data.object_type(),
            }
            .into());
        }
    }
    super::validation::validate_object_data(&object.data)?;
    Ok(())
}

fn apply_required<T: Clone>(
    current: &mut T,
    patch: &Patch<T>,
    field: &'static str,
) -> Result<(), DomainError> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::RequiredField { field }),
        Patch::Set(value) => {
            current.clone_from(value);
            Ok(())
        }
    }
}

fn apply_optional<T: Clone>(current: &mut Option<T>, patch: &Patch<T>) {
    match patch {
        Patch::Unchanged => {}
        Patch::Clear => *current = None,
        Patch::Set(value) => *current = Some(value.clone()),
    }
}

fn apply_required_relation<T: Clone>(
    current: &mut Option<T>,
    patch: &Patch<T>,
    relation: &'static str,
) -> Result<(), DomainError> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::MissingRequiredRelation { relation }),
        Patch::Set(value) => {
            *current = Some(value.clone());
            Ok(())
        }
    }
}

fn require_revision(revision: u64) -> Result<(), DomainError> {
    if revision > MAX_SAFE_REVISION {
        Err(DomainError::RevisionOutOfRange {
            revision,
            max: MAX_SAFE_REVISION,
        })
    } else {
        Ok(())
    }
}

fn require_uuid_v4(object_id: Uuid) -> Result<(), DomainError> {
    if object_id.get_version_num() == 4 && object_id.get_variant() == uuid::Variant::RFC4122 {
        Ok(())
    } else {
        Err(DomainError::InvalidObjectId { object_id })
    }
}

fn next_revision(current: u64) -> Result<u64, DomainError> {
    let next = current
        .checked_add(1)
        .filter(|revision| *revision <= MAX_SAFE_REVISION)
        .ok_or(DomainError::RevisionExhausted)?;
    Ok(next)
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

fn deserialize_optional_non_null<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[cfg(test)]
mod tests {
    use super::super::{DocumentCoordinate, DocumentTargetState};
    use super::*;
    use crate::ProjectProfile;

    fn actor() -> PublicKey {
        PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .expect("fixed test public key")
    }

    fn initialized_state(now: DateTime<Utc>) -> ProjectViewStateV3 {
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let profile = ProjectViewObjectV3 {
            id: *project_id.as_uuid(),
            object_type: ProjectViewObjectType::ProjectProfile,
            object_revision: 1,
            project_revision: 1,
            created_at: now,
            updated_at: now,
            created_by: actor(),
            updated_by: actor(),
            data: ProjectViewObjectDataV3::ProjectProfile(ProjectProfile {
                name: "Project".to_owned(),
                positioning: "Position".to_owned(),
                purpose: "Purpose".to_owned(),
                problem: "Problem".to_owned(),
                scope: "Scope".to_owned(),
            }),
            relations: ProjectViewRelations::default(),
            context_references: Vec::new(),
        };
        ProjectViewStateV3::from_snapshot(
            project_id,
            1,
            Some(now),
            Some(now),
            [ProjectViewEntryV3::Active(Box::new(profile))],
            [],
        )
        .expect("valid state")
    }

    #[test]
    fn v2_resource_wire_is_rejected_by_v3_parser() {
        let content = serde_json::json!({
            "schema_version": 3,
            "expected_project_revision": 1,
            "request": {
                "type": "create",
                "object": {
                    "object_type": "resource",
                    "id": Uuid::new_v4(),
                    "name": "repository",
                    "resource_type": "repository",
                    "locator": {"locator_type":"url", "value":"https://example.com"},
                    "description": "legacy"
                }
            }
        })
        .to_string();
        assert!(ProjectObjectCommandV3::from_json(&content).is_err());
    }

    #[test]
    fn resource_create_requires_active_guide_proof() {
        let now = Utc::now();
        let state = initialized_state(now);
        let resource_id = Uuid::new_v4();
        let guide_id = Uuid::new_v4();
        let command = ProjectObjectCommandV3::new(
            1,
            None,
            ProjectObjectRequestV3::Create(CreateProjectObjectV3 {
                object: NewProjectViewObjectV3::Resource {
                    id: resource_id,
                    name: "Source".to_owned(),
                    resource_kind: "repository".to_owned(),
                    summary: None,
                    guide_document_id: guide_id,
                    context_references: Vec::new(),
                },
            }),
        );
        let capabilities = V3ReducerCapabilities::stage4(true);
        assert!(matches!(
            state.reduce(
                &command,
                actor(),
                now,
                capabilities,
                &ReferenceTargetProof::new(),
            ),
            Err(V3ProjectObjectError::Reference(
                V3ReferenceError::MissingDocumentProof { .. }
            ))
        ));
        let proof = ReferenceTargetProof::from_documents([(
            DocumentCoordinate::live(guide_id),
            DocumentTargetState::CurrentActive {
                current_revision: 1,
            },
        )])
        .expect("valid proof");
        let (next, outcome) = state
            .reduce(&command, actor(), now, capabilities, &proof)
            .expect("create Resource");
        assert_eq!(outcome.project_revision, 2);
        assert!(matches!(
            next.entry(resource_id),
            Some(ProjectViewEntryV3::Active(object))
                if matches!(object.data, ProjectViewObjectDataV3::Resource(_))
        ));
    }

    #[test]
    fn context_flag_off_rejects_addition_but_allows_removal() {
        let now = Utc::now();
        let mut state = initialized_state(now);
        let resource_id = Uuid::new_v4();
        let guide_id = Uuid::new_v4();
        let resource = ProjectViewObjectV3 {
            id: resource_id,
            object_type: ProjectViewObjectType::Resource,
            object_revision: 1,
            project_revision: 1,
            created_at: now,
            updated_at: now,
            created_by: actor(),
            updated_by: actor(),
            data: ProjectViewObjectDataV3::Resource(ProjectResourceV3 {
                name: "Resource".to_owned(),
                resource_kind: "service".to_owned(),
                summary: None,
                guide_document_id: guide_id,
            }),
            relations: ProjectViewRelations::default(),
            context_references: vec![ProjectContextReference::Document {
                document_id: Uuid::new_v4(),
                mode: super::super::DocumentReferenceMode::Pinned,
                document_revision: Some(1),
            }],
        };
        state
            .entries
            .insert(resource_id, ProjectViewEntryV3::Active(Box::new(resource)));
        let remove = ProjectObjectCommandV3::new(
            1,
            None,
            ProjectObjectRequestV3::Update(UpdateProjectObjectV3::Resource {
                object_id: resource_id,
                patch: ResourcePatchV3 {
                    context_references: Some(Vec::new()),
                    ..ResourcePatchV3::default()
                },
            }),
        );
        assert!(state
            .reduce(
                &remove,
                actor(),
                now,
                V3ReducerCapabilities::stage4(false),
                &ReferenceTargetProof::new(),
            )
            .is_ok());
    }
}
