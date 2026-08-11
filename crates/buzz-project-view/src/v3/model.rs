//! Canonical Project View v3 object model.
//!
//! The public wire types in this module are deliberately independent from the
//! v1/v2 object envelope. Reusing the legacy enum would allow a v3 parser to
//! accept the locator-bearing Resource shape that v3 removes.

use buzz_core::PublicKey;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ProjectContextReference, ProjectResourceV3, RoleDefinitionV3, V3ContractError};
use crate::v2::RoleLevel;
use crate::{
    Goal, ProjectIssue, ProjectPlan, ProjectProfile, ProjectRole, ProjectStage,
    ProjectViewObjectType, ProjectViewRelations, ProjectWork, Requirement,
};

/// Complete business body of one active Project View v3 object.
///
/// Non-Resource bodies reuse their stable value types, but the tagged union is
/// new so the legacy Resource variant can never be accepted on the v3 wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "object_type",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ProjectViewObjectDataV3 {
    /// Project profile data.
    ProjectProfile(ProjectProfile),
    /// Goal data.
    Goal(Goal),
    /// Project Role business data. Governance level remains canonical
    /// continuity metadata rather than client-editable Role body data.
    Role(ProjectRole),
    /// Plan data.
    Plan(ProjectPlan),
    /// Stage data.
    Stage(ProjectStage),
    /// Requirement data.
    Requirement(Requirement),
    /// Issue data.
    Issue(ProjectIssue),
    /// Work data.
    Work(ProjectWork),
    /// Locator-free Resource v3 data with a mandatory Guide Document.
    Resource(ProjectResourceV3),
}

impl ProjectViewObjectDataV3 {
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

    /// Return the source-owned retrieval summary, when present.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        match self {
            Self::ProjectProfile(value) => value.summary.as_deref(),
            Self::Goal(value) => value.summary.as_deref(),
            Self::Role(value) => value.summary.as_deref(),
            Self::Plan(value) => value.summary.as_deref(),
            Self::Stage(value) => value.summary.as_deref(),
            Self::Requirement(value) => value.summary.as_deref(),
            Self::Issue(value) => value.summary.as_deref(),
            Self::Work(value) => value.summary.as_deref(),
            Self::Resource(value) => value.summary.as_deref(),
        }
    }

    /// Return the canonical human-readable title or name used by lightweight
    /// source previews and semantic extraction.
    #[must_use]
    pub fn title(&self) -> &str {
        match self {
            Self::ProjectProfile(value) => &value.name,
            Self::Goal(value) => &value.title,
            Self::Role(value) => &value.name,
            Self::Plan(value) => &value.title,
            Self::Stage(value) => &value.title,
            Self::Requirement(value) => &value.title,
            Self::Issue(value) => &value.title,
            Self::Work(value) => &value.title,
            Self::Resource(value) => &value.name,
        }
    }

    /// Return source-native lifecycle status metadata when this object type
    /// defines one. This value is for filtering and is not semantic text.
    #[must_use]
    pub fn source_status(&self) -> Option<&'static str> {
        match self {
            Self::Role(value) => Some(if value.active { "active" } else { "inactive" }),
            Self::Plan(value) => Some(value.status.as_str()),
            Self::Stage(value) => Some(value.status.as_str()),
            Self::Requirement(value) => Some(value.status.as_str()),
            Self::Issue(value) => Some(value.status.as_str()),
            Self::Work(value) => Some(value.status.as_str()),
            Self::ProjectProfile(_) | Self::Goal(_) | Self::Resource(_) => None,
        }
    }
}

/// One active canonical Project View v3 object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewObjectV3 {
    /// Stable object identifier.
    pub id: Uuid,
    /// Immutable declared object type.
    pub object_type: ProjectViewObjectType,
    /// Object-local revision, starting at one.
    pub object_revision: u64,
    /// Project revision at which this object last changed.
    pub project_revision: u64,
    /// Canonical creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical last-change time.
    pub updated_at: DateTime<Utc>,
    /// Verified creator.
    pub created_by: PublicKey,
    /// Verified latest business editor.
    pub updated_by: PublicKey,
    /// Complete typed v3 body.
    pub data: ProjectViewObjectDataV3,
    /// Existing structural Project View relations.
    pub relations: ProjectViewRelations,
    /// Canonically ordered Context Reference set.
    pub context_references: Vec<ProjectContextReference>,
}

impl ProjectViewObjectV3 {
    /// Build the single active RoleDefinitionV3 head for this object.
    pub fn role_definition(&self, level: RoleLevel) -> Result<RoleDefinitionV3, V3ContractError> {
        let ProjectViewObjectDataV3::Role(role) = &self.data else {
            return Err(V3ContractError::InvalidWire(
                "RoleDefinitionV3 requires a Role object".to_owned(),
            ));
        };
        let definition = RoleDefinitionV3 {
            role_id: self.id,
            name: role.name.clone(),
            purpose: role.purpose.clone(),
            responsibilities: role.responsibilities.clone(),
            boundaries: role.boundaries.clone(),
            level,
            active: role.active,
            summary: role.summary.clone(),
            context_references: self.context_references.clone(),
            object_revision: self.object_revision,
            project_revision: self.project_revision,
            created_at: self.created_at,
            updated_at: self.updated_at,
            created_by: self.created_by,
            updated_by: self.updated_by,
        };
        definition.validate()?;
        Ok(definition)
    }
}

/// Minimal canonical record retained after a v3 object is deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectViewTombstoneV3 {
    /// Stable object identifier.
    pub id: Uuid,
    /// Immutable object type.
    pub object_type: ProjectViewObjectType,
    /// Object-local deletion revision.
    pub object_revision: u64,
    /// Project revision assigned to deletion.
    pub project_revision: u64,
    /// Original creation time.
    pub created_at: DateTime<Utc>,
    /// Canonical deletion time.
    pub deleted_at: DateTime<Utc>,
    /// Original creator.
    pub created_by: PublicKey,
    /// Verified deleting actor.
    pub deleted_by: PublicKey,
}

/// One occupied Project View v3 object identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectViewEntryV3 {
    /// Complete active v3 object.
    Active(Box<ProjectViewObjectV3>),
    /// Bodyless tombstone; IDs are never reusable.
    Tombstone(ProjectViewTombstoneV3),
}

impl ProjectViewEntryV3 {
    /// Stable object identifier.
    #[must_use]
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Active(object) => object.id,
            Self::Tombstone(tombstone) => tombstone.id,
        }
    }

    /// Immutable object type.
    #[must_use]
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::Active(object) => object.object_type,
            Self::Tombstone(tombstone) => tombstone.object_type,
        }
    }

    /// Current object-local revision.
    #[must_use]
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.object_revision,
            Self::Tombstone(tombstone) => tombstone.object_revision,
        }
    }

    /// Project revision at which this entry last changed.
    #[must_use]
    pub const fn project_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.project_revision,
            Self::Tombstone(tombstone) => tombstone.project_revision,
        }
    }

    /// Canonical Context set, empty for tombstones.
    #[must_use]
    pub fn context_references(&self) -> &[ProjectContextReference] {
        match self {
            Self::Active(object) => &object.context_references,
            Self::Tombstone(_) => &[],
        }
    }
}
