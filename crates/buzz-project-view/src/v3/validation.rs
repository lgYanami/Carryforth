//! Project View v3 object, patch, relation, and aggregate validation.

use std::collections::HashSet;

use uuid::Uuid;

use super::{
    ProjectResourceV3, ProjectViewEntryV3, ProjectViewObjectDataV3, ProjectViewObjectV3,
    ProjectViewStateV3, UpdateProjectObjectV3, V3ContractError, V3ProjectObjectError,
};
use crate::{
    DomainError, GoalPatch, IssuePatch, MutationRequest, Patch, PlanPatch, ProfilePatch,
    ProjectViewObjectData, ProjectViewObjectType, RequirementPatch, RolePatch, StagePatch,
    UpdateMutation, WorkPatch, MAX_SAFE_REVISION,
};

/// Validate one v3 business body without consulting other objects.
pub(super) fn validate_object_data(
    data: &ProjectViewObjectDataV3,
) -> Result<(), V3ProjectObjectError> {
    match data {
        ProjectViewObjectDataV3::Resource(resource) => resource.validate().map_err(Into::into),
        _ => crate::validation::validate_data(&legacy_data(data)?).map_err(Into::into),
    }
}

/// Validate which structural relation slots an object type may carry.
pub(super) fn validate_relation_shape(
    object_type: ProjectViewObjectType,
    relations: &crate::ProjectViewRelations,
) -> Result<(), V3ProjectObjectError> {
    crate::validation::validate_relation_shape(object_type, relations).map_err(Into::into)
}

/// Validate a v3 update's local body patch rules.
pub(super) fn validate_update(update: &UpdateProjectObjectV3) -> Result<(), V3ProjectObjectError> {
    let context_changed = update.context_references().is_some();
    let summary_changed = !update.summary_patch().is_unchanged();
    if let Patch::Set(summary) = update.summary_patch() {
        crate::validation::validate_summary(&Some(summary.clone()))?;
    }
    let legacy_update = match update {
        UpdateProjectObjectV3::ProjectProfile { object_id, patch } => {
            UpdateMutation::ProjectProfile {
                object_id: *object_id,
                patch: ProfilePatch {
                    name: patch.name.clone(),
                    positioning: patch.positioning.clone(),
                    purpose: patch.purpose.clone(),
                    problem: patch.problem.clone(),
                    scope: patch.scope.clone(),
                },
            }
        }
        UpdateProjectObjectV3::Goal { object_id, patch } => UpdateMutation::Goal {
            object_id: *object_id,
            patch: GoalPatch {
                title: patch.title.clone(),
                desired_outcome: patch.desired_outcome.clone(),
                directions: patch.directions.clone(),
            },
        },
        UpdateProjectObjectV3::Role { object_id, patch } => UpdateMutation::Role {
            object_id: *object_id,
            patch: RolePatch {
                name: patch.name.clone(),
                purpose: patch.purpose.clone(),
                responsibilities: patch.responsibilities.clone(),
                boundaries: patch.boundaries.clone(),
                active: patch.active.clone(),
            },
        },
        UpdateProjectObjectV3::Plan { object_id, patch } => UpdateMutation::Plan {
            object_id: *object_id,
            patch: PlanPatch {
                title: patch.title.clone(),
                description: patch.description.clone(),
                status: patch.status.clone(),
                under_goal_id: patch.under_goal_id.clone(),
            },
        },
        UpdateProjectObjectV3::Stage { object_id, patch } => UpdateMutation::Stage {
            object_id: *object_id,
            patch: StagePatch {
                title: patch.title.clone(),
                description: patch.description.clone(),
                status: patch.status.clone(),
                under_plan_id: patch.under_plan_id.clone(),
            },
        },
        UpdateProjectObjectV3::Requirement { object_id, patch } => UpdateMutation::Requirement {
            object_id: *object_id,
            patch: RequirementPatch {
                title: patch.title.clone(),
                description: patch.description.clone(),
                status: patch.status.clone(),
                priority: patch.priority.clone(),
                planned_in_stage_id: patch.planned_in_stage_id.clone(),
            },
        },
        UpdateProjectObjectV3::Issue { object_id, patch } => UpdateMutation::Issue {
            object_id: *object_id,
            patch: IssuePatch {
                title: patch.title.clone(),
                description: patch.description.clone(),
                status: patch.status.clone(),
                priority: patch.priority.clone(),
                planned_in_stage_id: patch.planned_in_stage_id.clone(),
                about: patch.about.clone(),
            },
        },
        UpdateProjectObjectV3::Work { object_id, patch } => UpdateMutation::Work {
            object_id: *object_id,
            patch: WorkPatch {
                title: patch.title.clone(),
                description: patch.description.clone(),
                status: patch.status.clone(),
                priority: patch.priority.clone(),
                handles: patch.handles.clone(),
            },
        },
        UpdateProjectObjectV3::Resource { patch, .. } => {
            return validate_resource_patch(patch, context_changed);
        }
    };
    match crate::validation::validate_mutation_input(&MutationRequest::Update(legacy_update)) {
        Err(DomainError::NoChanges) if context_changed || summary_changed => Ok(()),
        result => result.map_err(Into::into),
    }
}

fn validate_resource_patch(
    patch: &super::ResourcePatchV3,
    context_changed: bool,
) -> Result<(), V3ProjectObjectError> {
    use crate::Patch;

    let mut changed = context_changed;
    let mut resource = ProjectResourceV3 {
        name: "resource".to_owned(),
        resource_kind: "resource".to_owned(),
        summary: None,
        guide_document_id: Uuid::from_bytes([0, 0, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 1]),
    };
    match &patch.name {
        Patch::Unchanged => {}
        Patch::Clear => return Err(DomainError::RequiredField { field: "name" }.into()),
        Patch::Set(value) => {
            changed = true;
            resource.name.clone_from(value);
        }
    }
    match &patch.resource_kind {
        Patch::Unchanged => {}
        Patch::Clear => {
            return Err(DomainError::RequiredField {
                field: "resource_kind",
            }
            .into());
        }
        Patch::Set(value) => {
            changed = true;
            resource.resource_kind.clone_from(value);
        }
    }
    match &patch.summary {
        Patch::Unchanged => {}
        Patch::Clear => changed = true,
        Patch::Set(value) => {
            changed = true;
            resource.summary = Some(value.clone());
        }
    }
    match &patch.guide_document_id {
        Patch::Unchanged => {}
        Patch::Clear => {
            return Err(DomainError::RequiredField {
                field: "guide_document_id",
            }
            .into());
        }
        Patch::Set(value) => {
            changed = true;
            resource.guide_document_id = *value;
        }
    }
    if !changed {
        return Err(DomainError::NoChanges.into());
    }
    resource.validate().map_err(Into::into)
}

/// Validate complete canonical v3 object state.
pub(super) fn validate_state(state: &ProjectViewStateV3) -> Result<(), V3ProjectObjectError> {
    crate::validation::validate_revision(state.project_revision())?;
    if !state.is_initialized() {
        if state.project_revision() == 0
            && state.entries().is_empty()
            && state.updated_at().is_none()
            && state.role_levels().is_empty()
        {
            return Ok(());
        }
        return Err(DomainError::InvalidFinalState {
            reason: "an uninitialized v3 state must have revision zero and no canonical rows"
                .to_owned(),
        }
        .into());
    }
    if state.project_revision() == 0 {
        return Err(DomainError::InvalidFinalState {
            reason: "an initialized v3 state must have a positive revision".to_owned(),
        }
        .into());
    }
    let Some(state_updated_at) = state.updated_at() else {
        return Err(DomainError::InvalidFinalState {
            reason: "an initialized v3 state must have an update time".to_owned(),
        }
        .into());
    };
    if state
        .initialized_at()
        .is_some_and(|initialized_at| initialized_at > state_updated_at)
    {
        return Err(DomainError::InvalidFinalState {
            reason: "Project initialization time cannot follow its update time".to_owned(),
        }
        .into());
    }

    let project_uuid = *state.project_id().as_uuid();
    let mut profile_count = 0usize;
    let mut role_ids = HashSet::new();
    for (entry_id, entry) in state.entries() {
        if *entry_id != entry.id() {
            return Err(DomainError::InvalidFinalState {
                reason: format!(
                    "entry map key {entry_id} does not match canonical object {}",
                    entry.id()
                ),
            }
            .into());
        }
        if entry.object_type() == ProjectViewObjectType::Role {
            role_ids.insert(entry.id());
        }
        match entry {
            ProjectViewEntryV3::Active(object) => {
                validate_object(object)?;
                if object.project_revision > state.project_revision() {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!("object {} has a future Project revision", object.id),
                    }
                    .into());
                }
                if object.updated_at > state_updated_at {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!("object {} was updated after aggregate state", object.id),
                    }
                    .into());
                }
                if object.object_type == ProjectViewObjectType::ProjectProfile {
                    profile_count += 1;
                    if object.id != project_uuid {
                        return Err(DomainError::InvalidFinalState {
                            reason: "Profile ID must equal the server-resolved Community ID"
                                .to_owned(),
                        }
                        .into());
                    }
                } else {
                    crate::validation::validate_client_object_id(project_uuid, object.id)?;
                }
            }
            ProjectViewEntryV3::Tombstone(tombstone) => {
                crate::validation::validate_revision(tombstone.object_revision)?;
                crate::validation::validate_revision(tombstone.project_revision)?;
                if tombstone.object_revision == 0
                    || tombstone.project_revision == 0
                    || tombstone.project_revision > state.project_revision()
                    || tombstone.deleted_at < tombstone.created_at
                    || tombstone.deleted_at > state_updated_at
                {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!("invalid v3 tombstone {} lifecycle", tombstone.id),
                    }
                    .into());
                }
                if tombstone.object_type == ProjectViewObjectType::ProjectProfile {
                    return Err(DomainError::InvalidFinalState {
                        reason: "the Project Profile cannot be tombstoned".to_owned(),
                    }
                    .into());
                }
                crate::validation::validate_client_object_id(project_uuid, tombstone.id)?;
            }
        }
    }
    if profile_count != 1 {
        return Err(DomainError::InvalidFinalState {
            reason: format!("expected exactly one active Profile, found {profile_count}"),
        }
        .into());
    }
    let level_ids = state.role_levels().keys().copied().collect::<HashSet<_>>();
    if role_ids != level_ids {
        return Err(V3ProjectObjectError::InvalidRoleLevels(
            "every Role identity, including tombstones, must have exactly one level".to_owned(),
        ));
    }
    state.validate_relation_targets_and_context()
}

pub(super) fn validate_object(object: &ProjectViewObjectV3) -> Result<(), V3ProjectObjectError> {
    if object.object_type != object.data.object_type() {
        return Err(DomainError::DataTypeMismatch {
            declared: object.object_type,
            actual: object.data.object_type(),
        }
        .into());
    }
    if object.object_revision == 0 || object.project_revision == 0 {
        return Err(DomainError::InvalidFinalState {
            reason: format!("active object {} must have positive revisions", object.id),
        }
        .into());
    }
    if object.object_revision > MAX_SAFE_REVISION || object.project_revision > MAX_SAFE_REVISION {
        return Err(DomainError::RevisionOutOfRange {
            revision: object.object_revision.max(object.project_revision),
            max: MAX_SAFE_REVISION,
        }
        .into());
    }
    if object.updated_at < object.created_at {
        return Err(DomainError::InvalidFinalState {
            reason: format!("object {} was updated before creation", object.id),
        }
        .into());
    }
    validate_object_data(&object.data)?;
    validate_relation_shape(object.object_type, &object.relations)?;
    let canonical = super::canonicalize_context_references(object.context_references.clone())?;
    if canonical != object.context_references {
        return Err(V3ContractError::InvalidContext(
            "Context References are not in canonical order".to_owned(),
        )
        .into());
    }
    Ok(())
}

/// Validate structural relation targets against active v3 objects.
pub(super) fn validate_relation_targets(
    state: &ProjectViewStateV3,
    object: &ProjectViewObjectV3,
) -> Result<(), V3ProjectObjectError> {
    if let Some(target_id) = object.relations.under_goal_id {
        validate_typed_target(
            state,
            "under_goal_id",
            target_id,
            ProjectViewObjectType::Goal,
        )?;
    }
    if let Some(target_id) = object.relations.under_plan_id {
        validate_typed_target(
            state,
            "under_plan_id",
            target_id,
            ProjectViewObjectType::Plan,
        )?;
    }
    if let Some(target_id) = object.relations.planned_in_stage_id {
        validate_typed_target(
            state,
            "planned_in_stage_id",
            target_id,
            ProjectViewObjectType::Stage,
        )?;
    }
    if let Some(target) = object.relations.about {
        if target.object_id == object.id {
            return Err(DomainError::SelfReference {
                relation: "about",
                object_id: object.id,
            }
            .into());
        }
        validate_declared_target(state, "about", target)?;
    }
    if let Some(target) = object.relations.handles {
        if !matches!(
            target.object_type,
            ProjectViewObjectType::Requirement | ProjectViewObjectType::Issue
        ) {
            return Err(DomainError::InvalidWorkTarget {
                actual: target.object_type,
            }
            .into());
        }
        validate_declared_target(state, "handles", target)?;
    }
    Ok(())
}

fn validate_typed_target(
    state: &ProjectViewStateV3,
    relation: &'static str,
    target_id: Uuid,
    expected: ProjectViewObjectType,
) -> Result<(), V3ProjectObjectError> {
    let actual = active_target(state, relation, target_id)?;
    if actual.object_type != expected {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id,
            declared: expected,
            actual: actual.object_type,
        }
        .into());
    }
    Ok(())
}

fn validate_declared_target(
    state: &ProjectViewStateV3,
    relation: &'static str,
    target: crate::ObjectRef,
) -> Result<(), V3ProjectObjectError> {
    let actual = active_target(state, relation, target.object_id)?;
    if actual.object_type != target.object_type {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id: target.object_id,
            declared: target.object_type,
            actual: actual.object_type,
        }
        .into());
    }
    Ok(())
}

fn active_target<'a>(
    state: &'a ProjectViewStateV3,
    relation: &'static str,
    target_id: Uuid,
) -> Result<&'a ProjectViewObjectV3, V3ProjectObjectError> {
    match state.entry(target_id) {
        Some(ProjectViewEntryV3::Active(object)) => Ok(object),
        Some(ProjectViewEntryV3::Tombstone(_)) => Err(DomainError::RelationTargetDeleted {
            relation,
            target_id,
        }
        .into()),
        None => Err(DomainError::RelationTargetNotFound {
            relation,
            target_id,
        }
        .into()),
    }
}

fn legacy_data(
    data: &ProjectViewObjectDataV3,
) -> Result<ProjectViewObjectData, V3ProjectObjectError> {
    Ok(match data {
        ProjectViewObjectDataV3::ProjectProfile(value) => {
            ProjectViewObjectData::ProjectProfile(value.clone())
        }
        ProjectViewObjectDataV3::Goal(value) => ProjectViewObjectData::Goal(value.clone()),
        ProjectViewObjectDataV3::Role(value) => ProjectViewObjectData::Role(value.clone()),
        ProjectViewObjectDataV3::Plan(value) => ProjectViewObjectData::Plan(value.clone()),
        ProjectViewObjectDataV3::Stage(value) => ProjectViewObjectData::Stage(value.clone()),
        ProjectViewObjectDataV3::Requirement(value) => {
            ProjectViewObjectData::Requirement(value.clone())
        }
        ProjectViewObjectDataV3::Issue(value) => ProjectViewObjectData::Issue(value.clone()),
        ProjectViewObjectDataV3::Work(value) => ProjectViewObjectData::Work(value.clone()),
        ProjectViewObjectDataV3::Resource(_) => {
            return Err(V3ProjectObjectError::Contract(
                V3ContractError::InvalidWire(
                    "legacy conversion cannot represent a v3 Resource".to_owned(),
                ),
            ));
        }
    })
}
