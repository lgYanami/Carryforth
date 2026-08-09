//! Deterministic Project View v3 projection planning.

use std::collections::HashSet;

use uuid::Uuid;

use super::{ProjectObjectOutcomeV3, ProjectViewEntryV3, RoleDefinitionV3, V3ProjectObjectError};
use crate::v2::RoleLevel;
use crate::ProjectViewObjectType;

/// One canonical changed head emitted for a v3 object transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectedHeadV3 {
    /// The single RoleDefinitionV3 entity head for a non-tombstoned Role.
    Role(RoleDefinitionV3),
    /// Ordinary active object or any object tombstone, including Role
    /// tombstones.
    Object(ProjectViewEntryV3),
}

impl ProjectedHeadV3 {
    /// Stable object identity represented by this head.
    #[must_use]
    pub const fn object_id(&self) -> Uuid {
        match self {
            Self::Role(role) => role.role_id,
            Self::Object(entry) => entry.id(),
        }
    }

    /// Current per-object revision.
    #[must_use]
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Role(role) => role.object_revision,
            Self::Object(entry) => entry.object_revision(),
        }
    }

    /// Whether this is the unified active Role entity head.
    #[must_use]
    pub const fn is_role_definition(&self) -> bool {
        matches!(self, Self::Role(_))
    }
}

/// Deterministic set of v3 changed heads for one Project revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionPlanV3 {
    /// Exact Project revision represented by the plan.
    pub project_revision: u64,
    /// One and only one head per changed object identity.
    pub heads: Vec<ProjectedHeadV3>,
}

impl ProjectionPlanV3 {
    /// Build a plan from a pure reducer outcome and exact Role levels.
    pub fn for_object_outcome(
        outcome: &ProjectObjectOutcomeV3,
        role_level: impl Fn(Uuid) -> Option<RoleLevel>,
    ) -> Result<Self, V3ProjectObjectError> {
        let mut heads = Vec::with_capacity(outcome.changed_entries.len());
        let mut seen = HashSet::with_capacity(outcome.changed_entries.len());
        for entry in &outcome.changed_entries {
            if !seen.insert(entry.id()) {
                return Err(V3ProjectObjectError::InvalidRoleLevels(format!(
                    "duplicate changed head for object {}",
                    entry.id()
                )));
            }
            let head = match entry {
                ProjectViewEntryV3::Active(object)
                    if object.object_type == ProjectViewObjectType::Role =>
                {
                    let level = role_level(object.id).ok_or_else(|| {
                        V3ProjectObjectError::InvalidRoleLevels(format!(
                            "active Role {} has no governance level",
                            object.id
                        ))
                    })?;
                    ProjectedHeadV3::Role(object.role_definition(level)?)
                }
                _ => ProjectedHeadV3::Object(entry.clone()),
            };
            heads.push(head);
        }
        heads.sort_by_key(ProjectedHeadV3::object_id);
        Ok(Self {
            project_revision: outcome.project_revision,
            heads,
        })
    }

    /// Validate that active Roles never receive a second ordinary head.
    pub fn validate_single_head_per_object(&self) -> Result<(), V3ProjectObjectError> {
        let mut seen = HashSet::with_capacity(self.heads.len());
        for head in &self.heads {
            if !seen.insert(head.object_id()) {
                return Err(V3ProjectObjectError::InvalidRoleLevels(format!(
                    "duplicate v3 projection head for {}",
                    head.object_id()
                )));
            }
            if matches!(
                head,
                ProjectedHeadV3::Object(ProjectViewEntryV3::Active(object))
                    if object.object_type == ProjectViewObjectType::Role
            ) {
                return Err(V3ProjectObjectError::InvalidRoleLevels(
                    "active Role must project only as RoleDefinitionV3".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use buzz_core::PublicKey;
    use chrono::Utc;

    use super::*;
    use crate::v3::{ProjectViewObjectDataV3, ProjectViewObjectV3, ProjectViewTombstoneV3};
    use crate::{ProjectRole, ProjectViewRelations};

    fn actor() -> PublicKey {
        PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798")
            .expect("fixed test key")
    }

    #[test]
    fn active_and_inactive_roles_have_one_entity_head_but_tombstones_use_object_head() {
        let now = Utc::now();
        for active in [true, false] {
            let role_id = Uuid::new_v4();
            let object = ProjectViewObjectV3 {
                id: role_id,
                object_type: ProjectViewObjectType::Role,
                object_revision: 2,
                project_revision: 4,
                created_at: now,
                updated_at: now,
                created_by: actor(),
                updated_by: actor(),
                data: ProjectViewObjectDataV3::Role(ProjectRole {
                    name: "Role".to_owned(),
                    purpose: "Own the work".to_owned(),
                    responsibilities: Vec::new(),
                    boundaries: Vec::new(),
                    active,
                    summary: None,
                }),
                relations: ProjectViewRelations::default(),
                context_references: Vec::new(),
            };
            let outcome = ProjectObjectOutcomeV3 {
                project_revision: 4,
                changed_entries: vec![ProjectViewEntryV3::Active(Box::new(object))],
                document_target_delta: Default::default(),
            };
            let plan = ProjectionPlanV3::for_object_outcome(&outcome, |_| Some(RoleLevel::Member))
                .expect("valid plan");
            assert_eq!(plan.heads.len(), 1);
            assert!(plan.heads[0].is_role_definition());
        }

        let role_id = Uuid::new_v4();
        let outcome = ProjectObjectOutcomeV3 {
            project_revision: 5,
            changed_entries: vec![ProjectViewEntryV3::Tombstone(ProjectViewTombstoneV3 {
                id: role_id,
                object_type: ProjectViewObjectType::Role,
                object_revision: 3,
                project_revision: 5,
                created_at: now,
                deleted_at: now,
                created_by: actor(),
                deleted_by: actor(),
            })],
            document_target_delta: Default::default(),
        };
        let plan = ProjectionPlanV3::for_object_outcome(&outcome, |_| Some(RoleLevel::Member))
            .expect("valid tombstone plan");
        assert!(!plan.heads[0].is_role_definition());
    }
}
