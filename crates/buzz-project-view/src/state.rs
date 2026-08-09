//! Pure in-memory Project View state and atomic mutation reducer.

use std::collections::BTreeMap;

use buzz_core::{CommunityId, PublicKey};
use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::mutation::{
    DeleteMutation, InitializeMutation, MutationRequest, NewProjectViewObject, UpdateMutation,
    MUTATION_SCHEMA_VERSION,
};
use crate::validation::{validate_client_object_id, validate_data, validate_state};
use crate::{
    DomainError, DomainResult, Mutation, MutationOutcome, Patch, ProjectViewObject,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, MAX_INITIAL_GOALS,
    MAX_SAFE_REVISION,
};

/// Minimal canonical record retained after an object is deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewTombstone {
    /// Stable object identifier, permanently reserved after deletion.
    pub id: Uuid,
    /// Immutable canonical object type.
    pub object_type: ProjectViewObjectType,
    /// Object revision assigned to the deletion.
    pub object_revision: u64,
    /// Project revision assigned to the deletion.
    pub project_revision: u64,
    /// Canonical creation time of the former object.
    pub created_at: DateTime<Utc>,
    /// Canonical deletion time supplied by the relay.
    pub deleted_at: DateTime<Utc>,
    /// Verified actor that created the former object.
    pub created_by: PublicKey,
    /// Verified actor that deleted the object.
    pub deleted_by: PublicKey,
}

/// One occupied object ID in canonical Project View state.
// Keep the established public legacy state shape; boxing this variant would be
// a separate API migration unrelated to the additive summary field.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectViewEntry {
    /// An active object with its complete business data.
    Active(ProjectViewObject),
    /// A deleted object whose ID can never be reused.
    Tombstone(ProjectViewTombstone),
}

impl ProjectViewEntry {
    /// Returns the stable object ID.
    pub const fn id(&self) -> Uuid {
        match self {
            Self::Active(object) => object.id,
            Self::Tombstone(tombstone) => tombstone.id,
        }
    }

    /// Returns the immutable canonical object type.
    pub const fn object_type(&self) -> ProjectViewObjectType {
        match self {
            Self::Active(object) => object.object_type,
            Self::Tombstone(tombstone) => tombstone.object_type,
        }
    }

    /// Returns the latest object revision.
    pub const fn object_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.object_revision,
            Self::Tombstone(tombstone) => tombstone.object_revision,
        }
    }

    /// Returns the project revision at which this entry last changed.
    pub const fn project_revision(&self) -> u64 {
        match self {
            Self::Active(object) => object.project_revision,
            Self::Tombstone(tombstone) => tombstone.project_revision,
        }
    }
}

/// Canonical state of one server-resolved Community's Project View.
///
/// State starts uninitialized at project revision zero. All mutation methods
/// use clone-then-commit semantics so an error leaves this value unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectViewState {
    project_id: CommunityId,
    project_revision: u64,
    initialized_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
    entries: BTreeMap<Uuid, ProjectViewEntry>,
}

impl ProjectViewState {
    /// Creates an empty, uninitialized state for a server-resolved Community.
    pub const fn new(project_id: CommunityId) -> Self {
        Self {
            project_id,
            project_revision: 0,
            initialized_at: None,
            updated_at: None,
            entries: BTreeMap::new(),
        }
    }

    /// Reconstructs and validates canonical state from a trusted snapshot.
    ///
    /// The input order has no semantic effect. Duplicate IDs are rejected,
    /// including collisions between active objects and tombstones.
    pub fn from_snapshot(
        project_id: CommunityId,
        project_revision: u64,
        initialized_at: Option<DateTime<Utc>>,
        updated_at: Option<DateTime<Utc>>,
        entries: impl IntoIterator<Item = ProjectViewEntry>,
    ) -> DomainResult<Self> {
        let mut by_id = BTreeMap::new();
        for entry in entries {
            let object_id = entry.id();
            if by_id.insert(object_id, entry).is_some() {
                return Err(DomainError::ObjectIdAlreadyUsed { object_id });
            }
        }
        let state = Self {
            project_id,
            project_revision,
            initialized_at,
            updated_at,
            entries: by_id,
        };
        state.validate()?;
        Ok(state)
    }

    /// Returns the server-resolved project/community identifier.
    pub const fn project_id(&self) -> CommunityId {
        self.project_id
    }

    /// Returns the current optimistic-concurrency revision.
    pub const fn project_revision(&self) -> u64 {
        self.project_revision
    }

    /// Returns whether the profile and at least one goal have been initialized.
    pub const fn is_initialized(&self) -> bool {
        self.initialized_at.is_some()
    }

    /// Returns the canonical initialization time, when initialized.
    pub const fn initialized_at(&self) -> Option<DateTime<Utc>> {
        self.initialized_at
    }

    /// Returns the canonical time of the latest successful mutation.
    pub const fn updated_at(&self) -> Option<DateTime<Utc>> {
        self.updated_at
    }

    /// Returns every occupied object ID, including tombstones.
    pub const fn entries(&self) -> &BTreeMap<Uuid, ProjectViewEntry> {
        &self.entries
    }

    /// Looks up an active object or tombstone by its stable ID.
    pub fn entry(&self, object_id: Uuid) -> Option<&ProjectViewEntry> {
        self.entries.get(&object_id)
    }

    /// Iterates over all active canonical objects.
    pub fn active_objects(&self) -> impl Iterator<Item = &ProjectViewObject> {
        self.entries.values().filter_map(|entry| match entry {
            ProjectViewEntry::Active(object) => Some(object),
            ProjectViewEntry::Tombstone(_) => None,
        })
    }

    /// Validates every field, relation, revision, and aggregate invariant.
    pub fn validate(&self) -> DomainResult<()> {
        validate_state(self)
    }

    /// Applies a mutation atomically.
    ///
    /// On error this state is byte-for-byte logically unchanged.
    pub fn apply(
        &mut self,
        mutation: &Mutation,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<MutationOutcome> {
        let (next, outcome) = self.reduce(mutation, actor, now)?;
        *self = next;
        Ok(outcome)
    }

    /// Computes a mutation result without changing this state.
    pub fn reduce(
        &self,
        mutation: &Mutation,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<(Self, MutationOutcome)> {
        self.validate()?;
        validate_mutation_envelope(self, mutation)?;

        let mut next = self.clone();
        let changed_entries = match &mutation.request {
            MutationRequest::Initialize(initialize) => {
                next.apply_initialize(initialize, actor, now)?
            }
            MutationRequest::Create(create) => {
                next.require_initialized()?;
                vec![next.apply_create(create.object.clone(), actor, now)?]
            }
            MutationRequest::Update(update) => {
                next.require_initialized()?;
                vec![next.apply_update(update, actor, now)?]
            }
            MutationRequest::Delete(delete) => {
                next.require_initialized()?;
                vec![next.apply_delete(delete, actor, now)?]
            }
        };

        next.validate()?;
        let outcome = MutationOutcome {
            project_revision: next.project_revision,
            changed_entries,
        };
        Ok((next, outcome))
    }

    fn require_initialized(&self) -> DomainResult<()> {
        if self.is_initialized() {
            Ok(())
        } else {
            Err(DomainError::NotInitialized)
        }
    }

    fn apply_initialize(
        &mut self,
        initialize: &InitializeMutation,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<Vec<ProjectViewEntry>> {
        if self.is_initialized() {
            return Err(DomainError::AlreadyInitialized);
        }
        if !(1..=MAX_INITIAL_GOALS).contains(&initialize.goals.len()) {
            return Err(DomainError::InvalidInitialGoalCount {
                min: 1,
                max: MAX_INITIAL_GOALS,
                actual: initialize.goals.len(),
            });
        }

        let project_revision = next_revision(self.project_revision)?;
        let profile_id = *self.project_id.as_uuid();
        let profile = ProjectViewObject {
            id: profile_id,
            object_type: ProjectViewObjectType::ProjectProfile,
            object_revision: 1,
            project_revision,
            created_at: now,
            updated_at: now,
            created_by: actor,
            updated_by: actor,
            data: ProjectViewObjectData::ProjectProfile(initialize.profile.clone()),
            relations: ProjectViewRelations::default(),
        };
        validate_data(&profile.data)?;

        let mut changed = vec![ProjectViewEntry::Active(profile.clone())];
        self.insert_new(ProjectViewEntry::Active(profile))?;

        for initial_goal in &initialize.goals {
            validate_client_object_id(profile_id, initial_goal.id)?;
            let goal = ProjectViewObject {
                id: initial_goal.id,
                object_type: ProjectViewObjectType::Goal,
                object_revision: 1,
                project_revision,
                created_at: now,
                updated_at: now,
                created_by: actor,
                updated_by: actor,
                data: ProjectViewObjectData::Goal(initial_goal.clone().into_goal()),
                relations: ProjectViewRelations::default(),
            };
            validate_data(&goal.data)?;
            changed.push(ProjectViewEntry::Active(goal.clone()));
            self.insert_new(ProjectViewEntry::Active(goal))?;
        }

        self.project_revision = project_revision;
        self.initialized_at = Some(now);
        self.updated_at = Some(now);
        changed.sort_by_key(ProjectViewEntry::id);
        Ok(changed)
    }

    fn apply_create(
        &mut self,
        create: NewProjectViewObject,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<ProjectViewEntry> {
        let project_revision = next_revision(self.project_revision)?;
        let (object_id, data, relations) = create.into_parts();
        validate_client_object_id(*self.project_id.as_uuid(), object_id)?;
        if self.entries.contains_key(&object_id) {
            return Err(DomainError::ObjectIdAlreadyUsed { object_id });
        }
        validate_data(&data)?;

        let object = ProjectViewObject {
            id: object_id,
            object_type: data.object_type(),
            object_revision: 1,
            project_revision,
            created_at: now,
            updated_at: now,
            created_by: actor,
            updated_by: actor,
            data,
            relations,
        };
        let entry = ProjectViewEntry::Active(object);
        self.insert_new(entry.clone())?;
        self.project_revision = project_revision;
        self.updated_at = Some(now);
        Ok(entry)
    }

    fn apply_update(
        &mut self,
        update: &UpdateMutation,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<ProjectViewEntry> {
        let object_id = update.object_id();
        let expected_type = update.object_type();
        let current = match self.entries.get(&object_id) {
            Some(ProjectViewEntry::Active(object)) => object.clone(),
            Some(ProjectViewEntry::Tombstone(_)) => {
                return Err(DomainError::ObjectDeleted { object_id });
            }
            None => return Err(DomainError::ObjectNotFound { object_id }),
        };
        if current.object_type != expected_type {
            return Err(DomainError::ObjectTypeMismatch {
                object_id,
                expected: expected_type,
                actual: current.object_type,
            });
        }

        let mut updated = current.clone();
        apply_typed_patch(&mut updated, update)?;
        if updated.data == current.data && updated.relations == current.relations {
            return Err(DomainError::NoChanges);
        }
        validate_data(&updated.data)?;

        let project_revision = next_revision(self.project_revision)?;
        updated.object_revision = next_revision(current.object_revision)?;
        updated.project_revision = project_revision;
        updated.updated_at = now;
        updated.updated_by = actor;

        let entry = ProjectViewEntry::Active(updated);
        self.entries.insert(object_id, entry.clone());
        self.project_revision = project_revision;
        self.updated_at = Some(now);
        Ok(entry)
    }

    fn apply_delete(
        &mut self,
        delete: &DeleteMutation,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> DomainResult<ProjectViewEntry> {
        let current = match self.entries.get(&delete.object_id) {
            Some(ProjectViewEntry::Active(object)) => object.clone(),
            Some(ProjectViewEntry::Tombstone(_)) => {
                return Err(DomainError::ObjectDeleted {
                    object_id: delete.object_id,
                });
            }
            None => {
                return Err(DomainError::ObjectNotFound {
                    object_id: delete.object_id,
                });
            }
        };
        if current.object_type != delete.object_type {
            return Err(DomainError::ObjectTypeMismatch {
                object_id: delete.object_id,
                expected: delete.object_type,
                actual: current.object_type,
            });
        }
        if current.object_type == ProjectViewObjectType::ProjectProfile {
            return Err(DomainError::ProfileDeletionForbidden);
        }
        if current.object_type == ProjectViewObjectType::Goal
            && self
                .active_objects()
                .filter(|object| object.object_type == ProjectViewObjectType::Goal)
                .count()
                == 1
        {
            return Err(DomainError::LastGoalDeletionForbidden);
        }
        if let Some(relation) = self.first_incoming_relation(current.id) {
            return Err(DomainError::ObjectStillReferenced {
                object_id: current.id,
                relation,
            });
        }

        let project_revision = next_revision(self.project_revision)?;
        let tombstone = ProjectViewTombstone {
            id: current.id,
            object_type: current.object_type,
            object_revision: next_revision(current.object_revision)?,
            project_revision,
            created_at: current.created_at,
            deleted_at: now,
            created_by: current.created_by,
            deleted_by: actor,
        };
        let entry = ProjectViewEntry::Tombstone(tombstone);
        self.entries.insert(current.id, entry.clone());
        self.project_revision = project_revision;
        self.updated_at = Some(now);
        Ok(entry)
    }

    fn insert_new(&mut self, entry: ProjectViewEntry) -> DomainResult<()> {
        let object_id = entry.id();
        if self.entries.insert(object_id, entry).is_some() {
            return Err(DomainError::ObjectIdAlreadyUsed { object_id });
        }
        Ok(())
    }

    fn first_incoming_relation(&self, target_id: Uuid) -> Option<&'static str> {
        for source in self.active_objects() {
            if source.relations.under_goal_id == Some(target_id) {
                return Some("under_goal_id");
            }
            if source.relations.under_plan_id == Some(target_id) {
                return Some("under_plan_id");
            }
            if source.relations.planned_in_stage_id == Some(target_id) {
                return Some("planned_in_stage_id");
            }
            if source
                .relations
                .about
                .is_some_and(|reference| reference.object_id == target_id)
            {
                return Some("about");
            }
            if source
                .relations
                .handles
                .is_some_and(|reference| reference.object_id == target_id)
            {
                return Some("handles");
            }
        }
        None
    }
}

fn validate_mutation_envelope(state: &ProjectViewState, mutation: &Mutation) -> DomainResult<()> {
    if mutation.schema_version != MUTATION_SCHEMA_VERSION {
        return Err(DomainError::UnsupportedSchemaVersion {
            got: u32::from(mutation.schema_version),
            supported: u32::from(MUTATION_SCHEMA_VERSION),
        });
    }
    crate::validation::validate_revision(mutation.expected_project_revision)?;
    if mutation.expected_project_revision != state.project_revision {
        return Err(DomainError::RevisionConflict {
            expected: mutation.expected_project_revision,
            actual: state.project_revision,
        });
    }
    Ok(())
}

fn next_revision(current: u64) -> DomainResult<u64> {
    let next = current
        .checked_add(1)
        .ok_or(DomainError::RevisionExhausted)?;
    if next > MAX_SAFE_REVISION {
        return Err(DomainError::RevisionExhausted);
    }
    Ok(next)
}

fn apply_typed_patch(object: &mut ProjectViewObject, update: &UpdateMutation) -> DomainResult<()> {
    match (update, &mut object.data) {
        (
            UpdateMutation::ProjectProfile { patch, .. },
            ProjectViewObjectData::ProjectProfile(profile),
        ) => {
            apply_required(&mut profile.name, &patch.name, "name")?;
            apply_required(&mut profile.positioning, &patch.positioning, "positioning")?;
            apply_required(&mut profile.purpose, &patch.purpose, "purpose")?;
            apply_required(&mut profile.problem, &patch.problem, "problem")?;
            apply_required(&mut profile.scope, &patch.scope, "scope")?;
        }
        (UpdateMutation::Goal { patch, .. }, ProjectViewObjectData::Goal(goal)) => {
            apply_required(&mut goal.title, &patch.title, "title")?;
            apply_required(
                &mut goal.desired_outcome,
                &patch.desired_outcome,
                "desired_outcome",
            )?;
            apply_required(&mut goal.directions, &patch.directions, "directions")?;
        }
        (UpdateMutation::Role { patch, .. }, ProjectViewObjectData::Role(role)) => {
            apply_required(&mut role.name, &patch.name, "name")?;
            apply_required(&mut role.purpose, &patch.purpose, "purpose")?;
            apply_required(
                &mut role.responsibilities,
                &patch.responsibilities,
                "responsibilities",
            )?;
            apply_required(&mut role.boundaries, &patch.boundaries, "boundaries")?;
            apply_required(&mut role.active, &patch.active, "active")?;
        }
        (UpdateMutation::Plan { patch, .. }, ProjectViewObjectData::Plan(plan)) => {
            apply_required(&mut plan.title, &patch.title, "title")?;
            apply_required(&mut plan.description, &patch.description, "description")?;
            apply_required(&mut plan.status, &patch.status, "status")?;
            apply_optional(&mut object.relations.under_goal_id, &patch.under_goal_id);
        }
        (UpdateMutation::Stage { patch, .. }, ProjectViewObjectData::Stage(stage)) => {
            apply_required(&mut stage.title, &patch.title, "title")?;
            apply_required(&mut stage.description, &patch.description, "description")?;
            apply_required(&mut stage.status, &patch.status, "status")?;
            apply_required_relation(
                &mut object.relations.under_plan_id,
                &patch.under_plan_id,
                "under_plan_id",
            )?;
        }
        (
            UpdateMutation::Requirement { patch, .. },
            ProjectViewObjectData::Requirement(requirement),
        ) => {
            apply_required(&mut requirement.title, &patch.title, "title")?;
            apply_required(
                &mut requirement.description,
                &patch.description,
                "description",
            )?;
            apply_required(&mut requirement.status, &patch.status, "status")?;
            apply_required(&mut requirement.priority, &patch.priority, "priority")?;
            apply_optional(
                &mut object.relations.planned_in_stage_id,
                &patch.planned_in_stage_id,
            );
        }
        (UpdateMutation::Issue { patch, .. }, ProjectViewObjectData::Issue(issue)) => {
            apply_required(&mut issue.title, &patch.title, "title")?;
            apply_required(&mut issue.description, &patch.description, "description")?;
            apply_required(&mut issue.status, &patch.status, "status")?;
            apply_required(&mut issue.priority, &patch.priority, "priority")?;
            apply_optional(
                &mut object.relations.planned_in_stage_id,
                &patch.planned_in_stage_id,
            );
            apply_optional(&mut object.relations.about, &patch.about);
        }
        (UpdateMutation::Work { patch, .. }, ProjectViewObjectData::Work(work)) => {
            apply_required(&mut work.title, &patch.title, "title")?;
            apply_required(&mut work.description, &patch.description, "description")?;
            apply_required(&mut work.status, &patch.status, "status")?;
            apply_required(&mut work.priority, &patch.priority, "priority")?;
            apply_required_relation(&mut object.relations.handles, &patch.handles, "handles")?;
        }
        (UpdateMutation::Resource { patch, .. }, ProjectViewObjectData::Resource(resource)) => {
            apply_required(&mut resource.name, &patch.name, "name")?;
            apply_required(
                &mut resource.resource_type,
                &patch.resource_type,
                "resource_type",
            )?;
            apply_required(&mut resource.locator, &patch.locator, "locator")?;
            apply_required(&mut resource.description, &patch.description, "description")?;
        }
        _ => {
            return Err(DomainError::DataTypeMismatch {
                declared: object.object_type,
                actual: object.data.object_type(),
            });
        }
    }
    Ok(())
}

fn apply_required<T: Clone + PartialEq>(
    current: &mut T,
    patch: &Patch<T>,
    field: &'static str,
) -> DomainResult<()> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::RequiredField { field }),
        Patch::Set(value) => {
            current.clone_from(value);
            Ok(())
        }
    }
}

fn apply_optional<T: Clone + PartialEq>(current: &mut Option<T>, patch: &Patch<T>) {
    match patch {
        Patch::Unchanged => {}
        Patch::Clear => *current = None,
        Patch::Set(value) => *current = Some(value.clone()),
    }
}

fn apply_required_relation<T: Clone + PartialEq>(
    current: &mut Option<T>,
    patch: &Patch<T>,
    relation: &'static str,
) -> DomainResult<()> {
    match patch {
        Patch::Unchanged => Ok(()),
        Patch::Clear => Err(DomainError::MissingRequiredRelation { relation }),
        Patch::Set(value) => {
            *current = Some(value.clone());
            Ok(())
        }
    }
}
