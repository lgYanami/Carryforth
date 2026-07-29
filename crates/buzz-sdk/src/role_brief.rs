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
    CommunityMemberRole, ProposalStatus, RoleAssignment, RoleAssignmentProposal,
    RoleContinuityChange, RoleDefinition, RoleHandoff, RoleLevel,
};
use buzz_project_view::{
    Goal, ObjectRef, ProjectIssue, ProjectRole, ProjectView, ProjectViewEntry, ProjectViewObject,
    ProjectViewObjectData, ProjectViewObjectType, ProjectViewRelations, ProjectViewState,
    ProjectViewTombstone, ProjectWork,
};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::project_view_v2::{
    V2EntityProjection, V2MembershipProjection, V2MetaProjection, V2ProjectObjectProjection,
    V2ProjectedObject, V2ProjectionSource,
};
use crate::SdkError;

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

/// Minimal project-wide context carried by every Role Brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoleBriefProjectSummary {
    /// The single Project Profile.
    pub profile: RoleBriefObject,
    /// Every active Goal in deterministic order.
    pub goals: Vec<RoleBriefObject>,
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
    /// Candidate or assigned entry state.
    pub state: RoleBriefMemberState,
    /// Role-related Issues and their handling Work in deterministic order.
    pub related_objects: Vec<RoleBriefObject>,
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
    handoffs: BTreeMap<Uuid, EntityHead<RoleHandoff>>,
    view: ProjectView,
}

#[derive(Debug, Clone)]
struct ObjectHead {
    object: ProjectViewObject,
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
                    objects.insert(object.id, ObjectHead { object, source });
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
            objects.len() + roles.len(),
            &proposals,
            &assignments,
            handoffs.len(),
        )?;
        let entries_by_id = entries
            .iter()
            .cloned()
            .map(|entry| (entry.id(), entry))
            .collect::<BTreeMap<_, _>>();
        let view = validate_project_state(&meta, entries)?;
        validate_continuity(
            &entries_by_id,
            &roles,
            &proposals,
            &assignments,
            &handoffs,
            &membership,
        )?;

        Ok(Self {
            meta,
            membership,
            entries: entries_by_id,
            objects,
            roles,
            proposals,
            assignments,
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

        let active_assignment = self
            .assignments
            .values()
            .find(|head| head.entity.member_pubkey == member_pubkey && head.entity.is_active());
        let (state, related_objects) = if let Some(assignment) = active_assignment {
            let role = self
                .roles
                .get(&assignment.entity.role_id)
                .ok_or_else(|| invalid("active Assignment references a missing verified Role"))?;
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
                self.related_objects(role.entity.role_id),
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
            (
                RoleBriefMemberState::Candidate { open_proposals },
                Vec::new(),
            )
        };
        let community_role = self
            .membership
            .members
            .iter()
            .find(|member| member.pubkey == member_pubkey)
            .map(|member| member.role);

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
            state,
            related_objects,
            source_revisions: RoleBriefSourceRevisions {
                meta_event_id: self.meta.event_id,
                meta_change_id: self.meta.source.change_id(),
                membership_event_id: self.membership.event_id,
                project_updated_at: self.meta.updated_at,
            },
        })
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

fn validate_counts(
    meta: &V2MetaProjection,
    active_objects: usize,
    proposals: &BTreeMap<Uuid, EntityHead<RoleAssignmentProposal>>,
    assignments: &BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    handoffs: usize,
) -> Result<(), SdkError> {
    let open_proposals = proposals
        .values()
        .filter(|head| head.entity.status == ProposalStatus::Open)
        .count();
    let active_assignments = assignments
        .values()
        .filter(|head| head.entity.is_active())
        .count();
    let counts = meta.entity_counts;
    if usize::try_from(counts.active_objects).ok() != Some(active_objects)
        || usize::try_from(counts.open_proposals).ok() != Some(open_proposals)
        || usize::try_from(counts.active_assignments).ok() != Some(active_assignments)
        || usize::try_from(counts.handoffs).ok() != Some(handoffs)
        || counts.active_commitments != 0
        || counts.checkpoints != 0
    {
        return Err(invalid(
            "v2 metadata counts disagree with the complete verified heads",
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

fn validate_continuity(
    entries: &BTreeMap<Uuid, ProjectViewEntry>,
    roles: &BTreeMap<Uuid, EntityHead<RoleDefinition>>,
    proposals: &BTreeMap<Uuid, EntityHead<RoleAssignmentProposal>>,
    assignments: &BTreeMap<Uuid, EntityHead<RoleAssignment>>,
    handoffs: &BTreeMap<Uuid, EntityHead<RoleHandoff>>,
    membership: &V2MembershipProjection,
) -> Result<(), SdkError> {
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
        if !proposals.contains_key(&assignment.entity.proposal_id) {
            return Err(invalid("Assignment references a missing Proposal"));
        }
    }
    for handoff in handoffs.values() {
        if entries
            .get(&handoff.entity.role_id)
            .is_none_or(|entry| entry.object_type() != ProjectViewObjectType::Role)
            || !assignments.contains_key(&handoff.entity.from_assignment_id)
            || handoff
                .entity
                .to_assignment_id
                .is_some_and(|id| !assignments.contains_key(&id))
        {
            return Err(invalid("Handoff references a missing Role or Assignment"));
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
        source: head.source.clone(),
    }
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
    use buzz_project_view::v2::{ProposalType, RoleContinuityChange};
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
        assert_eq!(brief.related_objects.len(), 2);
        assert_eq!(
            brief
                .related_objects
                .iter()
                .map(|item| item.object.object_type)
                .collect::<Vec<_>>(),
            vec![ProjectViewObjectType::Issue, ProjectViewObjectType::Work]
        );

        let markdown = render_role_brief_markdown(&brief);
        assert!(markdown.starts_with("[Role Brief]\nState: assigned"));
        assert!(markdown.contains("Community role: member"));
        assert!(markdown.contains(&fixture.assignment_id.to_string()));
        assert!(markdown.contains("Related Project View slice:"));
        assert!(markdown.contains("Source revisions: project=4 generation=1"));
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
        let markdown = render_role_brief_markdown(&brief);
        assert!(markdown.contains("State: candidate"));
        assert!(markdown.contains("no active Assignment is verified"));
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
        let issue_id = Uuid::new_v4();
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
            Uuid::new_v4(),
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
            projected_object(1, project_id, profile, source.clone(), now),
            projected_object(2, project_id, goal, source.clone(), now),
            projected_object(3, project_id, issue, source.clone(), now),
            projected_object(4, project_id, work, source.clone(), now),
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
        ];

        let membership_event_id = event_id(8);
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
            event_id: event_id(9),
            project_id,
            projection_generation: 1,
            project_revision: 4,
            entity_counts: V2EntityCounts {
                active_objects: 5,
                open_proposals: 0,
                active_assignments: 1,
                active_commitments: 0,
                checkpoints: 0,
                handoffs: 0,
            },
            membership_snapshot_event_id: membership_event_id,
            reset: true,
            changed_heads: Vec::new(),
            source,
            updated_at: now,
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
        entities
    }
}
