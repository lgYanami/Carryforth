//! Verified, deterministic Role Brief assembly from Project View v2 heads.
//!
//! This module deliberately performs no network I/O. Callers first fetch and
//! cryptographically verify the Relay-authored projections with
//! [`crate::project_view_v2`], then pass the complete bounded snapshot here.
//! JSON consumers, prompt rendering, and desktop presentation therefore share
//! one interpretation of the current Role and Assignment.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt::Write as _;

use buzz_core::{EventId, PublicKey};
use buzz_project_view::v2::{
    CommunityMemberRole, ProposalStatus, RoleAssignment, RoleAssignmentProposal, RoleCheckpoint,
    RoleContinuityChange, RoleContinuityReference, RoleDefinition, RoleHandoff, RoleLevel,
    WorkCommitment,
};
use buzz_project_view::{
    Goal, ObjectRef, ProjectIssue, ProjectRole, ProjectView, ProjectViewEntry, ProjectViewObject,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, ProjectViewState,
    ProjectViewTombstone, ProjectWork, WorkStatus,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project_view_v2::{
    V2EntityProjection, V2MembershipProjection, V2MetaProjection, V2ProjectObjectProjection,
    V2ProjectedObject, V2ProjectionSource,
};
use crate::SdkError;

const ROLE_DIRECTORY_MAX_ENTRIES: usize = 32;
const ROLE_DIRECTORY_PURPOSE_MAX_CHARS: usize = 160;

/// Stable source reference for one object or Role-continuity entity in a Brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefSourceReference {
    /// Signed Relay projection event.
    pub event_id: EventId,
    /// Project revision at which this head was written.
    pub project_revision: u64,
    /// Object or entity-local revision.
    pub item_revision: u64,
    /// Stable accepted-change identifier.
    pub change_id: EventId,
    /// Closed projection source spelling.
    pub source_type: String,
}

/// One active Project View object and the projection that proves its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefObject {
    /// Complete canonical object.
    pub object: ProjectViewObject,
    /// Stable Role responsible for an active Work, when this object is Work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responsible_role_id: Option<Uuid>,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One canonical Role and the projection that proves its version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefRole {
    /// Complete Role definition, including its Community permission level.
    pub role: RoleDefinition,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One Assignment and the projection that proves its current lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefAssignment {
    /// Complete Assignment tenure.
    pub assignment: RoleAssignment,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One open Proposal relevant to a candidate and its signed source revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefProposal {
    /// Complete open Proposal.
    pub proposal: RoleAssignmentProposal,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One Work Commitment and the projection that proves its lifecycle state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefCommitment {
    /// Complete Commitment tenure.
    pub commitment: WorkCommitment,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One append-only Checkpoint and the projection that proves its attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefCheckpoint {
    /// Complete structured Checkpoint.
    pub checkpoint: RoleCheckpoint,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// One append-only Handoff and the projection that proves its attribution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefHandoff {
    /// Complete structured Handoff.
    pub handoff: RoleHandoff,
    /// Exact signed source revision.
    pub source: RoleBriefSourceReference,
}

/// Execution state derived for one Work owned by the assigned Role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleBriefWorkState {
    /// The current Assignment has explicitly accepted the Work.
    Committed {
        /// Active Commitment that attributes execution to this Assignment.
        commitment: Box<RoleBriefCommitment>,
    },
    /// The Role still owns the Work, but no current Assignment has accepted it.
    WaitingForContinuation,
}

/// One non-terminal Work for which the assigned Role is responsible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefResponsibleWork {
    /// Canonical Work object and signed projection source.
    pub work: RoleBriefObject,
    /// Derived current execution state.
    pub state: RoleBriefWorkState,
}

/// Minimal project-wide context carried by every Role Brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefProjectSummary {
    /// The single Project Profile.
    pub profile: RoleBriefObject,
    /// Every active Goal in deterministic order.
    pub goals: Vec<RoleBriefObject>,
}

/// Current staffing state for one Role Directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleBriefRoleDirectoryAssignment {
    /// One active Assignment currently occupies the Role.
    Assigned {
        /// Immutable active Assignment tenure.
        assignment_id: Uuid,
        /// Stable public key of the assigned Member.
        member_pubkey: PublicKey,
        /// Exact signed Assignment source revision.
        source: RoleBriefSourceReference,
    },
    /// No active Assignment currently occupies the Role.
    Vacant,
}

/// One bounded navigation entry for an active Project Role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefRoleDirectoryEntry {
    /// Stable Project Role identifier.
    pub role_id: Uuid,
    /// Human-readable Role name.
    pub name: String,
    /// Community permission level granted by the Role.
    pub level: RoleLevel,
    /// Deterministic one-line, length-bounded purpose summary.
    pub purpose_summary: String,
    /// Current active Assignment or an explicit vacancy.
    pub assignment: RoleBriefRoleDirectoryAssignment,
    /// Whether this is the Role occupied by the Brief's target Member.
    pub is_current_member_role: bool,
    /// Exact signed Role source revision.
    pub role_source: RoleBriefSourceReference,
}

/// Bounded active-Role directory derived from the same verified Brief snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefRoleDirectory {
    /// Total active Roles in the verified Project snapshot.
    pub total_active_roles: u32,
    /// Deterministically selected Role entries carried by this Brief.
    pub entries: Vec<RoleBriefRoleDirectoryEntry>,
    /// Active Roles omitted because the prompt-safe directory bound was reached.
    pub omitted_active_roles: u32,
}

/// Member entry state derived from the verified active Assignment set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleBriefMemberState {
    /// The member has no active Assignment and must not act as a Project Role.
    Candidate {
        /// Open Proposals addressed to this candidate at `generated_at`.
        open_proposals: Vec<RoleBriefProposal>,
    },
    /// The member currently acts through exactly one active Assignment.
    Assigned {
        /// Assigned semantic Role.
        role: Box<RoleBriefRole>,
        /// Immutable Assignment tenure used as the write fence.
        assignment: Box<RoleBriefAssignment>,
    },
}

impl RoleBriefMemberState {
    /// Stable state spelling.
    #[must_use]
    pub const fn status(&self) -> &'static str {
        match self {
            Self::Candidate { .. } => "candidate",
            Self::Assigned { .. } => "assigned",
        }
    }

    /// Active Assignment ID when this is an assigned Brief.
    #[must_use]
    pub const fn assignment_id(&self) -> Option<Uuid> {
        match self {
            Self::Candidate { .. } => None,
            Self::Assigned { assignment, .. } => Some(assignment.assignment.assignment_id),
        }
    }
}

/// Snapshot-level signed revisions shared by every section of a Role Brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefSourceRevisions {
    /// Exact metadata head that bounded the snapshot.
    pub meta_event_id: EventId,
    /// Stable change represented by the metadata head.
    pub meta_change_id: EventId,
    /// Exact NIP-43 membership snapshot referenced by metadata.
    pub membership_event_id: EventId,
    /// Canonical Relay update time of the metadata head.
    pub project_updated_at: DateTime<Utc>,
}

/// Canonical minimal Role Brief shared by CLI, ACP prompts, and Desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBrief {
    /// Time at which candidate/Proposal state was evaluated.
    pub generated_at: DateTime<Utc>,
    /// Server-resolved Community/Project UUID.
    pub project_id: Uuid,
    /// Current optimistic-concurrency revision.
    pub project_revision: u64,
    /// Current Relay projection signer generation.
    pub projection_generation: u64,
    /// Member for whom this Brief was assembled.
    pub member_pubkey: PublicKey,
    /// Community permission in the exact membership snapshot, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub community_role: Option<CommunityMemberRole>,
    /// Profile and Goal summary shared by candidate and assigned states.
    pub project: RoleBriefProjectSummary,
    /// Bounded active-Role directory from the same verified snapshot.
    pub role_directory: RoleBriefRoleDirectory,
    /// Candidate or assigned entry state.
    pub state: RoleBriefMemberState,
    /// Non-terminal Work owned by the assigned Role, with current Commitment
    /// or an explicit waiting-for-continuation state.
    pub responsible_work: Vec<RoleBriefResponsibleWork>,
    /// Role-related Issues and their handling Work in deterministic order.
    pub related_objects: Vec<RoleBriefObject>,
    /// Latest structured situation entry for the current or proposed Role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_checkpoint: Option<RoleBriefCheckpoint>,
    /// Most recent Handoffs for the current or proposed Role.
    pub recent_handoffs: Vec<RoleBriefHandoff>,
    /// Signed snapshot boundaries.
    pub source_revisions: RoleBriefSourceRevisions,
}

impl RoleBrief {
    /// Active Assignment ID when the member is assigned.
    #[must_use]
    pub const fn assignment_id(&self) -> Option<Uuid> {
        self.state.assignment_id()
    }
}

/// A complete, internally consistent Project View v2 projection snapshot.
///
/// Construction validates projection basis, counts, Project View relations,
/// Role/Assignment uniqueness, and NIP-43 membership coupling before any
/// caller can obtain a Brief.
#[derive(Debug, Clone)]
pub struct VerifiedRoleBriefSnapshot {
    meta: V2MetaProjection,
    membership: V2MembershipProjection,
    entries: BTreeMap<Uuid, ProjectViewEntry>,
    objects: BTreeMap<Uuid, ObjectHead>,
    roles: BTreeMap<Uuid, EntityHead<RoleDefinition>>,
    proposals: BTreeMap<Uuid, EntityHead<RoleAssignmentProposal>>,
    assignments: BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    commitments: BTreeMap<Uuid, EntityHead<WorkCommitment>>,
    checkpoints: BTreeMap<Uuid, EntityHead<RoleCheckpoint>>,
    handoffs: BTreeMap<Uuid, EntityHead<RoleHandoff>>,
    view: ProjectView,
}

#[derive(Debug, Clone)]
struct ObjectHead {
    object: ProjectViewObject,
    responsible_role_id: Option<Uuid>,
    source: RoleBriefSourceReference,
}

#[derive(Debug, Clone)]
struct EntityHead<T> {
    entity: T,
    source: RoleBriefSourceReference,
}

impl VerifiedRoleBriefSnapshot {
    /// Validate and retain one complete set of verified v2 heads.
    pub fn new(
        meta: V2MetaProjection,
        membership: V2MembershipProjection,
        object_projections: Vec<V2ProjectObjectProjection>,
        entity_projections: Vec<V2EntityProjection>,
    ) -> Result<Self, SdkError> {
        Self::build(
            meta,
            membership,
            object_projections,
            entity_projections,
            true,
        )
    }

    /// Validate current heads plus a bounded, Relay-signed history slice.
    ///
    /// Current-object, open-Proposal, active-Assignment, and
    /// active-Commitment counts remain exact. Checkpoint/Handoff counts are
    /// upper bounds because a revision-pinned history page intentionally does
    /// not claim to be the complete append-only history.
    pub fn new_with_partial_history(
        meta: V2MetaProjection,
        membership: V2MembershipProjection,
        object_projections: Vec<V2ProjectObjectProjection>,
        entity_projections: Vec<V2EntityProjection>,
    ) -> Result<Self, SdkError> {
        Self::build(
            meta,
            membership,
            object_projections,
            entity_projections,
            false,
        )
    }

    fn build(
        meta: V2MetaProjection,
        membership: V2MembershipProjection,
        object_projections: Vec<V2ProjectObjectProjection>,
        entity_projections: Vec<V2EntityProjection>,
        complete_history: bool,
    ) -> Result<Self, SdkError> {
        validate_membership_pointer(&meta, &membership)?;

        let mut event_ids = HashSet::with_capacity(
            object_projections.len()
                + entity_projections.len()
                + usize::from(meta.event_id != membership.event_id),
        );
        event_ids.insert(meta.event_id);
        if !event_ids.insert(membership.event_id) {
            return Err(invalid("metadata and membership event IDs collide"));
        }

        let mut entries = Vec::with_capacity(object_projections.len() + entity_projections.len());
        let mut occupied_object_ids = HashSet::new();
        let mut objects = BTreeMap::new();
        for projection in object_projections {
            validate_projection_basis(
                projection.project_id,
                projection.projection_generation,
                projection.project_revision,
                &meta,
            )?;
            if !event_ids.insert(projection.event_id) {
                return Err(invalid("v2 snapshot contains a duplicate signed event"));
            }
            let source = source_reference(
                projection.event_id,
                projection.project_revision,
                projection.object.object_revision(),
                &projection.source,
            );
            let responsible_role_id = projection.responsible_role_id;
            match projection.object {
                V2ProjectedObject::Active(object) => {
                    let object = *object;
                    if object.project_revision != projection.project_revision
                        || object.updated_at != projection.updated_at
                    {
                        return Err(invalid(
                            "ordinary object body disagrees with its projection revision",
                        ));
                    }
                    if !occupied_object_ids.insert(object.id) {
                        return Err(invalid("v2 snapshot contains a duplicate object ID"));
                    }
                    entries.push(ProjectViewEntry::Active(object.clone()));
                    objects.insert(
                        object.id,
                        ObjectHead {
                            object,
                            responsible_role_id,
                            source,
                        },
                    );
                }
                V2ProjectedObject::Tombstone(tombstone) => {
                    if tombstone.project_revision != projection.project_revision
                        || tombstone.deleted_at != projection.updated_at
                    {
                        return Err(invalid(
                            "ordinary tombstone disagrees with its projection revision",
                        ));
                    }
                    if !occupied_object_ids.insert(tombstone.object_id) {
                        return Err(invalid("v2 snapshot contains a duplicate object ID"));
                    }
                    entries.push(ProjectViewEntry::Tombstone(ProjectViewTombstone {
                        id: tombstone.object_id,
                        object_type: tombstone.object_type,
                        object_revision: tombstone.object_revision,
                        project_revision: tombstone.project_revision,
                        created_at: tombstone.created_at,
                        deleted_at: tombstone.deleted_at,
                        created_by: tombstone.created_by,
                        deleted_by: tombstone.deleted_by,
                    }));
                }
            }
        }

        let mut entity_keys = BTreeSet::new();
        let mut roles = BTreeMap::new();
        let mut proposals = BTreeMap::new();
        let mut assignments = BTreeMap::new();
        let mut commitments = BTreeMap::new();
        let mut checkpoints = BTreeMap::new();
        let mut handoffs = BTreeMap::new();
        for projection in entity_projections {
            validate_projection_basis(
                projection.project_id,
                projection.projection_generation,
                projection.project_revision,
                &meta,
            )?;
            if !event_ids.insert(projection.event_id) {
                return Err(invalid("v2 snapshot contains a duplicate signed event"));
            }
            let key = (
                projection.entity.entity_type(),
                projection.entity.entity_id(),
            );
            if !entity_keys.insert(key) {
                return Err(invalid("v2 snapshot contains a duplicate entity head"));
            }
            if projection.entity.entity_revision() != projection.entity_revision {
                return Err(invalid(
                    "v2 entity body disagrees with its projection revision",
                ));
            }
            let source = source_reference(
                projection.event_id,
                projection.project_revision,
                projection.entity_revision,
                &projection.source,
            );
            match projection.entity {
                RoleContinuityChange::Role(role) => {
                    if role.project_revision != projection.project_revision
                        || role.updated_at != projection.updated_at
                    {
                        return Err(invalid("Role body disagrees with its projection revision"));
                    }
                    if !occupied_object_ids.insert(role.role_id) {
                        return Err(invalid(
                            "v2 Role head collides with another Project object ID",
                        ));
                    }
                    let object = project_object_from_role(&role);
                    entries.push(ProjectViewEntry::Active(object));
                    roles.insert(
                        role.role_id,
                        EntityHead {
                            entity: role,
                            source,
                        },
                    );
                }
                RoleContinuityChange::Proposal(proposal) => {
                    if proposal.project_revision != projection.project_revision {
                        return Err(invalid(
                            "Proposal body disagrees with its projection revision",
                        ));
                    }
                    proposals.insert(
                        proposal.proposal_id,
                        EntityHead {
                            entity: proposal,
                            source,
                        },
                    );
                }
                RoleContinuityChange::Assignment(assignment) => {
                    if assignment.project_revision != projection.project_revision {
                        return Err(invalid(
                            "Assignment body disagrees with its projection revision",
                        ));
                    }
                    assignments.insert(
                        assignment.assignment_id,
                        EntityHead {
                            entity: assignment,
                            source,
                        },
                    );
                }
                RoleContinuityChange::Commitment(commitment) => {
                    if commitment.project_revision != projection.project_revision {
                        return Err(invalid(
                            "Commitment body disagrees with its projection revision",
                        ));
                    }
                    commitments.insert(
                        commitment.commitment_id,
                        EntityHead {
                            entity: commitment,
                            source,
                        },
                    );
                }
                RoleContinuityChange::Checkpoint(checkpoint) => {
                    if checkpoint.project_revision != projection.project_revision {
                        return Err(invalid(
                            "Checkpoint body disagrees with its projection revision",
                        ));
                    }
                    checkpoints.insert(
                        checkpoint.checkpoint_id,
                        EntityHead {
                            entity: checkpoint,
                            source,
                        },
                    );
                }
                RoleContinuityChange::Handoff(handoff) => {
                    if handoff.project_revision != projection.project_revision {
                        return Err(invalid(
                            "Handoff body disagrees with its projection revision",
                        ));
                    }
                    handoffs.insert(
                        handoff.handoff_id,
                        EntityHead {
                            entity: handoff,
                            source,
                        },
                    );
                }
            }
        }

        validate_counts(
            &meta,
            VerifiedCountInputs {
                active_objects: objects.len() + roles.len(),
                proposals: &proposals,
                assignments: &assignments,
                commitments: &commitments,
                checkpoints: checkpoints.len(),
                handoffs: handoffs.len(),
                complete_history,
            },
        )?;
        let entries_by_id = entries
            .iter()
            .cloned()
            .map(|entry| (entry.id(), entry))
            .collect::<BTreeMap<_, _>>();
        let view = validate_project_state(&meta, entries)?;
        validate_continuity(
            ContinuityHeads {
                entries: &entries_by_id,
                roles: &roles,
                proposals: &proposals,
                assignments: &assignments,
                commitments: &commitments,
                checkpoints: &checkpoints,
                handoffs: &handoffs,
                objects: &objects,
            },
            &membership,
            complete_history,
        )?;

        Ok(Self {
            meta,
            membership,
            entries: entries_by_id,
            objects,
            roles,
            proposals,
            assignments,
            commitments,
            checkpoints,
            handoffs,
            view,
        })
    }

    /// Metadata head that bounds this verified snapshot.
    #[must_use]
    pub const fn meta(&self) -> &V2MetaProjection {
        &self.meta
    }

    /// Exact NIP-43 snapshot referenced by [`Self::meta`].
    #[must_use]
    pub const fn membership(&self) -> &V2MembershipProjection {
        &self.membership
    }

    /// Deterministically assembled logical Project View.
    #[must_use]
    pub const fn project_view(&self) -> &ProjectView {
        &self.view
    }

    /// Look up one active object or immutable tombstone by stable object ID.
    #[must_use]
    pub fn entry(&self, object_id: Uuid) -> Option<&ProjectViewEntry> {
        self.entries.get(&object_id)
    }

    /// Return one active object together with its verified responsibility and
    /// exact projection source. Tombstones intentionally have no active head.
    #[must_use]
    pub fn active_object(&self, object_id: Uuid) -> Option<RoleBriefObject> {
        self.objects.get(&object_id).map(|head| RoleBriefObject {
            object: head.object.clone(),
            responsible_role_id: head.responsible_role_id,
            source: head.source.clone(),
        })
    }

    /// Iterate over canonical Role definitions by stable Role ID.
    pub fn roles(&self) -> impl Iterator<Item = &RoleDefinition> {
        self.roles.values().map(|head| &head.entity)
    }

    /// Iterate over Proposal heads by stable Proposal ID.
    pub fn proposals(&self) -> impl Iterator<Item = &RoleAssignmentProposal> {
        self.proposals.values().map(|head| &head.entity)
    }

    /// Iterate over Assignment heads by stable Assignment ID.
    pub fn assignments(&self) -> impl Iterator<Item = &RoleAssignment> {
        self.assignments.values().map(|head| &head.entity)
    }

    /// Iterate over Work Commitment heads by stable Commitment ID.
    pub fn commitments(&self) -> impl Iterator<Item = &WorkCommitment> {
        self.commitments.values().map(|head| &head.entity)
    }

    /// Iterate over append-only Checkpoints by stable Checkpoint ID.
    pub fn checkpoints(&self) -> impl Iterator<Item = &RoleCheckpoint> {
        self.checkpoints.values().map(|head| &head.entity)
    }

    /// Iterate over Handoff heads by stable Handoff ID.
    pub fn handoffs(&self) -> impl Iterator<Item = &RoleHandoff> {
        self.handoffs.values().map(|head| &head.entity)
    }

    /// Assemble a canonical Brief for one member.
    pub fn brief_for(
        &self,
        member_pubkey: PublicKey,
        generated_at: DateTime<Utc>,
    ) -> Result<RoleBrief, SdkError> {
        let profile = self
            .objects
            .values()
            .find(|head| head.object.object_type == ProjectViewObjectType::ProjectProfile)
            .ok_or_else(|| invalid("verified Project View has no Profile"))?;
        let mut goals = self
            .objects
            .values()
            .filter(|head| head.object.object_type == ProjectViewObjectType::Goal)
            .collect::<Vec<_>>();
        goals.sort_by(|left, right| object_order(&left.object, &right.object));
        let role_directory = self.role_directory(member_pubkey)?;

        let active_assignment = self
            .assignments
            .values()
            .find(|head| head.entity.member_pubkey == member_pubkey && head.entity.is_active());
        let (state, responsible_work, related_objects, context_role_id) =
            if let Some(assignment) = active_assignment {
                let role = self.roles.get(&assignment.entity.role_id).ok_or_else(|| {
                    invalid("active Assignment references a missing verified Role")
                })?;
                (
                    RoleBriefMemberState::Assigned {
                        role: Box::new(RoleBriefRole {
                            role: role.entity.clone(),
                            source: role.source.clone(),
                        }),
                        assignment: Box::new(RoleBriefAssignment {
                            assignment: assignment.entity.clone(),
                            source: assignment.source.clone(),
                        }),
                    },
                    self.responsible_work(role.entity.role_id),
                    self.related_objects(role.entity.role_id),
                    Some(role.entity.role_id),
                )
            } else {
                let mut open_proposals = self
                    .proposals
                    .values()
                    .filter(|head| {
                        head.entity.candidate_pubkey == member_pubkey
                            && head.entity.effective_status(generated_at) == ProposalStatus::Open
                    })
                    .map(|head| RoleBriefProposal {
                        proposal: head.entity.clone(),
                        source: head.source.clone(),
                    })
                    .collect::<Vec<_>>();
                open_proposals.sort_by(|left, right| {
                    left.proposal
                        .created_at
                        .cmp(&right.proposal.created_at)
                        .then(left.proposal.proposal_id.cmp(&right.proposal.proposal_id))
                });
                let context_role_id = open_proposals
                    .first()
                    .map(|proposal| proposal.proposal.role_id);
                (
                    RoleBriefMemberState::Candidate { open_proposals },
                    Vec::new(),
                    Vec::new(),
                    context_role_id,
                )
            };
        let community_role = self
            .membership
            .members
            .iter()
            .find(|member| member.pubkey == member_pubkey)
            .map(|member| member.role);
        let latest_checkpoint = context_role_id
            .and_then(|role_id| self.latest_checkpoint(role_id))
            .map(|head| RoleBriefCheckpoint {
                checkpoint: head.entity.clone(),
                source: head.source.clone(),
            });
        let recent_handoffs = context_role_id
            .map_or_else(Vec::new, |role_id| self.recent_handoffs(role_id, 3))
            .into_iter()
            .map(|head| RoleBriefHandoff {
                handoff: head.entity.clone(),
                source: head.source.clone(),
            })
            .collect();

        Ok(RoleBrief {
            generated_at,
            project_id: *self.meta.project_id.as_uuid(),
            project_revision: self.meta.project_revision,
            projection_generation: self.meta.projection_generation,
            member_pubkey,
            community_role,
            project: RoleBriefProjectSummary {
                profile: role_brief_object(profile),
                goals: goals.into_iter().map(role_brief_object).collect(),
            },
            role_directory,
            state,
            responsible_work,
            related_objects,
            latest_checkpoint,
            recent_handoffs,
            source_revisions: RoleBriefSourceRevisions {
                meta_event_id: self.meta.event_id,
                meta_change_id: self.meta.source.change_id(),
                membership_event_id: self.membership.event_id,
                project_updated_at: self.meta.updated_at,
            },
        })
    }

    fn role_directory(&self, member_pubkey: PublicKey) -> Result<RoleBriefRoleDirectory, SdkError> {
        let active_assignments = self
            .assignments
            .values()
            .filter(|head| head.entity.is_active())
            .map(|head| (head.entity.role_id, head))
            .collect::<BTreeMap<_, _>>();
        let mut entries = self
            .roles
            .values()
            .filter(|head| head.entity.active)
            .map(|head| {
                let assignment = active_assignments.get(&head.entity.role_id);
                RoleBriefRoleDirectoryEntry {
                    role_id: head.entity.role_id,
                    name: head.entity.name.clone(),
                    level: head.entity.level,
                    purpose_summary: role_directory_purpose_summary(&head.entity.purpose),
                    assignment: assignment.map_or(
                        RoleBriefRoleDirectoryAssignment::Vacant,
                        |assignment| RoleBriefRoleDirectoryAssignment::Assigned {
                            assignment_id: assignment.entity.assignment_id,
                            member_pubkey: assignment.entity.member_pubkey,
                            source: assignment.source.clone(),
                        },
                    ),
                    is_current_member_role: assignment
                        .is_some_and(|assignment| assignment.entity.member_pubkey == member_pubkey),
                    role_source: head.source.clone(),
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by_cached_key(|entry| {
            (
                !entry.is_current_member_role,
                role_level_order(entry.level),
                entry.name.to_lowercase(),
                entry.role_id,
            )
        });

        let total_active_roles = u32::try_from(entries.len())
            .map_err(|_| invalid("active Role count exceeds Role Directory range"))?;
        entries.truncate(ROLE_DIRECTORY_MAX_ENTRIES);
        let shown_roles = u32::try_from(entries.len())
            .map_err(|_| invalid("shown Role count exceeds Role Directory range"))?;

        Ok(RoleBriefRoleDirectory {
            total_active_roles,
            entries,
            omitted_active_roles: total_active_roles - shown_roles,
        })
    }

    fn responsible_work(&self, role_id: Uuid) -> Vec<RoleBriefResponsibleWork> {
        let mut work = self
            .objects
            .values()
            .filter(|head| head.responsible_role_id == Some(role_id) && work_is_open(&head.object))
            .map(|head| {
                let state = self
                    .commitments
                    .values()
                    .find(|commitment| {
                        commitment.entity.work_id == head.object.id && commitment.entity.is_active()
                    })
                    .map_or(RoleBriefWorkState::WaitingForContinuation, |commitment| {
                        RoleBriefWorkState::Committed {
                            commitment: Box::new(RoleBriefCommitment {
                                commitment: commitment.entity.clone(),
                                source: commitment.source.clone(),
                            }),
                        }
                    });
                RoleBriefResponsibleWork {
                    work: role_brief_object(head),
                    state,
                }
            })
            .collect::<Vec<_>>();
        work.sort_by(|left, right| object_order(&left.work.object, &right.work.object));
        work
    }

    fn related_objects(&self, role_id: Uuid) -> Vec<RoleBriefObject> {
        let role_ref = ObjectRef {
            object_type: ProjectViewObjectType::Role,
            object_id: role_id,
        };
        let issue_ids = self
            .objects
            .values()
            .filter(|head| {
                head.object.object_type == ProjectViewObjectType::Issue
                    && head.object.relations.about == Some(role_ref)
            })
            .map(|head| head.object.id)
            .collect::<BTreeSet<_>>();
        let mut related = self
            .objects
            .values()
            .filter(|head| {
                issue_ids.contains(&head.object.id)
                    || matches!(
                        head.object.relations.handles,
                        Some(ObjectRef {
                            object_type: ProjectViewObjectType::Issue,
                            object_id,
                        }) if issue_ids.contains(&object_id)
                    )
            })
            .collect::<Vec<_>>();
        related.sort_by(|left, right| object_order(&left.object, &right.object));
        related.into_iter().map(role_brief_object).collect()
    }

    fn latest_checkpoint(&self, role_id: Uuid) -> Option<&EntityHead<RoleCheckpoint>> {
        self.checkpoints
            .values()
            .filter(|head| head.entity.role_id == role_id)
            .max_by(|left, right| {
                left.entity
                    .project_revision
                    .cmp(&right.entity.project_revision)
                    .then(left.entity.checkpoint_id.cmp(&right.entity.checkpoint_id))
            })
    }

    fn recent_handoffs(&self, role_id: Uuid, limit: usize) -> Vec<&EntityHead<RoleHandoff>> {
        let mut handoffs = self
            .handoffs
            .values()
            .filter(|head| head.entity.role_id == role_id)
            .collect::<Vec<_>>();
        handoffs.sort_by(|left, right| {
            right
                .entity
                .project_revision
                .cmp(&left.entity.project_revision)
                .then(right.entity.handoff_id.cmp(&left.entity.handoff_id))
        });
        handoffs.truncate(limit);
        handoffs
    }
}

/// Render the compact per-turn binding derived from a previously verified
/// canonical Role Brief.
///
/// This representation deliberately carries only identity and revision
/// coordinates. It is prompt context, not an authorization cache: managed
/// writers must still resolve the current Assignment before every write.
#[must_use]
pub fn render_role_binding_markdown(brief: &RoleBrief) -> String {
    let mut output = String::from("[Role Binding]\n");
    let _ = writeln!(output, "State: {}", brief.state.status());
    let _ = writeln!(output, "Project ID: {}", brief.project_id);
    match &brief.state {
        RoleBriefMemberState::Candidate { .. } => {
            output.push_str(
                "Role ID: none\nRole: none\nLevel: none\nAssignment: none\n\
                 Boundary: no active Assignment is verified for this meta head. Do not act as a \
                 Project Role or perform role-bearing Project View writes.\n",
            );
        }
        RoleBriefMemberState::Assigned { role, assignment } => {
            let _ = writeln!(output, "Role ID: {}", role.role.role_id);
            let _ = writeln!(output, "Role: {}", one_line(&role.role.name));
            let _ = writeln!(output, "Level: {}", role.role.level.as_str());
            let _ = writeln!(
                output,
                "Assignment: {}",
                assignment.assignment.assignment_id
            );
            output.push_str(
                "Boundary: this binding is context for the exact meta head below, not cached \
                 authorization. Re-resolve the current Assignment before every role-bearing \
                 Project View write; the Relay performs the final fence check.\n",
            );
        }
    }
    let _ = writeln!(
        output,
        "Source revisions: project={} generation={} meta={}",
        brief.project_revision, brief.projection_generation, brief.source_revisions.meta_event_id
    );
    output
}

/// Render the canonical human/Agent Markdown representation of a Role Brief.
#[must_use]
pub fn render_role_brief_markdown(brief: &RoleBrief) -> String {
    let mut output = String::from("[Role Brief]\n");
    let profile = match &brief.project.profile.object.data {
        ProjectViewObjectData::ProjectProfile(profile) => profile,
        _ => return unavailable_markdown("verified Brief contains a non-Profile project summary"),
    };
    let _ = writeln!(output, "State: {}", brief.state.status());
    let _ = writeln!(output, "Project: {}", one_line(&profile.name));
    let _ = writeln!(output, "Purpose: {}", one_line(&profile.purpose));
    let _ = writeln!(output, "Positioning: {}", one_line(&profile.positioning));
    let _ = writeln!(output, "Scope: {}", one_line(&profile.scope));
    let _ = writeln!(
        output,
        "Community role: {}",
        brief
            .community_role
            .map_or("none", CommunityMemberRole::as_str)
    );
    output.push_str("Goals:\n");
    for goal in &brief.project.goals {
        if let ProjectViewObjectData::Goal(Goal {
            title,
            desired_outcome,
            ..
        }) = &goal.object.data
        {
            let _ = writeln!(
                output,
                "- {} — {}",
                one_line(title),
                one_line(desired_outcome)
            );
        }
    }
    render_role_directory(&mut output, &brief.role_directory);

    match &brief.state {
        RoleBriefMemberState::Candidate { open_proposals } => {
            output.push_str(
                "Role: none\nAssignment: none\n\
                 Boundary: no active Assignment is verified. Do not act as a Project Role or \
                 perform role-bearing Project View writes.\n",
            );
            if !open_proposals.is_empty() {
                output.push_str("Open proposals:\n");
                for proposal in open_proposals {
                    let _ = writeln!(
                        output,
                        "- {} for Role {} (expires {})",
                        proposal.proposal.proposal_id,
                        proposal.proposal.role_id,
                        proposal
                            .proposal
                            .expires_at
                            .to_rfc3339_opts(SecondsFormat::Secs, true)
                    );
                }
            }
        }
        RoleBriefMemberState::Assigned { role, assignment } => {
            let _ = writeln!(output, "Role: {}", one_line(&role.role.name));
            let _ = writeln!(output, "Level: {}", role.role.level.as_str());
            let _ = writeln!(output, "Role purpose: {}", one_line(&role.role.purpose));
            output.push_str("Responsibilities:\n");
            for responsibility in &role.role.responsibilities {
                let _ = writeln!(output, "- {}", one_line(responsibility));
            }
            output.push_str("Boundaries:\n");
            for boundary in &role.role.boundaries {
                let _ = writeln!(output, "- {}", one_line(boundary));
            }
            let _ = writeln!(
                output,
                "Assignment: {} (active since {})",
                assignment.assignment.assignment_id,
                assignment
                    .assignment
                    .started_at
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            );
            output.push_str(
                "Boundary: this Assignment ID is the current role-bearing write fence. \
                 Re-resolve it before each write; never reuse it after replacement or end.\n",
            );
            output.push_str(
                "Continuity: append a structured Role Checkpoint after a material change to \
                 progress, blockers, risks, open questions, or next steps.\n",
            );
        }
    }

    if matches!(&brief.state, RoleBriefMemberState::Assigned { .. }) {
        if brief.responsible_work.is_empty() {
            output.push_str("Responsible Work: none\n");
        } else {
            output.push_str("Responsible Work:\n");
            for responsible in &brief.responsible_work {
                let ProjectViewObjectData::Work(work) = &responsible.work.object.data else {
                    continue;
                };
                match &responsible.state {
                    RoleBriefWorkState::Committed { commitment } => {
                        let _ = writeln!(
                            output,
                            "- {} [{}] — committed via {}",
                            one_line(&work.title),
                            work.status.as_str(),
                            commitment.commitment.commitment_id
                        );
                    }
                    RoleBriefWorkState::WaitingForContinuation => {
                        let _ = writeln!(
                            output,
                            "- {} [{}] — waiting for continuation",
                            one_line(&work.title),
                            work.status.as_str()
                        );
                    }
                }
            }
        }
    }

    if !brief.related_objects.is_empty() {
        output.push_str("Related Project View slice:\n");
        for related in &brief.related_objects {
            let _ = writeln!(
                output,
                "- {} {}: {}",
                related.object.object_type.as_str(),
                related.object.id,
                object_title(&related.object)
            );
        }
    }
    if let Some(latest) = &brief.latest_checkpoint {
        let checkpoint = &latest.checkpoint;
        let _ = writeln!(
            output,
            "Latest Role Checkpoint: {} (Assignment {}, based on project revision {})",
            checkpoint.checkpoint_id,
            checkpoint.assignment_id,
            checkpoint.based_on_project_revision
        );
        let _ = writeln!(
            output,
            "Situation: {}",
            one_line(&checkpoint.content.summary)
        );
        render_named_items(
            &mut output,
            "Current focus",
            &checkpoint.content.current_focus,
        );
        render_named_items(&mut output, "Progress", &checkpoint.content.progress);
        render_named_items(&mut output, "Blockers", &checkpoint.content.blockers);
        render_named_items(&mut output, "Risks", &checkpoint.content.risks);
        render_named_items(
            &mut output,
            "Open questions",
            &checkpoint.content.open_questions,
        );
        render_named_items(&mut output, "Next steps", &checkpoint.content.next_steps);
        render_references(
            &mut output,
            "Checkpoint references",
            &checkpoint.content.references,
        );
    } else if matches!(&brief.state, RoleBriefMemberState::Assigned { .. }) {
        output.push_str("Latest Role Checkpoint: none\n");
    }
    if !brief.recent_handoffs.is_empty() {
        output.push_str("Recent Role Handoffs:\n");
        for item in &brief.recent_handoffs {
            let handoff = &item.handoff;
            let summary = handoff
                .content
                .summary
                .as_deref()
                .map_or_else(|| "no summary".to_owned(), one_line);
            let _ = writeln!(
                output,
                "- {} [{}] from Assignment {}{} — {}",
                handoff.handoff_id,
                handoff.cause.as_str(),
                handoff.from_assignment_id,
                handoff
                    .to_assignment_id
                    .map_or_else(String::new, |id| format!(" to Assignment {id}")),
                summary
            );
            for unresolved in &handoff.content.unresolved_items {
                let _ = writeln!(output, "  - unresolved: {}", one_line(unresolved));
            }
            for reference in &handoff.content.references {
                let _ = writeln!(output, "  - reference: {}", reference_line(reference));
            }
        }
    }
    let _ = writeln!(
        output,
        "Source revisions: project={} generation={} meta={} membership={}",
        brief.project_revision,
        brief.projection_generation,
        brief.source_revisions.meta_event_id,
        brief.source_revisions.membership_event_id
    );
    let _ = writeln!(
        output,
        "Generated: {}",
        brief
            .generated_at
            .to_rfc3339_opts(SecondsFormat::Secs, true)
    );
    output
}

fn render_role_directory(output: &mut String, directory: &RoleBriefRoleDirectory) {
    if directory.total_active_roles == 0 {
        output.push_str("Role Directory: none (0 active)\n");
        return;
    }

    let _ = writeln!(
        output,
        "Role Directory: {}/{} active shown",
        directory.entries.len(),
        directory.total_active_roles
    );
    for entry in &directory.entries {
        let current = if entry.is_current_member_role {
            ", current"
        } else {
            ""
        };
        let staffing = match &entry.assignment {
            RoleBriefRoleDirectoryAssignment::Assigned {
                assignment_id,
                member_pubkey,
                ..
            } => format!(
                "assigned to {} via Assignment {}",
                member_pubkey.to_hex(),
                assignment_id
            ),
            RoleBriefRoleDirectoryAssignment::Vacant => "vacant".to_owned(),
        };
        let _ = writeln!(
            output,
            "- {} [{}{}] — Role {}; {} — {}",
            one_line(&entry.name),
            entry.level.as_str(),
            current,
            entry.role_id,
            staffing,
            entry.purpose_summary
        );
    }
    if directory.omitted_active_roles > 0 {
        let _ = writeln!(
            output,
            "Role Directory omitted: {} active Role(s). Run `buzz roles list` for the complete \
             directory; omitted Roles still exist.",
            directory.omitted_active_roles
        );
    }
}

fn render_named_items(output: &mut String, heading: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(output, "{heading}:");
    for item in items {
        let _ = writeln!(output, "- {}", one_line(item));
    }
}

fn render_references(output: &mut String, heading: &str, references: &[RoleContinuityReference]) {
    if references.is_empty() {
        return;
    }
    let _ = writeln!(output, "{heading}:");
    for reference in references {
        let _ = writeln!(output, "- {}", reference_line(reference));
    }
}

fn reference_line(reference: &RoleContinuityReference) -> String {
    match reference {
        RoleContinuityReference::Object { object_id, label } => {
            labeled_reference("object", object_id, label.as_deref())
        }
        RoleContinuityReference::Assignment {
            assignment_id,
            label,
        } => labeled_reference("assignment", assignment_id, label.as_deref()),
        RoleContinuityReference::Commitment {
            commitment_id,
            label,
        } => labeled_reference("commitment", commitment_id, label.as_deref()),
        RoleContinuityReference::NostrEvent { event_id, label } => {
            labeled_reference("event", event_id, label.as_deref())
        }
    }
}

fn labeled_reference(
    reference_type: &str,
    id: impl std::fmt::Display,
    label: Option<&str>,
) -> String {
    label.map_or_else(
        || format!("{reference_type} {id}"),
        |label| format!("{reference_type} {id} ({})", one_line(label)),
    )
}

/// Render a fail-closed dynamic prompt section when no current Brief can be
/// verified.
#[must_use]
pub fn unavailable_role_brief_markdown(code: &str, detail: &str) -> String {
    unavailable_markdown(&format!("{code}: {}", one_line(detail)))
}

fn validate_membership_pointer(
    meta: &V2MetaProjection,
    membership: &V2MembershipProjection,
) -> Result<(), SdkError> {
    if meta.membership_snapshot_event_id != membership.event_id {
        return Err(invalid(
            "metadata membership pointer differs from the supplied membership snapshot",
        ));
    }
    Ok(())
}

fn validate_projection_basis(
    project_id: buzz_core::CommunityId,
    projection_generation: u64,
    project_revision: u64,
    meta: &V2MetaProjection,
) -> Result<(), SdkError> {
    if project_id != meta.project_id {
        return Err(invalid("v2 head belongs to a different Project"));
    }
    if projection_generation != meta.projection_generation {
        return Err(invalid(
            "v2 head projection generation differs from metadata",
        ));
    }
    if project_revision > meta.project_revision {
        return Err(invalid("v2 head is newer than metadata"));
    }
    Ok(())
}

struct VerifiedCountInputs<'a> {
    active_objects: usize,
    proposals: &'a BTreeMap<Uuid, EntityHead<RoleAssignmentProposal>>,
    assignments: &'a BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    commitments: &'a BTreeMap<Uuid, EntityHead<WorkCommitment>>,
    checkpoints: usize,
    handoffs: usize,
    complete_history: bool,
}

fn validate_counts(
    meta: &V2MetaProjection,
    input: VerifiedCountInputs<'_>,
) -> Result<(), SdkError> {
    let open_proposals = input
        .proposals
        .values()
        .filter(|head| head.entity.status == ProposalStatus::Open)
        .count();
    let active_assignments = input
        .assignments
        .values()
        .filter(|head| head.entity.is_active())
        .count();
    let active_commitments = input
        .commitments
        .values()
        .filter(|head| head.entity.is_active())
        .count();
    let counts = meta.entity_counts;
    let history_counts_match = if input.complete_history {
        usize::try_from(counts.checkpoints).ok() == Some(input.checkpoints)
            && usize::try_from(counts.handoffs).ok() == Some(input.handoffs)
    } else {
        usize::try_from(counts.checkpoints).is_ok_and(|count| input.checkpoints <= count)
            && usize::try_from(counts.handoffs).is_ok_and(|count| input.handoffs <= count)
    };
    if usize::try_from(counts.active_objects).ok() != Some(input.active_objects)
        || usize::try_from(counts.open_proposals).ok() != Some(open_proposals)
        || usize::try_from(counts.active_assignments).ok() != Some(active_assignments)
        || usize::try_from(counts.active_commitments).ok() != Some(active_commitments)
        || !history_counts_match
    {
        return Err(invalid(
            "v2 metadata counts disagree with the verified current heads or history coverage",
        ));
    }
    Ok(())
}

fn validate_project_state(
    meta: &V2MetaProjection,
    entries: Vec<ProjectViewEntry>,
) -> Result<ProjectView, SdkError> {
    let initialized_at = entries.iter().find_map(|entry| match entry {
        ProjectViewEntry::Active(object)
            if object.object_type == ProjectViewObjectType::ProjectProfile =>
        {
            Some(object.created_at)
        }
        ProjectViewEntry::Active(_) | ProjectViewEntry::Tombstone(_) => None,
    });
    let state = ProjectViewState::from_snapshot(
        meta.project_id,
        meta.project_revision,
        initialized_at,
        Some(meta.updated_at),
        entries,
    )
    .map_err(|error| invalid(format!("invalid Project View state: {error}")))?;
    ProjectView::assemble(&state)
        .map_err(|error| invalid(format!("cannot assemble Project View: {error}")))
}

struct ContinuityHeads<'a> {
    entries: &'a BTreeMap<Uuid, ProjectViewEntry>,
    roles: &'a BTreeMap<Uuid, EntityHead<RoleDefinition>>,
    proposals: &'a BTreeMap<Uuid, EntityHead<RoleAssignmentProposal>>,
    assignments: &'a BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    commitments: &'a BTreeMap<Uuid, EntityHead<WorkCommitment>>,
    checkpoints: &'a BTreeMap<Uuid, EntityHead<RoleCheckpoint>>,
    handoffs: &'a BTreeMap<Uuid, EntityHead<RoleHandoff>>,
    objects: &'a BTreeMap<Uuid, ObjectHead>,
}

fn validate_continuity(
    heads: ContinuityHeads<'_>,
    membership: &V2MembershipProjection,
    complete_history: bool,
) -> Result<(), SdkError> {
    let ContinuityHeads {
        entries,
        roles,
        proposals,
        assignments,
        commitments,
        checkpoints,
        handoffs,
        objects,
    } = heads;

    for proposal in proposals.values() {
        if entries
            .get(&proposal.entity.role_id)
            .is_none_or(|entry| entry.object_type() != ProjectViewObjectType::Role)
        {
            return Err(invalid("Proposal references a missing Role"));
        }
        if proposal.entity.status == ProposalStatus::Open
            && !roles
                .get(&proposal.entity.role_id)
                .is_some_and(|role| role.entity.active)
        {
            return Err(invalid("open Proposal references an inactive Role"));
        }
    }
    for assignment in assignments.values() {
        if entries
            .get(&assignment.entity.role_id)
            .is_none_or(|entry| entry.object_type() != ProjectViewObjectType::Role)
        {
            return Err(invalid("Assignment references a missing Role"));
        }
        if !proposals.contains_key(&assignment.entity.proposal_id)
            && (complete_history || assignment.entity.is_active())
        {
            return Err(invalid("Assignment references a missing Proposal"));
        }
    }
    for checkpoint in checkpoints.values() {
        if entries
            .get(&checkpoint.entity.role_id)
            .is_none_or(|entry| entry.object_type() != ProjectViewObjectType::Role)
        {
            return Err(invalid("Checkpoint references a missing Role"));
        }
        match assignments.get(&checkpoint.entity.assignment_id) {
            Some(assignment)
                if assignment.entity.role_id == checkpoint.entity.role_id
                    && assignment.entity.member_pubkey == checkpoint.entity.created_by => {}
            Some(_) => {
                return Err(invalid(
                    "Checkpoint attribution disagrees with its Role or Assignment",
                ));
            }
            None if complete_history => {
                return Err(invalid("Checkpoint references a missing Assignment"));
            }
            None => {}
        }
        if checkpoint.entity.entity_revision != 1
            || checkpoint.entity.based_on_project_revision == 0
            || checkpoint.entity.based_on_project_revision >= checkpoint.entity.project_revision
        {
            return Err(invalid("Checkpoint revision basis is invalid"));
        }
        if let Some(superseded_id) = checkpoint.entity.supersedes_checkpoint_id {
            match checkpoints.get(&superseded_id) {
                Some(superseded)
                    if superseded.entity.role_id == checkpoint.entity.role_id
                        && superseded.entity.assignment_id == checkpoint.entity.assignment_id
                        && superseded.entity.project_revision
                            < checkpoint.entity.project_revision
                        && superseded.entity.checkpoint_id != checkpoint.entity.checkpoint_id => {}
                Some(_) => {
                    return Err(invalid(
                        "Checkpoint supersedes an unrelated or newer Checkpoint",
                    ));
                }
                None if complete_history => {
                    return Err(invalid("Checkpoint supersedes a missing Checkpoint"));
                }
                None => {}
            }
        }
        validate_continuity_references(
            &checkpoint.entity.content.references,
            entries,
            assignments,
            commitments,
            complete_history,
        )?;
    }
    for handoff in handoffs.values() {
        if entries
            .get(&handoff.entity.role_id)
            .is_none_or(|entry| entry.object_type() != ProjectViewObjectType::Role)
        {
            return Err(invalid("Handoff references a missing Role"));
        }
        let from_assignment = assignments.get(&handoff.entity.from_assignment_id);
        match from_assignment {
            Some(assignment) if assignment.entity.role_id == handoff.entity.role_id => {}
            Some(_) => {
                return Err(invalid(
                    "Handoff disagrees with its Role or source Assignment",
                ));
            }
            None if complete_history => {
                return Err(invalid("Handoff references a missing source Assignment"));
            }
            None => {}
        }
        if let Some(to_assignment_id) = handoff.entity.to_assignment_id {
            match assignments.get(&to_assignment_id) {
                Some(to_assignment) if to_assignment.entity.role_id == handoff.entity.role_id => {}
                Some(_) => return Err(invalid("Handoff target belongs to another Role")),
                None if complete_history => {
                    return Err(invalid("Handoff references a missing target Assignment"));
                }
                None => {}
            }
        }
        if let Some(checkpoint_id) = handoff.entity.checkpoint_id {
            match checkpoints.get(&checkpoint_id) {
                Some(checkpoint)
                    if checkpoint.entity.role_id == handoff.entity.role_id
                        && checkpoint.entity.assignment_id == handoff.entity.from_assignment_id => {
                }
                Some(_) => {
                    return Err(invalid(
                        "Handoff Checkpoint belongs to another Role or Assignment",
                    ));
                }
                None if complete_history => {
                    return Err(invalid("Handoff references a missing Checkpoint"));
                }
                None => {}
            }
        }
        for commitment_id in &handoff.entity.affected_commitment_ids {
            match commitments.get(commitment_id) {
                Some(commitment)
                    if commitment.entity.assignment_id == handoff.entity.from_assignment_id => {}
                Some(_) => {
                    return Err(invalid(
                        "Handoff affected Commitment belongs to another Assignment",
                    ));
                }
                None if complete_history => {
                    return Err(invalid("Handoff references a missing affected Commitment"));
                }
                None => {}
            }
        }
        match (handoff.entity.system_generated, handoff.entity.created_by) {
            (true, None) => {}
            (false, Some(author))
                if matches!(
                    handoff.entity.cause,
                    buzz_project_view::v2::HandoffCause::Planned
                        | buzz_project_view::v2::HandoffCause::Other
                ) && from_assignment
                    .is_none_or(|assignment| assignment.entity.member_pubkey == author) => {}
            _ => return Err(invalid("Handoff author or cause is invalid")),
        }
        validate_continuity_references(
            &handoff.entity.content.references,
            entries,
            assignments,
            commitments,
            complete_history,
        )?;
    }

    for work in objects
        .values()
        .filter(|head| head.object.object_type == ProjectViewObjectType::Work)
    {
        if let Some(role_id) = work.responsible_role_id {
            if !roles.get(&role_id).is_some_and(|role| role.entity.active) {
                return Err(invalid(
                    "Work responsibility references a missing or inactive Role",
                ));
            }
        }
    }

    let mut actively_committed_work = HashSet::new();
    for commitment in commitments.values() {
        let assignment = assignments
            .get(&commitment.entity.assignment_id)
            .ok_or_else(|| invalid("Commitment references a missing Assignment"))?;
        if assignment.entity.member_pubkey != commitment.entity.member_pubkey {
            return Err(invalid("Commitment member disagrees with its Assignment"));
        }
        if !matches!(
            (
                commitment.entity.ended_at,
                commitment.entity.ended_by,
                commitment.entity.ended_reason
            ),
            (None, None, None) | (Some(_), Some(_), Some(_))
        ) {
            return Err(invalid("Commitment terminal attribution is incomplete"));
        }
        if commitment.entity.is_active() {
            if !assignment.entity.is_active() {
                return Err(invalid("active Commitment references an ended Assignment"));
            }
            let work = objects
                .get(&commitment.entity.work_id)
                .ok_or_else(|| invalid("active Commitment references a missing Work"))?;
            if !work_is_open(&work.object) {
                return Err(invalid(
                    "active Commitment references terminal or non-Work state",
                ));
            }
            if work.responsible_role_id != Some(assignment.entity.role_id) {
                return Err(invalid(
                    "active Commitment disagrees with Work responsibility",
                ));
            }
            if !actively_committed_work.insert(commitment.entity.work_id) {
                return Err(invalid("one Work has multiple active Commitments"));
            }
        }
    }

    let members = membership
        .members
        .iter()
        .map(|member| (member.pubkey, member.role))
        .collect::<BTreeMap<_, _>>();
    let mut active_roles = HashSet::new();
    let mut active_members = HashSet::new();
    for assignment in assignments.values().filter(|head| head.entity.is_active()) {
        let role = roles
            .get(&assignment.entity.role_id)
            .ok_or_else(|| invalid("active Assignment references a missing Role"))?;
        if !role.entity.active {
            return Err(invalid("active Assignment references an inactive Role"));
        }
        if !active_roles.insert(assignment.entity.role_id) {
            return Err(invalid("one Role has multiple active Assignments"));
        }
        if !active_members.insert(assignment.entity.member_pubkey) {
            return Err(invalid("one Member has multiple active Assignments"));
        }
        let community_role = members
            .get(&assignment.entity.member_pubkey)
            .ok_or_else(|| invalid("active Assignment assignee is absent from membership"))?;
        let expected = match role.entity.level {
            RoleLevel::Admin => CommunityMemberRole::Admin,
            RoleLevel::Member => CommunityMemberRole::Member,
        };
        if *community_role != CommunityMemberRole::Owner && *community_role != expected {
            return Err(invalid(
                "active Assignment disagrees with Community membership",
            ));
        }
    }
    for (pubkey, community_role) in members {
        if community_role == CommunityMemberRole::Admin
            && !assignments.values().any(|assignment| {
                assignment.entity.is_active()
                    && assignment.entity.member_pubkey == pubkey
                    && roles
                        .get(&assignment.entity.role_id)
                        .is_some_and(|role| role.entity.level == RoleLevel::Admin)
            })
        {
            return Err(invalid(
                "non-owner Community admin has no active Leader Assignment",
            ));
        }
    }
    Ok(())
}

fn validate_continuity_references(
    references: &[RoleContinuityReference],
    entries: &BTreeMap<Uuid, ProjectViewEntry>,
    assignments: &BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    commitments: &BTreeMap<Uuid, EntityHead<WorkCommitment>>,
    complete_history: bool,
) -> Result<(), SdkError> {
    for reference in references {
        let exists = match reference {
            RoleContinuityReference::Object { object_id, .. } => entries.contains_key(object_id),
            RoleContinuityReference::Assignment { assignment_id, .. } => {
                !complete_history || assignments.contains_key(assignment_id)
            }
            RoleContinuityReference::Commitment { commitment_id, .. } => {
                !complete_history || commitments.contains_key(commitment_id)
            }
            // Raw event existence and Community scope are enforced while the
            // Relay commits the reference. The v2 projection snapshot does not
            // intentionally contain every historical Nostr event.
            RoleContinuityReference::NostrEvent { .. } => true,
        };
        if !exists {
            return Err(invalid(
                "Checkpoint or Handoff references missing canonical Project state",
            ));
        }
    }
    Ok(())
}

fn source_reference(
    event_id: EventId,
    project_revision: u64,
    item_revision: u64,
    source: &V2ProjectionSource,
) -> RoleBriefSourceReference {
    RoleBriefSourceReference {
        event_id,
        project_revision,
        item_revision,
        change_id: source.change_id(),
        source_type: source.source_type().to_owned(),
    }
}

fn role_brief_object(head: &ObjectHead) -> RoleBriefObject {
    RoleBriefObject {
        object: head.object.clone(),
        responsible_role_id: head.responsible_role_id,
        source: head.source.clone(),
    }
}

fn work_is_open(object: &ProjectViewObject) -> bool {
    matches!(
        &object.data,
        ProjectViewObjectData::Work(ProjectWork {
            status: WorkStatus::Pending
                | WorkStatus::InProgress
                | WorkStatus::Paused
                | WorkStatus::Submitted,
            ..
        })
    )
}

fn project_object_from_role(role: &RoleDefinition) -> ProjectViewObject {
    ProjectViewObject {
        id: role.role_id,
        object_type: ProjectViewObjectType::Role,
        object_revision: role.object_revision,
        project_revision: role.project_revision,
        created_at: role.created_at,
        updated_at: role.updated_at,
        created_by: role.created_by,
        updated_by: role.updated_by,
        data: ProjectViewObjectData::Role(ProjectRole {
            name: role.name.clone(),
            purpose: role.purpose.clone(),
            responsibilities: role.responsibilities.clone(),
            boundaries: role.boundaries.clone(),
            active: role.active,
        }),
        relations: ProjectViewRelations::default(),
    }
}

fn object_order(left: &ProjectViewObject, right: &ProjectViewObject) -> std::cmp::Ordering {
    left.created_at
        .cmp(&right.created_at)
        .then(left.id.cmp(&right.id))
}

fn object_title(object: &ProjectViewObject) -> String {
    match &object.data {
        ProjectViewObjectData::ProjectProfile(value) => one_line(&value.name),
        ProjectViewObjectData::Goal(value) => one_line(&value.title),
        ProjectViewObjectData::Role(value) => one_line(&value.name),
        ProjectViewObjectData::Plan(value) => one_line(&value.title),
        ProjectViewObjectData::Stage(value) => one_line(&value.title),
        ProjectViewObjectData::Requirement(value) => one_line(&value.title),
        ProjectViewObjectData::Issue(ProjectIssue { title, .. }) => one_line(title),
        ProjectViewObjectData::Work(ProjectWork { title, .. }) => one_line(title),
        ProjectViewObjectData::Resource(value) => one_line(&value.name),
    }
}

fn one_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn role_directory_purpose_summary(value: &str) -> String {
    let line = one_line(value);
    if line.chars().count() <= ROLE_DIRECTORY_PURPOSE_MAX_CHARS {
        return line;
    }

    let mut summary = line
        .chars()
        .take(ROLE_DIRECTORY_PURPOSE_MAX_CHARS - 1)
        .collect::<String>();
    summary.push('…');
    summary
}

const fn role_level_order(level: RoleLevel) -> u8 {
    match level {
        RoleLevel::Admin => 0,
        RoleLevel::Member => 1,
    }
}

fn unavailable_markdown(detail: &str) -> String {
    format!(
        "[Role Brief]\nState: unavailable\n\
         Boundary: current Project Role/Assignment could not be verified. Do not perform \
         role-bearing Project View writes. Diagnostic reads remain allowed.\n\
         Detail: {}\n",
        one_line(detail)
    )
}

fn invalid(message: impl Into<String>) -> SdkError {
    SdkError::InvalidProjection(message.into())
}

#[cfg(test)]
mod tests {
    use buzz_core::CommunityId;
    use buzz_project_view::v2::{
        HandoffCause, ProposalType, RoleCheckpointContent, RoleContinuityChange,
        RoleContinuityReference, RoleHandoffContent,
    };
    use buzz_project_view::{
        Goal, IssueStatus, Priority, ProjectIssue, ProjectProfile, ProjectViewObjectData,
        ProjectViewRelations, ProjectWork, WorkStatus,
    };
    use chrono::Duration;
    use nostr::{Keys, Timestamp};

    use super::*;
    use crate::project_view_v2::{
        V2EntityCounts, V2MembershipMember, V2ProjectedObject, V2ProjectionSource,
    };

    struct Fixture {
        snapshot: VerifiedRoleBriefSnapshot,
        agent: PublicKey,
        assignment_id: Uuid,
        role_id: Uuid,
    }

    #[test]
    fn assigned_brief_contains_fence_project_context_and_related_slice() {
        let fixture = fixture();
        let brief = fixture
            .snapshot
            .brief_for(fixture.agent, instant())
            .expect("assemble assigned Brief");

        assert_eq!(brief.assignment_id(), Some(fixture.assignment_id));
        assert!(matches!(
            &brief.state,
            RoleBriefMemberState::Assigned { role, .. }
                if role.role.role_id == fixture.role_id
        ));
        assert_eq!(brief.project.goals.len(), 1);
        assert_eq!(brief.responsible_work.len(), 1);
        assert!(matches!(
            brief.responsible_work[0].state,
            RoleBriefWorkState::Committed { .. }
        ));
        assert_eq!(brief.related_objects.len(), 2);
        let related_types = brief
            .related_objects
            .iter()
            .map(|item| item.object.object_type)
            .collect::<Vec<_>>();
        assert!(related_types.contains(&ProjectViewObjectType::Issue));
        assert!(related_types.contains(&ProjectViewObjectType::Work));
        assert_eq!(brief.role_directory.total_active_roles, 1);
        assert_eq!(brief.role_directory.omitted_active_roles, 0);
        let [directory_entry] = brief.role_directory.entries.as_slice() else {
            panic!("assigned Brief should contain its active Role");
        };
        assert_eq!(directory_entry.role_id, fixture.role_id);
        assert!(directory_entry.is_current_member_role);
        assert!(matches!(
            directory_entry.assignment,
            RoleBriefRoleDirectoryAssignment::Assigned {
                assignment_id,
                member_pubkey,
                ..
            } if assignment_id == fixture.assignment_id && member_pubkey == fixture.agent
        ));

        let markdown = render_role_brief_markdown(&brief);
        assert!(markdown.starts_with("[Role Brief]\nState: assigned"));
        assert!(markdown.contains("Community role: member"));
        assert!(markdown.contains("Role Directory: 1/1 active shown"));
        assert!(markdown.contains("[member, current]"));
        assert!(markdown.contains(&fixture.assignment_id.to_string()));
        assert!(markdown.contains("Responsible Work:"));
        assert!(markdown.contains("committed via"));
        assert!(markdown.contains("Related Project View slice:"));
        assert_eq!(fixture.snapshot.checkpoints().count(), 2);
        assert_eq!(
            brief.latest_checkpoint.as_ref().map(|latest| latest
                .checkpoint
                .content
                .summary
                .as_str()),
            Some("Timeline is ready for clients")
        );
        assert_eq!(brief.recent_handoffs.len(), 1);
        assert!(markdown.contains("Latest Role Checkpoint:"));
        assert!(markdown.contains("Recent Role Handoffs:"));
        assert!(markdown.contains("unresolved: publish the desktop timeline"));
        assert!(markdown.contains("Source revisions: project=7 generation=1"));

        let binding = render_role_binding_markdown(&brief);
        assert!(binding.starts_with("[Role Binding]\nState: assigned"));
        assert!(binding.contains(&format!("Role ID: {}", fixture.role_id)));
        assert!(binding.contains(&format!("Assignment: {}", fixture.assignment_id)));
        assert!(binding.contains("Level: member"));
        assert!(binding.contains("not cached authorization"));
        assert!(binding.contains("Source revisions: project=7 generation=1"));
        assert!(!binding.contains("Responsible Work:"));
        assert!(!binding.contains("Latest Role Checkpoint:"));
        assert!(!binding.contains("Role Directory:"));
    }

    #[test]
    fn bounded_history_slice_preserves_exact_current_counts_and_latest_brief() {
        let fixture = fixture();
        let objects = object_projections(&fixture.snapshot);
        let entities = entity_projections(&fixture.snapshot)
            .into_iter()
            .filter(|projection| {
                !matches!(
                    &projection.entity,
                    RoleContinuityChange::Checkpoint(checkpoint)
                        if checkpoint.content.summary == "Role Brief projection is wired"
                )
            })
            .collect::<Vec<_>>();
        assert!(VerifiedRoleBriefSnapshot::new(
            fixture.snapshot.meta.clone(),
            fixture.snapshot.membership.clone(),
            objects.clone(),
            entities.clone(),
        )
        .is_err());

        let snapshot = VerifiedRoleBriefSnapshot::new_with_partial_history(
            fixture.snapshot.meta.clone(),
            fixture.snapshot.membership.clone(),
            objects,
            entities,
        )
        .expect("bounded history slice");
        let brief = snapshot
            .brief_for(fixture.agent, instant())
            .expect("Brief from bounded history");
        assert_eq!(snapshot.checkpoints().count(), 1);
        assert_eq!(
            brief
                .latest_checkpoint
                .map(|item| item.checkpoint.content.summary),
            Some("Timeline is ready for clients".to_owned())
        );
    }

    #[test]
    fn unassigned_member_gets_candidate_boundary_without_assignment() {
        let fixture = fixture();
        let candidate = Keys::generate().public_key();
        let brief = fixture
            .snapshot
            .brief_for(candidate, instant())
            .expect("assemble candidate Brief");

        assert_eq!(brief.assignment_id(), None);
        assert!(matches!(
            brief.state,
            RoleBriefMemberState::Candidate { .. }
        ));
        assert_eq!(brief.role_directory.total_active_roles, 1);
        assert!(brief
            .role_directory
            .entries
            .iter()
            .all(|entry| !entry.is_current_member_role));
        assert!(matches!(
            brief.role_directory.entries[0].assignment,
            RoleBriefRoleDirectoryAssignment::Assigned {
                member_pubkey,
                ..
            } if member_pubkey == fixture.agent
        ));
        let markdown = render_role_brief_markdown(&brief);
        assert!(markdown.contains("State: candidate"));
        assert!(markdown.contains("no active Assignment is verified"));
        assert!(markdown.contains("Role Directory: 1/1 active shown"));

        let binding = render_role_binding_markdown(&brief);
        assert!(binding.starts_with("[Role Binding]\nState: candidate"));
        assert!(binding.contains("Role ID: none"));
        assert!(binding.contains("Assignment: none"));
        assert!(binding.contains("no active Assignment is verified for this meta head"));
        assert!(!binding.contains("Role Directory:"));
    }

    #[test]
    fn role_directory_is_bounded_sorted_and_explicit_about_omissions() {
        let fixture = fixture();
        let leader_id = Uuid::new_v4();
        let inactive_id = Uuid::new_v4();
        let mut roles = vec![
            role_definition(
                leader_id,
                "Architecture leader",
                &"Coordinate cross-role decisions ".repeat(12),
                RoleLevel::Admin,
                true,
                fixture.agent,
            ),
            role_definition(
                inactive_id,
                "Retired responsibility",
                "No longer assignable",
                RoleLevel::Member,
                false,
                fixture.agent,
            ),
        ];
        roles.extend((0..32).map(|index| {
            role_definition(
                Uuid::new_v4(),
                &format!("Module {index:02}"),
                "Maintain one stable module boundary",
                RoleLevel::Member,
                true,
                fixture.agent,
            )
        }));
        let snapshot = snapshot_with_additional_roles(&fixture.snapshot, roles);

        let assigned = snapshot
            .brief_for(fixture.agent, instant())
            .expect("assigned Brief with bounded directory");
        assert_eq!(assigned.role_directory.total_active_roles, 34);
        assert_eq!(
            assigned.role_directory.entries.len(),
            ROLE_DIRECTORY_MAX_ENTRIES
        );
        assert_eq!(assigned.role_directory.omitted_active_roles, 2);
        assert_eq!(
            assigned.role_directory.entries[0].role_id, fixture.role_id,
            "the target Member's Role sorts first"
        );
        assert_eq!(
            assigned.role_directory.entries[1].role_id, leader_id,
            "Leader Roles sort before remaining member Roles"
        );
        assert!(assigned
            .role_directory
            .entries
            .iter()
            .all(|entry| entry.role_id != inactive_id));
        let leader = assigned
            .role_directory
            .entries
            .iter()
            .find(|entry| entry.role_id == leader_id)
            .expect("Leader remains inside the bounded directory");
        assert!(matches!(
            leader.assignment,
            RoleBriefRoleDirectoryAssignment::Vacant
        ));
        assert_eq!(
            leader.purpose_summary.chars().count(),
            ROLE_DIRECTORY_PURPOSE_MAX_CHARS
        );
        assert!(leader.purpose_summary.ends_with('…'));
        let markdown = render_role_brief_markdown(&assigned);
        assert!(markdown.contains("Role Directory omitted: 2 active Role(s)"));
        assert!(markdown.contains("`buzz roles list`"));
        assert!(markdown.contains("omitted Roles still exist"));

        let candidate = snapshot
            .brief_for(Keys::generate().public_key(), instant())
            .expect("candidate Brief with bounded directory");
        assert_eq!(candidate.role_directory.entries[0].role_id, leader_id);
        assert!(candidate
            .role_directory
            .entries
            .iter()
            .all(|entry| !entry.is_current_member_role));
    }

    #[test]
    fn ended_assignment_history_never_staffs_a_role_directory_entry() {
        let fixture = fixture();
        let old_member = Keys::generate().public_key();
        let old_proposal_id = Uuid::new_v4();
        let old_assignment_id = Uuid::new_v4();
        let mut proposal = fixture
            .snapshot
            .proposals
            .values()
            .next()
            .expect("fixture Proposal")
            .entity
            .clone();
        proposal.proposal_id = old_proposal_id;
        proposal.candidate_pubkey = old_member;
        let mut assignment = fixture
            .snapshot
            .assignments
            .values()
            .next()
            .expect("fixture Assignment")
            .entity
            .clone();
        assignment.assignment_id = old_assignment_id;
        assignment.member_pubkey = old_member;
        assignment.proposal_id = old_proposal_id;
        assignment.ended_at = Some(instant());
        assignment.ended_by = Some(
            fixture
                .snapshot
                .membership
                .members
                .iter()
                .find(|member| member.role == CommunityMemberRole::Owner)
                .expect("fixture owner")
                .pubkey,
        );
        assignment.ended_reason = Some(buzz_project_view::v2::AssignmentEndReason::Replaced);
        assignment.replaced_by_assignment_id = Some(fixture.assignment_id);
        assignment.entity_revision = 2;
        assignment.project_revision = 6;

        let mut entities = entity_projections(&fixture.snapshot);
        entities.push(projected_entity(
            180,
            fixture.snapshot.meta.project_id,
            proposal.project_revision,
            proposal.entity_revision,
            RoleContinuityChange::Proposal(proposal),
            source(200),
            instant(),
        ));
        entities.push(projected_entity(
            181,
            fixture.snapshot.meta.project_id,
            assignment.project_revision,
            assignment.entity_revision,
            RoleContinuityChange::Assignment(assignment),
            source(201),
            instant(),
        ));
        let snapshot = VerifiedRoleBriefSnapshot::new(
            fixture.snapshot.meta.clone(),
            fixture.snapshot.membership.clone(),
            object_projections(&fixture.snapshot),
            entities,
        )
        .expect("snapshot with ended Assignment history");
        let brief = snapshot
            .brief_for(fixture.agent, instant())
            .expect("Brief after replacement history");
        let [entry] = brief.role_directory.entries.as_slice() else {
            panic!("fixture should expose one active Role");
        };
        assert!(matches!(
            entry.assignment,
            RoleBriefRoleDirectoryAssignment::Assigned {
                assignment_id,
                member_pubkey,
                ..
            } if assignment_id == fixture.assignment_id && member_pubkey == fixture.agent
        ));
    }

    #[test]
    fn responsible_work_without_commitment_is_explicitly_waiting() {
        let fixture = fixture();
        let mut meta = fixture.snapshot.meta.clone();
        meta.entity_counts.active_commitments = 0;
        let objects = object_projections(&fixture.snapshot);
        let entities = entity_projections(&fixture.snapshot)
            .into_iter()
            .filter(|projection| !matches!(&projection.entity, RoleContinuityChange::Commitment(_)))
            .collect();
        let snapshot = VerifiedRoleBriefSnapshot::new(
            meta,
            fixture.snapshot.membership.clone(),
            objects,
            entities,
        )
        .expect("snapshot without active Commitment");
        let brief = snapshot
            .brief_for(fixture.agent, instant())
            .expect("waiting Brief");

        assert!(matches!(
            brief.responsible_work.as_slice(),
            [RoleBriefResponsibleWork {
                state: RoleBriefWorkState::WaitingForContinuation,
                ..
            }]
        ));
        assert!(render_role_brief_markdown(&brief).contains("waiting for continuation"));
    }

    #[test]
    fn project_state_and_checkpoint_remain_a_recovery_path_without_handoff() {
        let fixture = fixture();
        let mut meta = fixture.snapshot.meta.clone();
        meta.entity_counts.handoffs = 0;
        let objects = object_projections(&fixture.snapshot);
        let entities = entity_projections(&fixture.snapshot)
            .into_iter()
            .filter(|projection| !matches!(&projection.entity, RoleContinuityChange::Handoff(_)))
            .collect();
        let snapshot = VerifiedRoleBriefSnapshot::new(
            meta,
            fixture.snapshot.membership.clone(),
            objects,
            entities,
        )
        .expect("snapshot without Handoff");
        let brief = snapshot
            .brief_for(fixture.agent, instant())
            .expect("recoverable Brief");

        assert!(brief.recent_handoffs.is_empty());
        assert!(brief.latest_checkpoint.is_some());
        assert_eq!(brief.responsible_work.len(), 1);
        assert_eq!(brief.project.goals.len(), 1);
    }

    #[test]
    fn inconsistent_metadata_fails_before_a_brief_can_be_obtained() {
        let Fixture {
            snapshot,
            agent,
            assignment_id: _,
            role_id: _,
        } = fixture();
        let mut meta = snapshot.meta.clone();
        meta.entity_counts.active_assignments = 0;
        let objects = object_projections(&snapshot);
        let entities = entity_projections(&snapshot);

        let result =
            VerifiedRoleBriefSnapshot::new(meta, snapshot.membership.clone(), objects, entities);
        assert!(matches!(result, Err(SdkError::InvalidProjection(_))));
        assert_eq!(agent, snapshot.assignments().next().unwrap().member_pubkey);
    }

    fn fixture() -> Fixture {
        let project_id = CommunityId::from_uuid(Uuid::new_v4());
        let owner = Keys::generate().public_key();
        let agent = Keys::generate().public_key();
        let role_id = Uuid::new_v4();
        let proposal_id = Uuid::new_v4();
        let assignment_id = Uuid::new_v4();
        let commitment_id = Uuid::new_v4();
        let first_checkpoint_id = Uuid::new_v4();
        let latest_checkpoint_id = Uuid::new_v4();
        let handoff_id = Uuid::new_v4();
        let issue_id = Uuid::new_v4();
        let work_id = Uuid::new_v4();
        let now = instant();
        let source = source(90);

        let profile = object(
            *project_id.as_uuid(),
            ProjectViewObjectType::ProjectProfile,
            1,
            ProjectViewObjectData::ProjectProfile(ProjectProfile {
                name: "Lora".to_owned(),
                positioning: "Project-owned continuity".to_owned(),
                purpose: "Keep project context available across runtimes".to_owned(),
                problem: "Agent-local continuity disappears".to_owned(),
                scope: "Project context and Role continuity".to_owned(),
            }),
            ProjectViewRelations::default(),
            owner,
            now,
        );
        let goal = object(
            Uuid::new_v4(),
            ProjectViewObjectType::Goal,
            1,
            ProjectViewObjectData::Goal(Goal {
                title: "Continuous project work".to_owned(),
                desired_outcome: "A successor resumes from verified state".to_owned(),
                directions: vec!["Keep context project-owned".to_owned()],
            }),
            ProjectViewRelations::default(),
            owner,
            now,
        );
        let issue = object(
            issue_id,
            ProjectViewObjectType::Issue,
            4,
            ProjectViewObjectData::Issue(ProjectIssue {
                title: "Role context may drift".to_owned(),
                description: "Refresh it before every turn".to_owned(),
                status: IssueStatus::Open,
                priority: Priority::High,
            }),
            ProjectViewRelations {
                about: Some(ObjectRef {
                    object_type: ProjectViewObjectType::Role,
                    object_id: role_id,
                }),
                ..ProjectViewRelations::default()
            },
            agent,
            now,
        );
        let work = object(
            work_id,
            ProjectViewObjectType::Work,
            4,
            ProjectViewObjectData::Work(ProjectWork {
                title: "Refresh Role Brief".to_owned(),
                description: "Resolve the active Assignment at turn start".to_owned(),
                status: WorkStatus::InProgress,
                priority: Priority::High,
            }),
            ProjectViewRelations {
                handles: Some(ObjectRef {
                    object_type: ProjectViewObjectType::Issue,
                    object_id: issue_id,
                }),
                ..ProjectViewRelations::default()
            },
            agent,
            now,
        );
        let ordinary = vec![
            projected_object(1, project_id, profile, None, source.clone(), now),
            projected_object(2, project_id, goal, None, source.clone(), now),
            projected_object(3, project_id, issue, None, source.clone(), now),
            projected_object(4, project_id, work, Some(role_id), source.clone(), now),
        ];

        let role = RoleDefinition {
            role_id,
            name: "Continuity developer".to_owned(),
            purpose: "Keep the role resumable".to_owned(),
            responsibilities: vec!["Maintain verified context".to_owned()],
            boundaries: vec!["Act only through the active Assignment".to_owned()],
            level: RoleLevel::Member,
            active: true,
            object_revision: 1,
            project_revision: 2,
            created_at: now,
            updated_at: now,
            created_by: owner,
            updated_by: owner,
        };
        let proposal = RoleAssignmentProposal {
            proposal_id,
            role_id,
            candidate_pubkey: agent,
            proposal_type: ProposalType::Offer,
            candidate_accepted_at: Some(now),
            authorized_by: Some(owner),
            authorized_at: Some(now),
            expected_target_assignment_id: None,
            expected_candidate_assignment_id: None,
            expires_at: now + Duration::days(7),
            status: ProposalStatus::Consumed,
            reason: None,
            created_by: owner,
            created_at: now,
            resolved_at: Some(now),
            entity_revision: 2,
            project_revision: 3,
        };
        let assignment = RoleAssignment {
            assignment_id,
            role_id,
            member_pubkey: agent,
            proposal_id,
            started_at: now,
            started_by: owner,
            replacement_requested_at: None,
            replacement_request_reason: None,
            unable_reported_at: None,
            unable_report_reason: None,
            ended_at: None,
            ended_by: None,
            ended_reason: None,
            replaced_by_assignment_id: None,
            entity_revision: 1,
            project_revision: 3,
        };
        let commitment = WorkCommitment {
            commitment_id,
            work_id,
            assignment_id,
            member_pubkey: agent,
            started_at: now,
            started_by: agent,
            ended_at: None,
            ended_by: None,
            ended_reason: None,
            entity_revision: 1,
            project_revision: 4,
        };
        let first_checkpoint = RoleCheckpoint {
            checkpoint_id: first_checkpoint_id,
            role_id,
            assignment_id,
            based_on_project_revision: 4,
            content: RoleCheckpointContent {
                summary: "Role Brief projection is wired".to_owned(),
                current_focus: vec!["trusted continuity history".to_owned()],
                progress: vec!["Checkpoint domain is complete".to_owned()],
                blockers: Vec::new(),
                risks: Vec::new(),
                open_questions: Vec::new(),
                next_steps: vec!["verify the timeline".to_owned()],
                references: vec![RoleContinuityReference::Object {
                    object_id: work_id,
                    label: Some("active Work".to_owned()),
                }],
            },
            supersedes_checkpoint_id: None,
            created_by: agent,
            created_at: now,
            entity_revision: 1,
            project_revision: 5,
        };
        let latest_checkpoint = RoleCheckpoint {
            checkpoint_id: latest_checkpoint_id,
            role_id,
            assignment_id,
            based_on_project_revision: 5,
            content: RoleCheckpointContent {
                summary: "Timeline is ready for clients".to_owned(),
                current_focus: vec!["desktop history".to_owned()],
                progress: vec!["trusted projection validates".to_owned()],
                blockers: Vec::new(),
                risks: vec!["stale snapshot".to_owned()],
                open_questions: Vec::new(),
                next_steps: vec!["publish the UI".to_owned()],
                references: Vec::new(),
            },
            supersedes_checkpoint_id: Some(first_checkpoint_id),
            created_by: agent,
            created_at: now + Duration::seconds(1),
            entity_revision: 1,
            project_revision: 6,
        };
        let handoff = RoleHandoff {
            handoff_id,
            role_id,
            from_assignment_id: assignment_id,
            to_assignment_id: None,
            checkpoint_id: Some(latest_checkpoint_id),
            affected_commitment_ids: Vec::new(),
            content: RoleHandoffContent {
                summary: Some("Prepare a future successor".to_owned()),
                unresolved_items: vec!["publish the desktop timeline".to_owned()],
                references: vec![RoleContinuityReference::Object {
                    object_id: work_id,
                    label: None,
                }],
            },
            cause: HandoffCause::Planned,
            system_generated: false,
            created_by: Some(agent),
            created_at: now + Duration::seconds(2),
            entity_revision: 1,
            project_revision: 7,
        };
        let entities = vec![
            projected_entity(
                5,
                project_id,
                2,
                1,
                RoleContinuityChange::Role(role),
                source.clone(),
                now,
            ),
            projected_entity(
                6,
                project_id,
                3,
                2,
                RoleContinuityChange::Proposal(proposal),
                source.clone(),
                now,
            ),
            projected_entity(
                7,
                project_id,
                3,
                1,
                RoleContinuityChange::Assignment(assignment),
                source.clone(),
                now,
            ),
            projected_entity(
                8,
                project_id,
                4,
                1,
                RoleContinuityChange::Commitment(commitment),
                source.clone(),
                now,
            ),
            projected_entity(
                11,
                project_id,
                5,
                1,
                RoleContinuityChange::Checkpoint(first_checkpoint),
                source.clone(),
                now,
            ),
            projected_entity(
                12,
                project_id,
                6,
                1,
                RoleContinuityChange::Checkpoint(latest_checkpoint),
                source.clone(),
                now + Duration::seconds(1),
            ),
            projected_entity(
                13,
                project_id,
                7,
                1,
                RoleContinuityChange::Handoff(handoff),
                source.clone(),
                now + Duration::seconds(2),
            ),
        ];

        let membership_event_id = event_id(14);
        let mut members = vec![
            V2MembershipMember {
                pubkey: owner,
                role: CommunityMemberRole::Owner,
            },
            V2MembershipMember {
                pubkey: agent,
                role: CommunityMemberRole::Member,
            },
        ];
        members.sort_by_key(|member| member.pubkey);
        let membership = V2MembershipProjection {
            event_id: membership_event_id,
            members,
            created_at: Timestamp::from(now.timestamp() as u64),
        };
        let meta = V2MetaProjection {
            event_id: event_id(15),
            project_id,
            projection_generation: 1,
            project_revision: 7,
            entity_counts: V2EntityCounts {
                active_objects: 5,
                open_proposals: 0,
                active_assignments: 1,
                active_commitments: 1,
                checkpoints: 2,
                handoffs: 1,
            },
            membership_snapshot_event_id: membership_event_id,
            reset: true,
            changed_heads: Vec::new(),
            source,
            updated_at: now + Duration::seconds(2),
        };
        let snapshot = VerifiedRoleBriefSnapshot::new(meta, membership, ordinary, entities)
            .expect("valid fixture");
        Fixture {
            snapshot,
            agent,
            assignment_id,
            role_id,
        }
    }

    fn object(
        id: Uuid,
        object_type: ProjectViewObjectType,
        project_revision: u64,
        data: ProjectViewObjectData,
        relations: ProjectViewRelations,
        actor: PublicKey,
        now: DateTime<Utc>,
    ) -> ProjectViewObject {
        ProjectViewObject {
            id,
            object_type,
            object_revision: 1,
            project_revision,
            created_at: now,
            updated_at: now,
            created_by: actor,
            updated_by: actor,
            data,
            relations,
        }
    }

    fn projected_object(
        byte: u8,
        project_id: CommunityId,
        object: ProjectViewObject,
        responsible_role_id: Option<Uuid>,
        source: V2ProjectionSource,
        now: DateTime<Utc>,
    ) -> V2ProjectObjectProjection {
        V2ProjectObjectProjection {
            event_id: event_id(byte),
            project_id,
            projection_generation: 1,
            project_revision: object.project_revision,
            source,
            object: V2ProjectedObject::Active(Box::new(object)),
            responsible_role_id,
            updated_at: now,
        }
    }

    fn projected_entity(
        byte: u8,
        project_id: CommunityId,
        project_revision: u64,
        entity_revision: u64,
        entity: RoleContinuityChange,
        source: V2ProjectionSource,
        now: DateTime<Utc>,
    ) -> V2EntityProjection {
        V2EntityProjection {
            event_id: event_id(byte),
            project_id,
            projection_generation: 1,
            project_revision,
            entity_revision,
            source,
            entity,
            updated_at: now,
        }
    }

    fn source(byte: u8) -> V2ProjectionSource {
        let source_event = event_id(byte);
        V2ProjectionSource::NostrEvent {
            change_id: source_event,
            event_id: source_event,
        }
    }

    fn event_id(byte: u8) -> EventId {
        EventId::from_byte_array([byte; 32])
    }

    fn instant() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).expect("valid fixture timestamp")
    }

    fn object_projections(snapshot: &VerifiedRoleBriefSnapshot) -> Vec<V2ProjectObjectProjection> {
        snapshot
            .objects
            .values()
            .enumerate()
            .map(|(index, head)| V2ProjectObjectProjection {
                event_id: event_id(u8::try_from(index + 20).unwrap()),
                project_id: snapshot.meta.project_id,
                projection_generation: snapshot.meta.projection_generation,
                project_revision: head.object.project_revision,
                source: source(u8::try_from(index + 40).unwrap()),
                object: V2ProjectedObject::Active(Box::new(head.object.clone())),
                responsible_role_id: head.responsible_role_id,
                updated_at: head.object.updated_at,
            })
            .collect()
    }

    fn entity_projections(snapshot: &VerifiedRoleBriefSnapshot) -> Vec<V2EntityProjection> {
        let mut entities = Vec::new();
        for (index, role) in snapshot.roles.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 60).unwrap(),
                snapshot.meta.project_id,
                role.entity.project_revision,
                role.entity.object_revision,
                RoleContinuityChange::Role(role.entity.clone()),
                source(u8::try_from(index + 70).unwrap()),
                role.entity.updated_at,
            ));
        }
        for (index, proposal) in snapshot.proposals.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 80).unwrap(),
                snapshot.meta.project_id,
                proposal.entity.project_revision,
                proposal.entity.entity_revision,
                RoleContinuityChange::Proposal(proposal.entity.clone()),
                source(u8::try_from(index + 90).unwrap()),
                instant(),
            ));
        }
        for (index, assignment) in snapshot.assignments.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 100).unwrap(),
                snapshot.meta.project_id,
                assignment.entity.project_revision,
                assignment.entity.entity_revision,
                RoleContinuityChange::Assignment(assignment.entity.clone()),
                source(u8::try_from(index + 110).unwrap()),
                instant(),
            ));
        }
        for (index, commitment) in snapshot.commitments.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 120).unwrap(),
                snapshot.meta.project_id,
                commitment.entity.project_revision,
                commitment.entity.entity_revision,
                RoleContinuityChange::Commitment(commitment.entity.clone()),
                source(u8::try_from(index + 130).unwrap()),
                instant(),
            ));
        }
        for (index, checkpoint) in snapshot.checkpoints.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 140).unwrap(),
                snapshot.meta.project_id,
                checkpoint.entity.project_revision,
                checkpoint.entity.entity_revision,
                RoleContinuityChange::Checkpoint(checkpoint.entity.clone()),
                source(u8::try_from(index + 150).unwrap()),
                checkpoint.entity.created_at,
            ));
        }
        for (index, handoff) in snapshot.handoffs.values().enumerate() {
            entities.push(projected_entity(
                u8::try_from(index + 160).unwrap(),
                snapshot.meta.project_id,
                handoff.entity.project_revision,
                handoff.entity.entity_revision,
                RoleContinuityChange::Handoff(handoff.entity.clone()),
                source(u8::try_from(index + 170).unwrap()),
                handoff.entity.created_at,
            ));
        }
        entities
    }

    fn role_definition(
        role_id: Uuid,
        name: &str,
        purpose: &str,
        level: RoleLevel,
        active: bool,
        actor: PublicKey,
    ) -> RoleDefinition {
        RoleDefinition {
            role_id,
            name: name.to_owned(),
            purpose: purpose.to_owned(),
            responsibilities: Vec::new(),
            boundaries: Vec::new(),
            level,
            active,
            object_revision: 1,
            project_revision: 7,
            created_at: instant(),
            updated_at: instant(),
            created_by: actor,
            updated_by: actor,
        }
    }

    fn snapshot_with_additional_roles(
        snapshot: &VerifiedRoleBriefSnapshot,
        roles: Vec<RoleDefinition>,
    ) -> VerifiedRoleBriefSnapshot {
        let mut meta = snapshot.meta.clone();
        meta.entity_counts.active_objects +=
            u32::try_from(roles.len()).expect("test Role count fits u32");
        let mut entities = entity_projections(snapshot);
        entities.extend(roles.into_iter().enumerate().map(|(index, role)| {
            projected_entity(
                u8::try_from(180 + index).expect("test projection ID fits u8"),
                snapshot.meta.project_id,
                role.project_revision,
                role.object_revision,
                RoleContinuityChange::Role(role),
                source(220),
                instant(),
            )
        }));
        VerifiedRoleBriefSnapshot::new(
            meta,
            snapshot.membership.clone(),
            object_projections(snapshot),
            entities,
        )
        .expect("snapshot with additional Roles")
    }
}
