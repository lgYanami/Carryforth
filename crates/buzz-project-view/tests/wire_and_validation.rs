use buzz_core::{CommunityId, Keys, PublicKey};
use buzz_project_view::{
    CreateMutation, DeleteMutation, DomainError, GoalPatch, InitializeGoal, InitializeMutation,
    LocatorType, Mutation, MutationRequest, NewProjectViewObject, Patch, PlanPatch,
    ProjectViewEntry, ProjectViewObject, ProjectViewObjectData, ProjectViewObjectType,
    ProjectViewState, ResourceLocator, ResourceType, RolePatch, UpdateMutation,
    MAX_MUTATION_CONTENT_BYTES, MAX_MUTATION_JSON_DEPTH, MAX_SAFE_REVISION,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

fn uuid_v4(value: u128) -> Uuid {
    let mut bytes = value.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn project_id() -> CommunityId {
    CommunityId::from_uuid(uuid_v4(0xfeed))
}

fn actor() -> PublicKey {
    Keys::generate().public_key()
}

fn at(offset_seconds: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(1_700_000_000 + offset_seconds, 0)
        .expect("fixed test timestamp must be valid")
}

fn initial_goal_id() -> Uuid {
    uuid_v4(1)
}

fn initialize_mutation() -> Mutation {
    Mutation::new(
        0,
        MutationRequest::Initialize(InitializeMutation {
            profile: buzz_project_view::ProjectProfile {
                name: "Buzz".to_owned(),
                positioning: "A shared project surface".to_owned(),
                purpose: "Coordinate humans and agents".to_owned(),
                problem: "Project state is fragmented".to_owned(),
                scope: "Project View v0".to_owned(),
                summary: None,
            },
            goals: vec![InitializeGoal {
                id: initial_goal_id(),
                title: "Ship Project View".to_owned(),
                desired_outcome: "Members share one current view".to_owned(),
                directions: vec!["Build the domain first".to_owned()],
                summary: None,
            }],
        }),
    )
}

fn initialized_state() -> ProjectViewState {
    let mut state = ProjectViewState::new(project_id());
    state
        .apply(&initialize_mutation(), actor(), at(0))
        .expect("valid initialization must succeed");
    state
}

fn apply_fails_unchanged(
    state: &mut ProjectViewState,
    mutation: &Mutation,
    expected_code: &str,
    offset_seconds: i64,
) -> DomainError {
    let before = state.clone();
    let error = state
        .apply(mutation, actor(), at(offset_seconds))
        .expect_err("mutation must fail");
    assert_eq!(error.code(), expected_code);
    assert_eq!(*state, before, "failed mutation changed canonical state");
    error
}

fn active_object(state: &ProjectViewState, object_id: Uuid) -> &ProjectViewObject {
    match state
        .entry(object_id)
        .expect("test object must occupy its ID")
    {
        ProjectViewEntry::Active(object) => object,
        ProjectViewEntry::Tombstone(_) => panic!("test object must still be active"),
    }
}

fn assert_exact_round_trip(value: Value) {
    let mutation: Mutation =
        serde_json::from_value(value.clone()).expect("schema v1 example must parse");
    assert_eq!(
        serde_json::to_value(mutation).expect("mutation must serialize"),
        value
    );
}

#[test]
fn schema_v1_examples_round_trip_with_the_exact_wire_shape() {
    let goal_id = initial_goal_id().to_string();
    let plan_id = uuid_v4(2).to_string();
    let issue_id = uuid_v4(3).to_string();
    let stage_id = uuid_v4(4).to_string();
    let resource_id = uuid_v4(5).to_string();

    assert_exact_round_trip(json!({
        "schema_version": 1,
        "expected_project_revision": 0,
        "request": {
            "type": "initialize",
            "profile": {
                "name": "Buzz",
                "positioning": "A shared project surface",
                "purpose": "Coordinate humans and agents",
                "problem": "Project state is fragmented",
                "scope": "Project View v0"
            },
            "goals": [{
                "id": goal_id,
                "title": "Ship Project View",
                "desired_outcome": "Members share one current view",
                "directions": []
            }]
        }
    }));

    assert_exact_round_trip(json!({
        "schema_version": 1,
        "expected_project_revision": 1,
        "request": {
            "type": "create",
            "object": {
                "object_type": "plan",
                "id": plan_id,
                "title": "MVP",
                "description": "Deliver the first useful slice",
                "status": "active",
                "under_goal_id": null
            }
        }
    }));

    assert_exact_round_trip(json!({
        "schema_version": 1,
        "expected_project_revision": 12,
        "request": {
            "type": "update",
            "object_type": "issue",
            "object_id": issue_id,
            "patch": {
                "status": "in_progress",
                "planned_in_stage_id": stage_id
            }
        }
    }));

    assert_exact_round_trip(json!({
        "schema_version": 1,
        "expected_project_revision": 13,
        "request": {
            "type": "delete",
            "object_type": "resource",
            "object_id": resource_id
        }
    }));
}

#[test]
fn closed_mutation_schema_rejects_unknown_fields_and_known_wrong_types() {
    let goal_id = initial_goal_id().to_string();
    let invalid_values = [
        json!({
            "schema_version": 1,
            "expected_project_revision": 0,
            "future": true,
            "request": {
                "type": "initialize",
                "profile": {
                    "name": "Buzz",
                    "positioning": "positioning",
                    "purpose": "purpose",
                    "problem": "problem",
                    "scope": "scope"
                },
                "goals": [{
                    "id": goal_id,
                    "title": "Goal",
                    "desired_outcome": "Outcome",
                    "directions": []
                }]
            }
        }),
        json!({
            "schema_version": "1",
            "expected_project_revision": 0,
            "request": {"type": "delete", "object_type": "goal", "object_id": goal_id}
        }),
        json!({
            "schema_version": 1,
            "expected_project_revision": 1,
            "request": {
                "type": "create",
                "object": {
                    "object_type": "plan",
                    "id": uuid_v4(10),
                    "title": null,
                    "description": "description",
                    "status": "active",
                    "under_goal_id": null
                }
            }
        }),
        json!({
            "schema_version": 1,
            "expected_project_revision": 1,
            "request": {
                "type": "update",
                "object_type": "goal",
                "object_id": goal_id,
                "patch": {"title": "new", "typo": true}
            }
        }),
        json!({
            "schema_version": 1,
            "expected_project_revision": 1,
            "request": {
                "type": "update",
                "object_type": "plan",
                "object_id": uuid_v4(10),
                "patch": {"status": 7}
            }
        }),
    ];

    for value in invalid_values {
        assert!(
            serde_json::from_value::<Mutation>(value).is_err(),
            "invalid wire input was accepted"
        );
    }
}

#[test]
fn nullable_patch_fields_preserve_missing_null_and_value_semantics() {
    let plan_id = uuid_v4(20);
    let goal_id = initial_goal_id();
    let mut state = initialized_state();
    let create = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Plan {
                id: plan_id,
                title: "MVP".to_owned(),
                description: "First slice".to_owned(),
                status: buzz_project_view::PlanStatus::Active,
                under_goal_id: Some(goal_id),
            },
        }),
    );
    state
        .apply(&create, actor(), at(1))
        .expect("plan creation must succeed");

    let missing_json = json!({
        "schema_version": 1,
        "expected_project_revision": state.project_revision(),
        "request": {
            "type": "update",
            "object_type": "plan",
            "object_id": plan_id,
            "patch": {"description": "Still attached"}
        }
    });
    let missing: Mutation =
        serde_json::from_value(missing_json.clone()).expect("missing nullable field must parse");
    match &missing.request {
        MutationRequest::Update(UpdateMutation::Plan { patch, .. }) => {
            assert_eq!(patch.under_goal_id, Patch::Unchanged);
        }
        other => panic!("unexpected mutation: {other:?}"),
    }
    assert_eq!(
        serde_json::to_value(&missing).expect("missing patch must serialize"),
        missing_json
    );
    state
        .apply(&missing, actor(), at(2))
        .expect("unrelated patch must succeed");
    assert_eq!(
        active_object(&state, plan_id).relations.under_goal_id,
        Some(goal_id)
    );

    let clear: Mutation = serde_json::from_value(json!({
        "schema_version": 1,
        "expected_project_revision": state.project_revision(),
        "request": {
            "type": "update",
            "object_type": "plan",
            "object_id": plan_id,
            "patch": {"under_goal_id": null}
        }
    }))
    .expect("null nullable field must parse");
    match &clear.request {
        MutationRequest::Update(UpdateMutation::Plan { patch, .. }) => {
            assert_eq!(patch.under_goal_id, Patch::Clear);
        }
        other => panic!("unexpected mutation: {other:?}"),
    }
    state
        .apply(&clear, actor(), at(3))
        .expect("explicit relation clear must succeed");
    assert_eq!(active_object(&state, plan_id).relations.under_goal_id, None);

    let set: Mutation = serde_json::from_value(json!({
        "schema_version": 1,
        "expected_project_revision": state.project_revision(),
        "request": {
            "type": "update",
            "object_type": "plan",
            "object_id": plan_id,
            "patch": {"under_goal_id": goal_id}
        }
    }))
    .expect("nullable value must parse");
    match &set.request {
        MutationRequest::Update(UpdateMutation::Plan { patch, .. }) => {
            assert_eq!(patch.under_goal_id, Patch::Set(goal_id));
        }
        other => panic!("unexpected mutation: {other:?}"),
    }
    state
        .apply(&set, actor(), at(4))
        .expect("explicit relation set must succeed");
    assert_eq!(
        active_object(&state, plan_id).relations.under_goal_id,
        Some(goal_id)
    );
}

#[test]
fn field_list_and_locator_limits_fail_without_changing_state() {
    let mut state = initialized_state();

    let too_many_bytes = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Goal {
                id: uuid_v4(30),
                title: "界".repeat(86),
                desired_outcome: "Outcome".to_owned(),
                directions: Vec::new(),
            },
        }),
    );
    let error = apply_fails_unchanged(&mut state, &too_many_bytes, "field_too_long", 1);
    assert!(matches!(
        error,
        DomainError::FieldTooLong {
            field: "title",
            max: 256,
            actual: 258
        }
    ));

    let empty_required = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Goal {
                id: uuid_v4(31),
                title: " \t ".to_owned(),
                desired_outcome: "Outcome".to_owned(),
                directions: Vec::new(),
            },
        }),
    );
    let error = apply_fails_unchanged(&mut state, &empty_required, "required_field", 2);
    assert!(matches!(
        error,
        DomainError::RequiredField { field: "title" }
    ));

    let too_many_list_items = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Goal {
                id: uuid_v4(32),
                title: "Goal".to_owned(),
                desired_outcome: "Outcome".to_owned(),
                directions: vec!["direction".to_owned(); 65],
            },
        }),
    );
    let error = apply_fails_unchanged(&mut state, &too_many_list_items, "too_many_items", 3);
    assert!(matches!(
        error,
        DomainError::TooManyItems {
            field: "directions",
            max: 64,
            actual: 65
        }
    ));

    let url_with_userinfo = create_resource(
        &state,
        uuid_v4(33),
        LocatorType::Url,
        "https://user:secret@example.com/project",
    );
    let error = apply_fails_unchanged(&mut state, &url_with_userinfo, "invalid_locator", 4);
    assert!(matches!(error, DomainError::InvalidLocator { .. }));

    let locator_with_control = create_resource(
        &state,
        uuid_v4(34),
        LocatorType::NostrAddress,
        "30617:abc:\nrepo",
    );
    let error = apply_fails_unchanged(&mut state, &locator_with_control, "invalid_locator", 5);
    assert!(matches!(error, DomainError::InvalidLocator { .. }));
}

fn create_resource(
    state: &ProjectViewState,
    id: Uuid,
    locator_type: LocatorType,
    locator: &str,
) -> Mutation {
    Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Resource {
                id,
                name: "Resource".to_owned(),
                resource_type: ResourceType::Url,
                locator: ResourceLocator {
                    locator_type,
                    value: locator.to_owned(),
                },
                description: "A resource".to_owned(),
            },
        }),
    )
}

#[test]
fn object_id_guards_and_tombstones_prevent_reuse_atomically() {
    let mut state = initialized_state();

    let invalid_uuid = create_role(&state, Uuid::from_u128(9));
    let error = apply_fails_unchanged(&mut state, &invalid_uuid, "invalid_object_id", 1);
    assert!(matches!(error, DomainError::InvalidObjectId { .. }));

    let reserved_id = *state.project_id().as_uuid();
    let reserved = create_role(&state, reserved_id);
    let error = apply_fails_unchanged(&mut state, &reserved, "reserved_profile_id", 2);
    assert!(matches!(error, DomainError::ReservedProfileId { .. }));

    let role_id = uuid_v4(40);
    state
        .apply(&create_role(&state, role_id), actor(), at(3))
        .expect("role creation must succeed");
    let delete = Mutation::new(
        state.project_revision(),
        MutationRequest::Delete(DeleteMutation {
            object_type: ProjectViewObjectType::Role,
            object_id: role_id,
        }),
    );
    state
        .apply(&delete, actor(), at(4))
        .expect("unreferenced role deletion must succeed");
    assert!(matches!(
        state.entry(role_id),
        Some(ProjectViewEntry::Tombstone(_))
    ));

    let reuse = create_role(&state, role_id);
    let error = apply_fails_unchanged(&mut state, &reuse, "object_id_used", 5);
    assert!(matches!(error, DomainError::ObjectIdAlreadyUsed { .. }));
}

fn create_role(state: &ProjectViewState, id: Uuid) -> Mutation {
    Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Role {
                id,
                name: "Maintainer".to_owned(),
                purpose: "Keep the system coherent".to_owned(),
                responsibilities: Vec::new(),
                boundaries: Vec::new(),
                active: true,
            },
        }),
    )
}

#[test]
fn semantic_null_noop_and_revision_limits_fail_without_changing_state() {
    let mut state = initialized_state();
    let goal_id = initial_goal_id();

    let clear_required: Mutation = serde_json::from_value(json!({
        "schema_version": 1,
        "expected_project_revision": state.project_revision(),
        "request": {
            "type": "update",
            "object_type": "goal",
            "object_id": goal_id,
            "patch": {"title": null}
        }
    }))
    .expect("explicit null has valid patch syntax");
    let error = apply_fails_unchanged(&mut state, &clear_required, "required_field", 1);
    assert!(matches!(
        error,
        DomainError::RequiredField { field: "title" }
    ));

    let no_op = Mutation::new(
        state.project_revision(),
        MutationRequest::Update(UpdateMutation::Goal {
            object_id: goal_id,
            patch: GoalPatch::default(),
        }),
    );
    let error = apply_fails_unchanged(&mut state, &no_op, "no_changes", 2);
    assert_eq!(error, DomainError::NoChanges);

    let over_safe_limit = Mutation::new(
        MAX_SAFE_REVISION + 1,
        MutationRequest::Update(UpdateMutation::Goal {
            object_id: goal_id,
            patch: GoalPatch {
                title: Patch::Set("A changed goal".to_owned()),
                ..GoalPatch::default()
            },
        }),
    );
    let error = apply_fails_unchanged(&mut state, &over_safe_limit, "revision_out_of_range", 3);
    assert!(matches!(
        error,
        DomainError::RevisionOutOfRange {
            revision,
            max: MAX_SAFE_REVISION
        } if revision == MAX_SAFE_REVISION + 1
    ));

    let mut exhausted = ProjectViewState::from_snapshot(
        state.project_id(),
        MAX_SAFE_REVISION,
        state.initialized_at(),
        state.updated_at(),
        state.entries().values().cloned(),
    )
    .expect("maximum safe revision is a valid snapshot revision");
    let at_limit = create_role(&exhausted, uuid_v4(50));
    let error = apply_fails_unchanged(&mut exhausted, &at_limit, "revision_exhausted", 4);
    assert_eq!(error, DomainError::RevisionExhausted);
}

#[test]
fn non_nullable_relation_null_is_rejected_by_the_reducer() {
    let mut state = initialized_state();
    let plan_id = uuid_v4(60);
    let stage_id = uuid_v4(61);
    let plan = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Plan {
                id: plan_id,
                title: "Plan".to_owned(),
                description: "Plan description".to_owned(),
                status: buzz_project_view::PlanStatus::Active,
                under_goal_id: None,
            },
        }),
    );
    state
        .apply(&plan, actor(), at(1))
        .expect("plan creation must succeed");
    let stage = Mutation::new(
        state.project_revision(),
        MutationRequest::Create(CreateMutation {
            object: NewProjectViewObject::Stage {
                id: stage_id,
                title: "Stage".to_owned(),
                description: "Stage description".to_owned(),
                status: buzz_project_view::StageStatus::Active,
                under_plan_id: plan_id,
            },
        }),
    );
    state
        .apply(&stage, actor(), at(2))
        .expect("stage creation must succeed");

    let clear_parent: Mutation = serde_json::from_value(json!({
        "schema_version": 1,
        "expected_project_revision": state.project_revision(),
        "request": {
            "type": "update",
            "object_type": "stage",
            "object_id": stage_id,
            "patch": {"under_plan_id": null}
        }
    }))
    .expect("explicit null has valid patch syntax");
    let error = apply_fails_unchanged(&mut state, &clear_parent, "missing_relation", 3);
    assert!(matches!(
        error,
        DomainError::MissingRequiredRelation {
            relation: "under_plan_id"
        }
    ));
}

#[test]
fn serialized_patch_omits_missing_fields_but_preserves_explicit_null() {
    let missing = PlanPatch::default();
    assert_eq!(
        serde_json::to_value(missing).expect("patch must serialize"),
        json!({})
    );

    let clear = PlanPatch {
        under_goal_id: Patch::Clear,
        ..PlanPatch::default()
    };
    assert_eq!(
        serde_json::to_value(clear).expect("patch must serialize"),
        json!({"under_goal_id": null})
    );

    let role_patch = RolePatch {
        active: Patch::Set(false),
        ..RolePatch::default()
    };
    assert_eq!(
        serde_json::to_value(role_patch).expect("patch must serialize"),
        json!({"active": false})
    );

    let state = initialized_state();
    let active = active_object(&state, initial_goal_id());
    assert!(matches!(active.data, ProjectViewObjectData::Goal(_)));
}

#[test]
fn bounded_json_parser_enforces_content_depth_and_closed_schema() {
    let mutation = initialize_mutation();
    let content = serde_json::to_string(&mutation).expect("mutation must serialize");
    assert_eq!(
        Mutation::from_json(&content).expect("valid bounded content must parse"),
        mutation
    );

    let oversized = " ".repeat(MAX_MUTATION_CONTENT_BYTES + 1);
    assert!(matches!(
        Mutation::from_json(&oversized),
        Err(DomainError::MutationContentTooLarge {
            max: MAX_MUTATION_CONTENT_BYTES,
            actual,
        }) if actual == MAX_MUTATION_CONTENT_BYTES + 1
    ));

    let too_deep = format!(
        "{}0{}",
        "[".repeat(MAX_MUTATION_JSON_DEPTH),
        "]".repeat(MAX_MUTATION_JSON_DEPTH)
    );
    assert!(matches!(
        Mutation::from_json(&too_deep),
        Err(DomainError::MutationJsonTooDeep {
            max: MAX_MUTATION_JSON_DEPTH,
            actual,
        }) if actual == MAX_MUTATION_JSON_DEPTH + 1
    ));

    let mut unknown = serde_json::to_value(mutation).expect("mutation must serialize");
    unknown["unknown"] = json!(true);
    let unknown = serde_json::to_string(&unknown).expect("JSON value must serialize");
    assert!(matches!(
        Mutation::from_json(&unknown),
        Err(DomainError::InvalidMutationJson { .. })
    ));

    let mut unsupported =
        serde_json::to_value(initialize_mutation()).expect("mutation must serialize");
    unsupported["schema_version"] = json!(2);
    assert!(matches!(
        Mutation::from_json(
            &serde_json::to_string(&unsupported).expect("JSON value must serialize")
        ),
        Err(DomainError::UnsupportedSchemaVersion {
            got: 2,
            supported: 1,
        })
    ));

    let mut unsafe_revision =
        serde_json::to_value(initialize_mutation()).expect("mutation must serialize");
    unsafe_revision["expected_project_revision"] = json!(MAX_SAFE_REVISION + 1);
    assert!(matches!(
        Mutation::from_json(
            &serde_json::to_string(&unsafe_revision).expect("JSON value must serialize")
        ),
        Err(DomainError::RevisionOutOfRange {
            revision,
            max: MAX_SAFE_REVISION,
        }) if revision == MAX_SAFE_REVISION + 1
    ));
}
