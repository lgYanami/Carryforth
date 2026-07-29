import { invokeTauri } from "@/shared/api/tauri";
import type { ProjectView } from "@/shared/api/tauriProjectView";
import { ProjectViewIntegrityError } from "@/shared/api/tauriProjectViewIntegrity";

export type ProjectRoleLevel = "admin" | "member";
export type ProjectRoleProposalType = "request" | "offer";
export type ProjectRoleProposalStatus =
  | "open"
  | "consumed"
  | "rejected"
  | "withdrawn"
  | "expired";
export type ProjectRoleAssignmentEndReason =
  | "revoked"
  | "replaced"
  | "unrecoverable"
  | "membership_ended"
  | "role_deactivated";
export type ProjectWorkCommitmentEndReason =
  | "released"
  | "replaced"
  | "assignment_ended"
  | "work_closed";
export type ProjectCommunityMemberRole = "owner" | "admin" | "member";

type RawProjectRoleDefinition = {
  role_id: string;
  name: string;
  purpose: string;
  responsibilities: string[];
  boundaries: string[];
  level: ProjectRoleLevel;
  active: boolean;
  object_revision: number;
  project_revision: number;
  created_at: string;
  updated_at: string;
  created_by: string;
  updated_by: string;
};

type RawProjectRoleProposal = {
  proposal_id: string;
  role_id: string;
  candidate_pubkey: string;
  proposal_type: ProjectRoleProposalType;
  candidate_accepted_at?: string;
  authorized_by?: string;
  authorized_at?: string;
  expected_target_assignment_id?: string;
  expected_candidate_assignment_id?: string;
  expires_at: string;
  status: ProjectRoleProposalStatus;
  reason?: string;
  created_by: string;
  created_at: string;
  resolved_at?: string;
  entity_revision: number;
  project_revision: number;
};

type RawProjectRoleAssignment = {
  assignment_id: string;
  role_id: string;
  member_pubkey: string;
  proposal_id?: string;
  started_at: string;
  started_by: string;
  replacement_requested_at?: string;
  replacement_request_reason?: string;
  unable_reported_at?: string;
  unable_report_reason?: string;
  ended_at?: string;
  ended_by?: string;
  ended_reason?: ProjectRoleAssignmentEndReason;
  replaced_by_assignment_id?: string;
  entity_revision: number;
  project_revision: number;
};

type RawProjectWorkCommitment = {
  commitment_id: string;
  work_id: string;
  assignment_id: string;
  member_pubkey: string;
  started_at: string;
  started_by: string;
  ended_at?: string;
  ended_by?: string;
  ended_reason?: ProjectWorkCommitmentEndReason;
  entity_revision: number;
  project_revision: number;
};

type RawProjectRoleHandoff = {
  handoff_id: string;
  role_id: string;
  from_assignment_id: string;
  to_assignment_id?: string;
  affected_commitment_ids: string[];
  cause: ProjectRoleAssignmentEndReason;
  created_at: string;
  entity_revision: number;
  project_revision: number;
};

type RawRoleBriefSource = {
  event_id: string;
  project_revision: number;
  item_revision: number;
  change_id: string;
  source_type: string;
};

type RawRoleBriefObject = {
  object: {
    id: string;
    object_type: string;
    data: {
      object_type: string;
      data: Record<string, unknown>;
    };
  };
  responsible_role_id?: string;
  source: RawRoleBriefSource;
};

type RawProjectRoleBrief = {
  generated_at: string;
  project_id: string;
  project_revision: number;
  projection_generation: number;
  member_pubkey: string;
  community_role?: ProjectCommunityMemberRole;
  project: {
    profile: RawRoleBriefObject;
    goals: RawRoleBriefObject[];
  };
  state:
    | {
        status: "candidate";
        open_proposals: Array<{
          proposal: RawProjectRoleProposal;
          source: RawRoleBriefSource;
        }>;
      }
    | {
        status: "assigned";
        role: {
          role: RawProjectRoleDefinition;
          source: RawRoleBriefSource;
        };
        assignment: {
          assignment: RawProjectRoleAssignment;
          source: RawRoleBriefSource;
        };
      };
  responsible_work: Array<{
    work: RawRoleBriefObject;
    state:
      | {
          status: "committed";
          commitment: {
            commitment: RawProjectWorkCommitment;
            source: RawRoleBriefSource;
          };
        }
      | { status: "waiting_for_continuation" };
  }>;
  related_objects: RawRoleBriefObject[];
  source_revisions: {
    meta_event_id: string;
    meta_change_id: string;
    membership_event_id: string;
    project_updated_at: string;
  };
};

export type RawProjectViewRoleContinuity = {
  roles: RawProjectRoleDefinition[];
  proposals: RawProjectRoleProposal[];
  assignments: RawProjectRoleAssignment[];
  commitments: RawProjectWorkCommitment[];
  workResponsibilities: Array<{ workId: string; roleId: string }>;
  handoffs: RawProjectRoleHandoff[];
  members: Array<{ pubkey: string; role: ProjectCommunityMemberRole }>;
  briefs: RawProjectRoleBrief[];
};

export type ProjectRoleDefinition = {
  roleId: string;
  name: string;
  purpose: string;
  responsibilities: string[];
  boundaries: string[];
  level: ProjectRoleLevel;
  active: boolean;
  objectRevision: number;
  projectRevision: number;
  createdAt: string;
  updatedAt: string;
  createdBy: string;
  updatedBy: string;
};

export type ProjectRoleProposal = {
  proposalId: string;
  roleId: string;
  candidatePubkey: string;
  proposalType: ProjectRoleProposalType;
  candidateAcceptedAt?: string;
  authorizedBy?: string;
  authorizedAt?: string;
  expectedTargetAssignmentId?: string;
  expectedCandidateAssignmentId?: string;
  expiresAt: string;
  status: ProjectRoleProposalStatus;
  reason?: string;
  createdBy: string;
  createdAt: string;
  resolvedAt?: string;
  entityRevision: number;
  projectRevision: number;
};

export type ProjectRoleAssignment = {
  assignmentId: string;
  roleId: string;
  memberPubkey: string;
  proposalId?: string;
  startedAt: string;
  startedBy: string;
  replacementRequestedAt?: string;
  replacementRequestReason?: string;
  unableReportedAt?: string;
  unableReportReason?: string;
  endedAt?: string;
  endedBy?: string;
  endedReason?: ProjectRoleAssignmentEndReason;
  replacedByAssignmentId?: string;
  entityRevision: number;
  projectRevision: number;
};

export type ProjectWorkCommitment = {
  commitmentId: string;
  workId: string;
  assignmentId: string;
  memberPubkey: string;
  startedAt: string;
  startedBy: string;
  endedAt?: string;
  endedBy?: string;
  endedReason?: ProjectWorkCommitmentEndReason;
  entityRevision: number;
  projectRevision: number;
};

export type ProjectRoleHandoff = {
  handoffId: string;
  roleId: string;
  fromAssignmentId: string;
  toAssignmentId?: string;
  affectedCommitmentIds: string[];
  cause: ProjectRoleAssignmentEndReason;
  createdAt: string;
  entityRevision: number;
  projectRevision: number;
};

export type ProjectRoleBrief = {
  generatedAt: string;
  projectId: string;
  projectRevision: number;
  projectionGeneration: number;
  memberPubkey: string;
  communityRole?: ProjectCommunityMemberRole;
  project: {
    name: string;
    purpose: string;
    positioning: string;
    scope: string;
    goals: Array<{ title: string; desiredOutcome: string }>;
  };
  state:
    | {
        status: "candidate";
        openProposalIds: string[];
      }
    | {
        status: "assigned";
        roleId: string;
        roleName: string;
        level: ProjectRoleLevel;
        assignmentId: string;
        startedAt: string;
      };
  responsibleWork: Array<{
    workId: string;
    title: string;
    status: string;
    responsibleRoleId: string;
    commitment:
      | {
          status: "committed";
          commitmentId: string;
          assignmentId: string;
          memberPubkey: string;
          startedAt: string;
        }
      | { status: "waiting_for_continuation" };
  }>;
  relatedObjects: Array<{
    id: string;
    objectType: string;
    title: string;
  }>;
  sourceRevisions: {
    metaEventId: string;
    metaChangeId: string;
    membershipEventId: string;
    projectUpdatedAt: string;
  };
};

export type ProjectViewRoleContinuity = {
  roles: ProjectRoleDefinition[];
  proposals: ProjectRoleProposal[];
  assignments: ProjectRoleAssignment[];
  commitments: ProjectWorkCommitment[];
  workResponsibilities: Array<{ workId: string; roleId: string }>;
  handoffs: ProjectRoleHandoff[];
  members: Array<{ pubkey: string; role: ProjectCommunityMemberRole }>;
  briefs: ProjectRoleBrief[];
};

function briefObjectTitle(object: RawRoleBriefObject["object"]) {
  const data = object.data.data;
  const candidate =
    data.title ??
    data.name ??
    data.description ??
    `${object.object_type} ${object.id}`;
  return typeof candidate === "string" ? candidate : String(candidate);
}

function normalizeRoleBrief(
  raw: RawProjectRoleBrief,
  projectRevision: number,
): ProjectRoleBrief {
  const profile = raw.project.profile.object;
  if (
    profile.object_type !== "project_profile" ||
    profile.data.object_type !== "project_profile" ||
    raw.project_revision !== projectRevision
  ) {
    throw new ProjectViewIntegrityError(
      "Role Brief does not match the verified Project snapshot",
    );
  }
  const profileData = profile.data.data;
  const stringField = (name: string) => {
    const value = profileData[name];
    if (typeof value !== "string") {
      throw new ProjectViewIntegrityError(
        `Role Brief Project Profile is missing ${name}`,
      );
    }
    return value;
  };
  const goals = raw.project.goals.map((goal) => {
    const data = goal.object.data.data;
    if (
      goal.object.object_type !== "goal" ||
      goal.object.data.object_type !== "goal" ||
      typeof data.title !== "string" ||
      typeof data.desired_outcome !== "string"
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief contains an invalid Goal summary",
      );
    }
    return {
      title: data.title,
      desiredOutcome: data.desired_outcome,
    };
  });
  const state: ProjectRoleBrief["state"] =
    raw.state.status === "assigned"
      ? {
          status: "assigned",
          roleId: raw.state.role.role.role_id,
          roleName: raw.state.role.role.name,
          level: raw.state.role.role.level,
          assignmentId: raw.state.assignment.assignment.assignment_id,
          startedAt: raw.state.assignment.assignment.started_at,
        }
      : {
          status: "candidate",
          openProposalIds: raw.state.open_proposals.map(
            (proposal) => proposal.proposal.proposal_id,
          ),
        };
  const responsibleWork = raw.responsible_work.map((item) => {
    const object = item.work.object;
    const data = object.data.data;
    if (
      object.object_type !== "work" ||
      object.data.object_type !== "work" ||
      typeof data.title !== "string" ||
      typeof data.status !== "string" ||
      !item.work.responsible_role_id
    ) {
      throw new ProjectViewIntegrityError(
        "Role Brief contains invalid responsible Work",
      );
    }
    return {
      workId: object.id,
      title: data.title,
      status: data.status,
      responsibleRoleId: item.work.responsible_role_id,
      commitment:
        item.state.status === "committed"
          ? {
              status: "committed" as const,
              commitmentId: item.state.commitment.commitment.commitment_id,
              assignmentId: item.state.commitment.commitment.assignment_id,
              memberPubkey: item.state.commitment.commitment.member_pubkey,
              startedAt: item.state.commitment.commitment.started_at,
            }
          : { status: "waiting_for_continuation" as const },
    };
  });
  return {
    generatedAt: raw.generated_at,
    projectId: raw.project_id,
    projectRevision: raw.project_revision,
    projectionGeneration: raw.projection_generation,
    memberPubkey: raw.member_pubkey,
    communityRole: raw.community_role,
    project: {
      name: stringField("name"),
      purpose: stringField("purpose"),
      positioning: stringField("positioning"),
      scope: stringField("scope"),
      goals,
    },
    state,
    responsibleWork,
    relatedObjects: raw.related_objects.map((related) => ({
      id: related.object.id,
      objectType: related.object.object_type,
      title: briefObjectTitle(related.object),
    })),
    sourceRevisions: {
      metaEventId: raw.source_revisions.meta_event_id,
      metaChangeId: raw.source_revisions.meta_change_id,
      membershipEventId: raw.source_revisions.membership_event_id,
      projectUpdatedAt: raw.source_revisions.project_updated_at,
    },
  };
}

export function normalizeRoleContinuity(
  raw: RawProjectViewRoleContinuity,
  view: ProjectView,
  projectRevision: number,
): ProjectViewRoleContinuity {
  const roles = raw.roles.map<ProjectRoleDefinition>((role) => ({
    roleId: role.role_id,
    name: role.name,
    purpose: role.purpose,
    responsibilities: role.responsibilities,
    boundaries: role.boundaries,
    level: role.level,
    active: role.active,
    objectRevision: role.object_revision,
    projectRevision: role.project_revision,
    createdAt: role.created_at,
    updatedAt: role.updated_at,
    createdBy: role.created_by,
    updatedBy: role.updated_by,
  }));
  const proposals = raw.proposals.map<ProjectRoleProposal>((proposal) => ({
    proposalId: proposal.proposal_id,
    roleId: proposal.role_id,
    candidatePubkey: proposal.candidate_pubkey,
    proposalType: proposal.proposal_type,
    candidateAcceptedAt: proposal.candidate_accepted_at,
    authorizedBy: proposal.authorized_by,
    authorizedAt: proposal.authorized_at,
    expectedTargetAssignmentId: proposal.expected_target_assignment_id,
    expectedCandidateAssignmentId: proposal.expected_candidate_assignment_id,
    expiresAt: proposal.expires_at,
    status: proposal.status,
    reason: proposal.reason,
    createdBy: proposal.created_by,
    createdAt: proposal.created_at,
    resolvedAt: proposal.resolved_at,
    entityRevision: proposal.entity_revision,
    projectRevision: proposal.project_revision,
  }));
  const assignments = raw.assignments.map<ProjectRoleAssignment>(
    (assignment) => ({
      assignmentId: assignment.assignment_id,
      roleId: assignment.role_id,
      memberPubkey: assignment.member_pubkey,
      proposalId: assignment.proposal_id,
      startedAt: assignment.started_at,
      startedBy: assignment.started_by,
      replacementRequestedAt: assignment.replacement_requested_at,
      replacementRequestReason: assignment.replacement_request_reason,
      unableReportedAt: assignment.unable_reported_at,
      unableReportReason: assignment.unable_report_reason,
      endedAt: assignment.ended_at,
      endedBy: assignment.ended_by,
      endedReason: assignment.ended_reason,
      replacedByAssignmentId: assignment.replaced_by_assignment_id,
      entityRevision: assignment.entity_revision,
      projectRevision: assignment.project_revision,
    }),
  );
  const commitments = raw.commitments.map<ProjectWorkCommitment>(
    (commitment) => ({
      commitmentId: commitment.commitment_id,
      workId: commitment.work_id,
      assignmentId: commitment.assignment_id,
      memberPubkey: commitment.member_pubkey,
      startedAt: commitment.started_at,
      startedBy: commitment.started_by,
      endedAt: commitment.ended_at,
      endedBy: commitment.ended_by,
      endedReason: commitment.ended_reason,
      entityRevision: commitment.entity_revision,
      projectRevision: commitment.project_revision,
    }),
  );
  const workResponsibilities = raw.workResponsibilities;
  const handoffs = raw.handoffs.map<ProjectRoleHandoff>((handoff) => ({
    handoffId: handoff.handoff_id,
    roleId: handoff.role_id,
    fromAssignmentId: handoff.from_assignment_id,
    toAssignmentId: handoff.to_assignment_id,
    affectedCommitmentIds: handoff.affected_commitment_ids,
    cause: handoff.cause,
    createdAt: handoff.created_at,
    entityRevision: handoff.entity_revision,
    projectRevision: handoff.project_revision,
  }));
  const briefs = raw.briefs.map((brief) =>
    normalizeRoleBrief(brief, projectRevision),
  );
  const roleObjectIds = new Set(view.roles.map((role) => role.id));
  const roleIds = new Set<string>();
  for (const role of roles) {
    if (
      roleIds.has(role.roleId) ||
      !roleObjectIds.has(role.roleId) ||
      role.projectRevision > projectRevision
    ) {
      throw new ProjectViewIntegrityError(
        `Role continuity definition ${role.roleId} disagrees with the assembled View`,
      );
    }
    roleIds.add(role.roleId);
  }
  if (roleIds.size !== roleObjectIds.size) {
    throw new ProjectViewIntegrityError(
      "Role continuity definitions do not cover every active Role",
    );
  }
  const activeRoleIds = new Set<string>();
  const activeMemberPubkeys = new Set<string>();
  for (const assignment of assignments) {
    if (!roleIds.has(assignment.roleId)) {
      throw new ProjectViewIntegrityError(
        `Assignment ${assignment.assignmentId} references a missing Role`,
      );
    }
    if (!assignment.endedAt) {
      if (
        activeRoleIds.has(assignment.roleId) ||
        activeMemberPubkeys.has(assignment.memberPubkey)
      ) {
        throw new ProjectViewIntegrityError(
          "verified Role continuity contains duplicate active Assignments",
        );
      }
      activeRoleIds.add(assignment.roleId);
      activeMemberPubkeys.add(assignment.memberPubkey);
    }
  }
  const assignmentsById = new Map(
    assignments.map((assignment) => [assignment.assignmentId, assignment]),
  );
  const responsibleRolesByWork = new Map(
    workResponsibilities.map((responsibility) => [
      responsibility.workId,
      responsibility.roleId,
    ]),
  );
  const activeCommittedWork = new Set<string>();
  for (const commitment of commitments) {
    const assignment = assignmentsById.get(commitment.assignmentId);
    if (
      !assignment ||
      assignment.memberPubkey.toLowerCase() !==
        commitment.memberPubkey.toLowerCase()
    ) {
      throw new ProjectViewIntegrityError(
        `Commitment ${commitment.commitmentId} disagrees with its Assignment`,
      );
    }
    if (
      !commitment.endedAt &&
      (assignment.endedAt ||
        activeCommittedWork.has(commitment.workId) ||
        responsibleRolesByWork.get(commitment.workId) !== assignment.roleId)
    ) {
      throw new ProjectViewIntegrityError(
        "verified Role continuity contains an invalid active Commitment",
      );
    }
    if (!commitment.endedAt) activeCommittedWork.add(commitment.workId);
  }
  for (const brief of briefs) {
    if (brief.state.status === "assigned") {
      const assignedState = brief.state;
      const assignment = assignments.find(
        (candidate) =>
          candidate.assignmentId === assignedState.assignmentId &&
          !candidate.endedAt,
      );
      if (
        !assignment ||
        assignment.memberPubkey.toLowerCase() !==
          brief.memberPubkey.toLowerCase() ||
        assignment.roleId !== assignedState.roleId
      ) {
        throw new ProjectViewIntegrityError(
          "Role Brief Assignment disagrees with Role continuity",
        );
      }
    }
  }
  return {
    roles,
    proposals,
    assignments,
    commitments,
    workResponsibilities,
    handoffs,
    members: raw.members,
    briefs,
  };
}

type ProjectViewRoleMutationBase = {
  expectedProjectRevision: number;
  actingAssignmentId?: string;
};

export type ProjectViewRoleMutationIntent =
  | (ProjectViewRoleMutationBase & {
      operation: "request_role";
      roleId: string;
      expiresInHours?: number;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "offer_role";
      roleId: string;
      candidatePubkey: string;
      expiresInHours?: number;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "accept_proposal";
      proposalId: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "reject_proposal";
      proposalId: string;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "withdraw_proposal";
      proposalId: string;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "authorize_proposal";
      proposalId: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "end_assignment";
      assignmentId: string;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "set_work_responsibility";
      workId: string;
      responsibleRoleId?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "accept_work";
      workId: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "end_commitment";
      commitmentId: string;
      reason?: string;
    })
  | (ProjectViewRoleMutationBase & {
      operation: "replace_commitment";
      workId: string;
      expectedCommitmentId: string;
    });

export type RawProjectViewRoleMutationResult =
  | {
      status: "applied";
      event_id: string;
      project_revision: number;
      operation: string;
      proposal_id?: string;
      assignment_id?: string;
      target_assignment_id?: string;
      work_id?: string;
      responsible_role_id?: string;
      commitment_id?: string;
      changed_entities: Array<{
        entityType: string;
        entityId: string;
        entityRevision: number;
      }>;
    }
  | {
      status: "conflict";
      expected_project_revision: number;
      current_project_revision?: number;
      message: string;
    };

export type ProjectViewRoleMutationResult =
  | {
      status: "applied";
      eventId: string;
      projectRevision: number;
      operation: string;
      proposalId?: string;
      assignmentId?: string;
      targetAssignmentId?: string;
      workId?: string;
      responsibleRoleId?: string;
      commitmentId?: string;
      changedEntities: Array<{
        entityType: string;
        entityId: string;
        entityRevision: number;
      }>;
    }
  | {
      status: "conflict";
      expectedProjectRevision: number;
      currentProjectRevision?: number;
      message: string;
    };

export function serializeProjectViewRoleMutationIntent(
  intent: ProjectViewRoleMutationIntent,
): Record<string, unknown> {
  const common = {
    operation: intent.operation,
    expected_project_revision: intent.expectedProjectRevision,
    acting_assignment_id: intent.actingAssignmentId,
  };
  switch (intent.operation) {
    case "request_role":
      return {
        ...common,
        role_id: intent.roleId,
        expires_in_hours: intent.expiresInHours ?? 72,
        reason: intent.reason,
      };
    case "offer_role":
      return {
        ...common,
        role_id: intent.roleId,
        candidate_pubkey: intent.candidatePubkey,
        expires_in_hours: intent.expiresInHours ?? 72,
        reason: intent.reason,
      };
    case "accept_proposal":
    case "authorize_proposal":
      return { ...common, proposal_id: intent.proposalId };
    case "reject_proposal":
    case "withdraw_proposal":
      return {
        ...common,
        proposal_id: intent.proposalId,
        reason: intent.reason,
      };
    case "end_assignment":
      return {
        ...common,
        assignment_id: intent.assignmentId,
        reason: intent.reason,
      };
    case "set_work_responsibility":
      return {
        ...common,
        work_id: intent.workId,
        responsible_role_id: intent.responsibleRoleId,
      };
    case "accept_work":
      return { ...common, work_id: intent.workId };
    case "end_commitment":
      return {
        ...common,
        commitment_id: intent.commitmentId,
        reason: intent.reason,
      };
    case "replace_commitment":
      return {
        ...common,
        work_id: intent.workId,
        expected_commitment_id: intent.expectedCommitmentId,
      };
  }
}

export async function mutateProjectViewRole(
  intent: ProjectViewRoleMutationIntent,
): Promise<ProjectViewRoleMutationResult> {
  const raw = await invokeTauri<RawProjectViewRoleMutationResult>(
    "mutate_project_view_role",
    { input: serializeProjectViewRoleMutationIntent(intent) },
  );
  if (raw.status === "conflict") {
    return {
      status: raw.status,
      expectedProjectRevision: raw.expected_project_revision,
      currentProjectRevision: raw.current_project_revision,
      message: raw.message,
    };
  }
  return {
    status: raw.status,
    eventId: raw.event_id,
    projectRevision: raw.project_revision,
    operation: raw.operation,
    proposalId: raw.proposal_id,
    assignmentId: raw.assignment_id,
    targetAssignmentId: raw.target_assignment_id,
    workId: raw.work_id,
    responsibleRoleId: raw.responsible_role_id,
    commitmentId: raw.commitment_id,
    changedEntities: raw.changed_entities,
  };
}
