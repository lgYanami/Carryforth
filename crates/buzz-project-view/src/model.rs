//! Canonical Project View domain objects and closed vocabularies.
//!
//! The types in this module contain no tenant supplied by a client and perform
//! no I/O. The relay binds a mutation to its server-resolved community before
//! constructing or changing these values.

use std::fmt;

use buzz_core::PublicKey;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! string_enum {
    (
        $(#[$enum_meta:meta])*
        pub enum $name:ident {
            $(
                $(#[$variant_meta:meta])*
                $variant:ident => $wire:literal
            ),+ $(,)?
        }
    ) => {
        $(#[$enum_meta])*
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            PartialOrd,
            Ord,
            Serialize,
            Deserialize,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $(
                $(#[$variant_meta])*
                $variant,
            )+
        }

        impl $name {
            /// Return the stable wire and database spelling.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire,)+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// The kind of object stored in a Project View.
    pub enum ProjectViewObjectType {
        /// The project's single descriptive profile.
        ProjectProfile => "project_profile",
        /// A desired project outcome.
        Goal => "goal",
        /// A stable semantic responsibility position.
        Role => "role",
        /// A body of planning logic.
        Plan => "plan",
        /// A stable segment within a plan.
        Stage => "stage",
        /// Something the project intends to satisfy.
        Requirement => "requirement",
        /// A discovered problem, gap, exception, or feedback item.
        Issue => "issue",
        /// A unit of execution handling one requirement or issue.
        Work => "work",
        /// A stable locator for a project-related asset.
        Resource => "resource",
    }
}

string_enum! {
    /// Explicit priority assigned to a requirement, issue, or work item.
    pub enum Priority {
        /// Work can be deferred behind normal-priority items.
        Low => "low",
        /// The ordinary default priority.
        Normal => "normal",
        /// Work should be handled ahead of normal-priority items.
        High => "high",
        /// Work needs immediate attention.
        Urgent => "urgent",
    }
}

string_enum! {
    /// Explicit lifecycle status of a project plan.
    pub enum PlanStatus {
        /// The plan is still being prepared.
        Draft => "draft",
        /// The plan is currently active.
        Active => "active",
        /// Progress on the plan is intentionally paused.
        Paused => "paused",
        /// The plan has been completed.
        Completed => "completed",
        /// The plan has been cancelled.
        Cancelled => "cancelled",
    }
}

string_enum! {
    /// Explicit lifecycle status of a stage.
    pub enum StageStatus {
        /// The stage is planned but not active.
        Planned => "planned",
        /// The stage is currently active.
        Active => "active",
        /// Progress on the stage is intentionally paused.
        Paused => "paused",
        /// The stage has been completed.
        Completed => "completed",
        /// The stage has been cancelled.
        Cancelled => "cancelled",
    }
}

string_enum! {
    /// Explicit lifecycle status of a requirement.
    pub enum RequirementStatus {
        /// The requirement has been proposed but is not ready for execution.
        Proposed => "proposed",
        /// The requirement is ready to be handled.
        Ready => "ready",
        /// Work on the requirement is in progress.
        InProgress => "in_progress",
        /// The requirement has been satisfied.
        Satisfied => "satisfied",
        /// The requirement has been withdrawn.
        Withdrawn => "withdrawn",
    }
}

string_enum! {
    /// Explicit lifecycle status of an issue.
    pub enum IssueStatus {
        /// The issue is open.
        Open => "open",
        /// Work on the issue is in progress.
        InProgress => "in_progress",
        /// The issue's underlying problem has been resolved.
        Resolved => "resolved",
        /// The issue is closed.
        Closed => "closed",
    }
}

string_enum! {
    /// Explicit lifecycle status of a work item.
    pub enum WorkStatus {
        /// The work has not started.
        Pending => "pending",
        /// The work is in progress.
        InProgress => "in_progress",
        /// Progress on the work is intentionally paused.
        Paused => "paused",
        /// The work has been submitted for review or acceptance.
        Submitted => "submitted",
        /// The work has been completed.
        Completed => "completed",
        /// The work has been cancelled.
        Cancelled => "cancelled",
    }
}

string_enum! {
    /// The semantic kind of a project resource.
    pub enum ResourceType {
        /// A source-code or artifact repository.
        Repository => "repository",
        /// A document.
        Document => "document",
        /// A design asset or design workspace.
        Design => "design",
        /// A running or deployable service.
        Service => "service",
        /// A development, test, staging, or production environment.
        Environment => "environment",
        /// A produced build, package, report, or other artifact.
        Artifact => "artifact",
        /// A generic URL resource.
        Url => "url",
    }
}

string_enum! {
    /// The syntax used by a [`ResourceLocator`].
    pub enum LocatorType {
        /// An HTTP or HTTPS URL.
        Url => "url",
        /// A Nostr address, such as a NIP-34 repository coordinate.
        NostrAddress => "nostr_address",
        /// A concrete Nostr event identifier or event reference.
        NostrEvent => "nostr_event",
        /// A `buzz://` deep link.
        BuzzDeepLink => "buzz_deep_link",
    }
}

/// A typed reference to one active Project View object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObjectRef {
    /// The referenced object's declared type.
    pub object_type: ProjectViewObjectType,
    /// The referenced object's stable project-local identifier.
    pub object_id: Uuid,
}

/// A typed, inert locator for a project resource.
///
/// A locator is descriptive data only. The domain layer never resolves or
/// fetches it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLocator {
    /// The syntax used by [`Self::value`].
    pub locator_type: LocatorType,
    /// The locator text in the syntax selected by [`Self::locator_type`].
    pub value: String,
}

/// The project's single editable description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectProfile {
    /// Human-readable project name.
    pub name: String,
    /// The project's intended position relative to its environment.
    pub positioning: String,
    /// Why the project exists.
    pub purpose: String,
    /// The problem the project addresses.
    pub problem: String,
    /// The project's declared boundary.
    pub scope: String,
    /// Optional retrieval summary owned by this Project Profile.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A desired outcome of the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    /// Human-readable goal title.
    pub title: String,
    /// The outcome that would make this goal meaningful.
    pub desired_outcome: String,
    /// Explicit directions that guide pursuit of the goal.
    pub directions: Vec<String>,
    /// Optional retrieval summary owned by this Goal.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A stable semantic responsibility position within the project.
///
/// This is not a Buzz membership or authorization role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRole {
    /// Human-readable role name.
    pub name: String,
    /// Why this responsibility position exists.
    pub purpose: String,
    /// Responsibilities belonging to this role.
    pub responsibilities: Vec<String>,
    /// Boundaries that constrain this role.
    pub boundaries: Vec<String>,
    /// Whether the semantic role is currently active.
    pub active: bool,
    /// Optional retrieval summary owned by this Role.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// Planning logic used to advance the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectPlan {
    /// Human-readable plan title.
    pub title: String,
    /// The plan's current logic and intended structure.
    pub description: String,
    /// Explicit plan status.
    pub status: PlanStatus,
    /// Optional retrieval summary owned by this Plan.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A stable segment within one project plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectStage {
    /// Human-readable stage title.
    pub title: String,
    /// What this stage represents.
    pub description: String,
    /// Explicit stage status.
    pub status: StageStatus,
    /// Optional retrieval summary owned by this Stage.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// Something the project intends to implement, change, or satisfy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requirement {
    /// Human-readable requirement title.
    pub title: String,
    /// Detailed requirement description.
    pub description: String,
    /// Explicit requirement status.
    pub status: RequirementStatus,
    /// Explicit requirement priority.
    pub priority: Priority,
    /// Optional retrieval summary owned by this Requirement.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A discovered project problem, gap, exception, or feedback item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectIssue {
    /// Human-readable issue title.
    pub title: String,
    /// Detailed issue description.
    pub description: String,
    /// Explicit issue status.
    pub status: IssueStatus,
    /// Explicit issue priority.
    pub priority: Priority,
    /// Optional retrieval summary owned by this Issue.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A unit of execution that handles exactly one requirement or issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectWork {
    /// Human-readable work title.
    pub title: String,
    /// Detailed work description.
    pub description: String,
    /// Explicit work status.
    pub status: WorkStatus,
    /// Explicit work priority.
    pub priority: Priority,
    /// Optional retrieval summary owned by this Work.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::serde_helpers::deserialize_optional_non_null"
    )]
    pub summary: Option<String>,
}

/// A stable entry point to a project-related asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectResource {
    /// Human-readable resource name.
    pub name: String,
    /// The resource's semantic kind.
    pub resource_type: ResourceType,
    /// The typed, inert resource locator.
    pub locator: ResourceLocator,
    /// Human-readable resource description.
    pub description: String,
}

/// The body of one Project View object.
///
/// This enum is explicitly tagged because several object bodies have
/// overlapping fields and overlapping status spellings. Untagged
/// deserialization would therefore be ambiguous.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "object_type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProjectViewObjectData {
    /// Project profile data.
    ProjectProfile(ProjectProfile),
    /// Goal data.
    Goal(Goal),
    /// Project role data.
    Role(ProjectRole),
    /// Project plan data.
    Plan(ProjectPlan),
    /// Project stage data.
    Stage(ProjectStage),
    /// Requirement data.
    Requirement(Requirement),
    /// Project issue data.
    Issue(ProjectIssue),
    /// Project work data.
    Work(ProjectWork),
    /// Project resource data.
    Resource(ProjectResource),
}

impl ProjectViewObjectData {
    /// Return the object type carried by this data variant.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::ProjectProfile(_) => ProjectViewObjectType::ProjectProfile,
            Self::Goal(_) => ProjectViewObjectType::Goal,
            Self::Role(_) => ProjectViewObjectType::Role,
            Self::Plan(_) => ProjectViewObjectType::Plan,
            Self::Stage(_) => ProjectViewObjectType::Stage,
            Self::Requirement(_) => ProjectViewObjectType::Requirement,
            Self::Issue(_) => ProjectViewObjectType::Issue,
            Self::Work(_) => ProjectViewObjectType::Work,
            Self::Resource(_) => ProjectViewObjectType::Resource,
        }
    }
}

/// All fixed relation slots available to a Project View object.
///
/// Validation determines which slots are allowed or required for each source
/// object type. Keeping the slots in one structure mirrors the canonical
/// database row and prevents relations from being hidden in free-form JSON.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewRelations {
    /// Optional Goal containing a Plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under_goal_id: Option<Uuid>,
    /// Required Plan containing a Stage; absent for every other object type.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under_plan_id: Option<Uuid>,
    /// Optional Stage planning a Requirement or Issue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_in_stage_id: Option<Uuid>,
    /// Optional object about which an Issue was raised.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about: Option<ObjectRef>,
    /// Required Requirement-or-Issue handled by a Work item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handles: Option<ObjectRef>,
}

impl ProjectViewRelations {
    /// Return whether every relation slot is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.under_goal_id.is_none()
            && self.under_plan_id.is_none()
            && self.planned_in_stage_id.is_none()
            && self.about.is_none()
            && self.handles.is_none()
    }
}

/// One active canonical Project View object.
///
/// `object_type` is repeated beside the strongly typed `data` so database and
/// projection code can index it without inspecting the body. Validation must
/// require it to equal [`ProjectViewObjectData::object_type`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewObject {
    /// Stable identifier, unique across all object types in this project.
    pub id: Uuid,
    /// Declared object type.
    pub object_type: ProjectViewObjectType,
    /// Revision of this object, starting at one.
    pub object_revision: u64,
    /// Project revision at which this object was last changed.
    pub project_revision: u64,
    /// Canonical creation time supplied by the relay.
    pub created_at: DateTime<Utc>,
    /// Canonical last-update time supplied by the relay.
    pub updated_at: DateTime<Utc>,
    /// Verified actor that created the object.
    pub created_by: PublicKey,
    /// Verified actor that most recently changed the object.
    pub updated_by: PublicKey,
    /// Strongly typed object body.
    pub data: ProjectViewObjectData,
    /// Fixed relationship slots for the object.
    pub relations: ProjectViewRelations,
}
