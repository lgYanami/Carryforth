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

export type RawProjectViewRoleContinuity = {
  roles: RawProjectRoleDefinition[];
  proposals: RawProjectRoleProposal[];
  assignments: RawProjectRoleAssignment[];
  handoffs: RawProjectRoleHandoff[];
  members: Array<{ pubkey: string; role: ProjectCommunityMemberRole }>;
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

export type ProjectViewRoleContinuity = {
  roles: ProjectRoleDefinition[];
  proposals: ProjectRoleProposal[];
  assignments: ProjectRoleAssignment[];
  handoffs: ProjectRoleHandoff[];
  members: Array<{ pubkey: string; role: ProjectCommunityMemberRole }>;
};

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
  return {
    roles,
    proposals,
    assignments,
    handoffs,
    members: raw.members,
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
    changedEntities: raw.changed_entities,
  };
}
