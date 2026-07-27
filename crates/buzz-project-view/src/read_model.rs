//! Deterministic assembly of the logical Project View hierarchy.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DomainError, DomainResult};
use crate::model::{
    ObjectRef, ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType,
    ProjectViewRelations,
};
use crate::state::ProjectViewState;
use crate::validation::validate_state;

/// The complete logical read model for one initialized project.
///
/// Every active canonical object appears in exactly one owning position.
/// Issues additionally appear as lightweight references under their `about`
/// targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectView {
    /// The project's unique profile.
    pub profile: ProjectViewObject,
    /// Goals and the plans currently organized beneath them.
    pub goals: Vec<GoalView>,
    /// Plans that are not currently organized beneath a goal.
    pub unbound_plans: Vec<PlanView>,
    /// Requirements that are not currently planned in a stage.
    pub unplanned_requirements: Vec<RequirementView>,
    /// Issues that are not currently planned in a stage.
    pub unplanned_issues: Vec<IssueView>,
    /// Semantic project roles, in deterministic canonical order.
    pub roles: Vec<ProjectViewObject>,
    /// Project resources, in deterministic canonical order.
    pub resources: Vec<ProjectViewObject>,
    /// Lightweight Issue references grouped by their `about` target.
    ///
    /// Keys are globally unique active target IDs and values are typed Issue
    /// references. Full Issue objects remain in their single canonical planned
    /// or unplanned position.
    pub issue_references_by_target: BTreeMap<Uuid, Vec<ObjectRef>>,
}

/// One goal and the plans currently organized beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalView {
    /// The canonical goal object.
    pub goal: ProjectViewObject,
    /// Plans whose `under_goal_id` names this goal.
    pub plans: Vec<PlanView>,
}

/// One plan and all of its stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    /// The canonical plan object.
    pub plan: ProjectViewObject,
    /// Stages whose required `under_plan_id` names this plan.
    pub stages: Vec<StageView>,
}

/// One stage and the requirements and issues planned within it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StageView {
    /// The canonical stage object.
    pub stage: ProjectViewObject,
    /// Requirements planned in this stage.
    pub requirements: Vec<RequirementView>,
    /// Issues planned in this stage.
    pub issues: Vec<IssueView>,
}

/// One requirement and all work items that handle it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementView {
    /// The canonical requirement object.
    pub requirement: ProjectViewObject,
    /// Work items whose required `handles` relation names this requirement.
    pub works: Vec<ProjectViewObject>,
}

/// One issue and all work items that handle it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueView {
    /// The canonical issue object.
    pub issue: ProjectViewObject,
    /// Work items whose required `handles` relation names this issue.
    pub works: Vec<ProjectViewObject>,
}

impl ProjectView {
    /// Assemble a deterministic logical hierarchy from active canonical state.
    ///
    /// Collections are ordered by `(created_at, id)`. Missing targets,
    /// mistyped targets, illegal relation slots, duplicate identities, and
    /// invalid project-wide cardinalities fail explicitly instead of being
    /// silently moved to an unbound or unplanned section.
    pub fn assemble(state: &ProjectViewState) -> DomainResult<Self> {
        if !state.is_initialized() {
            return Err(DomainError::NotInitialized);
        }
        validate_state(state)?;

        let mut objects: Vec<&ProjectViewObject> = state.active_objects().collect();
        objects.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });

        let objects_by_id = index_objects(&objects)?;

        let mut profiles = Vec::new();
        let mut goals = Vec::new();
        let mut roles = Vec::new();
        let mut plans = Vec::new();
        let mut stages = Vec::new();
        let mut requirements = Vec::new();
        let mut issues = Vec::new();
        let mut works = Vec::new();
        let mut resources = Vec::new();

        for object in objects {
            validate_object_shape(object)?;
            match &object.data {
                ProjectViewObjectData::ProjectProfile(_) => profiles.push(object),
                ProjectViewObjectData::Goal(_) => goals.push(object),
                ProjectViewObjectData::Role(_) => roles.push((*object).clone()),
                ProjectViewObjectData::Plan(_) => plans.push(object),
                ProjectViewObjectData::Stage(_) => stages.push(object),
                ProjectViewObjectData::Requirement(_) => requirements.push(object),
                ProjectViewObjectData::Issue(_) => issues.push(object),
                ProjectViewObjectData::Work(_) => works.push(object),
                ProjectViewObjectData::Resource(_) => resources.push((*object).clone()),
            }
        }

        let profile = exactly_one_profile(profiles)?;
        if goals.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason: "an initialized Project View must contain at least one active goal"
                    .to_owned(),
            });
        }

        let mut works_by_target = group_works(works, &objects_by_id)?;
        let issue_references_by_target = group_issue_references(&issues, &objects_by_id)?;

        let mut requirements_by_stage: HashMap<Uuid, Vec<RequirementView>> = HashMap::new();
        let mut unplanned_requirements = Vec::new();
        for requirement in requirements {
            let view = RequirementView {
                requirement: (*requirement).clone(),
                works: works_by_target.remove(&requirement.id).unwrap_or_default(),
            };
            match requirement.relations.planned_in_stage_id {
                Some(stage_id) => {
                    require_target_type(
                        "planned_in_stage_id",
                        stage_id,
                        ProjectViewObjectType::Stage,
                        &objects_by_id,
                    )?;
                    requirements_by_stage
                        .entry(stage_id)
                        .or_default()
                        .push(view);
                }
                None => unplanned_requirements.push(view),
            }
        }

        let mut issues_by_stage: HashMap<Uuid, Vec<IssueView>> = HashMap::new();
        let mut unplanned_issues = Vec::new();
        for issue in issues {
            let view = IssueView {
                issue: (*issue).clone(),
                works: works_by_target.remove(&issue.id).unwrap_or_default(),
            };
            match issue.relations.planned_in_stage_id {
                Some(stage_id) => {
                    require_target_type(
                        "planned_in_stage_id",
                        stage_id,
                        ProjectViewObjectType::Stage,
                        &objects_by_id,
                    )?;
                    issues_by_stage.entry(stage_id).or_default().push(view);
                }
                None => unplanned_issues.push(view),
            }
        }

        if !works_by_target.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason:
                    "one or more active work items could not be placed under their handles target"
                        .to_owned(),
            });
        }

        let mut stages_by_plan: HashMap<Uuid, Vec<StageView>> = HashMap::new();
        for stage in stages {
            let plan_id =
                stage
                    .relations
                    .under_plan_id
                    .ok_or(DomainError::MissingRequiredRelation {
                        relation: "under_plan_id",
                    })?;
            require_target_type(
                "under_plan_id",
                plan_id,
                ProjectViewObjectType::Plan,
                &objects_by_id,
            )?;
            stages_by_plan.entry(plan_id).or_default().push(StageView {
                stage: (*stage).clone(),
                requirements: requirements_by_stage.remove(&stage.id).unwrap_or_default(),
                issues: issues_by_stage.remove(&stage.id).unwrap_or_default(),
            });
        }

        if !requirements_by_stage.is_empty() || !issues_by_stage.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason:
                    "one or more planned requirements or issues could not be placed under a stage"
                        .to_owned(),
            });
        }

        let mut plans_by_goal: HashMap<Uuid, Vec<PlanView>> = HashMap::new();
        let mut unbound_plans = Vec::new();
        for plan in plans {
            let view = PlanView {
                plan: (*plan).clone(),
                stages: stages_by_plan.remove(&plan.id).unwrap_or_default(),
            };
            match plan.relations.under_goal_id {
                Some(goal_id) => {
                    require_target_type(
                        "under_goal_id",
                        goal_id,
                        ProjectViewObjectType::Goal,
                        &objects_by_id,
                    )?;
                    plans_by_goal.entry(goal_id).or_default().push(view);
                }
                None => unbound_plans.push(view),
            }
        }

        if !stages_by_plan.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason: "one or more stages could not be placed under their required plan"
                    .to_owned(),
            });
        }

        let goals = goals
            .into_iter()
            .map(|goal| GoalView {
                goal: (*goal).clone(),
                plans: plans_by_goal.remove(&goal.id).unwrap_or_default(),
            })
            .collect();

        if !plans_by_goal.is_empty() {
            return Err(DomainError::InvalidFinalState {
                reason: "one or more plans could not be placed under their goal".to_owned(),
            });
        }

        Ok(Self {
            profile: (*profile).clone(),
            goals,
            unbound_plans,
            unplanned_requirements,
            unplanned_issues,
            roles,
            resources,
            issue_references_by_target,
        })
    }
}

impl TryFrom<&ProjectViewState> for ProjectView {
    type Error = DomainError;

    /// Assemble the deterministic read model from canonical state.
    fn try_from(state: &ProjectViewState) -> Result<Self, Self::Error> {
        Self::assemble(state)
    }
}

fn index_objects<'a>(
    objects: &[&'a ProjectViewObject],
) -> DomainResult<HashMap<Uuid, &'a ProjectViewObject>> {
    let mut by_id = HashMap::with_capacity(objects.len());
    for object in objects {
        if by_id.insert(object.id, *object).is_some() {
            return Err(DomainError::InvalidFinalState {
                reason: format!("duplicate active object id {}", object.id),
            });
        }
    }
    Ok(by_id)
}

fn exactly_one_profile(profiles: Vec<&ProjectViewObject>) -> DomainResult<&ProjectViewObject> {
    if profiles.len() != 1 {
        return Err(DomainError::InvalidFinalState {
            reason: format!(
                "an initialized Project View must contain exactly one active profile, found {}",
                profiles.len()
            ),
        });
    }
    profiles
        .into_iter()
        .next()
        .ok_or_else(|| DomainError::InvalidFinalState {
            reason: "the active Project View profile is missing".to_owned(),
        })
}

fn validate_object_shape(object: &ProjectViewObject) -> DomainResult<()> {
    let actual_type = object.data.object_type();
    if object.object_type != actual_type {
        return Err(DomainError::DataTypeMismatch {
            declared: object.object_type,
            actual: actual_type,
        });
    }

    let allowed = match object.object_type {
        ProjectViewObjectType::ProjectProfile
        | ProjectViewObjectType::Goal
        | ProjectViewObjectType::Role
        | ProjectViewObjectType::Resource => AllowedRelations::NONE,
        ProjectViewObjectType::Plan => AllowedRelations::UNDER_GOAL,
        ProjectViewObjectType::Stage => AllowedRelations::UNDER_PLAN,
        ProjectViewObjectType::Requirement => AllowedRelations::PLANNED_IN_STAGE,
        ProjectViewObjectType::Issue => {
            AllowedRelations::PLANNED_IN_STAGE.union(AllowedRelations::ABOUT)
        }
        ProjectViewObjectType::Work => AllowedRelations::HANDLES,
    };
    reject_disallowed_relations(object.object_type, object.relations, allowed)?;

    if object.object_type == ProjectViewObjectType::Stage
        && object.relations.under_plan_id.is_none()
    {
        return Err(DomainError::MissingRequiredRelation {
            relation: "under_plan_id",
        });
    }
    if object.object_type == ProjectViewObjectType::Work && object.relations.handles.is_none() {
        return Err(DomainError::MissingRequiredRelation {
            relation: "handles",
        });
    }
    Ok(())
}

fn group_works(
    works: Vec<&ProjectViewObject>,
    objects_by_id: &HashMap<Uuid, &ProjectViewObject>,
) -> DomainResult<HashMap<Uuid, Vec<ProjectViewObject>>> {
    let mut by_target: HashMap<Uuid, Vec<ProjectViewObject>> = HashMap::new();
    for work in works {
        let target = work
            .relations
            .handles
            .ok_or(DomainError::MissingRequiredRelation {
                relation: "handles",
            })?;
        let actual = require_typed_target("handles", target, objects_by_id)?;
        if !matches!(
            actual.object_type,
            ProjectViewObjectType::Requirement | ProjectViewObjectType::Issue
        ) {
            return Err(DomainError::InvalidWorkTarget {
                actual: actual.object_type,
            });
        }
        by_target
            .entry(target.object_id)
            .or_default()
            .push((*work).clone());
    }
    Ok(by_target)
}

fn group_issue_references(
    issues: &[&ProjectViewObject],
    objects_by_id: &HashMap<Uuid, &ProjectViewObject>,
) -> DomainResult<BTreeMap<Uuid, Vec<ObjectRef>>> {
    let mut by_target: BTreeMap<Uuid, Vec<ObjectRef>> = BTreeMap::new();
    for issue in issues {
        let Some(target) = issue.relations.about else {
            continue;
        };
        if target.object_id == issue.id {
            return Err(DomainError::SelfReference {
                relation: "about",
                object_id: issue.id,
            });
        }
        require_typed_target("about", target, objects_by_id)?;
        by_target
            .entry(target.object_id)
            .or_default()
            .push(ObjectRef {
                object_type: ProjectViewObjectType::Issue,
                object_id: issue.id,
            });
    }
    Ok(by_target)
}

fn require_typed_target<'a>(
    relation: &'static str,
    target: ObjectRef,
    objects_by_id: &HashMap<Uuid, &'a ProjectViewObject>,
) -> DomainResult<&'a ProjectViewObject> {
    let actual = objects_by_id.get(&target.object_id).copied().ok_or(
        DomainError::RelationTargetNotFound {
            relation,
            target_id: target.object_id,
        },
    )?;
    if target.object_type != actual.object_type {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id: target.object_id,
            declared: target.object_type,
            actual: actual.object_type,
        });
    }
    Ok(actual)
}

fn require_target_type<'a>(
    relation: &'static str,
    target_id: Uuid,
    expected: ProjectViewObjectType,
    objects_by_id: &HashMap<Uuid, &'a ProjectViewObject>,
) -> DomainResult<&'a ProjectViewObject> {
    let actual =
        objects_by_id
            .get(&target_id)
            .copied()
            .ok_or(DomainError::RelationTargetNotFound {
                relation,
                target_id,
            })?;
    if actual.object_type != expected {
        return Err(DomainError::RelationTargetTypeMismatch {
            relation,
            target_id,
            declared: expected,
            actual: actual.object_type,
        });
    }
    Ok(actual)
}

fn reject_disallowed_relations(
    object_type: ProjectViewObjectType,
    relations: ProjectViewRelations,
    allowed: AllowedRelations,
) -> DomainResult<()> {
    let relation = if relations.under_goal_id.is_some()
        && !allowed.contains(AllowedRelations::UNDER_GOAL)
    {
        Some("under_goal_id")
    } else if relations.under_plan_id.is_some() && !allowed.contains(AllowedRelations::UNDER_PLAN) {
        Some("under_plan_id")
    } else if relations.planned_in_stage_id.is_some()
        && !allowed.contains(AllowedRelations::PLANNED_IN_STAGE)
    {
        Some("planned_in_stage_id")
    } else if relations.about.is_some() && !allowed.contains(AllowedRelations::ABOUT) {
        Some("about")
    } else if relations.handles.is_some() && !allowed.contains(AllowedRelations::HANDLES) {
        Some("handles")
    } else {
        None
    };

    match relation {
        Some(relation) => Err(DomainError::RelationNotAllowed {
            relation,
            object_type,
        }),
        None => Ok(()),
    }
}

#[derive(Debug, Clone, Copy)]
struct AllowedRelations(u8);

impl AllowedRelations {
    const NONE: Self = Self(0);
    const UNDER_GOAL: Self = Self(1 << 0);
    const UNDER_PLAN: Self = Self(1 << 1);
    const PLANNED_IN_STAGE: Self = Self(1 << 2);
    const ABOUT: Self = Self(1 << 3);
    const HANDLES: Self = Self(1 << 4);

    const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
