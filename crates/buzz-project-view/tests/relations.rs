mod support;

use buzz_project_view::{
    CreateMutation, DomainError, GoalView, IssuePatch, Mutation, MutationRequest,
    NewProjectViewObject, ObjectRef, Patch, PlanPatch, PlanView, ProjectView, ProjectViewEntry,
    ProjectViewObjectData, ProjectViewObjectType, RequirementPatch, StagePatch, UpdateMutation,
    WorkPatch, WorkStatus,
};
use uuid::Uuid;

use support::{
    initialize_request, new_goal, new_issue, new_plan, new_requirement, new_resource, new_role,
    new_stage, new_work, object_id, object_ref, Fixture,
};

fn goal_view(view: &ProjectView, goal_id: Uuid) -> &GoalView {
    view.goals
        .iter()
        .find(|goal| goal.goal.id == goal_id)
        .unwrap_or_else(|| panic!("goal {goal_id} must be present in the read model"))
}

fn plan_view(view: &ProjectView, plan_id: Uuid) -> &PlanView {
    view.goals
        .iter()
        .flat_map(|goal| &goal.plans)
        .chain(&view.unbound_plans)
        .find(|plan| plan.plan.id == plan_id)
        .unwrap_or_else(|| panic!("plan {plan_id} must be present in the read model"))
}

fn full_issue_occurrences(view: &ProjectView, issue_id: Uuid) -> usize {
    let in_plans = view
        .goals
        .iter()
        .flat_map(|goal| &goal.plans)
        .chain(&view.unbound_plans)
        .flat_map(|plan| &plan.stages)
        .flat_map(|stage| &stage.issues)
        .filter(|issue| issue.issue.id == issue_id)
        .count();
    let unplanned = view
        .unplanned_issues
        .iter()
        .filter(|issue| issue.issue.id == issue_id)
        .count();
    in_plans + unplanned
}

fn assert_reference_error(error: DomainError, object_id: Uuid, relation: &'static str) {
    assert_eq!(
        error,
        DomainError::ObjectStillReferenced {
            object_id,
            relation,
        }
    );
}

fn assert_tombstone(fixture: &Fixture, object_id: Uuid, object_type: ProjectViewObjectType) {
    match fixture.state.entry(object_id) {
        Some(ProjectViewEntry::Tombstone(tombstone)) => {
            assert_eq!(tombstone.object_type, object_type);
        }
        other => panic!("expected {object_id} to be tombstoned, got {other:?}"),
    }
}

// Relation checklist #1.
#[test]
fn project_view_has_exactly_one_profile() {
    let mut fixture = Fixture::initialized();
    let profile_id = fixture.profile_id();

    assert_eq!(
        fixture.active_count(ProjectViewObjectType::ProjectProfile),
        1
    );
    assert_eq!(fixture.view().profile.id, profile_id);

    fixture.reject_unchanged(initialize_request([object_id(2)]), "already_initialized");
    fixture.reject_delete_unchanged(
        ProjectViewObjectType::ProjectProfile,
        profile_id,
        "profile_delete_forbidden",
    );

    assert_eq!(
        fixture.active_count(ProjectViewObjectType::ProjectProfile),
        1
    );
}

// Relation checklist #2.
#[test]
fn initialized_project_view_always_has_at_least_one_goal() {
    let mut fixture = Fixture::uninitialized(2);

    fixture.reject_unchanged(initialize_request([]), "initial_goal_count");
    assert!(!fixture.state.is_initialized());
    assert!(fixture.state.entries().is_empty());

    fixture.apply(initialize_request([object_id(20)]));
    assert!(fixture.state.is_initialized());
    assert_eq!(fixture.active_count(ProjectViewObjectType::Goal), 1);
    assert_eq!(fixture.view().goals.len(), 1);
}

// Relation checklist #3.
#[test]
fn uninitialized_community_is_not_an_invalid_initialized_view() {
    let mut fixture = Fixture::uninitialized(3);

    assert_eq!(
        ProjectView::assemble(&fixture.state).unwrap_err(),
        DomainError::NotInitialized
    );
    fixture.reject_unchanged(
        MutationRequest::Create(CreateMutation {
            object: new_goal(object_id(30)),
        }),
        "not_initialized",
    );
    assert!(fixture.state.validate().is_ok());
    assert_eq!(fixture.state.project_revision(), 0);
}

// Relation checklist #4.
#[test]
fn deleting_last_goal_is_rejected_without_state_change() {
    let mut fixture = Fixture::initialized();
    let goal_id = fixture.initial_goal_id();

    let error = fixture.reject_delete_unchanged(
        ProjectViewObjectType::Goal,
        goal_id,
        "last_goal_delete_forbidden",
    );
    assert_eq!(error, DomainError::LastGoalDeletionForbidden);
    assert_eq!(fixture.active_count(ProjectViewObjectType::Goal), 1);
}

// Relation checklist #5.
#[test]
fn plan_is_unbound_or_under_exactly_one_goal() {
    let mut fixture = Fixture::initialized();
    let first_goal_id = fixture.initial_goal_id();
    let second_goal_id = fixture.create(new_goal(object_id(51)));
    let plan_id = fixture.create(new_plan(object_id(52), None));
    let plan_data = fixture.object(plan_id).data.clone();

    assert_eq!(fixture.view().unbound_plans[0].plan.id, plan_id);

    fixture.update(UpdateMutation::Plan {
        object_id: plan_id,
        patch: PlanPatch {
            under_goal_id: Patch::Set(first_goal_id),
            ..PlanPatch::default()
        },
    });
    assert_eq!(goal_view(&fixture.view(), first_goal_id).plans.len(), 1);

    fixture.update(UpdateMutation::Plan {
        object_id: plan_id,
        patch: PlanPatch {
            under_goal_id: Patch::Set(second_goal_id),
            ..PlanPatch::default()
        },
    });
    let view = fixture.view();
    assert!(goal_view(&view, first_goal_id).plans.is_empty());
    assert_eq!(
        goal_view(&view, second_goal_id)
            .plans
            .iter()
            .filter(|plan| plan.plan.id == plan_id)
            .count(),
        1
    );
    assert_eq!(
        fixture.object(plan_id).relations.under_goal_id,
        Some(second_goal_id)
    );

    fixture.update(UpdateMutation::Plan {
        object_id: plan_id,
        patch: PlanPatch {
            under_goal_id: Patch::Clear,
            ..PlanPatch::default()
        },
    });
    let view = fixture.view();
    assert_eq!(
        view.unbound_plans
            .iter()
            .filter(|plan| plan.plan.id == plan_id)
            .count(),
        1
    );
    assert_eq!(fixture.object(plan_id).data, plan_data);
}

// Relation checklist #6.
#[test]
fn stage_requires_exactly_one_plan() {
    let mut fixture = Fixture::initialized();
    let first_plan_id = fixture.create(new_plan(object_id(61), None));
    let second_plan_id = fixture.create(new_plan(object_id(62), None));
    let stage_id = fixture.create(new_stage(object_id(63), first_plan_id));

    fixture.reject_unchanged(
        MutationRequest::Update(UpdateMutation::Stage {
            object_id: stage_id,
            patch: StagePatch {
                under_plan_id: Patch::Clear,
                ..StagePatch::default()
            },
        }),
        "missing_relation",
    );

    fixture.update(UpdateMutation::Stage {
        object_id: stage_id,
        patch: StagePatch {
            under_plan_id: Patch::Set(second_plan_id),
            ..StagePatch::default()
        },
    });

    let view = fixture.view();
    assert!(plan_view(&view, first_plan_id).stages.is_empty());
    assert_eq!(
        plan_view(&view, second_plan_id)
            .stages
            .iter()
            .filter(|stage| stage.stage.id == stage_id)
            .count(),
        1
    );
    assert_eq!(
        fixture.object(stage_id).relations.under_plan_id,
        Some(second_plan_id)
    );
}

// Relation checklist #7.
#[test]
fn requirement_is_unplanned_or_in_exactly_one_stage() {
    let mut fixture = Fixture::initialized();
    let plan_id = fixture.create(new_plan(object_id(71), None));
    let first_stage_id = fixture.create(new_stage(object_id(72), plan_id));
    let second_stage_id = fixture.create(new_stage(object_id(73), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(74), None));

    assert_eq!(
        fixture.view().unplanned_requirements[0].requirement.id,
        requirement_id
    );

    fixture.update(UpdateMutation::Requirement {
        object_id: requirement_id,
        patch: RequirementPatch {
            planned_in_stage_id: Patch::Set(first_stage_id),
            ..RequirementPatch::default()
        },
    });
    fixture.update(UpdateMutation::Requirement {
        object_id: requirement_id,
        patch: RequirementPatch {
            planned_in_stage_id: Patch::Set(second_stage_id),
            ..RequirementPatch::default()
        },
    });

    let plan = plan_view(&fixture.view(), plan_id).clone();
    let first = plan
        .stages
        .iter()
        .find(|stage| stage.stage.id == first_stage_id)
        .expect("first stage must exist");
    let second = plan
        .stages
        .iter()
        .find(|stage| stage.stage.id == second_stage_id)
        .expect("second stage must exist");
    assert!(first.requirements.is_empty());
    assert_eq!(
        second
            .requirements
            .iter()
            .filter(|requirement| requirement.requirement.id == requirement_id)
            .count(),
        1
    );

    fixture.update(UpdateMutation::Requirement {
        object_id: requirement_id,
        patch: RequirementPatch {
            planned_in_stage_id: Patch::Clear,
            ..RequirementPatch::default()
        },
    });
    assert_eq!(
        fixture
            .view()
            .unplanned_requirements
            .iter()
            .filter(|requirement| requirement.requirement.id == requirement_id)
            .count(),
        1
    );
}

// Relation checklist #8.
#[test]
fn issue_is_unplanned_or_in_exactly_one_stage() {
    let mut fixture = Fixture::initialized();
    let plan_id = fixture.create(new_plan(object_id(81), None));
    let first_stage_id = fixture.create(new_stage(object_id(82), plan_id));
    let second_stage_id = fixture.create(new_stage(object_id(83), plan_id));
    let issue_id = fixture.create(new_issue(object_id(84), None, None));

    assert_eq!(fixture.view().unplanned_issues[0].issue.id, issue_id);

    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            planned_in_stage_id: Patch::Set(first_stage_id),
            ..IssuePatch::default()
        },
    });
    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            planned_in_stage_id: Patch::Set(second_stage_id),
            ..IssuePatch::default()
        },
    });

    let plan = plan_view(&fixture.view(), plan_id).clone();
    let first = plan
        .stages
        .iter()
        .find(|stage| stage.stage.id == first_stage_id)
        .expect("first stage must exist");
    let second = plan
        .stages
        .iter()
        .find(|stage| stage.stage.id == second_stage_id)
        .expect("second stage must exist");
    assert!(first.issues.is_empty());
    assert_eq!(
        second
            .issues
            .iter()
            .filter(|issue| issue.issue.id == issue_id)
            .count(),
        1
    );

    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            planned_in_stage_id: Patch::Clear,
            ..IssuePatch::default()
        },
    });
    assert_eq!(
        fixture
            .view()
            .unplanned_issues
            .iter()
            .filter(|issue| issue.issue.id == issue_id)
            .count(),
        1
    );
}

// Relation checklist #9.
#[test]
fn stage_can_contain_requirements_and_issues_together() {
    let mut fixture = Fixture::initialized();
    let plan_id = fixture.create(new_plan(object_id(91), None));
    let stage_id = fixture.create(new_stage(object_id(92), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(93), Some(stage_id)));
    let issue_id = fixture.create(new_issue(object_id(94), Some(stage_id), None));

    let view = fixture.view();
    let stage = &plan_view(&view, plan_id).stages[0];
    assert_eq!(stage.requirements[0].requirement.id, requirement_id);
    assert_eq!(stage.issues[0].issue.id, issue_id);
}

// Relation checklist #10.
#[test]
fn issue_may_have_no_about_target() {
    let mut fixture = Fixture::initialized();
    let issue_id = fixture.create(new_issue(object_id(101), None, None));

    assert!(fixture.object(issue_id).relations.about.is_none());
    let view = fixture.view();
    assert_eq!(view.unplanned_issues[0].issue.id, issue_id);
    assert!(view.issue_references_by_target.is_empty());
}

// Relation checklist #11.
#[test]
fn issue_about_accepts_every_same_project_element_except_self() {
    let mut fixture = Fixture::initialized();
    let profile_id = fixture.profile_id();
    let goal_id = fixture.initial_goal_id();
    let role_id = fixture.create(new_role(object_id(111)));
    let plan_id = fixture.create(new_plan(object_id(112), None));
    let stage_id = fixture.create(new_stage(object_id(113), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(114), Some(stage_id)));
    let target_issue_id = fixture.create(new_issue(object_id(115), Some(stage_id), None));
    let work_id = fixture.create(new_work(
        object_id(116),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));
    let resource_id = fixture.create(new_resource(object_id(117)));

    let targets = [
        (ProjectViewObjectType::ProjectProfile, profile_id),
        (ProjectViewObjectType::Goal, goal_id),
        (ProjectViewObjectType::Role, role_id),
        (ProjectViewObjectType::Plan, plan_id),
        (ProjectViewObjectType::Stage, stage_id),
        (ProjectViewObjectType::Requirement, requirement_id),
        (ProjectViewObjectType::Issue, target_issue_id),
        (ProjectViewObjectType::Work, work_id),
        (ProjectViewObjectType::Resource, resource_id),
    ];

    let mut expected_references = Vec::new();
    for (index, (object_type, target_id)) in targets.into_iter().enumerate() {
        let issue_id = fixture.create(new_issue(
            object_id(120 + index as u128),
            None,
            Some(object_ref(object_type, target_id)),
        ));
        expected_references.push((target_id, issue_id));
    }

    let view = fixture.view();
    for (target_id, issue_id) in expected_references {
        assert_eq!(
            view.issue_references_by_target.get(&target_id),
            Some(&vec![object_ref(ProjectViewObjectType::Issue, issue_id)])
        );
    }

    let self_id = object_id(140);
    let error = fixture.reject_unchanged(
        MutationRequest::Create(CreateMutation {
            object: new_issue(
                self_id,
                None,
                Some(object_ref(ProjectViewObjectType::Issue, self_id)),
            ),
        }),
        "self_reference",
    );
    assert_eq!(
        error,
        DomainError::SelfReference {
            relation: "about",
            object_id: self_id,
        }
    );
}

// Relation checklist #12.
#[test]
fn issue_about_is_independent_from_planned_in() {
    let mut fixture = Fixture::initialized();
    let first_goal_id = fixture.initial_goal_id();
    let second_goal_id = fixture.create(new_goal(object_id(151)));
    let first_plan_id = fixture.create(new_plan(object_id(152), Some(first_goal_id)));
    let second_plan_id = fixture.create(new_plan(object_id(153), Some(second_goal_id)));
    let first_stage_id = fixture.create(new_stage(object_id(154), first_plan_id));
    let second_stage_id = fixture.create(new_stage(object_id(155), second_plan_id));
    let target_requirement_id =
        fixture.create(new_requirement(object_id(156), Some(second_stage_id)));
    let issue_id = fixture.create(new_issue(
        object_id(157),
        Some(first_stage_id),
        Some(object_ref(
            ProjectViewObjectType::Requirement,
            target_requirement_id,
        )),
    ));

    let view = fixture.view();
    let issue_stage = &plan_view(&view, first_plan_id).stages[0];
    assert_eq!(issue_stage.issues[0].issue.id, issue_id);
    let target_stage = &plan_view(&view, second_plan_id).stages[0];
    assert_eq!(
        target_stage.requirements[0].requirement.id,
        target_requirement_id
    );
    assert_eq!(
        view.issue_references_by_target.get(&target_requirement_id),
        Some(&vec![object_ref(ProjectViewObjectType::Issue, issue_id)])
    );
}

// Relation checklist #13.
#[test]
fn work_can_handle_issue_without_requirement() {
    let mut fixture = Fixture::initialized();
    let issue_id = fixture.create(new_issue(object_id(161), None, None));
    let work_id = fixture.create(new_work(
        object_id(162),
        object_ref(ProjectViewObjectType::Issue, issue_id),
    ));

    assert_eq!(fixture.active_count(ProjectViewObjectType::Requirement), 0);
    let view = fixture.view();
    assert_eq!(view.unplanned_issues[0].issue.id, issue_id);
    assert_eq!(view.unplanned_issues[0].works[0].id, work_id);
}

// Relation checklist #14.
#[test]
fn work_handles_exactly_one_requirement_or_issue() {
    let mut fixture = Fixture::initialized();
    let requirement_id = fixture.create(new_requirement(object_id(171), None));
    let issue_id = fixture.create(new_issue(object_id(172), None, None));
    let work_id = fixture.create(new_work(
        object_id(173),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));

    fixture.update(UpdateMutation::Work {
        object_id: work_id,
        patch: WorkPatch {
            handles: Patch::Set(object_ref(ProjectViewObjectType::Issue, issue_id)),
            ..WorkPatch::default()
        },
    });
    assert_eq!(
        fixture.object(work_id).relations.handles,
        Some(object_ref(ProjectViewObjectType::Issue, issue_id))
    );
    let view = fixture.view();
    assert!(view.unplanned_requirements[0].works.is_empty());
    assert_eq!(view.unplanned_issues[0].works[0].id, work_id);

    fixture.reject_unchanged(
        MutationRequest::Update(UpdateMutation::Work {
            object_id: work_id,
            patch: WorkPatch {
                handles: Patch::Clear,
                ..WorkPatch::default()
            },
        }),
        "missing_relation",
    );

    fixture.reject_unchanged(
        MutationRequest::Create(CreateMutation {
            object: new_work(
                object_id(174),
                object_ref(ProjectViewObjectType::Goal, fixture.initial_goal_id()),
            ),
        }),
        "invalid_work_target",
    );

    let valid_wire = Mutation::new(
        fixture.state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: new_work(
                object_id(175),
                object_ref(ProjectViewObjectType::Requirement, requirement_id),
            ),
        }),
    );
    let mut missing_handles =
        serde_json::to_value(valid_wire).expect("valid Work mutation must serialize");
    missing_handles["request"]["object"]
        .as_object_mut()
        .expect("Work payload must be an object")
        .remove("handles");
    assert!(
        serde_json::from_value::<Mutation>(missing_handles).is_err(),
        "wire Work without handles must be rejected"
    );
}

// Relation checklist #15.
#[test]
fn requirement_or_issue_can_have_multiple_work_items() {
    let mut fixture = Fixture::initialized();
    let requirement_id = fixture.create(new_requirement(object_id(181), None));
    let issue_id = fixture.create(new_issue(object_id(182), None, None));
    let requirement_works = [
        fixture.create(new_work(
            object_id(183),
            object_ref(ProjectViewObjectType::Requirement, requirement_id),
        )),
        fixture.create(new_work(
            object_id(184),
            object_ref(ProjectViewObjectType::Requirement, requirement_id),
        )),
    ];
    let issue_works = [
        fixture.create(new_work(
            object_id(185),
            object_ref(ProjectViewObjectType::Issue, issue_id),
        )),
        fixture.create(new_work(
            object_id(186),
            object_ref(ProjectViewObjectType::Issue, issue_id),
        )),
    ];

    let view = fixture.view();
    let actual_requirement_works: Vec<_> = view.unplanned_requirements[0]
        .works
        .iter()
        .map(|work| work.id)
        .collect();
    let actual_issue_works: Vec<_> = view.unplanned_issues[0]
        .works
        .iter()
        .map(|work| work.id)
        .collect();
    assert_eq!(actual_requirement_works, requirement_works);
    assert_eq!(actual_issue_works, issue_works);
}

// Relation checklist #16.
#[test]
fn relationships_cannot_cross_project_boundary() {
    let mut local = Fixture::initialized_for(16);
    let mut foreign = Fixture::initialized_for(17);
    let foreign_goal_id = foreign.create(new_goal(object_id(701)));
    let foreign_plan_id = foreign.create(new_plan(object_id(702), Some(foreign_goal_id)));
    let foreign_stage_id = foreign.create(new_stage(object_id(703), foreign_plan_id));
    let foreign_requirement_id =
        foreign.create(new_requirement(object_id(704), Some(foreign_stage_id)));
    let foreign_resource_id = foreign.create(new_resource(object_id(705)));
    let foreign_before = foreign.state.clone();

    let attempts = [
        NewProjectViewObject::Plan {
            id: object_id(710),
            title: "Cross-project Plan".to_owned(),
            description: "Must fail closed".to_owned(),
            status: buzz_project_view::PlanStatus::Draft,
            under_goal_id: Some(foreign_goal_id),
        },
        NewProjectViewObject::Stage {
            id: object_id(711),
            title: "Cross-project Stage".to_owned(),
            description: "Must fail closed".to_owned(),
            status: buzz_project_view::StageStatus::Planned,
            under_plan_id: foreign_plan_id,
        },
        new_requirement(object_id(712), Some(foreign_stage_id)),
        new_issue(object_id(713), Some(foreign_stage_id), None),
        new_issue(
            object_id(714),
            None,
            Some(object_ref(
                ProjectViewObjectType::Resource,
                foreign_resource_id,
            )),
        ),
        new_work(
            object_id(715),
            object_ref(ProjectViewObjectType::Requirement, foreign_requirement_id),
        ),
    ];

    for attempt in attempts {
        local.reject_unchanged(
            MutationRequest::Create(CreateMutation { object: attempt }),
            "relation_target_not_found",
        );
    }
    assert_eq!(foreign.state, foreign_before);
}

// Relation checklist #17.
#[test]
fn unbound_plan_is_grouped_under_unbound_plans() {
    let mut fixture = Fixture::initialized();
    let goal_id = fixture.initial_goal_id();
    let plan_id = fixture.create(new_plan(object_id(201), Some(goal_id)));
    let stage_id = fixture.create(new_stage(object_id(202), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(203), Some(stage_id)));
    let issue_id = fixture.create(new_issue(object_id(204), Some(stage_id), None));
    let requirement_work_id = fixture.create(new_work(
        object_id(205),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));
    let issue_work_id = fixture.create(new_work(
        object_id(206),
        object_ref(ProjectViewObjectType::Issue, issue_id),
    ));
    let plan_data = fixture.object(plan_id).data.clone();

    fixture.update(UpdateMutation::Plan {
        object_id: plan_id,
        patch: PlanPatch {
            under_goal_id: Patch::Clear,
            ..PlanPatch::default()
        },
    });

    let view = fixture.view();
    assert!(goal_view(&view, goal_id).plans.is_empty());
    let unbound = view
        .unbound_plans
        .iter()
        .find(|plan| plan.plan.id == plan_id)
        .expect("unbound plan must remain in the view");
    assert_eq!(unbound.stages[0].stage.id, stage_id);
    assert_eq!(
        unbound.stages[0].requirements[0].requirement.id,
        requirement_id
    );
    assert_eq!(
        unbound.stages[0].requirements[0].works[0].id,
        requirement_work_id
    );
    assert_eq!(unbound.stages[0].issues[0].issue.id, issue_id);
    assert_eq!(unbound.stages[0].issues[0].works[0].id, issue_work_id);
    assert_eq!(fixture.object(plan_id).data, plan_data);
}

// Relation checklist #18.
#[test]
fn unplanned_items_are_grouped_by_type() {
    let mut fixture = Fixture::initialized();
    let plan_id = fixture.create(new_plan(object_id(211), None));
    let stage_id = fixture.create(new_stage(object_id(212), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(213), Some(stage_id)));
    let issue_id = fixture.create(new_issue(object_id(214), Some(stage_id), None));
    let requirement_work_id = fixture.create(new_work(
        object_id(215),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));
    let issue_work_id = fixture.create(new_work(
        object_id(216),
        object_ref(ProjectViewObjectType::Issue, issue_id),
    ));

    fixture.update(UpdateMutation::Requirement {
        object_id: requirement_id,
        patch: RequirementPatch {
            planned_in_stage_id: Patch::Clear,
            ..RequirementPatch::default()
        },
    });
    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            planned_in_stage_id: Patch::Clear,
            ..IssuePatch::default()
        },
    });

    let view = fixture.view();
    let stage = &plan_view(&view, plan_id).stages[0];
    assert!(stage.requirements.is_empty());
    assert!(stage.issues.is_empty());
    assert_eq!(
        view.unplanned_requirements[0].requirement.id,
        requirement_id
    );
    assert_eq!(
        view.unplanned_requirements[0].works[0].id,
        requirement_work_id
    );
    assert_eq!(view.unplanned_issues[0].issue.id, issue_id);
    assert_eq!(view.unplanned_issues[0].works[0].id, issue_work_id);
}

// Relation checklist #19.
#[test]
fn issue_has_one_canonical_placement_and_reverse_about_reference() {
    let mut fixture = Fixture::initialized();
    let first_plan_id = fixture.create(new_plan(object_id(221), Some(fixture.initial_goal_id())));
    let second_plan_id = fixture.create(new_plan(object_id(222), None));
    let first_stage_id = fixture.create(new_stage(object_id(223), first_plan_id));
    let second_stage_id = fixture.create(new_stage(object_id(224), second_plan_id));
    let target_requirement_id =
        fixture.create(new_requirement(object_id(225), Some(second_stage_id)));
    let issue_id = fixture.create(new_issue(
        object_id(226),
        Some(first_stage_id),
        Some(object_ref(
            ProjectViewObjectType::Requirement,
            target_requirement_id,
        )),
    ));

    let view = fixture.view();
    assert_eq!(full_issue_occurrences(&view, issue_id), 1);
    assert_eq!(
        view.issue_references_by_target.get(&target_requirement_id),
        Some(&vec![ObjectRef {
            object_type: ProjectViewObjectType::Issue,
            object_id: issue_id,
        }])
    );
    assert_eq!(
        plan_view(&view, second_plan_id).stages[0].requirements[0]
            .requirement
            .id,
        target_requirement_id
    );
}

// Relation checklist #20.
#[test]
fn completing_work_does_not_cascade_status_changes() {
    let mut fixture = Fixture::initialized();
    let profile_id = fixture.profile_id();
    let goal_id = fixture.initial_goal_id();
    let plan_id = fixture.create(new_plan(object_id(231), Some(goal_id)));
    let stage_id = fixture.create(new_stage(object_id(232), plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(233), Some(stage_id)));
    let issue_id = fixture.create(new_issue(object_id(234), Some(stage_id), None));
    let work_id = fixture.create(new_work(
        object_id(235),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));
    let unchanged_ids = [
        profile_id,
        goal_id,
        plan_id,
        stage_id,
        requirement_id,
        issue_id,
    ];
    let unchanged_entries: Vec<_> = unchanged_ids
        .iter()
        .map(|object_id| {
            fixture
                .state
                .entry(*object_id)
                .expect("fixture entry must exist")
                .clone()
        })
        .collect();
    let work_revision = fixture.object(work_id).object_revision;

    fixture.update(UpdateMutation::Work {
        object_id: work_id,
        patch: WorkPatch {
            status: Patch::Set(WorkStatus::Completed),
            ..WorkPatch::default()
        },
    });

    for (object_id, expected) in unchanged_ids.into_iter().zip(unchanged_entries) {
        assert_eq!(
            fixture.state.entry(object_id),
            Some(&expected),
            "completing Work changed related object {object_id}"
        );
    }
    let work = fixture.object(work_id);
    assert_eq!(work.object_revision, work_revision + 1);
    match &work.data {
        ProjectViewObjectData::Work(work) => assert_eq!(work.status, WorkStatus::Completed),
        other => panic!("expected Work data, got {other:?}"),
    }
}

fn create_isolated_about_target(
    fixture: &mut Fixture,
    object_type: ProjectViewObjectType,
    base_slot: u128,
) -> Uuid {
    match object_type {
        ProjectViewObjectType::ProjectProfile => fixture.profile_id(),
        ProjectViewObjectType::Goal => fixture.create(new_goal(object_id(base_slot))),
        ProjectViewObjectType::Role => fixture.create(new_role(object_id(base_slot))),
        ProjectViewObjectType::Plan => fixture.create(new_plan(object_id(base_slot), None)),
        ProjectViewObjectType::Stage => {
            let plan_id = fixture.create(new_plan(object_id(base_slot + 1), None));
            fixture.create(new_stage(object_id(base_slot), plan_id))
        }
        ProjectViewObjectType::Requirement => {
            fixture.create(new_requirement(object_id(base_slot), None))
        }
        ProjectViewObjectType::Issue => fixture.create(new_issue(object_id(base_slot), None, None)),
        ProjectViewObjectType::Work => {
            let requirement_id = fixture.create(new_requirement(object_id(base_slot + 1), None));
            fixture.create(new_work(
                object_id(base_slot),
                object_ref(ProjectViewObjectType::Requirement, requirement_id),
            ))
        }
        ProjectViewObjectType::Resource => fixture.create(new_resource(object_id(base_slot))),
    }
}

// Relation checklist #21.
#[test]
fn deleting_referenced_objects_is_rejected_without_cascade() {
    let mut fixture = Fixture::initialized();
    let profile_id = fixture.profile_id();
    let first_goal_id = fixture.initial_goal_id();
    let _second_goal_id = fixture.create(new_goal(object_id(241)));
    let first_plan_id = fixture.create(new_plan(object_id(242), Some(first_goal_id)));
    let second_plan_id = fixture.create(new_plan(object_id(243), None));
    let stage_id = fixture.create(new_stage(object_id(244), first_plan_id));
    let requirement_id = fixture.create(new_requirement(object_id(245), Some(stage_id)));
    let resource_id = fixture.create(new_resource(object_id(246)));
    let issue_id = fixture.create(new_issue(
        object_id(247),
        Some(stage_id),
        Some(object_ref(ProjectViewObjectType::Resource, resource_id)),
    ));
    let requirement_work_id = fixture.create(new_work(
        object_id(248),
        object_ref(ProjectViewObjectType::Requirement, requirement_id),
    ));
    let issue_work_id = fixture.create(new_work(
        object_id(249),
        object_ref(ProjectViewObjectType::Issue, issue_id),
    ));

    fixture.reject_delete_unchanged(
        ProjectViewObjectType::ProjectProfile,
        profile_id,
        "profile_delete_forbidden",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Goal,
            first_goal_id,
            "object_referenced",
        ),
        first_goal_id,
        "under_goal_id",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Plan,
            first_plan_id,
            "object_referenced",
        ),
        first_plan_id,
        "under_plan_id",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Stage,
            stage_id,
            "object_referenced",
        ),
        stage_id,
        "planned_in_stage_id",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Requirement,
            requirement_id,
            "object_referenced",
        ),
        requirement_id,
        "handles",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Issue,
            issue_id,
            "object_referenced",
        ),
        issue_id,
        "handles",
    );
    assert_reference_error(
        fixture.reject_delete_unchanged(
            ProjectViewObjectType::Resource,
            resource_id,
            "object_referenced",
        ),
        resource_id,
        "about",
    );

    fixture.update(UpdateMutation::Plan {
        object_id: first_plan_id,
        patch: PlanPatch {
            under_goal_id: Patch::Clear,
            ..PlanPatch::default()
        },
    });
    fixture.delete(ProjectViewObjectType::Goal, first_goal_id);
    assert_tombstone(&fixture, first_goal_id, ProjectViewObjectType::Goal);
    assert!(matches!(
        fixture.state.entry(first_plan_id),
        Some(ProjectViewEntry::Active(_))
    ));

    fixture.update(UpdateMutation::Stage {
        object_id: stage_id,
        patch: StagePatch {
            under_plan_id: Patch::Set(second_plan_id),
            ..StagePatch::default()
        },
    });
    fixture.delete(ProjectViewObjectType::Plan, first_plan_id);
    assert_tombstone(&fixture, first_plan_id, ProjectViewObjectType::Plan);
    assert_eq!(
        fixture.object(stage_id).relations.under_plan_id,
        Some(second_plan_id)
    );

    fixture.update(UpdateMutation::Requirement {
        object_id: requirement_id,
        patch: RequirementPatch {
            planned_in_stage_id: Patch::Clear,
            ..RequirementPatch::default()
        },
    });
    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            planned_in_stage_id: Patch::Clear,
            ..IssuePatch::default()
        },
    });
    fixture.delete(ProjectViewObjectType::Stage, stage_id);
    assert_tombstone(&fixture, stage_id, ProjectViewObjectType::Stage);
    assert!(matches!(
        fixture.state.entry(requirement_id),
        Some(ProjectViewEntry::Active(_))
    ));
    assert!(matches!(
        fixture.state.entry(issue_id),
        Some(ProjectViewEntry::Active(_))
    ));

    fixture.update(UpdateMutation::Issue {
        object_id: issue_id,
        patch: IssuePatch {
            about: Patch::Clear,
            ..IssuePatch::default()
        },
    });
    fixture.delete(ProjectViewObjectType::Resource, resource_id);
    assert_tombstone(&fixture, resource_id, ProjectViewObjectType::Resource);
    assert!(matches!(
        fixture.state.entry(issue_id),
        Some(ProjectViewEntry::Active(_))
    ));

    fixture.update(UpdateMutation::Work {
        object_id: requirement_work_id,
        patch: WorkPatch {
            handles: Patch::Set(object_ref(ProjectViewObjectType::Issue, issue_id)),
            ..WorkPatch::default()
        },
    });
    fixture.delete(ProjectViewObjectType::Requirement, requirement_id);
    assert_tombstone(&fixture, requirement_id, ProjectViewObjectType::Requirement);
    assert!(matches!(
        fixture.state.entry(requirement_work_id),
        Some(ProjectViewEntry::Active(_))
    ));

    let replacement_requirement_id = fixture.create(new_requirement(object_id(250), None));
    for work_id in [requirement_work_id, issue_work_id] {
        fixture.update(UpdateMutation::Work {
            object_id: work_id,
            patch: WorkPatch {
                handles: Patch::Set(object_ref(
                    ProjectViewObjectType::Requirement,
                    replacement_requirement_id,
                )),
                ..WorkPatch::default()
            },
        });
    }
    fixture.delete(ProjectViewObjectType::Issue, issue_id);
    assert_tombstone(&fixture, issue_id, ProjectViewObjectType::Issue);
    assert!(matches!(
        fixture.state.entry(requirement_work_id),
        Some(ProjectViewEntry::Active(_))
    ));
    assert!(matches!(
        fixture.state.entry(issue_work_id),
        Some(ProjectViewEntry::Active(_))
    ));
    assert!(fixture.state.validate().is_ok());
    assert!(ProjectView::assemble(&fixture.state).is_ok());

    for (index, object_type) in [
        ProjectViewObjectType::ProjectProfile,
        ProjectViewObjectType::Goal,
        ProjectViewObjectType::Role,
        ProjectViewObjectType::Plan,
        ProjectViewObjectType::Stage,
        ProjectViewObjectType::Requirement,
        ProjectViewObjectType::Issue,
        ProjectViewObjectType::Work,
        ProjectViewObjectType::Resource,
    ]
    .into_iter()
    .enumerate()
    {
        let mut about_fixture = Fixture::initialized_for(100 + index as u128);
        let target_id = create_isolated_about_target(
            &mut about_fixture,
            object_type,
            1_000 + index as u128 * 10,
        );
        let source_issue_id = object_id(1_090 + index as u128);
        about_fixture.create(new_issue(
            source_issue_id,
            None,
            Some(object_ref(object_type, target_id)),
        ));

        let expected_code = if object_type == ProjectViewObjectType::ProjectProfile {
            "profile_delete_forbidden"
        } else {
            "object_referenced"
        };
        let error = about_fixture.reject_delete_unchanged(object_type, target_id, expected_code);
        if object_type == ProjectViewObjectType::ProjectProfile {
            assert_eq!(error, DomainError::ProfileDeletionForbidden);
        } else {
            assert_reference_error(error, target_id, "about");
        }
        assert!(matches!(
            about_fixture.state.entry(source_issue_id),
            Some(ProjectViewEntry::Active(_))
        ));
    }
}
