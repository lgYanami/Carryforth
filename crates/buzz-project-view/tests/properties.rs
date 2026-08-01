use std::collections::BTreeSet;
use std::fmt::Debug;

use buzz_core::{CommunityId, PublicKey};
use buzz_project_view::{
    CreateMutation, DeleteMutation, DomainError, InitializeGoal, InitializeMutation, IssuePatch,
    IssueStatus, LocatorType, Mutation, MutationRequest, NewProjectViewObject, ObjectRef, Patch,
    PlanPatch, PlanStatus, PlanView, Priority, ProjectProfile, ProjectView, ProjectViewEntry,
    ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType, ProjectViewState,
    RequirementPatch, RequirementStatus, ResourceLocator, ResourceType, StagePatch, StageStatus,
    UpdateMutation, WorkPatch, WorkStatus,
};
use chrono::{DateTime, Utc};
use proptest::prelude::*;
use uuid::Uuid;

const GOAL_ONE: u128 = 1;
const GOAL_TWO: u128 = 2;
const PLAN_ONE: u128 = 10;
const PLAN_TWO: u128 = 11;
const STAGE_ONE: u128 = 20;
const STAGE_TWO: u128 = 21;
const REQUIREMENT_ONE: u128 = 30;
const REQUIREMENT_TWO: u128 = 31;
const ISSUE_ONE: u128 = 40;
const ISSUE_TWO: u128 = 41;
const WORK_ONE: u128 = 50;
const WORK_TWO: u128 = 51;
const ROLE_ONE: u128 = 60;
const RESOURCE_ONE: u128 = 61;

#[derive(Clone, Copy, Debug)]
struct StepSeed {
    operation: u8,
    choice: u8,
}

fn step_strategy() -> impl Strategy<Value = StepSeed> {
    (0u8..8, any::<u8>()).prop_map(|(operation, choice)| StepSeed { operation, choice })
}

fn sequence_strategy(max_len: usize) -> impl Strategy<Value = Vec<StepSeed>> {
    prop::collection::vec(step_strategy(), 1..=max_len)
}

fn must<T, E: Debug>(result: Result<T, E>, context: &str) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("{context}: {error:?}"),
    }
}

fn project_id() -> CommunityId {
    CommunityId::from_uuid(Uuid::from_u128(0xfeed_face_cafe_beef))
}

fn actor() -> PublicKey {
    must(
        PublicKey::from_hex("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"),
        "fixed actor public key must be valid",
    )
}

fn canonical_time(revision: u64) -> DateTime<Utc> {
    let seconds = 1_700_000_000_i64 + i64::try_from(revision).unwrap_or(i64::MAX / 2);
    match DateTime::from_timestamp(seconds, 0) {
        Some(value) => value,
        None => panic!("fixed canonical timestamp must be representable"),
    }
}

fn object_id(seed: u128) -> Uuid {
    let mut bytes = seed.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn apply_request(state: &mut ProjectViewState, request: MutationRequest) {
    let mutation = Mutation::new(state.project_revision(), request);
    let next_revision = state.project_revision() + 1;
    must(
        state.apply(&mutation, actor(), canonical_time(next_revision)),
        "generated request must be valid",
    );
}

fn baseline_state() -> ProjectViewState {
    let mut state = ProjectViewState::new(project_id());

    apply_request(
        &mut state,
        MutationRequest::Initialize(InitializeMutation {
            profile: ProjectProfile {
                name: "Project View".to_owned(),
                positioning: "Shared project state".to_owned(),
                purpose: "Coordinate human and agent work".to_owned(),
                problem: "Project state is otherwise fragmented".to_owned(),
                scope: "Deterministic domain model".to_owned(),
            },
            goals: vec![
                InitializeGoal {
                    id: object_id(GOAL_ONE),
                    title: "Goal one".to_owned(),
                    desired_outcome: "First outcome".to_owned(),
                    directions: vec!["Direction one".to_owned()],
                },
                InitializeGoal {
                    id: object_id(GOAL_TWO),
                    title: "Goal two".to_owned(),
                    desired_outcome: "Second outcome".to_owned(),
                    directions: vec![],
                },
            ],
        }),
    );

    for object in [
        NewProjectViewObject::Plan {
            id: object_id(PLAN_ONE),
            title: "Plan one".to_owned(),
            description: "First plan".to_owned(),
            status: PlanStatus::Active,
            under_goal_id: Some(object_id(GOAL_ONE)),
        },
        NewProjectViewObject::Plan {
            id: object_id(PLAN_TWO),
            title: "Plan two".to_owned(),
            description: "Unbound plan".to_owned(),
            status: PlanStatus::Draft,
            under_goal_id: None,
        },
        NewProjectViewObject::Stage {
            id: object_id(STAGE_ONE),
            title: "Stage one".to_owned(),
            description: "First stage".to_owned(),
            status: StageStatus::Active,
            under_plan_id: object_id(PLAN_ONE),
        },
        NewProjectViewObject::Stage {
            id: object_id(STAGE_TWO),
            title: "Stage two".to_owned(),
            description: "Second stage".to_owned(),
            status: StageStatus::Planned,
            under_plan_id: object_id(PLAN_TWO),
        },
        NewProjectViewObject::Requirement {
            id: object_id(REQUIREMENT_ONE),
            title: "Requirement one".to_owned(),
            description: "Planned requirement".to_owned(),
            status: RequirementStatus::Ready,
            priority: Priority::High,
            planned_in_stage_id: Some(object_id(STAGE_ONE)),
        },
        NewProjectViewObject::Requirement {
            id: object_id(REQUIREMENT_TWO),
            title: "Requirement two".to_owned(),
            description: "Unplanned requirement".to_owned(),
            status: RequirementStatus::Proposed,
            priority: Priority::Normal,
            planned_in_stage_id: None,
        },
        NewProjectViewObject::Issue {
            id: object_id(ISSUE_ONE),
            title: "Issue one".to_owned(),
            description: "Planned issue".to_owned(),
            status: IssueStatus::Open,
            priority: Priority::Urgent,
            planned_in_stage_id: Some(object_id(STAGE_ONE)),
            about: Some(ObjectRef {
                object_type: ProjectViewObjectType::Requirement,
                object_id: object_id(REQUIREMENT_ONE),
            }),
        },
        NewProjectViewObject::Issue {
            id: object_id(ISSUE_TWO),
            title: "Issue two".to_owned(),
            description: "Unplanned issue".to_owned(),
            status: IssueStatus::InProgress,
            priority: Priority::Normal,
            planned_in_stage_id: None,
            about: Some(ObjectRef {
                object_type: ProjectViewObjectType::Issue,
                object_id: object_id(ISSUE_ONE),
            }),
        },
        NewProjectViewObject::Work {
            id: object_id(WORK_ONE),
            title: "Work one".to_owned(),
            description: "Handle requirement one".to_owned(),
            status: WorkStatus::Pending,
            priority: Priority::High,
            handles: ObjectRef {
                object_type: ProjectViewObjectType::Requirement,
                object_id: object_id(REQUIREMENT_ONE),
            },
        },
        NewProjectViewObject::Work {
            id: object_id(WORK_TWO),
            title: "Work two".to_owned(),
            description: "Handle issue one".to_owned(),
            status: WorkStatus::InProgress,
            priority: Priority::Urgent,
            handles: ObjectRef {
                object_type: ProjectViewObjectType::Issue,
                object_id: object_id(ISSUE_ONE),
            },
        },
        NewProjectViewObject::Role {
            id: object_id(ROLE_ONE),
            name: "Coordinator".to_owned(),
            purpose: "Keep the project coherent".to_owned(),
            responsibilities: vec!["Coordinate".to_owned()],
            boundaries: vec!["No authorization semantics".to_owned()],
            active: true,
        },
        NewProjectViewObject::Resource {
            id: object_id(RESOURCE_ONE),
            name: "Repository".to_owned(),
            resource_type: ResourceType::Repository,
            locator: ResourceLocator {
                locator_type: LocatorType::Url,
                value: "https://example.com/project.git".to_owned(),
            },
            description: "Canonical source repository".to_owned(),
        },
    ] {
        apply_request(
            &mut state,
            MutationRequest::Create(CreateMutation { object }),
        );
    }

    state
}

fn active_object(state: &ProjectViewState, id: Uuid) -> &ProjectViewObject {
    match state.entry(id) {
        Some(ProjectViewEntry::Active(object)) => object,
        Some(ProjectViewEntry::Tombstone(_)) => panic!("fixture object {id} was tombstoned"),
        None => panic!("fixture object {id} is missing"),
    }
}

fn choose_other<T: Copy + Eq>(options: &[T], current: T, choice: u8) -> T {
    let choices: Vec<T> = options
        .iter()
        .copied()
        .filter(|candidate| *candidate != current)
        .collect();
    choices[usize::from(choice) % choices.len()]
}

fn optional_patch<T>(value: Option<T>) -> Patch<T> {
    match value {
        Some(value) => Patch::Set(value),
        None => Patch::Clear,
    }
}

fn generated_mutation(state: &ProjectViewState, seed: StepSeed, step_index: usize) -> Mutation {
    let request = match seed.operation % 8 {
        0 => {
            let current = active_object(state, object_id(PLAN_ONE))
                .relations
                .under_goal_id;
            let next = choose_other(
                &[None, Some(object_id(GOAL_ONE)), Some(object_id(GOAL_TWO))],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Plan {
                object_id: object_id(PLAN_ONE),
                patch: PlanPatch {
                    under_goal_id: optional_patch(next),
                    ..PlanPatch::default()
                },
            })
        }
        1 => {
            let current = active_object(state, object_id(STAGE_ONE))
                .relations
                .under_plan_id;
            let next = choose_other(
                &[Some(object_id(PLAN_ONE)), Some(object_id(PLAN_TWO))],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Stage {
                object_id: object_id(STAGE_ONE),
                patch: StagePatch {
                    under_plan_id: optional_patch(next),
                    ..StagePatch::default()
                },
            })
        }
        2 => {
            let current = active_object(state, object_id(REQUIREMENT_ONE))
                .relations
                .planned_in_stage_id;
            let next = choose_other(
                &[None, Some(object_id(STAGE_ONE)), Some(object_id(STAGE_TWO))],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Requirement {
                object_id: object_id(REQUIREMENT_ONE),
                patch: RequirementPatch {
                    planned_in_stage_id: optional_patch(next),
                    ..RequirementPatch::default()
                },
            })
        }
        3 => {
            let current = active_object(state, object_id(ISSUE_ONE))
                .relations
                .planned_in_stage_id;
            let next = choose_other(
                &[None, Some(object_id(STAGE_ONE)), Some(object_id(STAGE_TWO))],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Issue {
                object_id: object_id(ISSUE_ONE),
                patch: IssuePatch {
                    planned_in_stage_id: optional_patch(next),
                    ..IssuePatch::default()
                },
            })
        }
        4 => {
            let current = active_object(state, object_id(ISSUE_ONE)).relations.about;
            let next = choose_other(
                &[
                    None,
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Requirement,
                        object_id: object_id(REQUIREMENT_ONE),
                    }),
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Issue,
                        object_id: object_id(ISSUE_TWO),
                    }),
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Plan,
                        object_id: object_id(PLAN_ONE),
                    }),
                ],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Issue {
                object_id: object_id(ISSUE_ONE),
                patch: IssuePatch {
                    about: optional_patch(next),
                    ..IssuePatch::default()
                },
            })
        }
        5 => {
            let current = active_object(state, object_id(WORK_ONE)).relations.handles;
            let next = choose_other(
                &[
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Requirement,
                        object_id: object_id(REQUIREMENT_ONE),
                    }),
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Requirement,
                        object_id: object_id(REQUIREMENT_TWO),
                    }),
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Issue,
                        object_id: object_id(ISSUE_ONE),
                    }),
                    Some(ObjectRef {
                        object_type: ProjectViewObjectType::Issue,
                        object_id: object_id(ISSUE_TWO),
                    }),
                ],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Work {
                object_id: object_id(WORK_ONE),
                patch: WorkPatch {
                    handles: optional_patch(next),
                    ..WorkPatch::default()
                },
            })
        }
        6 => {
            let current = match &active_object(state, object_id(PLAN_TWO)).data {
                ProjectViewObjectData::Plan(plan) => plan.status,
                _ => panic!("plan fixture must carry plan data"),
            };
            let next = choose_other(
                &[
                    PlanStatus::Draft,
                    PlanStatus::Active,
                    PlanStatus::Paused,
                    PlanStatus::Completed,
                    PlanStatus::Cancelled,
                ],
                current,
                seed.choice,
            );
            MutationRequest::Update(UpdateMutation::Plan {
                object_id: object_id(PLAN_TWO),
                patch: PlanPatch {
                    status: Patch::Set(next),
                    ..PlanPatch::default()
                },
            })
        }
        _ => {
            let active_role = state
                .active_objects()
                .find(|object| object.object_type == ProjectViewObjectType::Role)
                .map(|object| object.id);
            match (seed.choice % 2, active_role) {
                (1, Some(role_id)) => MutationRequest::Delete(DeleteMutation {
                    object_type: ProjectViewObjectType::Role,
                    object_id: role_id,
                }),
                _ => MutationRequest::Create(CreateMutation {
                    object: NewProjectViewObject::Role {
                        id: object_id(1_000 + step_index as u128),
                        name: format!("Role {step_index}"),
                        purpose: "Generated semantic role".to_owned(),
                        responsibilities: vec![],
                        boundaries: vec![],
                        active: !seed.choice.is_multiple_of(3),
                    },
                }),
            }
        }
    };
    Mutation::new(state.project_revision(), request)
}

fn run_sequence(steps: &[StepSeed]) -> ProjectViewState {
    let mut state = baseline_state();
    for (index, step) in steps.iter().copied().enumerate() {
        let before_revision = state.project_revision();
        let mutation = generated_mutation(&state, step, index);
        let outcome = must(
            state.apply(
                &mutation,
                actor(),
                canonical_time(before_revision.saturating_add(1)),
            ),
            "generated legal mutation must apply",
        );
        assert_eq!(state.project_revision(), before_revision + 1);
        assert_eq!(outcome.project_revision, state.project_revision());
        must(state.validate(), "state must remain valid after every step");
        must(
            ProjectView::assemble(&state),
            "read model must assemble after every step",
        );
        assert!(core_invariants_hold(&state));
    }
    state
}

fn active_target_type(state: &ProjectViewState, id: Uuid) -> Option<ProjectViewObjectType> {
    match state.entry(id) {
        Some(ProjectViewEntry::Active(object)) => Some(object.object_type),
        Some(ProjectViewEntry::Tombstone(_)) | None => None,
    }
}

fn core_invariants_hold(state: &ProjectViewState) -> bool {
    if !state.is_initialized() || state.project_revision() == 0 {
        return false;
    }

    let active: Vec<&ProjectViewObject> = state.active_objects().collect();
    let profiles: Vec<&ProjectViewObject> = active
        .iter()
        .copied()
        .filter(|object| object.object_type == ProjectViewObjectType::ProjectProfile)
        .collect();
    if profiles.len() != 1
        || profiles[0].id != *state.project_id().as_uuid()
        || !active
            .iter()
            .any(|object| object.object_type == ProjectViewObjectType::Goal)
    {
        return false;
    }

    for object in active {
        if object.object_type != object.data.object_type()
            || object.object_revision == 0
            || object.project_revision > state.project_revision()
        {
            return false;
        }

        let shape_is_valid = match object.object_type {
            ProjectViewObjectType::ProjectProfile
            | ProjectViewObjectType::Goal
            | ProjectViewObjectType::Role
            | ProjectViewObjectType::Resource => object.relations.is_empty(),
            ProjectViewObjectType::Plan => {
                object.relations.under_plan_id.is_none()
                    && object.relations.planned_in_stage_id.is_none()
                    && object.relations.about.is_none()
                    && object.relations.handles.is_none()
            }
            ProjectViewObjectType::Stage => {
                object.relations.under_goal_id.is_none()
                    && object.relations.under_plan_id.is_some()
                    && object.relations.planned_in_stage_id.is_none()
                    && object.relations.about.is_none()
                    && object.relations.handles.is_none()
            }
            ProjectViewObjectType::Requirement => {
                object.relations.under_goal_id.is_none()
                    && object.relations.under_plan_id.is_none()
                    && object.relations.about.is_none()
                    && object.relations.handles.is_none()
            }
            ProjectViewObjectType::Issue => {
                object.relations.under_goal_id.is_none()
                    && object.relations.under_plan_id.is_none()
                    && object.relations.handles.is_none()
            }
            ProjectViewObjectType::Work => {
                object.relations.under_goal_id.is_none()
                    && object.relations.under_plan_id.is_none()
                    && object.relations.planned_in_stage_id.is_none()
                    && object.relations.about.is_none()
                    && object.relations.handles.is_some()
            }
        };
        if !shape_is_valid {
            return false;
        }

        if object.relations.under_goal_id.is_some_and(|target| {
            active_target_type(state, target) != Some(ProjectViewObjectType::Goal)
        }) || object.relations.under_plan_id.is_some_and(|target| {
            active_target_type(state, target) != Some(ProjectViewObjectType::Plan)
        }) || object.relations.planned_in_stage_id.is_some_and(|target| {
            active_target_type(state, target) != Some(ProjectViewObjectType::Stage)
        }) {
            return false;
        }

        if let Some(target) = object.relations.about {
            if target.object_id == object.id
                || active_target_type(state, target.object_id) != Some(target.object_type)
            {
                return false;
            }
        }
        if let Some(target) = object.relations.handles {
            if !matches!(
                target.object_type,
                ProjectViewObjectType::Requirement | ProjectViewObjectType::Issue
            ) || active_target_type(state, target.object_id) != Some(target.object_type)
            {
                return false;
            }
        }
    }
    true
}

fn invalid_mutation(state: &ProjectViewState, selector: u8) -> Mutation {
    let request = match selector % 7 {
        0 => MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Goal {
                id: object_id(GOAL_ONE),
                title: "Duplicate goal".to_owned(),
                desired_outcome: "Must fail".to_owned(),
                directions: vec![],
            },
        }),
        1 => MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Stage {
                id: object_id(9_001),
                title: "Orphan stage".to_owned(),
                description: "Missing plan target".to_owned(),
                status: StageStatus::Planned,
                under_plan_id: object_id(9_002),
            },
        }),
        2 => MutationRequest::Update(UpdateMutation::Issue {
            object_id: object_id(ISSUE_ONE),
            patch: IssuePatch {
                about: Patch::Set(ObjectRef {
                    object_type: ProjectViewObjectType::Issue,
                    object_id: object_id(ISSUE_ONE),
                }),
                ..IssuePatch::default()
            },
        }),
        3 => MutationRequest::Update(UpdateMutation::Work {
            object_id: object_id(WORK_ONE),
            patch: WorkPatch {
                handles: Patch::Clear,
                ..WorkPatch::default()
            },
        }),
        4 => MutationRequest::Update(UpdateMutation::Plan {
            object_id: object_id(PLAN_ONE),
            patch: PlanPatch::default(),
        }),
        5 => MutationRequest::Delete(DeleteMutation {
            object_type: ProjectViewObjectType::ProjectProfile,
            object_id: *state.project_id().as_uuid(),
        }),
        _ => MutationRequest::Delete(DeleteMutation {
            object_type: ProjectViewObjectType::Goal,
            object_id: object_id(RESOURCE_ONE),
        }),
    };
    Mutation::new(state.project_revision(), request)
}

fn collect_plan_issue_ids(plan: &PlanView, ids: &mut Vec<Uuid>) {
    for stage in &plan.stages {
        ids.extend(stage.issues.iter().map(|issue| issue.issue.id));
    }
}

fn canonical_issue_ids(view: &ProjectView) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for goal in &view.goals {
        for plan in &goal.plans {
            collect_plan_issue_ids(plan, &mut ids);
        }
    }
    for plan in &view.unbound_plans {
        collect_plan_issue_ids(plan, &mut ids);
    }
    ids.extend(view.unplanned_issues.iter().map(|issue| issue.issue.id));
    ids
}

fn scramble_key(id: Uuid, seed: u64) -> u128 {
    let mut value = id.as_u128() ^ u128::from(seed);
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 29;
    value
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn legal_mutation_sequences_preserve_invariants_and_assemble(
        steps in sequence_strategy(32),
    ) {
        let state = run_sequence(&steps);
        prop_assert!(core_invariants_hold(&state));
        prop_assert!(state.validate().is_ok());
        prop_assert!(ProjectView::assemble(&state).is_ok());
    }

    #[test]
    fn invalid_mutations_leave_state_unchanged(
        steps in prop::collection::vec(step_strategy(), 0..12),
        selector in any::<u8>(),
    ) {
        let mut state = if steps.is_empty() {
            baseline_state()
        } else {
            run_sequence(&steps)
        };
        let before = state.clone();
        let mutation = invalid_mutation(&state, selector);
        let result = state.apply(
            &mutation,
            actor(),
            canonical_time(state.project_revision().saturating_add(1)),
        );

        prop_assert!(result.is_err(), "invalid mutation unexpectedly succeeded");
        prop_assert_eq!(&state, &before);
        prop_assert!(state.validate().is_ok());
        prop_assert!(ProjectView::assemble(&state).is_ok());
    }

    #[test]
    fn replaying_the_same_old_revision_mutation_leaves_state_unchanged(
        prefix in prop::collection::vec(step_strategy(), 0..12),
        next_step in step_strategy(),
    ) {
        let mut state = if prefix.is_empty() {
            baseline_state()
        } else {
            run_sequence(&prefix)
        };
        let mutation = generated_mutation(&state, next_step, prefix.len() + 100);
        let first_revision = state.project_revision().saturating_add(1);
        must(
            state.apply(&mutation, actor(), canonical_time(first_revision)),
            "first application must succeed",
        );
        let after_first = state.clone();

        let replay = state.apply(
            &mutation,
            actor(),
            canonical_time(state.project_revision().saturating_add(1)),
        );
        let replay_conflicted = matches!(replay, Err(DomainError::RevisionConflict { .. }));
        prop_assert!(replay_conflicted, "replay must fail with a revision conflict");
        prop_assert_eq!(&state, &after_first);
    }

    #[test]
    fn snapshot_input_order_does_not_change_state_or_read_model(
        steps in prop::collection::vec(step_strategy(), 0..24),
        order_seed in any::<u64>(),
    ) {
        let state = if steps.is_empty() {
            baseline_state()
        } else {
            run_sequence(&steps)
        };
        let expected_view = must(ProjectView::assemble(&state), "baseline view must assemble");
        let mut entries: Vec<ProjectViewEntry> = state.entries().values().cloned().collect();
        entries.sort_by_key(|entry| scramble_key(entry.id(), order_seed));

        let reconstructed = must(
            ProjectViewState::from_snapshot(
                state.project_id(),
                state.project_revision(),
                state.initialized_at(),
                state.updated_at(),
                entries,
            ),
            "permuted snapshot must reconstruct",
        );
        let reconstructed_view = must(
            ProjectView::assemble(&reconstructed),
            "reconstructed view must assemble",
        );

        prop_assert_eq!(reconstructed, state);
        prop_assert_eq!(reconstructed_view, expected_view);
    }

    #[test]
    fn issue_about_cycles_are_finite_and_each_issue_has_one_canonical_position(
        first_planned in any::<bool>(),
        second_planned in any::<bool>(),
    ) {
        let mut state = baseline_state();
        if !first_planned {
            apply_request(
                &mut state,
                MutationRequest::Update(UpdateMutation::Issue {
                    object_id: object_id(ISSUE_ONE),
                    patch: IssuePatch {
                        planned_in_stage_id: Patch::Clear,
                        ..IssuePatch::default()
                    },
                }),
            );
        }
        if second_planned {
            apply_request(
                &mut state,
                MutationRequest::Update(UpdateMutation::Issue {
                    object_id: object_id(ISSUE_TWO),
                    patch: IssuePatch {
                        planned_in_stage_id: Patch::Set(object_id(STAGE_TWO)),
                        ..IssuePatch::default()
                    },
                }),
            );
        }
        apply_request(
            &mut state,
            MutationRequest::Update(UpdateMutation::Issue {
                object_id: object_id(ISSUE_ONE),
                patch: IssuePatch {
                    about: Patch::Set(ObjectRef {
                        object_type: ProjectViewObjectType::Issue,
                        object_id: object_id(ISSUE_TWO),
                    }),
                    ..IssuePatch::default()
                },
            }),
        );

        let view = must(ProjectView::assemble(&state), "cyclic about graph must assemble");
        let issue_ids = canonical_issue_ids(&view);
        let unique_ids: BTreeSet<Uuid> = issue_ids.iter().copied().collect();
        let issue_two_reference = [ObjectRef {
            object_type: ProjectViewObjectType::Issue,
            object_id: object_id(ISSUE_TWO),
        }];
        let issue_one_reference = [ObjectRef {
            object_type: ProjectViewObjectType::Issue,
            object_id: object_id(ISSUE_ONE),
        }];

        prop_assert_eq!(issue_ids.len(), 2);
        prop_assert_eq!(
            unique_ids,
            BTreeSet::from([object_id(ISSUE_ONE), object_id(ISSUE_TWO)])
        );
        prop_assert_eq!(
            view.issue_references_by_target
                .get(&object_id(ISSUE_ONE))
                .map(Vec::as_slice),
            Some(issue_two_reference.as_slice())
        );
        prop_assert_eq!(
            view.issue_references_by_target
                .get(&object_id(ISSUE_TWO))
                .map(Vec::as_slice),
            Some(issue_one_reference.as_slice())
        );
    }

    #[test]
    fn applying_the_same_sequence_is_deterministic(
        steps in sequence_strategy(32),
    ) {
        let first = run_sequence(&steps);
        let second = run_sequence(&steps);
        let first_view = must(ProjectView::assemble(&first), "first view must assemble");
        let second_view = must(ProjectView::assemble(&second), "second view must assemble");

        prop_assert_eq!(first, second);
        prop_assert_eq!(first_view, second_view);
    }
}
