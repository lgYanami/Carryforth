//! Field, relation-shape, and whole-state validation.

use std::collections::HashSet;

use url::Url;
use uuid::{Uuid, Variant};

use crate::{
    DomainError, DomainResult, LocatorType, MutationRequest, Patch, ProjectViewEntry,
    ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType, ProjectViewState,
    ResourceLocator, UpdateMutation, MAX_INITIAL_GOALS, MAX_SAFE_REVISION,
};

const SHORT_TEXT_MAX_BYTES: usize = 256;
const LONG_TEXT_MAX_BYTES: usize = 32 * 1024;
const LIST_MAX_ITEMS: usize = 64;
const LIST_ITEM_MAX_BYTES: usize = 512;
const LOCATOR_MAX_BYTES: usize = 4_096;

pub(crate) fn validate_client_object_id(project_id: Uuid, object_id: Uuid) -> DomainResult<()> {
    if object_id == project_id {
        return Err(DomainError::ReservedProfileId { object_id });
    }
    if object_id.get_version_num() != 4 || object_id.get_variant() != Variant::RFC4122 {
        return Err(DomainError::InvalidObjectId { object_id });
    }
    Ok(())
}

pub(crate) fn validate_mutation_input(request: &MutationRequest) -> DomainResult<()> {
    match request {
        MutationRequest::Initialize(initialize) => {
            if !(1..=MAX_INITIAL_GOALS).contains(&initialize.goals.len()) {
                return Err(DomainError::InvalidInitialGoalCount {
                    min: 1,
                    max: MAX_INITIAL_GOALS,
                    actual: initialize.goals.len(),
                });
            }
            validate_data(&ProjectViewObjectData::ProjectProfile(
                initialize.profile.clone(),
            ))?;
            let mut ids = HashSet::with_capacity(initialize.goals.len());
            for goal in &initialize.goals {
                validate_client_generated_id(goal.id)?;
                if !ids.insert(goal.id) {
                    return Err(DomainError::ObjectIdAlreadyUsed { object_id: goal.id });
                }
                validate_data(&ProjectViewObjectData::Goal(goal.clone().into_goal()))?;
            }
            Ok(())
        }
        MutationRequest::Create(create) => {
            validate_client_generated_id(create.object.id())?;
            let (object_id, data, relations) = create.object.clone().into_parts();
            validate_data(&data)?;
            validate_relation_shape(data.object_type(), &relations)?;
            match &data {
                ProjectViewObjectData::Issue(_)
                    if relations
                        .about
                        .is_some_and(|reference| reference.object_id == object_id) =>
                {
                    return Err(DomainError::SelfReference {
                        relation: "about",
                        object_id,
                    });
                }
                ProjectViewObjectData::Work(_) => {
                    if let Some(reference) = relations.handles {
                        validate_work_target(reference.object_type)?;
                    }
                }
                _ => {}
            }
            Ok(())
        }
        MutationRequest::Update(update) => validate_update_input(update),
        MutationRequest::Delete(delete) => {
            if delete.object_type == ProjectViewObjectType::ProjectProfile {
                return Err(DomainError::ProfileDeletionForbidden);
            }
            validate_client_generated_id(delete.object_id)
        }
    }
}

fn validate_client_generated_id(object_id: Uuid) -> DomainResult<()> {
    if object_id.get_version_num() != 4 || object_id.get_variant() != Variant::RFC4122 {
        return Err(DomainError::InvalidObjectId { object_id });
    }
    Ok(())
}

fn validate_update_input(update: &UpdateMutation) -> DomainResult<()> {
    if update.object_type() != ProjectViewObjectType::ProjectProfile {
        validate_client_generated_id(update.object_id())?;
    }

    let changed = match update {
        UpdateMutation::ProjectProfile { patch, .. } => {
            validate_short_patch(&patch.name, "name")?;
            validate_long_patch(&patch.positioning, "positioning")?;
            validate_long_patch(&patch.purpose, "purpose")?;
            validate_long_patch(&patch.problem, "problem")?;
            validate_long_patch(&patch.scope, "scope")?;
            !patch.name.is_unchanged()
                || !patch.positioning.is_unchanged()
                || !patch.purpose.is_unchanged()
                || !patch.problem.is_unchanged()
                || !patch.scope.is_unchanged()
        }
        UpdateMutation::Goal { patch, .. } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.desired_outcome, "desired_outcome")?;
            validate_list_patch(&patch.directions, "directions")?;
            !patch.title.is_unchanged()
                || !patch.desired_outcome.is_unchanged()
                || !patch.directions.is_unchanged()
        }
        UpdateMutation::Role { patch, .. } => {
            validate_short_patch(&patch.name, "name")?;
            validate_long_patch(&patch.purpose, "purpose")?;
            validate_list_patch(&patch.responsibilities, "responsibilities")?;
            validate_list_patch(&patch.boundaries, "boundaries")?;
            validate_required_patch(&patch.active, "active")?;
            !patch.name.is_unchanged()
                || !patch.purpose.is_unchanged()
                || !patch.responsibilities.is_unchanged()
                || !patch.boundaries.is_unchanged()
                || !patch.active.is_unchanged()
        }
        UpdateMutation::Plan { patch, .. } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.description, "description")?;
            validate_required_patch(&patch.status, "status")?;
            !patch.title.is_unchanged()
                || !patch.description.is_unchanged()
                || !patch.status.is_unchanged()
                || !patch.under_goal_id.is_unchanged()
        }
        UpdateMutation::Stage { patch, .. } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.description, "description")?;
            validate_required_patch(&patch.status, "status")?;
            validate_required_relation_patch(&patch.under_plan_id, "under_plan_id")?;
            !patch.title.is_unchanged()
                || !patch.description.is_unchanged()
                || !patch.status.is_unchanged()
                || !patch.under_plan_id.is_unchanged()
        }
        UpdateMutation::Requirement { patch, .. } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.description, "description")?;
            validate_required_patch(&patch.status, "status")?;
            validate_required_patch(&patch.priority, "priority")?;
            !patch.title.is_unchanged()
                || !patch.description.is_unchanged()
                || !patch.status.is_unchanged()
                || !patch.priority.is_unchanged()
                || !patch.planned_in_stage_id.is_unchanged()
        }
        UpdateMutation::Issue {
            object_id, patch, ..
        } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.description, "description")?;
            validate_required_patch(&patch.status, "status")?;
            validate_required_patch(&patch.priority, "priority")?;
            if let Patch::Set(reference) = patch.about {
                if reference.object_id == *object_id {
                    return Err(DomainError::SelfReference {
                        relation: "about",
                        object_id: *object_id,
                    });
                }
            }
            !patch.title.is_unchanged()
                || !patch.description.is_unchanged()
                || !patch.status.is_unchanged()
                || !patch.priority.is_unchanged()
                || !patch.planned_in_stage_id.is_unchanged()
                || !patch.about.is_unchanged()
        }
        UpdateMutation::Work { patch, .. } => {
            validate_short_patch(&patch.title, "title")?;
            validate_long_patch(&patch.description, "description")?;
            validate_required_patch(&patch.status, "status")?;
            validate_required_patch(&patch.priority, "priority")?;
            validate_required_relation_patch(&patch.handles, "handles")?;
            if let Patch::Set(reference) = patch.handles {
                validate_work_target(reference.object_type)?;
            }
            !patch.title.is_unchanged()
                || !patch.description.is_unchanged()
                || !patch.status.is_unchanged()
                || !patch.priority.is_unchanged()
                || !patch.handles.is_unchanged()
        }
        UpdateMutation::Resource { patch, .. } => {
            validate_short_patch(&patch.name, "name")?;
            validate_required_patch(&patch.resource_type, "resource_type")?;
            match &patch.locator {
                Patch::Clear => {
                    return Err(DomainError::RequiredField { field: "locator" });
                }
                Patch::Set(locator) => validate_locator(locator)?,
                Patch::Unchanged => {}
            }
            validate_long_patch(&patch.description, "description")?;
            !patch.name.is_unchanged()
                || !patch.resource_type.is_unchanged()
                || !patch.locator.is_unchanged()
                || !patch.description.is_unchanged()
        }
    };

    if changed {
        Ok(())
    } else {
        Err(DomainError::NoChanges)
    }
}

fn validate_short_patch(patch: &Patch<String>, field: &'static str) -> DomainResult<()> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::RequiredField { field }),
        Patch::Set(value) => validate_required_short(field, value),
    }
}

fn validate_long_patch(patch: &Patch<String>, field: &'static str) -> DomainResult<()> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::RequiredField { field }),
        Patch::Set(value) => validate_required_long(field, value),
    }
}

fn validate_list_patch(patch: &Patch<Vec<String>>, field: &'static str) -> DomainResult<()> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::RequiredField { field }),
        Patch::Set(values) => validate_string_list(field, values),
    }
}

fn validate_required_patch<T>(patch: &Patch<T>, field: &'static str) -> DomainResult<()> {
    if patch.is_clear() {
        Err(DomainError::RequiredField { field })
    } else {
        Ok(())
    }
}

fn validate_required_relation_patch<T>(
    patch: &Patch<T>,
    relation: &'static str,
) -> DomainResult<()> {
    if patch.is_clear() {
        Err(DomainError::MissingRequiredRelation { relation })
    } else {
        Ok(())
    }
}

fn validate_work_target(actual: ProjectViewObjectType) -> DomainResult<()> {
    if matches!(
        actual,
        ProjectViewObjectType::Requirement | ProjectViewObjectType::Issue
    ) {
        Ok(())
    } else {
        Err(DomainError::InvalidWorkTarget { actual })
    }
}

pub(crate) fn validate_object(object: &ProjectViewObject) -> DomainResult<()> {
    if object.object_type != object.data.object_type() {
        return Err(DomainError::DataTypeMismatch {
            declared: object.object_type,
            actual: object.data.object_type(),
        });
    }
    validate_revision(object.object_revision)?;
    validate_revision(object.project_revision)?;
    if object.object_revision == 0 || object.project_revision == 0 {
        return Err(DomainError::InvalidFinalState {
            reason: format!(
                "active object {} must have positive object and project revisions",
                object.id
            ),
        });
    }
    if object.updated_at < object.created_at {
        return Err(DomainError::InvalidFinalState {
            reason: format!(
                "active object {} was updated before it was created",
                object.id
            ),
        });
    }
    validate_data(&object.data)?;
    validate_relation_shape(object.object_type, &object.relations)
}

pub(crate) fn validate_revision(revision: u64) -> DomainResult<()> {
    if revision > MAX_SAFE_REVISION {
        return Err(DomainError::RevisionOutOfRange {
            revision,
            max: MAX_SAFE_REVISION,
        });
    }
    Ok(())
}

pub(crate) fn validate_data(data: &ProjectViewObjectData) -> DomainResult<()> {
    match data {
        ProjectViewObjectData::ProjectProfile(profile) => {
            validate_required_short("name", &profile.name)?;
            validate_required_long("positioning", &profile.positioning)?;
            validate_required_long("purpose", &profile.purpose)?;
            validate_required_long("problem", &profile.problem)?;
            validate_required_long("scope", &profile.scope)
        }
        ProjectViewObjectData::Goal(goal) => {
            validate_required_short("title", &goal.title)?;
            validate_required_long("desired_outcome", &goal.desired_outcome)?;
            validate_string_list("directions", &goal.directions)
        }
        ProjectViewObjectData::Role(role) => {
            validate_required_short("name", &role.name)?;
            validate_required_long("purpose", &role.purpose)?;
            validate_string_list("responsibilities", &role.responsibilities)?;
            validate_string_list("boundaries", &role.boundaries)
        }
        ProjectViewObjectData::Plan(plan) => {
            validate_required_short("title", &plan.title)?;
            validate_required_long("description", &plan.description)
        }
        ProjectViewObjectData::Stage(stage) => {
            validate_required_short("title", &stage.title)?;
            validate_required_long("description", &stage.description)
        }
        ProjectViewObjectData::Requirement(requirement) => {
            validate_required_short("title", &requirement.title)?;
            validate_required_long("description", &requirement.description)
        }
        ProjectViewObjectData::Issue(issue) => {
            validate_required_short("title", &issue.title)?;
            validate_required_long("description", &issue.description)
        }
        ProjectViewObjectData::Work(work) => {
            validate_required_short("title", &work.title)?;
            validate_required_long("description", &work.description)
        }
        ProjectViewObjectData::Resource(resource) => {
            validate_required_short("name", &resource.name)?;
            validate_locator(&resource.locator)?;
            validate_required_long("description", &resource.description)
        }
    }
}

pub(crate) fn validate_state(state: &ProjectViewState) -> DomainResult<()> {
    validate_revision(state.project_revision())?;

    if !state.is_initialized() {
        if state.project_revision() == 0
            && state.entries().is_empty()
            && state.updated_at().is_none()
        {
            return Ok(());
        }
        return Err(DomainError::InvalidFinalState {
            reason:
                "an uninitialized state must have revision zero, no objects, and no update time"
                    .to_owned(),
        });
    }

    if state.project_revision() == 0 {
        return Err(DomainError::InvalidFinalState {
            reason: "an initialized state must have a positive revision".to_owned(),
        });
    }
    let Some(state_updated_at) = state.updated_at() else {
        return Err(DomainError::InvalidFinalState {
            reason: "an initialized state must have an update time".to_owned(),
        });
    };
    if state
        .initialized_at()
        .is_some_and(|initialized_at| initialized_at > state_updated_at)
    {
        return Err(DomainError::InvalidFinalState {
            reason: "project initialization time cannot be after its update time".to_owned(),
        });
    }

    let project_id = *state.project_id().as_uuid();
    let mut profile_count = 0usize;
    let mut goal_count = 0usize;

    for (entry_id, entry) in state.entries() {
        if *entry_id != entry.id() {
            return Err(DomainError::InvalidFinalState {
                reason: format!(
                    "entry map key {entry_id} does not match canonical object id {}",
                    entry.id()
                ),
            });
        }

        match entry {
            ProjectViewEntry::Active(object) => {
                validate_object(object)?;
                if object.project_revision > state.project_revision() {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "object {} was changed at future project revision {}",
                            object.id, object.project_revision
                        ),
                    });
                }
                if object.updated_at > state_updated_at {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "object {} was updated after the aggregate update time",
                            object.id
                        ),
                    });
                }
                match object.object_type {
                    ProjectViewObjectType::ProjectProfile => {
                        profile_count += 1;
                        if object.id != project_id {
                            return Err(DomainError::InvalidFinalState {
                                reason: "profile id must equal the server-resolved community id"
                                    .to_owned(),
                            });
                        }
                    }
                    ProjectViewObjectType::Goal => {
                        goal_count += 1;
                        validate_client_object_id(project_id, object.id)?;
                    }
                    _ => {
                        validate_client_object_id(project_id, object.id)?;
                    }
                }
            }
            ProjectViewEntry::Tombstone(tombstone) => {
                validate_revision(tombstone.object_revision)?;
                validate_revision(tombstone.project_revision)?;
                if tombstone.object_revision == 0 || tombstone.project_revision == 0 {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "tombstone {} must have positive object and project revisions",
                            tombstone.id
                        ),
                    });
                }
                if tombstone.deleted_at < tombstone.created_at {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "tombstone {} was deleted before it was created",
                            tombstone.id
                        ),
                    });
                }
                if tombstone.project_revision > state.project_revision() {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "tombstone {} was changed at future project revision {}",
                            tombstone.id, tombstone.project_revision
                        ),
                    });
                }
                if tombstone.deleted_at > state_updated_at {
                    return Err(DomainError::InvalidFinalState {
                        reason: format!(
                            "tombstone {} was deleted after the aggregate update time",
                            tombstone.id
                        ),
                    });
                }
                if tombstone.object_type == ProjectViewObjectType::ProjectProfile {
                    return Err(DomainError::InvalidFinalState {
                        reason: "the project profile cannot be tombstoned".to_owned(),
                    });
                }
                validate_client_object_id(project_id, tombstone.id)?;
            }
        }
    }

    if profile_count != 1 {
        return Err(DomainError::InvalidFinalState {
            reason: format!("expected exactly one active profile, found {profile_count}"),
        });
    }
    if goal_count == 0 {
        return Err(DomainError::InvalidFinalState {
            reason: "expected at least one active goal".to_owned(),
        });
    }

    for object in state.active_objects() {
        validate_relation_targets(state, object)?;
    }

    Ok(())
}

pub(crate) fn validate_relation_shape(
    object_type: ProjectViewObjectType,
    relations: &crate::ProjectViewRelations,
) -> DomainResult<()> {
    let allowed = match object_type {
        ProjectViewObjectType::ProjectProfile
        | ProjectViewObjectType::Goal
        | ProjectViewObjectType::Role
        | ProjectViewObjectType::Resource => &[][..],
        ProjectViewObjectType::Plan => &["under_goal_id"][..],
        ProjectViewObjectType::Stage => &["under_plan_id"][..],
        ProjectViewObjectType::Requirement => &["planned_in_stage_id"][..],
        ProjectViewObjectType::Issue => &["planned_in_stage_id", "about"][..],
        ProjectViewObjectType::Work => &["handles"][..],
    };

    for (name, present) in [
        ("under_goal_id", relations.under_goal_id.is_some()),
        ("under_plan_id", relations.under_plan_id.is_some()),
        (
            "planned_in_stage_id",
            relations.planned_in_stage_id.is_some(),
        ),
        ("about", relations.about.is_some()),
        ("handles", relations.handles.is_some()),
    ] {
        if present && !allowed.contains(&name) {
            return Err(DomainError::RelationNotAllowed {
                relation: name,
                object_type,
            });
        }
    }

    match object_type {
        ProjectViewObjectType::Stage if relations.under_plan_id.is_none() => {
            Err(DomainError::MissingRequiredRelation {
                relation: "under_plan_id",
            })
        }
        ProjectViewObjectType::Work if relations.handles.is_none() => {
            Err(DomainError::MissingRequiredRelation {
                relation: "handles",
            })
        }
        _ => Ok(()),
    }
}

fn validate_relation_targets(
    state: &ProjectViewState,
    object: &ProjectViewObject,
) -> DomainResult<()> {
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
            });
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
            });
        }
        validate_declared_target(state, "handles", target)?;
    }
    Ok(())
}

fn validate_typed_target(
    state: &ProjectViewState,
    relation: &'static str,
    target_id: Uuid,
    expected: ProjectViewObjectType,
) -> DomainResult<()> {
    let target = active_relation_target(state, relation, target_id)?;
    if target.object_type != expected {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id,
            declared: expected,
            actual: target.object_type,
        });
    }
    Ok(())
}

fn validate_declared_target(
    state: &ProjectViewState,
    relation: &'static str,
    target: crate::ObjectRef,
) -> DomainResult<()> {
    let actual = active_relation_target(state, relation, target.object_id)?;
    if actual.object_type != target.object_type {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id: target.object_id,
            declared: target.object_type,
            actual: actual.object_type,
        });
    }
    Ok(())
}

fn active_relation_target<'a>(
    state: &'a ProjectViewState,
    relation: &'static str,
    target_id: Uuid,
) -> DomainResult<&'a ProjectViewObject> {
    match state.entries().get(&target_id) {
        Some(ProjectViewEntry::Active(object)) => Ok(object),
        Some(ProjectViewEntry::Tombstone(_)) => Err(DomainError::RelationTargetDeleted {
            relation,
            target_id,
        }),
        None => Err(DomainError::RelationTargetNotFound {
            relation,
            target_id,
        }),
    }
}

fn validate_required_short(field: &'static str, value: &str) -> DomainResult<()> {
    validate_required_text(field, value, SHORT_TEXT_MAX_BYTES)
}

fn validate_required_long(field: &'static str, value: &str) -> DomainResult<()> {
    validate_required_text(field, value, LONG_TEXT_MAX_BYTES)
}

fn validate_required_text(field: &'static str, value: &str, max_bytes: usize) -> DomainResult<()> {
    if value.trim().is_empty() {
        return Err(DomainError::RequiredField { field });
    }
    if value.len() > max_bytes {
        return Err(DomainError::FieldTooLong {
            field,
            max: max_bytes,
            actual: value.len(),
        });
    }
    Ok(())
}

fn validate_string_list(field: &'static str, values: &[String]) -> DomainResult<()> {
    if values.len() > LIST_MAX_ITEMS {
        return Err(DomainError::TooManyItems {
            field,
            max: LIST_MAX_ITEMS,
            actual: values.len(),
        });
    }
    for value in values {
        validate_required_text(field, value, LIST_ITEM_MAX_BYTES)?;
    }
    Ok(())
}

fn validate_locator(locator: &ResourceLocator) -> DomainResult<()> {
    if locator.value.trim().is_empty() {
        return Err(DomainError::RequiredField { field: "locator" });
    }
    if locator.value.len() > LOCATOR_MAX_BYTES {
        return Err(DomainError::FieldTooLong {
            field: "locator",
            max: LOCATOR_MAX_BYTES,
            actual: locator.value.len(),
        });
    }
    if locator.value.chars().any(char::is_control) {
        return Err(DomainError::InvalidLocator {
            reason: "control characters are not allowed".to_owned(),
        });
    }

    if locator.locator_type == LocatorType::Url {
        let parsed = Url::parse(&locator.value).map_err(|error| DomainError::InvalidLocator {
            reason: format!("URL cannot be parsed: {error}"),
        })?;
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(DomainError::InvalidLocator {
                reason: "URL user information is not allowed".to_owned(),
            });
        }
    }

    Ok(())
}
