//! Closed, typed mutation commands for Project View.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{
    DomainError, DomainResult, Goal, IssueStatus, ObjectRef, PlanStatus, Priority, ProjectIssue,
    ProjectPlan, ProjectProfile, ProjectResource, ProjectRole, ProjectStage, ProjectViewObjectData,
    ProjectViewObjectType, ProjectViewRelations, ProjectWork, Requirement, RequirementStatus,
    ResourceLocator, ResourceType, StageStatus, WorkStatus,
};
use crate::{Patch, ProjectViewEntry};

/// Wire schema version implemented by this crate.
pub const MUTATION_SCHEMA_VERSION: u16 = 1;
/// Maximum UTF-8 byte length of one mutation event's JSON content.
pub const MAX_MUTATION_CONTENT_BYTES: usize = 64 * 1024;
/// Maximum nesting depth of one mutation JSON value.
pub const MAX_MUTATION_JSON_DEPTH: usize = 16;

/// A revision-checked Project View mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    /// Major mutation schema version.
    pub schema_version: u16,
    /// Project revision that the caller observed before constructing the
    /// mutation.
    pub expected_project_revision: u64,
    /// Requested domain operation.
    pub request: MutationRequest,
}

impl Mutation {
    /// Creates a v1 mutation for an observed project revision.
    pub const fn new(expected_project_revision: u64, request: MutationRequest) -> Self {
        Self {
            schema_version: MUTATION_SCHEMA_VERSION,
            expected_project_revision,
            request,
        }
    }

    /// Parses a mutation from JSON while enforcing Project View's tighter
    /// content-size, depth, and closed-schema limits.
    pub fn from_json(content: &str) -> DomainResult<Self> {
        if content.len() > MAX_MUTATION_CONTENT_BYTES {
            return Err(DomainError::MutationContentTooLarge {
                max: MAX_MUTATION_CONTENT_BYTES,
                actual: content.len(),
            });
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
            });
        }
        let mutation: Self =
            serde_json::from_value(value).map_err(|error| DomainError::InvalidMutationJson {
                reason: error.to_string(),
            })?;
        if mutation.schema_version != MUTATION_SCHEMA_VERSION {
            return Err(DomainError::UnsupportedSchemaVersion {
                got: u32::from(mutation.schema_version),
                supported: u32::from(MUTATION_SCHEMA_VERSION),
            });
        }
        crate::validation::validate_revision(mutation.expected_project_revision)?;
        Ok(mutation)
    }

    /// Validates mutation input that does not depend on current server state.
    ///
    /// Typed SDK builders use this to reject malformed fields before signing.
    /// The Relay repeats these checks and additionally validates the current
    /// revision, object existence, relation targets, and aggregate invariants.
    pub fn validate_for_submission(&self) -> DomainResult<()> {
        if self.schema_version != MUTATION_SCHEMA_VERSION {
            return Err(DomainError::UnsupportedSchemaVersion {
                got: u32::from(self.schema_version),
                supported: u32::from(MUTATION_SCHEMA_VERSION),
            });
        }
        crate::validation::validate_revision(self.expected_project_revision)?;
        crate::validation::validate_mutation_input(&self.request)
    }
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 1,
    }
}

/// One of the four Project View operations supported by schema v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum MutationRequest {
    /// Atomically create the profile and initial goals.
    Initialize(InitializeMutation),
    /// Create one non-profile object.
    Create(CreateMutation),
    /// Patch one existing active object.
    Update(UpdateMutation),
    /// Tombstone one existing active object.
    Delete(DeleteMutation),
}

/// Initialization payload for an uninitialized Project View.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeMutation {
    /// The unique Project Profile body.
    pub profile: ProjectProfile,
    /// The initial goals, including their client-generated IDs.
    pub goals: Vec<InitializeGoal>,
}

/// A goal created as part of initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitializeGoal {
    /// Client-generated UUID v4 for the goal.
    pub id: Uuid,
    /// Goal title.
    pub title: String,
    /// Observable outcome that would satisfy the goal.
    pub desired_outcome: String,
    /// Strategic directions that guide work toward the goal.
    pub directions: Vec<String>,
}

impl InitializeGoal {
    /// Converts this initialization item into the canonical goal body.
    pub fn into_goal(self) -> Goal {
        Goal {
            title: self.title,
            desired_outcome: self.desired_outcome,
            directions: self.directions,
        }
    }
}

/// Create payload for one Project View object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateMutation {
    /// Complete typed object creation payload.
    pub object: NewProjectViewObject,
}

/// Typed creation payloads for every object except Project Profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "object_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NewProjectViewObject {
    /// A goal.
    Goal {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Goal title.
        title: String,
        /// Observable desired outcome.
        desired_outcome: String,
        /// Strategic directions.
        directions: Vec<String>,
    },
    /// A project role.
    Role {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Role name.
        name: String,
        /// Role purpose.
        purpose: String,
        /// Responsibilities owned by the role.
        responsibilities: Vec<String>,
        /// Explicit boundaries of the role.
        boundaries: Vec<String>,
        /// Whether the role is currently active.
        active: bool,
    },
    /// A project plan.
    Plan {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Plan title.
        title: String,
        /// Plan description.
        description: String,
        /// Plan status.
        status: PlanStatus,
        /// Optional goal that owns the plan.
        under_goal_id: Option<Uuid>,
    },
    /// An unordered project stage.
    Stage {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Stage title.
        title: String,
        /// Stage description.
        description: String,
        /// Stage status.
        status: StageStatus,
        /// Required parent plan.
        under_plan_id: Uuid,
    },
    /// A requirement.
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
        /// Optional planning stage.
        planned_in_stage_id: Option<Uuid>,
    },
    /// An issue.
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
        /// Optional planning stage.
        planned_in_stage_id: Option<Uuid>,
        /// Optional object that the issue is about.
        about: Option<ObjectRef>,
    },
    /// An actionable work item.
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
        /// Requirement or issue handled by this work item.
        handles: ObjectRef,
    },
    /// A stable project resource locator.
    Resource {
        /// Client-generated UUID v4.
        id: Uuid,
        /// Resource name.
        name: String,
        /// Resource category.
        resource_type: ResourceType,
        /// Strongly typed resource locator.
        locator: ResourceLocator,
        /// Resource description.
        description: String,
    },
}

impl NewProjectViewObject {
    /// Returns the client-generated object ID.
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

    /// Returns the canonical object type.
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

    /// Splits the create payload into its canonical body and relations.
    pub fn into_parts(self) -> (Uuid, ProjectViewObjectData, ProjectViewRelations) {
        match self {
            Self::Goal {
                id,
                title,
                desired_outcome,
                directions,
            } => (
                id,
                ProjectViewObjectData::Goal(Goal {
                    title,
                    desired_outcome,
                    directions,
                }),
                ProjectViewRelations::default(),
            ),
            Self::Role {
                id,
                name,
                purpose,
                responsibilities,
                boundaries,
                active,
            } => (
                id,
                ProjectViewObjectData::Role(ProjectRole {
                    name,
                    purpose,
                    responsibilities,
                    boundaries,
                    active,
                }),
                ProjectViewRelations::default(),
            ),
            Self::Plan {
                id,
                title,
                description,
                status,
                under_goal_id,
            } => (
                id,
                ProjectViewObjectData::Plan(ProjectPlan {
                    title,
                    description,
                    status,
                }),
                ProjectViewRelations {
                    under_goal_id,
                    ..ProjectViewRelations::default()
                },
            ),
            Self::Stage {
                id,
                title,
                description,
                status,
                under_plan_id,
            } => (
                id,
                ProjectViewObjectData::Stage(ProjectStage {
                    title,
                    description,
                    status,
                }),
                ProjectViewRelations {
                    under_plan_id: Some(under_plan_id),
                    ..ProjectViewRelations::default()
                },
            ),
            Self::Requirement {
                id,
                title,
                description,
                status,
                priority,
                planned_in_stage_id,
            } => (
                id,
                ProjectViewObjectData::Requirement(Requirement {
                    title,
                    description,
                    status,
                    priority,
                }),
                ProjectViewRelations {
                    planned_in_stage_id,
                    ..ProjectViewRelations::default()
                },
            ),
            Self::Issue {
                id,
                title,
                description,
                status,
                priority,
                planned_in_stage_id,
                about,
            } => (
                id,
                ProjectViewObjectData::Issue(ProjectIssue {
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
            ),
            Self::Work {
                id,
                title,
                description,
                status,
                priority,
                handles,
            } => (
                id,
                ProjectViewObjectData::Work(ProjectWork {
                    title,
                    description,
                    status,
                    priority,
                }),
                ProjectViewRelations {
                    handles: Some(handles),
                    ..ProjectViewRelations::default()
                },
            ),
            Self::Resource {
                id,
                name,
                resource_type,
                locator,
                description,
            } => (
                id,
                ProjectViewObjectData::Resource(ProjectResource {
                    name,
                    resource_type,
                    locator,
                    description,
                }),
                ProjectViewRelations::default(),
            ),
        }
    }
}

/// A typed update of one active Project View object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "object_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateMutation {
    /// Patch the unique Project Profile.
    ProjectProfile {
        /// Profile object ID.
        object_id: Uuid,
        /// Typed profile patch.
        patch: ProfilePatch,
    },
    /// Patch a goal.
    Goal {
        /// Goal object ID.
        object_id: Uuid,
        /// Typed goal patch.
        patch: GoalPatch,
    },
    /// Patch a role.
    Role {
        /// Role object ID.
        object_id: Uuid,
        /// Typed role patch.
        patch: RolePatch,
    },
    /// Patch a plan.
    Plan {
        /// Plan object ID.
        object_id: Uuid,
        /// Typed plan patch.
        patch: PlanPatch,
    },
    /// Patch a stage.
    Stage {
        /// Stage object ID.
        object_id: Uuid,
        /// Typed stage patch.
        patch: StagePatch,
    },
    /// Patch a requirement.
    Requirement {
        /// Requirement object ID.
        object_id: Uuid,
        /// Typed requirement patch.
        patch: RequirementPatch,
    },
    /// Patch an issue.
    Issue {
        /// Issue object ID.
        object_id: Uuid,
        /// Typed issue patch.
        patch: IssuePatch,
    },
    /// Patch a work item.
    Work {
        /// Work object ID.
        object_id: Uuid,
        /// Typed work patch.
        patch: WorkPatch,
    },
    /// Patch a resource.
    Resource {
        /// Resource object ID.
        object_id: Uuid,
        /// Typed resource patch.
        patch: ResourcePatch,
    },
}

impl UpdateMutation {
    /// Returns the target object ID.
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

    /// Returns the expected type of the target object.
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
}

macro_rules! patch_struct {
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
        }
    };
}

patch_struct! {
    /// Typed Project Profile patch.
    ProfilePatch {
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

patch_struct! {
    /// Typed goal patch.
    GoalPatch {
        /// Goal title.
        title: String,
        /// Observable desired outcome.
        desired_outcome: String,
        /// Strategic directions.
        directions: Vec<String>,
    }
}

patch_struct! {
    /// Typed role patch.
    RolePatch {
        /// Role name.
        name: String,
        /// Role purpose.
        purpose: String,
        /// Responsibilities owned by the role.
        responsibilities: Vec<String>,
        /// Explicit role boundaries.
        boundaries: Vec<String>,
        /// Whether the role is active.
        active: bool,
    }
}

patch_struct! {
    /// Typed plan patch.
    PlanPatch {
        /// Plan title.
        title: String,
        /// Plan description.
        description: String,
        /// Plan status.
        status: PlanStatus,
        /// Optional owning goal. `null` unbinds the plan.
        under_goal_id: Uuid,
    }
}

patch_struct! {
    /// Typed stage patch.
    StagePatch {
        /// Stage title.
        title: String,
        /// Stage description.
        description: String,
        /// Stage status.
        status: StageStatus,
        /// Required parent plan. It may be replaced but not cleared.
        under_plan_id: Uuid,
    }
}

patch_struct! {
    /// Typed requirement patch.
    RequirementPatch {
        /// Requirement title.
        title: String,
        /// Requirement description.
        description: String,
        /// Requirement status.
        status: RequirementStatus,
        /// Requirement priority.
        priority: Priority,
        /// Optional planning stage. `null` makes the requirement unplanned.
        planned_in_stage_id: Uuid,
    }
}

patch_struct! {
    /// Typed issue patch.
    IssuePatch {
        /// Issue title.
        title: String,
        /// Issue description.
        description: String,
        /// Issue status.
        status: IssueStatus,
        /// Issue priority.
        priority: Priority,
        /// Optional planning stage. `null` makes the issue unplanned.
        planned_in_stage_id: Uuid,
        /// Optional subject. `null` removes the reverse reference.
        about: ObjectRef,
    }
}

patch_struct! {
    /// Typed work patch.
    WorkPatch {
        /// Work title.
        title: String,
        /// Work description.
        description: String,
        /// Work status.
        status: WorkStatus,
        /// Work priority.
        priority: Priority,
        /// Required handled object. It may be replaced but not cleared.
        handles: ObjectRef,
    }
}

patch_struct! {
    /// Typed resource patch.
    ResourcePatch {
        /// Resource name.
        name: String,
        /// Resource category.
        resource_type: ResourceType,
        /// Strongly typed resource locator.
        locator: ResourceLocator,
        /// Resource description.
        description: String,
    }
}

/// Delete payload for one active object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteMutation {
    /// Type expected by the caller.
    pub object_type: ProjectViewObjectType,
    /// Object to tombstone.
    pub object_id: Uuid,
}

/// Result metadata returned after a successful atomic mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationOutcome {
    /// New project revision.
    pub project_revision: u64,
    /// Canonical entries changed by the mutation.
    pub changed_entries: Vec<ProjectViewEntry>,
}
