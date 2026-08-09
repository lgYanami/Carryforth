use buzz_core::{CommunityId, Keys, PublicKey};
use buzz_project_view::{
    CreateMutation, DeleteMutation, InitializeGoal, InitializeMutation, IssueStatus, LocatorType,
    Mutation, MutationOutcome, MutationRequest, NewProjectViewObject, ObjectRef, PlanStatus,
    Priority, ProjectProfile, ProjectView, ProjectViewEntry, ProjectViewObject,
    ProjectViewObjectType, ProjectViewState, RequirementStatus, ResourceLocator, ResourceType,
    StageStatus, UpdateMutation, WorkStatus,
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

pub const INITIAL_GOAL_SLOT: u128 = 1;

const TEST_SECRET_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const OBJECT_ID_BASE: u128 = 0x7076_0000_0000_4000_8000_0000_0000_0000;
const PROJECT_ID_BASE: u128 = 0xc011_0000_0000_0000_0000_0000_0000_0000;
const BASE_TIME_SECONDS: i64 = 1_735_689_600;

pub fn object_id(slot: u128) -> Uuid {
    let mut bytes = (OBJECT_ID_BASE.wrapping_add(slot)).to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

pub fn project_id(slot: u128) -> CommunityId {
    CommunityId::from_uuid(Uuid::from_u128(PROJECT_ID_BASE.wrapping_add(slot)))
}

pub fn actor() -> PublicKey {
    Keys::parse(TEST_SECRET_KEY)
        .expect("fixed test secret key must parse")
        .public_key()
}

pub fn at_tick(tick: i64) -> DateTime<Utc> {
    DateTime::from_timestamp(BASE_TIME_SECONDS + tick, 0)
        .expect("fixed Project View test timestamp must be valid")
}

pub fn object_ref(object_type: ProjectViewObjectType, object_id: Uuid) -> ObjectRef {
    ObjectRef {
        object_type,
        object_id,
    }
}

pub fn initialize_request(goal_ids: impl IntoIterator<Item = Uuid>) -> MutationRequest {
    MutationRequest::Initialize(InitializeMutation {
        profile: ProjectProfile {
            name: "Project View test project".to_owned(),
            positioning: "A deterministic domain-test fixture".to_owned(),
            purpose: "Exercise Project View invariants".to_owned(),
            problem: "Relationship regressions need precise failures".to_owned(),
            scope: "Pure in-memory domain behavior".to_owned(),
            summary: None,
        },
        goals: goal_ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| InitializeGoal {
                id,
                title: format!("Initial goal {index}"),
                desired_outcome: format!("Outcome {index} is observable"),
                directions: vec![format!("Direction {index}")],
                summary: None,
            })
            .collect(),
    })
}

#[derive(Debug)]
pub struct Fixture {
    pub state: ProjectViewState,
    next_tick: i64,
}

impl Fixture {
    pub fn uninitialized(project_slot: u128) -> Self {
        Self {
            state: ProjectViewState::new(project_id(project_slot)),
            next_tick: 0,
        }
    }

    pub fn initialized() -> Self {
        Self::initialized_for(1)
    }

    pub fn initialized_for(project_slot: u128) -> Self {
        let mut fixture = Self::uninitialized(project_slot);
        fixture.apply(initialize_request([object_id(INITIAL_GOAL_SLOT)]));
        fixture
    }

    pub fn profile_id(&self) -> Uuid {
        *self.state.project_id().as_uuid()
    }

    pub fn initial_goal_id(&self) -> Uuid {
        object_id(INITIAL_GOAL_SLOT)
    }

    pub fn apply(&mut self, request: MutationRequest) -> MutationOutcome {
        let mutation = Mutation::new(self.state.project_revision(), request);
        let now = self.take_time();
        self.state
            .apply(&mutation, actor(), now)
            .expect("fixture mutation must be accepted")
    }

    pub fn reject_unchanged(
        &mut self,
        request: MutationRequest,
        expected_code: &'static str,
    ) -> buzz_project_view::DomainError {
        let before = self.state.clone();
        let mutation = Mutation::new(self.state.project_revision(), request);
        let now = self.take_time();
        let error = self
            .state
            .apply(&mutation, actor(), now)
            .expect_err("fixture mutation must be rejected");
        assert_eq!(
            error.code(),
            expected_code,
            "unexpected domain error: {error}"
        );
        assert_eq!(
            self.state, before,
            "rejected mutation changed canonical Project View state"
        );
        error
    }

    pub fn create(&mut self, object: NewProjectViewObject) -> Uuid {
        let object_id = object.id();
        self.apply(MutationRequest::Create(CreateMutation { object }));
        object_id
    }

    pub fn update(&mut self, update: UpdateMutation) {
        self.apply(MutationRequest::Update(update));
    }

    pub fn delete(&mut self, object_type: ProjectViewObjectType, object_id: Uuid) {
        self.apply(MutationRequest::Delete(DeleteMutation {
            object_type,
            object_id,
        }));
    }

    pub fn reject_delete_unchanged(
        &mut self,
        object_type: ProjectViewObjectType,
        object_id: Uuid,
        expected_code: &'static str,
    ) -> buzz_project_view::DomainError {
        self.reject_unchanged(
            MutationRequest::Delete(DeleteMutation {
                object_type,
                object_id,
            }),
            expected_code,
        )
    }

    pub fn view(&self) -> ProjectView {
        ProjectView::assemble(&self.state).expect("fixture state must assemble")
    }

    pub fn object(&self, object_id: Uuid) -> &ProjectViewObject {
        match self.state.entry(object_id) {
            Some(ProjectViewEntry::Active(object)) => object,
            Some(ProjectViewEntry::Tombstone(_)) => {
                panic!("expected active object {object_id}, found tombstone")
            }
            None => panic!("expected active object {object_id}, found no entry"),
        }
    }

    pub fn active_count(&self, object_type: ProjectViewObjectType) -> usize {
        self.state
            .active_objects()
            .filter(|object| object.object_type == object_type)
            .count()
    }

    fn take_time(&mut self) -> DateTime<Utc> {
        let now = at_tick(self.next_tick);
        self.next_tick += 1;
        now
    }
}

pub fn new_goal(id: Uuid) -> NewProjectViewObject {
    NewProjectViewObject::Goal {
        id,
        title: format!("Goal {id}"),
        desired_outcome: "The goal's outcome is observable".to_owned(),
        directions: vec!["Advance deliberately".to_owned()],
    }
}

pub fn new_role(id: Uuid) -> NewProjectViewObject {
    NewProjectViewObject::Role {
        id,
        name: format!("Role {id}"),
        purpose: "Own a stable semantic responsibility".to_owned(),
        responsibilities: vec!["Keep the domain coherent".to_owned()],
        boundaries: vec!["Does not grant relay permissions".to_owned()],
        active: true,
    }
}

pub fn new_plan(id: Uuid, under_goal_id: Option<Uuid>) -> NewProjectViewObject {
    NewProjectViewObject::Plan {
        id,
        title: format!("Plan {id}"),
        description: "A deterministic test plan".to_owned(),
        status: PlanStatus::Active,
        under_goal_id,
    }
}

pub fn new_stage(id: Uuid, under_plan_id: Uuid) -> NewProjectViewObject {
    NewProjectViewObject::Stage {
        id,
        title: format!("Stage {id}"),
        description: "A deterministic test stage".to_owned(),
        status: StageStatus::Active,
        under_plan_id,
    }
}

pub fn new_requirement(id: Uuid, planned_in_stage_id: Option<Uuid>) -> NewProjectViewObject {
    NewProjectViewObject::Requirement {
        id,
        title: format!("Requirement {id}"),
        description: "A deterministic test requirement".to_owned(),
        status: RequirementStatus::InProgress,
        priority: Priority::High,
        planned_in_stage_id,
    }
}

pub fn new_issue(
    id: Uuid,
    planned_in_stage_id: Option<Uuid>,
    about: Option<ObjectRef>,
) -> NewProjectViewObject {
    NewProjectViewObject::Issue {
        id,
        title: format!("Issue {id}"),
        description: "A deterministic test issue".to_owned(),
        status: IssueStatus::Open,
        priority: Priority::Urgent,
        planned_in_stage_id,
        about,
    }
}

pub fn new_work(id: Uuid, handles: ObjectRef) -> NewProjectViewObject {
    NewProjectViewObject::Work {
        id,
        title: format!("Work {id}"),
        description: "A deterministic test work item".to_owned(),
        status: WorkStatus::Pending,
        priority: Priority::Normal,
        handles,
    }
}

pub fn new_resource(id: Uuid) -> NewProjectViewObject {
    NewProjectViewObject::Resource {
        id,
        name: format!("Resource {id}"),
        resource_type: ResourceType::Repository,
        locator: ResourceLocator {
            locator_type: LocatorType::Url,
            value: format!("https://example.test/resources/{id}"),
        },
        description: "A deterministic test resource".to_owned(),
    }
}
